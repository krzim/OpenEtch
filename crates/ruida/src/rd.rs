//! `.rd` job files (RDWorks, LightBurn "Save RD file"): the scrambled command stream a controller runs, from the
//! header to `D7` (end of file). The same bytes are what a host sends over UDP or serial to run or store a job.
//!
//! - **Read**: [`read_rd`] finds the key ([`detect_magic`]), unscrambles, parses ([`crate::command::parse_lenient`])
//!   and extracts geometry ([`job_from_commands`]): polylines per layer with the layer's speed, power and colour.
//! - **Write**: [`write_rd`] / [`job_commands`] turn polylines per layer into a job laid out the way MeerK40t writes
//!   one (header with bounds and per-layer settings, then each layer's settings and moves, then the file sum and
//!   `D7`).
//!
//! Coordinates are the controller's machine frame in mm (µm on the wire). Mirroring and origin choices belong to the
//! caller. Geometry details:
//!
//! - Points are rounded to the µm. Repeated points are dropped; a polyline with fewer than two distinct points is not
//!   written. A polyline that starts where the previous one ended is cut without a move in between, so it reads back
//!   joined to it (the head does not stop there either).
//! - The first move of a file is absolute (MeerK40t notes relative first moves misbehave); later moves and cuts are
//!   relative when both deltas fit 14 bits (-8.192..=8.191 mm), single-axis when one delta is zero.
//! - The file sum (`E5 05`) is the sum of every unscrambled byte before it plus `0xD7`, as MeerK40t computes it.
//!   Whether controllers check it is unverified.

// Layout and header sequence from meerk40t/meerk40t @ d511944 `meerk40t/ruida/rdjob.py` (`write_header`,
// `write_settings`, `write_layer_end`, `write_tail`, `jump`, `mark`, `determine_magic_via_histogram`), re-written in
// Rust (MIT License, Copyright (c) 2021 meerk40t; full notice in `swizzle.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::command::{
    ByteKind, Command, Laser, LayerFlag, OffsetKind, PartPointKind, PointKind, PowerKind, RefPoint, RepeatKind,
    SpeedKind, ValueKind, encode_all, parse_lenient, split,
};
use crate::num::{I14_MAX, I14_MIN, Power, U35_MAX, mm_s_to_um_s, mm_to_um};
use crate::swizzle::{DEFAULT_MAGIC, MAGIC_634XG, MAGIC_ALT, Swizzle, guess_magic_by_histogram};
use crate::{Result, RuidaError};

/// The most layers a file can carry: layer numbers are one 7-bit parameter byte.
pub const MAX_LAYERS: usize = 128;

/// One layer: settings and the polylines it cuts.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RdLayer {
    /// Display colour, RGB.
    pub color: [u8; 3],
    /// Cutting speed, mm/s.
    pub speed_mm_s: f64,
    /// Minimum power of laser 1 and laser 2, percent.
    pub min_power: [f64; 2],
    /// Maximum power of laser 1 and laser 2, percent.
    pub max_power: [f64; 2],
    /// `C6 60` frequency of laser 1 (raw; unit unverified). Not written when `None`.
    pub frequency: Option<u32>,
    /// Cut paths in mm (machine frame).
    pub polylines: Vec<Vec<[f64; 2]>>,
}

/// A whole job: layers in order (layer `i` is Ruida part `i`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RdJob {
    pub layers: Vec<RdLayer>,
}

/// Options for writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOptions {
    /// Scrambling key.
    pub magic: u8,
    /// When set, the job starts with `E7 01 name` so a controller stores it under that name (unverified on hardware;
    /// MeerK40t's emulator saves a job that way).
    pub filename: Option<String>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { magic: DEFAULT_MAGIC, filename: None }
    }
}

fn bgr(c: [u8; 3]) -> u32 {
    u32::from(c[0]) | (u32::from(c[1]) << 8) | (u32::from(c[2]) << 16)
}

fn rgb(bgr: u32) -> [u8; 3] {
    let b = bgr.to_le_bytes();
    [b[0], b[1], b[2]]
}

fn part_u8(i: usize) -> Result<u8> {
    u8::try_from(i).ok().filter(|&p| p < 0x80).ok_or(RuidaError::TooManyLayers(i + 1))
}

