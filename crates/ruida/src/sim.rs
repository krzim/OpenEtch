//! A Ruida controller simulator for tests (feature `sim`; always on for this crate's own tests).
//!
//! It takes UDP datagrams ([`Simulator::handle_udp`]) or serial bytes ([`Simulator::handle_serial`]), answers the
//! way [`crate::frame`] describes (ACK per datagram, NAK on a bad checksum as MeerK40t's emulator answers, data
//! replies for memory reads and document names), runs the commands, tracks the head position (X, Y, Z, U in µm) and
//! records every XY segment with whether the laser would be on: a cut (`A8`..`AB`) while any min or max power of
//! laser 1 or 2 is non-zero (min power is what the controller uses at corners and low speed, so a zero max power
//! alone does not keep the laser off). It keeps stored documents (a job that starts with `E7 01 name` is stored, not
//! run) for the controller file list; a stored document that starts a stored document does not run it (no
//! recursion).
//!
//! It models what MeerK40t's emulator and client agree on; nothing here was checked against a real controller. The
//! test hooks [`Simulator::refuse_next`] (answer ERR, as a busy controller might) and [`Simulator::lose_next_acks`]
//! (run the datagram but lose its ACK) are OpenEtch's own.

// Behaviour after meerk40t/meerk40t @ d511944 `meerk40t/ruida/emulator.py` (`checksum_write`, `_process_realtime`,
// `mem_lookup`, the `E8` document commands) (MIT License, Copyright (c) 2021 meerk40t; full notice in `swizzle.rs`).
// The code is OpenEtch's own.

use std::collections::BTreeMap;

use crate::command::{Axis, ByteKind, Command, DocKind, Laser, PowerKind, SpeedKind, parse_lenient};
use crate::frame::{Reply, unframe_udp};
use crate::mem::{self, MemAddr};
use crate::num::{Power, U35_MAX};
use crate::swizzle::Swizzle;

/// One straight XY motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimSegment {
    pub from: [i64; 2],
    pub to: [i64; 2],
    /// A cut command (the laser would be on if `power` > 0).
    pub cut: bool,
    pub part: u8,
    pub speed_um_s: u32,
    /// The highest of the laser 1 and laser 2 min and max powers in force.
    pub power: Power,
}

impl SimSegment {
    pub fn laser_on(&self) -> bool {
        self.cut && self.power > Power::ZERO
    }
}

/// A job stored on the simulated controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimDocument {
    pub name: String,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Copy, Default)]
struct PartSettings {
    speed: u32,
    /// Min and max power of lasers 1 and 2 from the header (`C6 31/41/32/42`).
    min: [Power; 2],
    max: [Power; 2],
}

/// The simulated controller.
#[derive(Debug, Clone)]
pub struct Simulator {
    swz: Swizzle,
    /// X, Y, Z, U in µm.
    pub pos: [i64; 4],
    pub part: u8,
    pub speed_um_s: u32,
    /// Min and max power of lasers 1 and 2.
    pub min_power: [Power; 2],
    pub max_power: [Power; 2],
    pub running: bool,
    pub paused: bool,
    /// The last command run was a cut with power: the laser is on right now.
    pub laser_on: bool,
    pub segments: Vec<SimSegment>,
    /// Every command received, in order (stored ones too).
    pub log: Vec<Command>,
    pub memory: BTreeMap<MemAddr, u64>,
    pub documents: Vec<SimDocument>,
    pub jobs_finished: usize,
    pub datagrams: usize,
    pub checksum_errors: usize,
    /// Refuse (ERR) this many of the next datagrams without running them, as a busy controller would.
    pub refuse_next: usize,
    /// Run this many of the next datagrams but lose their ACK (a dropped reply datagram).
    pub lose_next_acks: usize,
    receiving: Option<SimDocument>,
    /// A stored document is being run (one that starts another is not followed).
    running_document: bool,
    parts: BTreeMap<u8, PartSettings>,
}

