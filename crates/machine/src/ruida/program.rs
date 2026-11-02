//! [`RuidaProgram`]: what a Ruida run streams (or Send stores on the controller), and [`check_program`], the Ruida
//! counterpart of the galvo's `check_job` and GRBL's `check_program`: the exact commands that go out are checked
//! against the profile's hard limits before anything is sent.
//!
//! A program is the unscrambled command stream of an `.rd` job (`ruida::rd::write_rd_plain`: header, layers, file sum,
//! end of file) together with the commands parsed back from those same bytes, so what is checked is what is sent.

use ruida::command::{ByteKind, Command, LayerFlag, OffsetKind, PointKind, PowerKind, RefPoint, RepeatKind, SpeedKind, ValueKind};
use ruida::frame::{MAX_UDP_PAYLOAD, packetize};
use ruida::num::Power;
use ruida::rd::{RdJob, WriteOptions, write_rd_plain};
use ruida::{Swizzle, parse_lenient};

use super::profile::RuidaProfile;

/// Largest program accepted, bytes (unscrambled). Controllers keep stored files in a few MB of memory.
pub const MAX_PROGRAM_BYTES: usize = 32 * 1024 * 1024;
/// Longest stored-file name. Controllers show short names on their panel; the real limit is unverified.
pub const MAX_FILE_NAME: usize = 32;

/// A refused program or file name.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct ProgramError(pub String);

fn err<T>(m: impl Into<String>) -> Result<T, ProgramError> {
    Err(ProgramError(m.into()))
}

/// What a program is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Streamed and run at once (an armed run): it must not start with a file name.
    Run,
    /// Stored on the controller under a name (Send): it must start with `E7 01 name`.
    Store,
}

/// A job as the Ruida driver sends it. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct RuidaProgram {
    plain: Vec<u8>,
    commands: Vec<Command>,
    name: Option<String>,
    datagrams: usize,
}

/// Refuse names a controller's file list may not hold: 1..=[`MAX_FILE_NAME`] printable ASCII characters.
pub fn check_name(name: &str) -> Result<(), ProgramError> {
    if name.is_empty() || name.len() > MAX_FILE_NAME || !name.bytes().all(|b| (0x20..0x7F).contains(&b)) {
        return err(format!("a controller file name must be 1..={MAX_FILE_NAME} printable ASCII characters (got {name:?})"));
    }
    Ok(())
}

impl RuidaProgram {
    /// The job's commands (machine-frame mm in `job`), stored under `name` when one is given (Send).
    pub fn from_job(job: &RdJob, name: Option<&str>) -> Result<Self, ProgramError> {
        if let Some(n) = name {
            check_name(n)?;
        }
        let opts = WriteOptions { filename: name.map(str::to_owned), ..WriteOptions::default() };
        let plain = write_rd_plain(job, &opts).map_err(|e| ProgramError(e.to_string()))?;
        Self::from_plain(plain, name.map(str::to_owned))
    }

    fn from_plain(plain: Vec<u8>, name: Option<String>) -> Result<Self, ProgramError> {
        if plain.len() > MAX_PROGRAM_BYTES {
            return err(format!("the job is {} bytes; at most {MAX_PROGRAM_BYTES} are sent to a Ruida controller", plain.len()));
        }
        let commands = parse_lenient(&plain);
        let datagrams = packetize(&plain, MAX_UDP_PAYLOAD).map_err(|e| ProgramError(e.to_string()))?.len();
        Ok(RuidaProgram { plain, commands, name, datagrams })
    }