#[derive(Clone, Copy)]
struct Bounds {
    min: [i32; 2],
    max: [i32; 2],
}

impl Bounds {
    fn of<'a>(points: impl Iterator<Item = &'a [i32; 2]>) -> Option<Bounds> {
        let mut b: Option<Bounds> = None;
        for p in points {
            let nb = b.get_or_insert(Bounds { min: *p, max: *p });
            nb.min = [nb.min[0].min(p[0]), nb.min[1].min(p[1])];
            nb.max = [nb.max[0].max(p[0]), nb.max[1].max(p[1])];
        }
        b
    }

    fn zero() -> Bounds {
        Bounds { min: [0, 0], max: [0, 0] }
    }
}

/// Polylines in µm with repeated points dropped and degenerate polylines removed.
fn layer_um(layer: &RdLayer) -> Result<Vec<Vec<[i32; 2]>>> {
    let mut out = Vec::new();
    for pl in &layer.polylines {
        let mut q: Vec<[i32; 2]> = Vec::with_capacity(pl.len());
        for p in pl {
            let u = [mm_to_um(p[0], "x")?, mm_to_um(p[1], "y")?];
            if q.last() != Some(&u) {
                q.push(u);
            }
        }
        if q.len() >= 2 {
            out.push(q);
        }
    }
    Ok(out)
}

fn rel(d: i64) -> Option<i16> {
    i16::try_from(d).ok().filter(|v| (I14_MIN..=I14_MAX).contains(v))
}

struct Pen {
    pos: Option<[i32; 2]>,
}

impl Pen {
    fn go(&mut self, out: &mut Vec<Command>, to: [i32; 2], cut: bool) {
        let rel_d = self.pos.map(|p| (rel(i64::from(to[0]) - i64::from(p[0])), rel(i64::from(to[1]) - i64::from(p[1]))));
        let c = match rel_d {
            Some((Some(0), Some(0))) => return,
            Some((Some(dx), Some(0))) => {
                if cut { Command::CutRelX { dx } } else { Command::MoveRelX { dx } }
            }
            Some((Some(0), Some(dy))) => {
                if cut { Command::CutRelY { dy } } else { Command::MoveRelY { dy } }
            }
            Some((Some(dx), Some(dy))) => {
                if cut { Command::CutRel { dx, dy } } else { Command::MoveRel { dx, dy } }
            }
            _ => {
                if cut { Command::CutAbs { x: to[0], y: to[1] } } else { Command::MoveAbs { x: to[0], y: to[1] } }
            }
        };
        out.push(c);
        self.pos = Some(to);
    }
}