impl Simulator {
    /// A controller with key `magic`, a 600 x 400 mm bed and an RDC6442S card ID.
    pub fn new(magic: u8) -> Self {
        let mut memory = BTreeMap::new();
        memory.insert(mem::CARD_ID, mem::CARD_ID_RDC6442S);
        memory.insert(mem::BED_SIZE_X, 600_000);
        memory.insert(mem::BED_SIZE_Y, 400_000);
        Simulator {
            swz: Swizzle::new(magic),
            pos: [0; 4],
            part: 0,
            speed_um_s: 0,
            min_power: [Power::ZERO; 2],
            max_power: [Power::ZERO; 2],
            running: false,
            paused: false,
            laser_on: false,
            segments: Vec::new(),
            log: Vec::new(),
            memory,
            documents: Vec::new(),
            jobs_finished: 0,
            datagrams: 0,
            checksum_errors: 0,
            refuse_next: 0,
            lose_next_acks: 0,
            receiving: None,
            running_document: false,
            parts: BTreeMap::new(),
        }
    }

    pub fn swizzle(&self) -> &Swizzle {
        &self.swz
    }

    /// One datagram from the host; returns the datagrams sent back (scrambled): ACK (NAK on a bad checksum, ERR when
    /// refusing), then any data replies.
    pub fn handle_udp(&mut self, datagram: &[u8]) -> Vec<Vec<u8>> {
        self.datagrams += 1;
        if self.refuse_next > 0 {
            self.refuse_next -= 1;
            return vec![self.swz.scramble(&[0xCD])];
        }
        let Ok(plain) = unframe_udp(&self.swz, datagram) else {
            self.checksum_errors += 1;
            // MeerK40t's emulator answers a checksum mismatch with NAK (`CF`).
            return vec![self.swz.scramble(&[0xCF])];
        };
        let mut out = vec![self.swz.scramble(&[0xCC])];
        for cmd in parse_lenient(&plain) {
            if let Some(r) = self.execute(&cmd)
                && let Ok(w) = r.to_wire(&self.swz)
            {
                out.push(w);
            }
        }
        if self.lose_next_acks > 0 {
            self.lose_next_acks -= 1;
            out.remove(0);
        }
        out
    }

    /// Serial bytes from the host (whole commands); returns the scrambled data replies (no ACKs on serial).
    pub fn handle_serial(&mut self, bytes: &[u8]) -> Vec<u8> {
        let plain = self.swz.unscramble(bytes);
        let mut out = Vec::new();
        for cmd in parse_lenient(&plain) {
            if let Some(r) = self.execute(&cmd)
                && let Ok(w) = r.to_wire(&self.swz)
            {
                out.extend(w);
            }
        }
        out
    }