    /// The unscrambled bytes.
    pub fn plain(&self) -> &[u8] {
        &self.plain
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// The stored-file name (Send), if any.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The program as an `.rd` file (scrambled with `magic`).
    pub fn rd_file(&self, magic: u8) -> Vec<u8> {
        Swizzle::new(magic).scramble(&self.plain)
    }

    /// The UDP payloads, whole commands each.
    pub fn payloads(&self) -> Result<Vec<Vec<u8>>, ProgramError> {
        packetize(&self.plain, MAX_UDP_PAYLOAD).map_err(|e| ProgramError(e.to_string()))
    }

    /// How many UDP payloads [`RuidaProgram::payloads`] makes.
    pub fn datagram_count(&self) -> usize {
        self.datagrams
    }

    /// Bytes.
    pub fn len(&self) -> usize {
        self.plain.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plain.is_empty()
    }
}

/// Check every command of `program` against `profile` before anything is sent:
///
/// - only the commands of a plain job (the layout `ruida::rd` writes): settings, moves, cuts, bounds; no stored-file,
///   memory, homing, jog, rapid or single-axis commands and nothing this crate does not type;
/// - `Purpose::Store` programs start with the file name, `Purpose::Run` programs carry none;
/// - one start of process and one end of file, at the end;
/// - layer min and max powers (lasers 1 to 4) within `max_power_pct`; no immediate, end or through power;
/// - cutting speeds above zero and within `max_speed_mm_s`;
/// - the first motion absolute; every point (moves, cuts, bounds) on the bed;
/// - no repeat or array of the job (process and array repeat 1 x 1), no pen, layer or feed offsets.
pub fn check_program(profile: &RuidaProfile, program: &RuidaProgram, purpose: Purpose) -> Result<(), ProgramError> {
    use Command as C;
    profile.validate().map_err(|e| ProgramError(e.to_string()))?;
    let cmds = program.commands();
    let [w, h] = profile.work_area_mm;
    // µm, with 1 µm for rounding.
    let max_um = [(w * 1000.0).round() as i64 + 1, (h * 1000.0).round() as i64 + 1];
    let on_bed = |x: i64, y: i64, what: &str| -> Result<(), ProgramError> {
        if (-1..=max_um[0]).contains(&x) && (-1..=max_um[1]).contains(&y) {
            Ok(())
        } else {
            err(format!(
                "{what} ({:.3}, {:.3}) mm is off the {w} x {h} mm bed",
                x as f64 / 1000.0,
                y as f64 / 1000.0
            ))
        }
    };
    let power_limit = Power::from_percent(profile.max_power_pct).map_err(|e| ProgramError(e.to_string()))?;
    let power_ok = |p: Power| -> Result<(), ProgramError> {
        if p > power_limit {
            err(format!("a layer power of {:.1} % is above the profile's {} %", p.percent(), profile.max_power_pct))
        } else {
            Ok(())
        }
    };
    let speed_ok = |um_s: u32| -> Result<(), ProgramError> {
        let mm_s = f64::from(um_s) / 1000.0;
        if um_s == 0 || mm_s > profile.max_speed_mm_s + 1e-9 {
            err(format!("a layer speed of {mm_s} mm/s is outside 0..={} mm/s (the profile's limit)", profile.max_speed_mm_s))
        } else {
            Ok(())
        }
    };
    match (purpose, cmds.first()) {
        (Purpose::Store, Some(C::SetFilename(n))) => check_name(n)?,
        (Purpose::Store, _) => return err("a stored file must start with its name (E7 01)"),
        (Purpose::Run, _) => {}
    }
    match cmds.last() {
        Some(C::EndOfFile) => {}
        _ => return err("the job must end with end of file (D7)"),
    }
    let mut pos: Option<[i64; 2]> = None;
    let mut starts = 0;
    let mut ends = 0;
    for (i, c) in cmds.iter().enumerate() {
        let target = match c {
            C::MoveAbs { x, y } | C::CutAbs { x, y } => Some([i64::from(*x), i64::from(*y)]),
            C::MoveRel { dx, dy } | C::CutRel { dx, dy } => pos.map(|p| [p[0] + i64::from(*dx), p[1] + i64::from(*dy)]),
            C::MoveRelX { dx } | C::CutRelX { dx } => pos.map(|p| [p[0] + i64::from(*dx), p[1]]),
            C::MoveRelY { dy } | C::CutRelY { dy } => pos.map(|p| [p[0], p[1] + i64::from(*dy)]),
            _ => None,
        };
        let motion = matches!(
            c,
            C::MoveAbs { .. }
                | C::CutAbs { .. }
                | C::MoveRel { .. }
                | C::CutRel { .. }
                | C::MoveRelX { .. }
                | C::CutRelX { .. }
                | C::MoveRelY { .. }
                | C::CutRelY { .. }
        );
        if motion {
            let Some(t) = target else {
                return err("the first motion of the job is relative: its start is unknown");
            };
            on_bed(t[0], t[1], if c.is_cut() { "a cut to" } else { "a move to" })?;
            pos = Some(t);
            continue;
        }
        match c {
            C::SetFilename(_) if purpose == Purpose::Store && i == 0 => {}
            C::SetFilename(_) => return err("a file name (E7 01) inside the job; a run is not stored"),
            C::RefPoint(RefPoint::MachineZero) | C::SetAbsolute | C::RefPointSet | C::BlockEnd => {}
            C::StartProcess => starts += 1,
            C::EndOfFile => ends += 1,
            C::Byte { kind, value } => match kind {
                ByteKind::LayerNumber
                | ByteKind::MaxLayer
                | ByteKind::EnLaserTubeStart
                | ByteKind::SetCurrentElementIndex
                | ByteKind::EnableBlockCutting
                | ByteKind::SetFeedAutoPause
                | ByteKind::ArrayDirection
                | ByteKind::ArrayEnMirrorCut
                | ByteKind::ArrayMirror => {
                    if matches!(kind, ByteKind::SetFeedAutoPause | ByteKind::ArrayEnMirrorCut | ByteKind::ArrayMirror) && *value != 0 {
                        return err(format!("{kind:?} {value} is not allowed (feeding and mirrored arrays are not used)"));
                    }
                }
                other => return err(format!("the command {other:?} is not part of a plain job")),
            },
            C::FeedRepeat { v0: 0, v1: 0 } | C::Value { kind: ValueKind::FeedInfo, value: 0 } => {}
            C::Value { kind: ValueKind::FileSum, .. } => {}
            C::Offset { kind: OffsetKind::Pen | OffsetKind::Layer, value: 0, .. } => {}
            C::Point { kind: PointKind::DisplayOffset | PointKind::ArrayAdd, x: 0, y: 0 } => {}
            C::Point {
                kind:
                    PointKind::ProcessTopLeft
                    | PointKind::ProcessBottomRight
                    | PointKind::DocumentMinPoint
                    | PointKind::DocumentMaxPoint
                    | PointKind::ArrayMinPoint
                    | PointKind::ArrayMaxPoint,
                x,
                y,
            } => on_bed(i64::from(*x), i64::from(*y), "a job bound")?,
            C::PartPoint { x, y, .. } => on_bed(i64::from(*x), i64::from(*y), "a layer bound")?,
            C::Repeat { kind: RepeatKind::ProcessRepeat | RepeatKind::ArrayRepeat, values } => {
                if values[0] != 1 || values[1] != 1 {
                    return err(format!("the job repeats itself ({} x {}); one run is one copy", values[0], values[1]));
                }
            }
            C::ArrayStart(0) => {}
            C::SpeedPart { um_per_s, .. } | C::Speed { kind: SpeedKind::Laser1, um_per_s } => speed_ok(*um_per_s)?,
            C::Power { kind: PowerKind::Min | PowerKind::Max, power, .. } | C::PartPower { power, .. } => power_ok(*power)?,
            C::Delay { kind: ruida::command::DelayKind::LaserOnDelay | ruida::command::DelayKind::LaserOffDelay, us } => {
                if *us > 1_000_000 {
                    return err(format!("a laser delay of {us} µs is above 1 s"));
                }
            }
            C::LayerColorPart { .. } | C::WorkModePart { .. } | C::FrequencyPart { .. } => {}
            C::LayerFlag(LayerFlag::End | LayerFlag::LaserDevice(false) | LayerFlag::Laser2Offset(false)) => {}
            other => return err(format!("the command {other:?} is not part of a plain job")),
        }
    }
    if starts != 1 {
        return err(format!("the job starts its process {starts} times (once expected)"));
    }
    if ends != 1 {
        return err(format!("the job has {ends} ends of file (one expected)"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ruida::rd::RdLayer;

    use super::*;

    fn job(power: f64, speed: f64) -> RdJob {
        RdJob {
            layers: vec![RdLayer {
                color: [255, 0, 0],
                speed_mm_s: speed,
                min_power: [power, 0.0],
                max_power: [power, 0.0],
                frequency: None,
                polylines: vec![vec![[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 10.0]], vec![[500.0, 300.0], [501.0, 300.0]]],
            }],
        }
    }

    fn profile() -> RuidaProfile {
        RuidaProfile { max_power_pct: 60.0, max_speed_mm_s: 400.0, ..RuidaProfile::default() }
    }

    #[test]
    fn a_written_job_passes_for_its_purpose_only() {
        let p = profile();
        let run = RuidaProgram::from_job(&job(50.0, 100.0), None).unwrap();
        check_program(&p, &run, Purpose::Run).unwrap();
        assert!(check_program(&p, &run, Purpose::Store).is_err());
        let store = RuidaProgram::from_job(&job(50.0, 100.0), Some("PART 1")).unwrap();
        assert_eq!(store.name(), Some("PART 1"));
        check_program(&p, &store, Purpose::Store).unwrap();
        assert!(check_program(&p, &store, Purpose::Run).is_err(), "a run never stores");
        assert!(run.payloads().unwrap().iter().all(|d| d.len() <= MAX_UDP_PAYLOAD));
        assert_eq!(ruida::read_rd(&run.rd_file(0x88), None).unwrap().job.layers.len(), 1);
    }

    #[test]
    fn limits_are_refused_not_clamped() {
        let p = profile();
        for (j, why) in [(job(61.0, 100.0), "power"), (job(50.0, 401.0), "speed")] {
            let prog = RuidaProgram::from_job(&j, None).unwrap();
            let e = check_program(&p, &prog, Purpose::Run).unwrap_err();
            assert!(e.0.contains(why), "{e}");
        }
        let mut off = job(50.0, 100.0);
        off.layers[0].polylines.push(vec![[599.0, 10.0], [601.0, 10.0]]);
        let e = check_program(&p, &RuidaProgram::from_job(&off, None).unwrap(), Purpose::Run).unwrap_err();
        assert!(e.0.contains("off the"), "{e}");
        let mut neg = job(50.0, 100.0);
        neg.layers[0].polylines.push(vec![[1.0, -0.5], [2.0, 1.0]]);
        assert!(check_program(&p, &RuidaProgram::from_job(&neg, None).unwrap(), Purpose::Run).is_err());
    }

    #[test]
    fn foreign_commands_are_refused() {
        let p = profile();
        let base = RuidaProgram::from_job(&job(50.0, 100.0), None).unwrap();
        let with = |extra: Command, at: usize| {
            let mut cmds = base.commands().to_vec();
            cmds.insert(at, extra);
            RuidaProgram::from_plain(ruida::encode_all(&cmds).unwrap(), None).unwrap()
        };
        let n = base.commands().len();
        for bad in [
            Command::Document { kind: ruida::command::DocKind::Start, index: 1 },
            Command::DeleteDocument { index: 0, other: 0 },
            Command::SetMemory { addr: ruida::mem::LASER1_MAX_POWER, v0: 1, v1: 1 },
            Command::HomeXy,
            Command::Power { kind: PowerKind::Immediate, laser: ruida::command::Laser::L1, power: Power::FULL },
            Command::Repeat { kind: RepeatKind::ProcessRepeat, values: [3, 1, 0, 0, 0, 0, 0] },
            Command::StartProcess,
            Command::Unknown(vec![0xE7, 0x46]),
            Command::SetFilename("X".into()),
        ] {
            assert!(check_program(&p, &with(bad.clone(), n - 1), Purpose::Run).is_err(), "{bad:?}");
        }
        // A relative first motion has no known start.
        let rel = RuidaProgram::from_plain(
            ruida::encode_all(&[Command::StartProcess, Command::CutRelX { dx: 5 }, Command::EndOfFile]).unwrap(),
            None,
        )
        .unwrap();
        assert!(check_program(&p, &rel, Purpose::Run).unwrap_err().0.contains("relative"));
        // No end of file.
        let open = RuidaProgram::from_plain(ruida::encode_all(&base.commands()[..n - 1]).unwrap(), None).unwrap();
        assert!(check_program(&p, &open, Purpose::Run).is_err());
    }

    #[test]
    fn names() {
        check_name("JOB_01 a").unwrap();
        for bad in ["", "é", "a\u{0}b", &"x".repeat(MAX_FILE_NAME + 1)] {
            assert!(check_name(bad).is_err(), "{bad:?}");
            assert!(RuidaProgram::from_job(&job(1.0, 1.0), Some(bad)).is_err());
        }
    }
}