/// The job as a command list (before the file sum is known the caller can't add more; use [`write_rd`] for bytes).
pub fn job_commands(job: &RdJob, opts: &WriteOptions) -> Result<Vec<Command>> {
    use Command as C;
    if job.layers.len() > MAX_LAYERS {
        return Err(RuidaError::TooManyLayers(job.layers.len()));
    }
    struct Prepared {
        part: u8,
        speed: u32,
        min: [Power; 2],
        max: [Power; 2],
        color: u32,
        freq: Option<u32>,
        polys: Vec<Vec<[i32; 2]>>,
        bounds: Bounds,
    }
    let mut layers = Vec::with_capacity(job.layers.len());
    for (i, l) in job.layers.iter().enumerate() {
        let speed = mm_s_to_um_s(l.speed_mm_s, "layer speed")?;
        if speed == 0 {
            return Err(RuidaError::Zero { what: "layer speed" });
        }
        let polys = layer_um(l)?;
        let bounds = Bounds::of(polys.iter().flatten()).unwrap_or(Bounds::zero());
        layers.push(Prepared {
            part: part_u8(i)?,
            speed,
            min: [Power::from_percent(l.min_power[0])?, Power::from_percent(l.min_power[1])?],
            max: [Power::from_percent(l.max_power[0])?, Power::from_percent(l.max_power[1])?],
            color: bgr(l.color),
            freq: l.frequency,
            polys,
            bounds,
        });
    }
    let all = Bounds::of(layers.iter().flat_map(|l| l.polys.iter().flatten())).unwrap_or(Bounds::zero());
    let (min, max) = (all.min, all.max);

    let mut out = Vec::new();
    if let Some(name) = &opts.filename {
        out.push(C::SetFilename(name.clone()));
    }
    // Header (MeerK40t write_header). Its bounds put max X with min Y: Ruida's X runs mirrored on the machines it was
    // tested on ("Flip X"). Kept as written there; unverified.
    out.extend([
        C::RefPoint(RefPoint::MachineZero),
        C::SetAbsolute,
        C::RefPointSet,
        C::Byte { kind: ByteKind::EnableBlockCutting, value: 0 },
        C::StartProcess,
        C::FeedRepeat { v0: 0, v1: 0 },
        C::Byte { kind: ByteKind::SetFeedAutoPause, value: 0 },
        C::Point { kind: PointKind::ProcessTopLeft, x: max[0], y: min[1] },
        C::Point { kind: PointKind::ProcessBottomRight, x: min[0], y: max[1] },
        C::Point { kind: PointKind::DocumentMinPoint, x: max[0], y: min[1] },
        C::Point { kind: PointKind::DocumentMaxPoint, x: min[0], y: max[1] },
        C::Repeat { kind: RepeatKind::ProcessRepeat, values: [1, 1, 0, 0, 0, 0, 0] },
        C::Byte { kind: ByteKind::ArrayDirection, value: 0 },
    ]);
    for l in &layers {
        let p = l.part;
        out.push(C::SpeedPart { part: p, um_per_s: l.speed });
        if let Some(freq) = l.freq {
            out.push(C::FrequencyPart { laser: 0, part: p, freq });
        }
        for (k, laser) in [Laser::L1, Laser::L2].into_iter().enumerate() {
            out.push(C::PartPower { max: false, laser, part: p, power: l.min[k] });
            out.push(C::PartPower { max: true, laser, part: p, power: l.max[k] });
        }
        out.push(C::LayerColorPart { part: p, bgr: l.color });
        out.push(C::WorkModePart { part: p, mode: 0 });
        let b = l.bounds;
        for (kind, at) in [
            (PartPointKind::Min, b.min),
            (PartPointKind::Max, b.max),
            (PartPointKind::MinEx, b.min),
            (PartPointKind::MaxEx, b.max),
        ] {
            out.push(C::PartPoint { kind, part: p, x: at[0], y: at[1] });
        }
    }
    out.extend([
        C::Byte { kind: ByteKind::MaxLayer, value: layers.last().map_or(0, |l| l.part) },
        C::Offset { kind: OffsetKind::Pen, axis: 0, value: 0 },
        C::Offset { kind: OffsetKind::Pen, axis: 1, value: 0 },
        C::Offset { kind: OffsetKind::Layer, axis: 0, value: 0 },
        C::Offset { kind: OffsetKind::Layer, axis: 1, value: 0 },
        C::Point { kind: PointKind::DisplayOffset, x: 0, y: 0 },
        C::Value { kind: ValueKind::FeedInfo, value: 0 },
        C::ArrayStart(0),
        C::Byte { kind: ByteKind::SetCurrentElementIndex, value: 0 },
        C::Byte { kind: ByteKind::ArrayEnMirrorCut, value: 0 },
        C::Point { kind: PointKind::ArrayMinPoint, x: min[0], y: min[1] },
        C::Point { kind: PointKind::ArrayMaxPoint, x: max[0], y: max[1] },
        C::Point { kind: PointKind::ArrayAdd, x: 0, y: 0 },
        C::Byte { kind: ByteKind::ArrayMirror, value: 0 },
        // Values MeerK40t copies from RDWorks output ("Unknown").
        C::Repeat { kind: RepeatKind::ArrayRepeat, values: [1, 1, 0, 1123, -3328, 4, 3480] },
    ]);

    // Layers (MeerK40t write_settings / write_layer_end).
    let mut pen = Pen { pos: None };
    for l in &layers {
        out.extend([
            C::Byte { kind: ByteKind::LayerNumber, value: l.part },
            C::LayerFlag(LayerFlag::LaserDevice(false)),
            C::Speed { kind: SpeedKind::Laser1, um_per_s: l.speed },
            C::Delay { kind: crate::command::DelayKind::LaserOnDelay, us: 0 },
            C::Delay { kind: crate::command::DelayKind::LaserOffDelay, us: 0 },
            C::Power { kind: PowerKind::Min, laser: Laser::L1, power: l.min[0] },
            C::Power { kind: PowerKind::Max, laser: Laser::L1, power: l.max[0] },
            C::Power { kind: PowerKind::Min, laser: Laser::L2, power: l.min[1] },
            C::Power { kind: PowerKind::Max, laser: Laser::L2, power: l.max[1] },
            C::Byte { kind: ByteKind::EnLaserTubeStart, value: 1 },
        ]);
        for pl in &l.polys {
            pen.go(&mut out, pl[0], false);
            for &p in &pl[1..] {
                pen.go(&mut out, p, true);
            }
        }
        out.extend([C::BlockEnd, C::LayerFlag(LayerFlag::End), C::LayerFlag(LayerFlag::Laser2Offset(false))]);
    }
    Ok(out)
}