    /// Run one command; returns a data reply if it asks for one.
    pub fn execute(&mut self, cmd: &Command) -> Option<Reply> {
        use Command as C;
        self.log.push(cmd.clone());
        if let Some(doc) = &mut self.receiving {
            doc.commands.push(cmd.clone());
            if matches!(cmd, C::EndOfFile) {
                let doc = self.receiving.take().unwrap_or_else(|| SimDocument { name: String::new(), commands: vec![] });
                self.documents.push(doc);
            }
            return None;
        }
        self.laser_on = false;
        let [x, y, _, _] = self.pos;
        let xy = match cmd {
            C::MoveAbs { x: nx, y: ny } | C::CutAbs { x: nx, y: ny } => Some([i64::from(*nx), i64::from(*ny)]),
            C::MoveRel { dx, dy } | C::CutRel { dx, dy } => Some([x + i64::from(*dx), y + i64::from(*dy)]),
            C::MoveRelX { dx } | C::CutRelX { dx } => Some([x + i64::from(*dx), y]),
            C::MoveRelY { dy } | C::CutRelY { dy } => Some([x, y + i64::from(*dy)]),
            C::AxisMove { axis: Axis::X, pos } | C::RapidMove { axis: Axis::X, pos, .. } => Some([i64::from(*pos), y]),
            C::AxisMove { axis: Axis::Y, pos } | C::RapidMove { axis: Axis::Y, pos, .. } => Some([x, i64::from(*pos)]),
            C::RapidMoveXy { x: nx, y: ny, .. } => Some([i64::from(*nx), i64::from(*ny)]),
            C::RapidMoveXyu { x: nx, y: ny, u, .. } => {
                self.pos[3] = i64::from(*u);
                Some([i64::from(*nx), i64::from(*ny)])
            }
            _ => None,
        };
        if let Some(to) = xy {
            let seg = SimSegment {
                from: [x, y],
                to,
                cut: cmd.is_cut(),
                part: self.part,
                speed_um_s: self.speed_um_s,
                power: self.min_power.into_iter().chain(self.max_power).max().unwrap_or(Power::ZERO),
            };
            self.laser_on = seg.laser_on();
            self.segments.push(seg);
            self.pos[0] = to[0];
            self.pos[1] = to[1];
            return None;
        }
        match cmd {
            C::AxisMove { axis: Axis::Z, pos } | C::RapidMove { axis: Axis::Z, pos, .. } => self.pos[2] = i64::from(*pos),
            C::AxisMove { axis: Axis::U, pos } | C::RapidMove { axis: Axis::U, pos, .. } => self.pos[3] = i64::from(*pos),
            C::HomeXy => {
                self.pos[0] = 0;
                self.pos[1] = 0;
            }
            C::HomeZ => self.pos[2] = 0,
            C::HomeU => self.pos[3] = 0,
            C::Speed { kind: SpeedKind::Laser1, um_per_s } => self.speed_um_s = *um_per_s,
            C::SpeedPart { part, um_per_s } => self.parts.entry(*part).or_default().speed = *um_per_s,
            C::PartPower { max, laser: laser @ (Laser::L1 | Laser::L2), part, power } => {
                let p = self.parts.entry(*part).or_default();
                let k = usize::from(*laser == Laser::L2);
                if *max {
                    p.max[k] = *power;
                } else {
                    p.min[k] = *power;
                }
            }
            C::Power { kind: kind @ (PowerKind::Min | PowerKind::Max), laser: laser @ (Laser::L1 | Laser::L2), power } => {
                let k = usize::from(*laser == Laser::L2);
                if *kind == PowerKind::Max {
                    self.max_power[k] = *power;
                } else {
                    self.min_power[k] = *power;
                }
            }
            C::Byte { kind: ByteKind::LayerNumber, value } => {
                self.part = *value;
                if let Some(p) = self.parts.get(value) {
                    self.speed_um_s = p.speed;
                    self.min_power = p.min;
                    self.max_power = p.max;
                }
            }
            C::StartProcess => {
                self.running = true;
                self.paused = false;
            }
            C::StopProcess => {
                self.running = false;
                self.paused = false;
            }
            C::PauseProcess => self.paused = true,
            C::RestoreProcess => self.paused = false,
            C::EndOfFile => {
                if self.running {
                    self.jobs_finished += 1;
                }
                self.running = false;
            }
            C::GetMemory(addr) => {
                return Some(Reply::Memory { addr: *addr, value: self.read_memory(*addr), second: None });
            }
            C::SetMemory { addr, v0, .. } => {
                self.memory.insert(*addr, *v0);
            }
            C::SetFilename(name) => {
                self.receiving = Some(SimDocument { name: name.clone(), commands: Vec::new() });
            }
            C::Document { kind: DocKind::Name, index } => {
                let doc = usize::from(*index).checked_sub(1).and_then(|i| self.documents.get(i))?;
                return Some(Reply::DocumentName { index: *index, name: doc.name.clone() });
            }
            C::Document { kind: DocKind::Start, index } => {
                if self.running_document {
                    return None;
                }
                let doc = usize::from(*index).checked_sub(1).and_then(|i| self.documents.get(i))?.clone();
                self.running_document = true;
                for c in &doc.commands {
                    self.execute(c);
                }
                self.running_document = false;
            }
            C::DeleteDocument { index, .. } => {
                if *index == 0 {
                    self.documents.clear();
                } else if usize::from(*index) <= self.documents.len() {
                    self.documents.remove(usize::from(*index) - 1);
                }
            }
            _ => {}
        }
        None
    }