/// `E5 05`'s value: the sum of the unscrambled bytes before it, plus the `D7` that follows.
pub fn file_sum(plain_before: &[u8]) -> u64 {
    plain_before.iter().map(|&b| u64::from(b)).sum::<u64>() + 0xD7
}

/// The whole job as unscrambled bytes, file sum and end of file included.
pub fn write_rd_plain(job: &RdJob, opts: &WriteOptions) -> Result<Vec<u8>> {
    let mut plain = encode_all(&job_commands(job, opts)?)?;
    let sum = file_sum(&plain);
    if sum > U35_MAX {
        return Err(RuidaError::OutOfRange { what: "file sum", value: i64::try_from(sum).unwrap_or(i64::MAX), min: 0, max: 1 << 35 });
    }
    Command::Value { kind: ValueKind::FileSum, value: sum }.encode_into(&mut plain)?;
    Command::EndOfFile.encode_into(&mut plain)?;
    Ok(plain)
}

/// The whole job as an `.rd` file (scrambled with `opts.magic`).
pub fn write_rd(job: &RdJob, opts: &WriteOptions) -> Result<Vec<u8>> {
    Ok(Swizzle::new(opts.magic).scramble(&write_rd_plain(job, opts)?))
}

/// Fraction of the bytes of `plain` that belong to commands this crate types (not [`Command::Unknown`]).
fn known_fraction(plain: &[u8]) -> f64 {
    if plain.is_empty() {
        return 0.0;
    }
    let mut known = 0usize;
    for (_, c) in split(plain) {
        if matches!(Command::decode(c), Ok(cmd) if !matches!(cmd, Command::Unknown(_))) {
            known += c.len();
        }
    }
    // Byte counts are exact in f64 far beyond any file size.
    #[allow(clippy::cast_precision_loss)]
    let r = known as f64 / plain.len() as f64;
    r
}

/// The scrambling key of an `.rd` file: MeerK40t's histogram guess, then the known keys, then every key, scored on
/// the first 4 KiB by how much of it parses as known commands (a file must start with an opcode).
pub fn detect_magic(data: &[u8]) -> Option<u8> {
    let probe = &data[..data.len().min(4096)];
    let score = |m: u8| -> f64 {
        let plain = Swizzle::new(m).unscramble(probe);
        if plain.first().is_none_or(|&b| b < 0x80) { 0.0 } else { known_fraction(&plain) }
    };
    let mut candidates: Vec<u8> = guess_magic_by_histogram(probe).into_iter().collect();
    candidates.extend([DEFAULT_MAGIC, MAGIC_634XG, MAGIC_ALT]);
    for &m in &candidates {
        if score(m) >= 0.9 {
            return Some(m);
        }
    }
    let mut best: Option<(u8, f64)> = None;
    for m in 0..=255u8 {
        let s = score(m);
        if s > best.map_or(0.0, |b| b.1) {
            best = Some((m, s));
        }
    }
    best.filter(|b| b.1 >= 0.5).map(|b| b.0)
}

/// An `.rd` file read back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RdRead {
    pub magic: u8,
    pub commands: Vec<Command>,
    pub job: RdJob,
    /// Things that were skipped or looked wrong (unknown commands are counted, not listed).
    pub warnings: Vec<String>,
    /// Whether the `E5 05` file sum matches; `None` when the file has none.
    pub file_sum_ok: Option<bool>,
    /// Commands this crate does not type.
    pub unknown_commands: usize,
}

/// Read an `.rd` file; `magic: None` detects the key.
pub fn read_rd(data: &[u8], magic: Option<u8>) -> Result<RdRead> {
    let magic = match magic {
        Some(m) => m,
        None => detect_magic(data).ok_or(RuidaError::NoMagic)?,
    };
    let plain = Swizzle::new(magic).unscramble(data);
    if plain.first().is_none_or(|&b| b < 0x80) {
        return Err(RuidaError::Malformed { offset: 0, bytes: plain.iter().take(8).copied().collect() });
    }
    let commands = parse_lenient(&plain);
    let unknown_commands = commands.iter().filter(|c| matches!(c, Command::Unknown(_))).count();
    let file_sum_ok = split(&plain).find(|(_, c)| c.len() == 7 && c[0] == 0xE5 && c[1] == 0x05).map(|(off, c)| {
        matches!(Command::decode(c), Ok(Command::Value { value, .. }) if value == file_sum(&plain[..off]))
    });
    let (job, mut warnings) = job_from_commands(&commands);
    if unknown_commands > 0 {
        warnings.push(format!("{unknown_commands} commands not understood (kept as raw bytes)"));
    }
    if file_sum_ok == Some(false) {
        warnings.push("file sum (E5 05) does not match".into());
    }
    if !matches!(commands.last(), Some(Command::EndOfFile)) {
        warnings.push("no end of file (D7) at the end".into());
    }
    Ok(RdRead { magic, commands, job, warnings, file_sum_ok, unknown_commands })
}

#[derive(Default)]
struct LayerAcc {
    layer: RdLayer,
    seen: bool,
}

/// Geometry and per-layer settings from a command list. Moves end a polyline; cuts extend it (starting at the
/// current position). Settings come from the header's per-part commands and are overridden by the commands after
/// `CA 02 part` (speed `C9 02`, power `C6 01/02/21/22`, colour `CA 05`).
pub fn job_from_commands(cmds: &[Command]) -> (RdJob, Vec<String>) {
    use Command as C;
    let mut warnings = Vec::new();
    let mut layers: BTreeMap<u8, LayerAcc> = BTreeMap::new();
    let mut part: u8 = 0;
    let mut pos: [i64; 2] = [0, 0];
    let mut current: Option<Vec<[i64; 2]>> = None;
    let mut out_of_range = false;

    fn flush(layers: &mut BTreeMap<u8, LayerAcc>, part: u8, current: &mut Option<Vec<[i64; 2]>>) {
        if let Some(pl) = current.take()
            && pl.len() >= 2
        {
            let acc = layers.entry(part).or_default();
            acc.seen = true;
            acc.layer.polylines.push(pl.iter().map(|p| [um_f(p[0]), um_f(p[1])]).collect());
        }
    }

    for c in cmds {
        let target = match c {
            C::MoveAbs { x, y } | C::CutAbs { x, y } => Some([i64::from(*x), i64::from(*y)]),
            C::MoveRel { dx, dy } | C::CutRel { dx, dy } => Some([pos[0] + i64::from(*dx), pos[1] + i64::from(*dy)]),
            C::MoveRelX { dx } | C::CutRelX { dx } => Some([pos[0] + i64::from(*dx), pos[1]]),
            C::MoveRelY { dy } | C::CutRelY { dy } => Some([pos[0], pos[1] + i64::from(*dy)]),
            _ => None,
        };
        if let Some(t) = target {
            if t.iter().any(|v| i32::try_from(*v).is_err()) {
                out_of_range = true;
            }
            if c.is_cut() {
                let pl = current.get_or_insert_with(|| vec![pos]);
                pl.push(t);
            } else {
                flush(&mut layers, part, &mut current);
            }
            pos = t;
            continue;
        }
        match c {
            C::Byte { kind: ByteKind::LayerNumber, value } => {
                flush(&mut layers, part, &mut current);
                part = *value;
                layers.entry(part).or_default().seen = true;
            }
            C::BlockEnd | C::LayerFlag(LayerFlag::End) | C::EndOfFile => flush(&mut layers, part, &mut current),
            C::SpeedPart { part: p, um_per_s } => {
                let a = layers.entry(*p).or_default();
                a.seen = true;
                a.layer.speed_mm_s = f64::from(*um_per_s) / 1000.0;
            }
            C::Speed { kind: SpeedKind::Laser1, um_per_s } => {
                layers.entry(part).or_default().layer.speed_mm_s = f64::from(*um_per_s) / 1000.0;
            }
            C::PartPower { max, laser, part: p, power } => {
                let a = layers.entry(*p).or_default();
                a.seen = true;
                set_power(&mut a.layer, *max, *laser, *power);
            }
            C::Power { kind: kind @ (PowerKind::Min | PowerKind::Max), laser, power } => {
                set_power(&mut layers.entry(part).or_default().layer, *kind == PowerKind::Max, *laser, *power);
            }
            C::LayerColorPart { part: p, bgr } => {
                let a = layers.entry(*p).or_default();
                a.seen = true;
                a.layer.color = rgb(*bgr);
            }
            C::LayerColor { bgr } => layers.entry(part).or_default().layer.color = rgb(*bgr),
            C::FrequencyPart { laser: 0, part: p, freq } => layers.entry(*p).or_default().layer.frequency = Some(*freq),
            _ => {}
        }
    }
    flush(&mut layers, part, &mut current);
    if out_of_range {
        warnings.push("relative moves left the 32-bit coordinate range".into());
    }
    let job = RdJob { layers: layers.into_values().filter(|a| a.seen).map(|a| a.layer).collect() };
    (job, warnings)
}