    /// A register's current value.
    pub fn read_memory(&self, addr: MemAddr) -> u64 {
        let pos = |v: i64| (v & 0xFFFF_FFFF).cast_unsigned() & U35_MAX;
        match addr {
            mem::MACHINE_STATUS => {
                let mut s = 0;
                if self.running {
                    s |= mem::STATUS_JOB_RUNNING;
                }
                s
            }
            mem::POSITION_X => pos(self.pos[0]),
            mem::POSITION_Y => pos(self.pos[1]),
            mem::POSITION_Z => pos(self.pos[2]),
            mem::POSITION_U => pos(self.pos[3]),
            mem::TOTAL_DOC_NUMBER => u64::try_from(self.documents.len()).unwrap_or(U35_MAX),
            a => self.memory.get(&a).copied().unwrap_or(0),
        }
    }

    /// Whether any segment so far would have fired the laser.
    pub fn laser_fired(&self) -> bool {
        self.segments.iter().any(SimSegment::laser_on)
    }

    /// The segments that would have fired the laser.
    pub fn burns(&self) -> impl Iterator<Item = &SimSegment> {
        self.segments.iter().filter(|s| s.laser_on())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{frame_udp, frame_serial, packetize};
    use crate::rd::{RdJob, RdLayer, WriteOptions, write_rd_plain};
    use crate::swizzle::DEFAULT_MAGIC;

    fn send_all(sim: &mut Simulator, plain: &[u8]) -> Vec<Reply> {
        let swz = sim.swizzle().clone();
        let mut replies = Vec::new();
        for p in packetize(plain, 1470).unwrap() {
            let out = sim.handle_udp(&frame_udp(&swz, &p).unwrap());
            assert_eq!(Reply::from_wire(&swz, &out[0]), Reply::Ack);
            replies.extend(out[1..].iter().map(|r| Reply::from_wire(&swz, r)));
        }
        replies
    }

    fn square_job(power: f64) -> RdJob {
        RdJob {
            layers: vec![RdLayer {
                color: [0, 0, 0],
                speed_mm_s: 50.0,
                min_power: [power, 0.0],
                max_power: [power, 0.0],
                frequency: None,
                polylines: vec![vec![[10.0, 10.0], [12.0, 10.0], [12.0, 12.0], [10.0, 12.0], [10.0, 10.0]]],
            }],
        }
    }

    #[test]
    fn runs_a_job_over_udp() {
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        let plain = write_rd_plain(&square_job(30.0), &WriteOptions::default()).unwrap();
        assert!(send_all(&mut sim, &plain).is_empty());
        assert_eq!(sim.jobs_finished, 1);
        assert!(!sim.running);
        assert_eq!(sim.pos[..2], [10_000, 10_000]);
        let burns: Vec<_> = sim.burns().collect();
        assert_eq!(burns.len(), 4);
        assert_eq!(burns[0].from, [10_000, 10_000]);
        assert_eq!(burns[0].to, [12_000, 10_000]);
        assert_eq!(burns[0].speed_um_s, 50_000);
        assert!((burns[0].power.percent() - 30.0).abs() < 0.01);
        // The first segment is the travel move, laser off.
        assert!(!sim.segments[0].cut);
        assert!(!sim.laser_on, "laser off after the last move");
    }

    #[test]
    fn zero_power_job_never_fires() {
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        send_all(&mut sim, &write_rd_plain(&square_job(0.0), &WriteOptions::default()).unwrap());
        assert!(!sim.laser_fired());
        assert_eq!(sim.segments.iter().filter(|s| s.cut).count(), 4);
    }

    #[test]
    fn checksum_errors_get_nak_and_refusals_err() {
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        let swz = Swizzle::default();
        let mut pkt = frame_udp(&swz, &Command::MoveAbs { x: 5, y: 5 }.encode().unwrap()).unwrap();
        pkt[0] ^= 0xFF;
        // A checksum mismatch gets NAK (CF, 0x48 on the wire), as from MeerK40t's emulator.
        assert_eq!(sim.handle_udp(&pkt), vec![vec![0x48]]);
        assert_eq!(sim.checksum_errors, 1);
        assert!(sim.segments.is_empty());
        sim.refuse_next = 1;
        pkt[0] ^= 0xFF;
        assert_eq!(sim.handle_udp(&pkt), vec![vec![0x46]]);
        assert_eq!(sim.handle_udp(&pkt), vec![vec![0xC6]]);
        assert_eq!(sim.pos[..2], [5, 5]);
    }

    #[test]
    fn memory_reads_and_keepalive() {
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        let swz = sim.swizzle().clone();
        // Keepalive: ACK only.
        assert_eq!(sim.handle_udp(&frame_udp(&swz, &[0xCE]).unwrap()), vec![vec![0xC6]]);
        let cmds = [
            Command::MoveAbs { x: -1234, y: 5678 },
            Command::AxisMove { axis: Axis::Z, pos: 300 },
            Command::RapidMove { axis: Axis::U, option: crate::command::RapidOption::None, pos: -7 },
            Command::GetMemory(mem::POSITION_X),
            Command::GetMemory(mem::POSITION_Y),
            Command::GetMemory(mem::POSITION_Z),
            Command::GetMemory(mem::POSITION_U),
            Command::GetMemory(mem::CARD_ID),
            Command::SetMemory { addr: mem::G0_VELOCITY, v0: 200_000, v1: 200_000 },
            Command::GetMemory(mem::G0_VELOCITY),
            Command::GetMemory(mem::BED_SIZE_X),
        ];
        let replies = send_all(&mut sim, &crate::command::encode_all(&cmds).unwrap());
        let values: Vec<u64> = replies
            .iter()
            .map(|r| match r {
                Reply::Memory { value, .. } => *value,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(mem::position_um(values[0]), -1234);
        assert_eq!(mem::position_um(values[1]), 5678);
        assert_eq!(mem::position_um(values[2]), 300);
        assert_eq!(mem::position_um(values[3]), -7);
        assert_eq!(values[4], mem::CARD_ID_RDC6442S);
        assert_eq!(values[5], 200_000);
        assert_eq!(values[6], 600_000);
        // Status shows a running job between start and end of file.
        sim.execute(&Command::StartProcess);
        assert_eq!(sim.read_memory(mem::MACHINE_STATUS) & mem::STATUS_JOB_RUNNING, mem::STATUS_JOB_RUNNING);
        sim.execute(&Command::StopProcess);
        assert_eq!(sim.read_memory(mem::MACHINE_STATUS), 0);
    }

    #[test]
    fn stored_documents_file_list() {
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        for name in ["FIRST", "SECOND"] {
            let opts = WriteOptions { magic: DEFAULT_MAGIC, filename: Some(name.into()) };
            send_all(&mut sim, &write_rd_plain(&square_job(20.0), &opts).unwrap());
        }
        // Stored, not run.
        assert!(sim.segments.is_empty());
        assert_eq!(sim.jobs_finished, 0);
        let ask = [
            Command::GetMemory(mem::TOTAL_DOC_NUMBER),
            Command::Document { kind: DocKind::Name, index: 1 },
            Command::Document { kind: DocKind::Name, index: 2 },
        ];
        let replies = send_all(&mut sim, &crate::command::encode_all(&ask).unwrap());
        assert_eq!(replies[0], Reply::Memory { addr: mem::TOTAL_DOC_NUMBER, value: 2, second: None });
        assert_eq!(replies[1], Reply::DocumentName { index: 1, name: "FIRST".into() });
        assert_eq!(replies[2], Reply::DocumentName { index: 2, name: "SECOND".into() });
        // Run the second one, then delete the first.
        send_all(&mut sim, &Command::Document { kind: DocKind::Start, index: 2 }.encode().unwrap());
        assert_eq!(sim.jobs_finished, 1);
        assert!(sim.laser_fired());
        send_all(&mut sim, &Command::DeleteDocument { index: 1, other: 0 }.encode().unwrap());
        assert_eq!(sim.documents.len(), 1);
        assert_eq!(sim.documents[0].name, "SECOND");
        // Unknown index: no reply.
        assert!(send_all(&mut sim, &Command::Document { kind: DocKind::Name, index: 9 }.encode().unwrap()).is_empty());
        send_all(&mut sim, &Command::DeleteDocument { index: 0, other: 0 }.encode().unwrap());
        assert!(sim.documents.is_empty());
    }

    #[test]
    fn min_power_or_laser_2_alone_fires() {
        use crate::command::Command as C;
        let swz = Swizzle::default();
        let cut = crate::command::encode_all(&[C::MoveAbs { x: 0, y: 0 }, C::CutAbs { x: 1000, y: 0 }]).unwrap();
        let five = Power::from_percent(5.0).unwrap();
        // Max power 0 but min power 5 %: the controller can fire at min power (corners, low speed).
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        sim.execute(&C::Power { kind: PowerKind::Min, laser: Laser::L1, power: five });
        sim.handle_serial(&frame_serial(&swz, &cut));
        assert!(sim.laser_fired());
        // Laser 2 only.
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        sim.execute(&C::Power { kind: PowerKind::Max, laser: Laser::L2, power: five });
        sim.handle_serial(&frame_serial(&swz, &cut));
        assert!(sim.laser_fired());
        // A layer's header min power comes back with its layer number.
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        sim.execute(&C::PartPower { max: false, laser: Laser::L1, part: 2, power: five });
        sim.execute(&C::Byte { kind: ByteKind::LayerNumber, value: 2 });
        sim.handle_serial(&frame_serial(&swz, &cut));
        assert_eq!(sim.burns().count(), 1);
    }

    #[test]
    fn a_document_that_starts_itself_runs_once() {
        // Stored: "LOOP" = E8 03 1, start, a cut, D7; "OTHER" = E8 03 1. This overflowed the stack before.
        use crate::command::Command as C;
        let mut sim = Simulator::new(DEFAULT_MAGIC);
        sim.execute(&C::Power { kind: PowerKind::Max, laser: Laser::L1, power: Power::FULL });
        for c in [
            C::SetFilename("LOOP".into()),
            C::Document { kind: DocKind::Start, index: 1 },
            C::StartProcess,
            C::CutAbs { x: 5, y: 5 },
            C::EndOfFile,
            C::SetFilename("OTHER".into()),
            C::Document { kind: DocKind::Start, index: 1 },
            C::EndOfFile,
        ] {
            sim.execute(&c);
        }
        assert_eq!(sim.documents.len(), 2);
        sim.execute(&C::Document { kind: DocKind::Start, index: 1 });
        sim.execute(&C::Document { kind: DocKind::Start, index: 2 });
        assert_eq!(sim.burns().count(), 1);
        assert_eq!(sim.jobs_finished, 1);
    }

    #[test]
    fn serial_has_no_acks() {
        let mut sim = Simulator::new(0x11);
        let swz = Swizzle::new(0x11);
        assert!(sim.handle_serial(&frame_serial(&swz, &[0xCE])).is_empty());
        let out = sim.handle_serial(&frame_serial(&swz, &Command::GetMemory(mem::MACHINE_STATUS).encode().unwrap()));
        assert_eq!(Reply::from_wire(&swz, &out), Reply::Memory { addr: mem::MACHINE_STATUS, value: 0, second: None });
        sim.handle_serial(&frame_serial(&swz, &write_rd_plain(&square_job(10.0), &WriteOptions::default()).unwrap()));
        assert_eq!(sim.jobs_finished, 1);
        assert!(sim.laser_fired());
    }
}