#[allow(clippy::cast_precision_loss)] // exact below 2^53 µm
fn um_f(v: i64) -> f64 {
    v as f64 / 1000.0
}

fn set_power(l: &mut RdLayer, max: bool, laser: Laser, p: Power) {
    let k = match laser {
        Laser::L1 => 0,
        Laser::L2 => 1,
        _ => return,
    };
    if max {
        l.max_power[k] = p.percent();
    } else {
        l.min_power[k] = p.percent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_job() -> RdJob {
        RdJob {
            layers: vec![
                RdLayer {
                    color: [255, 0, 0],
                    speed_mm_s: 100.0,
                    min_power: [10.0, 10.0],
                    max_power: [60.0, 55.5],
                    frequency: None,
                    polylines: vec![
                        // A 20 mm square (relative cuts) and a long diagonal (absolute cut).
                        vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0], [10.0, 10.0]],
                        vec![[0.5, 0.5], [400.25, 300.125]],
                    ],
                },
                RdLayer { color: [0, 0, 255], speed_mm_s: 20.5, min_power: [0.0, 0.0], max_power: [100.0, 0.0], frequency: Some(20_000), polylines: vec![] },
                RdLayer {
                    color: [0, 128, 0],
                    speed_mm_s: 1.0,
                    min_power: [5.0, 5.0],
                    max_power: [5.0, 5.0],
                    frequency: None,
                    polylines: vec![vec![[-5.0, -5.0], [-5.0, 3.0], [3.191, 3.0], [-0.001, 3.0]]],
                },
            ],
        }
    }

    /// The job with every value snapped to what the file can hold.
    fn quantized(job: &RdJob) -> RdJob {
        let q = |v: f64| f64::from(mm_to_um(v, "q").unwrap()) / 1000.0;
        let qp = |v: f64| Power::from_percent(v).unwrap().percent();
        RdJob {
            layers: job
                .layers
                .iter()
                .map(|l| RdLayer {
                    color: l.color,
                    speed_mm_s: f64::from(mm_s_to_um_s(l.speed_mm_s, "s").unwrap()) / 1000.0,
                    min_power: l.min_power.map(qp),
                    max_power: l.max_power.map(qp),
                    frequency: l.frequency,
                    polylines: l.polylines.iter().map(|pl| pl.iter().map(|p| [q(p[0]), q(p[1])]).collect()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn write_read_round_trip() {
        let job = sample_job();
        for magic in [DEFAULT_MAGIC, MAGIC_634XG, MAGIC_ALT, 0x00, 0xFF] {
            let opts = WriteOptions { magic, filename: None };
            let bytes = write_rd(&job, &opts).unwrap();
            let read = read_rd(&bytes, Some(magic)).unwrap();
            assert_eq!(read.job, quantized(&job), "magic {magic:#04x}");
            assert_eq!(read.file_sum_ok, Some(true));
            assert_eq!(read.unknown_commands, 0);
            assert!(read.warnings.is_empty(), "{:?}", read.warnings);
            // Writing what was read gives the same bytes.
            assert_eq!(write_rd(&read.job, &opts).unwrap(), bytes);
        }
    }

    #[test]
    fn detects_the_key() {
        let job = sample_job();
        for magic in [DEFAULT_MAGIC, MAGIC_634XG, MAGIC_ALT, 0x42, 0x00] {
            let bytes = write_rd(&job, &WriteOptions { magic, filename: None }).unwrap();
            assert_eq!(detect_magic(&bytes), Some(magic));
            assert_eq!(read_rd(&bytes, None).unwrap().magic, magic);
        }
        assert_eq!(detect_magic(&[]), None);
        assert!(read_rd(&[], None).is_err());
    }

    #[test]
    fn layout_follows_meerk40t() {
        let cmds = job_commands(&sample_job(), &WriteOptions::default()).unwrap();
        assert_eq!(cmds[0], Command::RefPoint(RefPoint::MachineZero));
        assert_eq!(cmds[4], Command::StartProcess);
        // The first move is absolute, later short ones relative and single-axis where possible.
        let moves: Vec<&Command> =
            cmds.iter().filter(|c| matches!(c, Command::MoveAbs { .. } | Command::MoveRel { .. } | Command::MoveRelX { .. } | Command::MoveRelY { .. } | Command::CutAbs { .. } | Command::CutRel { .. } | Command::CutRelX { .. } | Command::CutRelY { .. })).collect();
        assert_eq!(moves[0], &Command::MoveAbs { x: 10_000, y: 10_000 });
        assert_eq!(moves[1], &Command::CutAbs { x: 30_000, y: 10_000 }); // 20 mm does not fit 14 bits
        assert!(moves.contains(&&Command::CutRelX { dx: 8191 }));
        assert!(moves.contains(&&Command::CutRelY { dy: 8000 }));
        assert!(moves.contains(&&Command::CutAbs { x: 400_250, y: 300_125 }));
        let plain = write_rd_plain(&sample_job(), &WriteOptions::default()).unwrap();
        assert_eq!(plain.last(), Some(&0xD7));
        let tail = parse_lenient(&plain);
        let n = tail.len();
        assert!(matches!(tail[n - 2], Command::Value { kind: ValueKind::FileSum, .. }));
    }

    #[test]
    fn filename_and_refusals() {
        let mut job = sample_job();
        let opts = WriteOptions { magic: DEFAULT_MAGIC, filename: Some("PART1".into()) };
        let cmds = job_commands(&job, &opts).unwrap();
        assert_eq!(cmds[0], Command::SetFilename("PART1".into()));
        job.layers[0].speed_mm_s = 0.0;
        assert!(matches!(job_commands(&job, &opts), Err(RuidaError::Zero { .. })));
        let mut job = sample_job();
        job.layers[0].max_power[0] = 101.0;
        assert!(job_commands(&job, &opts).is_err());
        let mut job = sample_job();
        job.layers[0].polylines[0][1][0] = f64::NAN;
        assert!(job_commands(&job, &opts).is_err());
        let job = RdJob { layers: vec![sample_job().layers[1].clone(); MAX_LAYERS + 1] };
        assert!(matches!(job_commands(&job, &opts), Err(RuidaError::TooManyLayers(_))));
        let job = RdJob { layers: vec![sample_job().layers[1].clone(); MAX_LAYERS] };
        assert!(write_rd(&job, &opts).is_ok());
    }

    #[test]
    fn degenerate_and_touching_polylines() {
        let job = RdJob {
            layers: vec![RdLayer {
                color: [1, 2, 3],
                speed_mm_s: 10.0,
                min_power: [1.0, 1.0],
                max_power: [2.0, 2.0],
                frequency: None,
                polylines: vec![
                    vec![[1.0, 1.0]],
                    vec![[2.0, 2.0], [2.0004, 2.0]], // one µm point after rounding
                    vec![[0.0, 0.0], [1.0, 0.0]],
                    vec![[1.0, 0.0], [1.0, 1.0]], // starts where the last ended: joined
                ],
            }],
        };
        let read = read_rd(&write_rd(&job, &WriteOptions::default()).unwrap(), None).unwrap();
        assert_eq!(read.job.layers[0].polylines, vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]]);
    }

    #[test]
    fn empty_job() {
        let bytes = write_rd(&RdJob::default(), &WriteOptions::default()).unwrap();
        let read = read_rd(&bytes, None).unwrap();
        assert!(read.job.layers.is_empty());
        assert_eq!(read.file_sum_ok, Some(true));
    }

    #[test]
    fn reader_tolerates_unknown_and_flags_bad_sum() {
        let mut plain = write_rd_plain(&sample_job(), &WriteOptions::default()).unwrap();
        // Insert an unknown command after the header's first command; the sum no longer matches.
        plain.splice(2..2, [0xE7, 0x46]);
        let bytes = Swizzle::default().scramble(&plain);
        let read = read_rd(&bytes, Some(DEFAULT_MAGIC)).unwrap();
        assert_eq!(read.unknown_commands, 1);
        assert_eq!(read.file_sum_ok, Some(false));
        assert_eq!(read.job, quantized(&sample_job()));
        assert_eq!(read.warnings.len(), 2);
    }

    #[test]
    fn hand_written_stream_geometry() {
        // Cuts before any layer number go to part 0; relative cuts start at the current position; colour per layer.
        use Command as C;
        let cmds = vec![
            C::MoveAbs { x: 1000, y: 2000 },
            C::CutRelX { dx: 500 },
            C::CutRel { dx: 0, dy: 500 },
            C::MoveRelY { dy: -100 },
            C::Byte { kind: ByteKind::LayerNumber, value: 3 },
            C::LayerColor { bgr: 0x00FF_0000 },
            C::Speed { kind: SpeedKind::Laser1, um_per_s: 5000 },
            C::Power { kind: PowerKind::Max, laser: Laser::L1, power: Power::FULL },
            C::CutAbs { x: 0, y: 0 },
            C::EndOfFile,
        ];
        let (job, w) = job_from_commands(&cmds);
        assert!(w.is_empty());
        assert_eq!(job.layers.len(), 2);
        assert_eq!(job.layers[0].polylines, vec![vec![[1.0, 2.0], [1.5, 2.0], [1.5, 2.5]]]);
        assert_eq!(job.layers[1].polylines, vec![vec![[1.5, 2.4], [0.0, 0.0]]]);
        assert_eq!(job.layers[1].color, [0, 0, 255]);
        assert_eq!(job.layers[1].speed_mm_s, 5.0);
        assert_eq!(job.layers[1].max_power[0], 100.0);
    }

    /// Real `.rd` files are never committed. Point `OPENETCH_RD_SAMPLES` at a folder of them to check the reader:
    /// every file must detect a key, parse and give geometry.
    #[test]
    fn real_samples_env_gated() {
        let Some(dir) = std::env::var_os("OPENETCH_RD_SAMPLES") else { return };
        let mut n = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("rd")) {
                let data = std::fs::read(&path).unwrap();
                let read = read_rd(&data, None).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                let points: usize = read.job.layers.iter().flat_map(|l| &l.polylines).map(Vec::len).sum();
                eprintln!(
                    "{}: key {:#04x}, {} commands ({} unknown), {} layers, {points} points, sum ok {:?}, warnings {:?}",
                    path.display(),
                    read.magic,
                    read.commands.len(),
                    read.unknown_commands,
                    read.job.layers.len(),
                    read.file_sum_ok,
                    read.warnings
                );
                n += 1;
            }
        }
        assert!(n > 0, "OPENETCH_RD_SAMPLES has no .rd files");
    }
}
