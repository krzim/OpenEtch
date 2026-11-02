//! Commands: typed encode/decode of the unscrambled command stream.
//!
//! A command is an opcode byte with bit 7 set, sometimes a sub-opcode, then parameters whose bytes all have bit 7
//! clear (see [`crate::num`]). A stream is split at every byte with bit 7 set ([`split`]). Units on the wire: µm for
//! coordinates, µm/s for speeds, µs for delays, 14-bit steps for power ([`Power`]).
//!
//! The full list, with its source, is in the crate documentation ("Command reference"). Opcodes this crate does not
//! type decode as [`Command::Unknown`] and re-encode byte for byte.

// Opcodes, parameter layouts and units from meerk40t/meerk40t @ d511944 `meerk40t/ruida/rdjob.py` (constants,
// `RDJob.process`, the encoder methods) and `meerk40t/ruida/emulator.py` (`_process_realtime`), re-written in Rust
// (MIT License, Copyright (c) 2021 meerk40t; full notice in `swizzle.rs`).

use serde::{Deserialize, Serialize};

use crate::mem::MemAddr;
use crate::num::{
    Power, decode_abs, decode_i14, decode_u14, decode_u32, decode_u35, encode_abs, encode_i14, encode_u14, encode_u32,
    encode_u35,
};
use crate::{Result, RuidaError};

/// One of the (up to four) laser sources a power command addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Laser {
    L1,
    L2,
    L3,
    L4,
}

impl Laser {
    pub const ALL: [Laser; 4] = [Laser::L1, Laser::L2, Laser::L3, Laser::L4];

    fn index(self) -> usize {
        match self {
            Laser::L1 => 0,
            Laser::L2 => 1,
            Laser::L3 => 2,
            Laser::L4 => 3,
        }
    }
}

/// Which power value a [`Command::Power`] sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerKind {
    /// Immediate power (`C7`, `C0`, `C2`, `C3`).
    Immediate,
    /// End power (`C8`, `C1`, `C4`, `C5`).
    End,
    /// Minimum power of the current layer (`C6 01/21/05/07`): used at corners and low speed.
    Min,
    /// Maximum power of the current layer (`C6 02/22/06/08`).
    Max,
    /// Power during the laser on/off delays (`C6 50/51/55/56`).
    Through,
}

/// Opcode bytes per laser, from rdjob.py.
const POWER_OPS: [(PowerKind, [&[u8]; 4]); 5] = [
    (PowerKind::Immediate, [&[0xC7], &[0xC0], &[0xC2], &[0xC3]]),
    (PowerKind::End, [&[0xC8], &[0xC1], &[0xC4], &[0xC5]]),
    (PowerKind::Min, [&[0xC6, 0x01], &[0xC6, 0x21], &[0xC6, 0x05], &[0xC6, 0x07]]),
    (PowerKind::Max, [&[0xC6, 0x02], &[0xC6, 0x22], &[0xC6, 0x06], &[0xC6, 0x08]]),
    (PowerKind::Through, [&[0xC6, 0x50], &[0xC6, 0x51], &[0xC6, 0x55], &[0xC6, 0x56]]),
];
/// Per-layer ("part") min and max power: `C6 xx part power`.
const PART_POWER_OPS: [(bool, [u8; 4]); 2] = [(false, [0x31, 0x41, 0x35, 0x37]), (true, [0x32, 0x42, 0x36, 0x38])];

fn power_op(kind: PowerKind, laser: Laser) -> &'static [u8] {
    let row = POWER_OPS.iter().find(|(k, _)| *k == kind).map_or(&POWER_OPS[0].1, |(_, r)| r);
    row[laser.index()]
}

/// The delays and intervals of `C6 1x` (µs on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DelayKind {
    LaserInterval,
    AddDelay,
    LaserOnDelay,
    LaserOffDelay,
    LaserOnDelay2,
    LaserOffDelay2,
}

const DELAY_OPS: [(DelayKind, u8); 6] = [
    (DelayKind::LaserInterval, 0x10),
    (DelayKind::AddDelay, 0x11),
    (DelayKind::LaserOnDelay, 0x12),
    (DelayKind::LaserOffDelay, 0x13),
    (DelayKind::LaserOnDelay2, 0x15),
    (DelayKind::LaserOffDelay2, 0x16),
];

/// The speeds of `C9 xx` (µm/s on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpeedKind {
    /// Cutting speed of the current layer (`C9 02`).
    Laser1,
    /// Axis speed (`C9 03`).
    Axis,
    /// Forced engraving speed (`C9 05`).
    ForceEngraving,
    /// Axis move speed (`C9 06`).
    AxisMove,
}

const SPEED_OPS: [(SpeedKind, u8); 4] =
    [(SpeedKind::Laser1, 0x02), (SpeedKind::Axis, 0x03), (SpeedKind::ForceEngraving, 0x05), (SpeedKind::AxisMove, 0x06)];

/// Axes of the single-axis moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
    U,
}

/// `80 00` X, `A0 00` Y, `80 08` Z, `A0 08` U: as MeerK40t's decoder reads them (its encoder constants disagree for
/// Z; unverified on hardware).
const AXIS_MOVE_OPS: [(Axis, [u8; 2]); 4] =
    [(Axis::X, [0x80, 0x00]), (Axis::Y, [0xA0, 0x00]), (Axis::Z, [0x80, 0x08]), (Axis::U, [0xA0, 0x08])];

const RAPID_AXIS_OPS: [(Axis, u8); 4] = [(Axis::X, 0x00), (Axis::Y, 0x01), (Axis::Z, 0x02), (Axis::U, 0x03)];

/// The option byte of the `D9` rapid moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RapidOption {
    /// `0x00`: relative to the origin.
    Origin,
    /// `0x01`: red light on, relative to the origin.
    LightOrigin,
    /// `0x02`: nothing.
    None,
    /// `0x03`: red light on.
    Light,
}

impl RapidOption {
    fn byte(self) -> u8 {
        match self {
            RapidOption::Origin => 0x00,
            RapidOption::LightOrigin => 0x01,
            RapidOption::None => 0x02,
            RapidOption::Light => 0x03,
        }
    }

    fn from_byte(b: u8) -> Option<Self> {
        Some(match b {
            0x00 => RapidOption::Origin,
            0x01 => RapidOption::LightOrigin,
            0x02 => RapidOption::None,
            0x03 => RapidOption::Light,
            _ => return None,
        })
    }
}

/// `D8 10/11/12`: which reference point coordinates are relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefPoint {
    /// `D8 10`: machine zero (absolute).
    MachineZero,
    /// `D8 11`: the anchor point (origin).
    Anchor,
    /// `D8 12`: the current position.
    Current,
}

/// Jog keys `D8 20..27` (pressed) and `D8 30..37` (released).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum JogKey {
    XLeft,
    XRight,
    YTop,
    YBottom,
    ZUp,
    ZDown,
    UForward,
    UBackward,
}

const JOG_KEYS: [JogKey; 8] = [
    JogKey::XLeft,
    JogKey::XRight,
    JogKey::YTop,
    JogKey::YBottom,
    JogKey::ZUp,
    JogKey::ZDown,
    JogKey::UForward,
    JogKey::UBackward,
];

/// `CA 01 xx` layer flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LayerFlag {
    /// `CA 01 00`: end of the layer's settings and moves.
    End,
    /// `CA 01 01/02/03/04/05/55`: work mode (the raw sub-byte; meaning unknown).
    WorkMode(u8),
    /// `CA 01 10/11`: laser device 0 or 1.
    LaserDevice(bool),
    /// `CA 01 12/13`: air assist off/on.
    AirAssist(bool),
    /// `CA 01 14`: dual head.
    DualHead,
    /// `CA 01 30/31`: laser 2 offset off/on.
    Laser2Offset(bool),
}

const WORK_MODES: [u8; 6] = [0x01, 0x02, 0x03, 0x04, 0x05, 0x55];

/// Commands with a single byte parameter (`op sub value`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ByteKind {
    /// `CA 02 part`: the layer the following settings and moves belong to.
    LayerNumber,
    /// `CA 03 v`: enable laser tube start.
    EnLaserTubeStart,
    /// `CA 04 v`: X sign map.
    XSignMap,
    /// `CA 10 v`: enable external IO.
    EnExIo,
    /// `CA 22 part`: the highest layer number in the file.
    MaxLayer,
    /// `CA 40 v`: Z/U map.
    ZuMap,
    /// `E7 05 v`
    ArrayDirection,
    /// `E7 0B v`
    ArrayEnMirrorCut,
    /// `E7 24 v`
    ArrayMirror,
    /// `E7 38 v`
    SetFeedAutoPause,
    /// `E7 60 v`
    SetCurrentElementIndex,
    /// `F1 00 v`
    ElementMaxIndex,
    /// `F1 01 v`
    ElementNameMaxIndex,
    /// `F1 02 v`
    EnableBlockCutting,
    /// `F1 04 v`
    FeedAutoCalc,
    /// `F2 00 v`
    ElementIndex,
    /// `F2 07 v`
    ElementArrayMirror,
}

const BYTE_OPS: [(ByteKind, [u8; 2]); 17] = [
    (ByteKind::LayerNumber, [0xCA, 0x02]),
    (ByteKind::EnLaserTubeStart, [0xCA, 0x03]),
    (ByteKind::XSignMap, [0xCA, 0x04]),
    (ByteKind::EnExIo, [0xCA, 0x10]),
    (ByteKind::MaxLayer, [0xCA, 0x22]),
    (ByteKind::ZuMap, [0xCA, 0x40]),
    (ByteKind::ArrayDirection, [0xE7, 0x05]),
    (ByteKind::ArrayEnMirrorCut, [0xE7, 0x0B]),
    (ByteKind::ArrayMirror, [0xE7, 0x24]),
    (ByteKind::SetFeedAutoPause, [0xE7, 0x38]),
    (ByteKind::SetCurrentElementIndex, [0xE7, 0x60]),
    (ByteKind::ElementMaxIndex, [0xF1, 0x00]),
    (ByteKind::ElementNameMaxIndex, [0xF1, 0x01]),
    (ByteKind::EnableBlockCutting, [0xF1, 0x02]),
    (ByteKind::FeedAutoCalc, [0xF1, 0x04]),
    (ByteKind::ElementIndex, [0xF2, 0x00]),
    (ByteKind::ElementArrayMirror, [0xF2, 0x07]),
];

/// Commands with one point (two absolute coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointKind {
    /// `E7 03`
    ProcessTopLeft,
    /// `E7 07`
    ProcessBottomRight,
    /// `E7 13`
    ArrayMinPoint,
    /// `E7 17`
    ArrayMaxPoint,
    /// `E7 23`
    ArrayAdd,
    /// `E7 37`
    ArrayEvenDistance,
    /// `E7 50`
    DocumentMinPoint,
    /// `E7 51`
    DocumentMaxPoint,
    /// `F1 03`
    DisplayOffset,
    /// `F2 03`
    ElementArrayMinPoint,
    /// `F2 04`
    ElementArrayMaxPoint,
    /// `F2 06`
    ElementArrayAdd,
}

const POINT_OPS: [(PointKind, [u8; 2]); 12] = [
    (PointKind::ProcessTopLeft, [0xE7, 0x03]),
    (PointKind::ProcessBottomRight, [0xE7, 0x07]),
    (PointKind::ArrayMinPoint, [0xE7, 0x13]),
    (PointKind::ArrayMaxPoint, [0xE7, 0x17]),
    (PointKind::ArrayAdd, [0xE7, 0x23]),
    (PointKind::ArrayEvenDistance, [0xE7, 0x37]),
    (PointKind::DocumentMinPoint, [0xE7, 0x50]),
    (PointKind::DocumentMaxPoint, [0xE7, 0x51]),
    (PointKind::DisplayOffset, [0xF1, 0x03]),
    (PointKind::ElementArrayMinPoint, [0xF2, 0x03]),
    (PointKind::ElementArrayMaxPoint, [0xF2, 0x04]),
    (PointKind::ElementArrayAdd, [0xF2, 0x06]),
];

/// Per-layer bounds `E7 52/53/61/62 part x y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartPointKind {
    Min,
    Max,
    MinEx,
    MaxEx,
}

const PART_POINT_OPS: [(PartPointKind, u8); 4] = [
    (PartPointKind::Min, 0x52),
    (PartPointKind::Max, 0x53),
    (PartPointKind::MinEx, 0x61),
    (PartPointKind::MaxEx, 0x62),
];

/// The seven 14-bit values of `E7 04`, `E7 08`, `F2 05` (meaning partly unknown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RepeatKind {
    ProcessRepeat,
    ArrayRepeat,
    ElementArray,
}

const REPEAT_OPS: [(RepeatKind, [u8; 2]); 3] = [
    (RepeatKind::ProcessRepeat, [0xE7, 0x04]),
    (RepeatKind::ArrayRepeat, [0xE7, 0x08]),
    (RepeatKind::ElementArray, [0xF2, 0x05]),
];

/// Commands with one 35-bit unsigned value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueKind {
    /// `E7 09`
    FeedLength,
    /// `E7 0A`
    FeedInfo,
    /// `E5 05`: the file checksum (see [`crate::rd`]).
    FileSum,
}

const VALUE_OPS: [(ValueKind, [u8; 2]); 3] =
    [(ValueKind::FeedLength, [0xE7, 0x09]), (ValueKind::FeedInfo, [0xE7, 0x0A]), (ValueKind::FileSum, [0xE5, 0x05])];

/// `E7 54` pen offset or `E7 55` layer offset: `axis(1) coord(5)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OffsetKind {
    Pen,
    Layer,
}

/// `E8 xx` stored-document (controller file list) commands, all `u14` file numbers (1-based; 0 means all for delete).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DocKind {
    /// `E8 01 n`: request document `n`'s name (reply `E8 01 n name 00`).
    Name,
    /// `E8 03 n`: start (run) stored document `n`.
    Start,
    /// `E8 04 n`: calculate document `n`'s run time.
    CalcTime,
}

const DOC_OPS: [(DocKind, u8); 3] = [(DocKind::Name, 0x01), (DocKind::Start, 0x03), (DocKind::CalcTime, 0x04)];

/// One host-to-controller command (also what `.rd` files hold).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    /// `88 x y`: move (laser off) to an absolute position, µm.
    MoveAbs { x: i32, y: i32 },
    /// `89 dx dy`: relative move, µm, each in -8192..=8191.
    MoveRel { dx: i16, dy: i16 },
    /// `8A dx`
    MoveRelX { dx: i16 },
    /// `8B dy`
    MoveRelY { dy: i16 },
    /// `A8 x y`: cut (laser on) to an absolute position.
    CutAbs { x: i32, y: i32 },
    /// `A9 dx dy`
    CutRel { dx: i16, dy: i16 },
    /// `AA dx`
    CutRelX { dx: i16 },
    /// `AB dy`
    CutRelY { dy: i16 },
    /// `80 00` / `A0 00` / `80 08` / `A0 08`: single-axis move (µm).
    AxisMove { axis: Axis, pos: i32 },
    /// `D9 00..03 opt pos`: rapid single-axis move (µm).
    RapidMove { axis: Axis, option: RapidOption, pos: i32 },
    /// `D9 10 opt x y`
    RapidMoveXy { option: RapidOption, x: i32, y: i32 },
    /// `D9 30 opt x y u`
    RapidMoveXyu { option: RapidOption, x: i32, y: i32, u: i32 },
    /// Power of the current layer (see [`PowerKind`]).
    Power { kind: PowerKind, laser: Laser, power: Power },
    /// `C6 31/32/41/42/35/36/37/38 part power`: a layer's min (`max: false`) or max power.
    PartPower { max: bool, laser: Laser, part: u8, power: Power },
    /// `C6 1x t`: delay in µs.
    Delay { kind: DelayKind, us: u32 },
    /// `C6 60 laser part freq`: a layer's frequency (unit unverified; MeerK40t reads the raw value).
    FrequencyPart { laser: u8, part: u8, freq: u32 },
    /// `C9 02/03/05/06 v`: speed, µm/s.
    Speed { kind: SpeedKind, um_per_s: u32 },
    /// `C9 04 part v`: a layer's speed, µm/s.
    SpeedPart { part: u8, um_per_s: u32 },
    /// `CA 01 xx`
    LayerFlag(LayerFlag),
    /// `op sub value`
    Byte { kind: ByteKind, value: u8 },
    /// `CA 05 c`: layer colour, `0xBBGGRR` (red in the low byte).
    LayerColor { bgr: u32 },
    /// `CA 06 part c`
    LayerColorPart { part: u8, bgr: u32 },
    /// `CA 30 n`
    UFileId(u16),
    /// `CA 41 part mode`
    WorkModePart { part: u8, mode: u8 },
    /// `CC`: acknowledge.
    Ack,
    /// `CD`: error.
    Err,
    /// `CE`: keepalive / enquiry.
    Enq,
    /// `CF`: negative acknowledge.
    Nak,
    /// `D7`: end of file.
    EndOfFile,
    /// `D8 00`
    StartProcess,
    /// `D8 01`
    StopProcess,
    /// `D8 02`
    PauseProcess,
    /// `D8 03`
    RestoreProcess,
    /// `D8 10/11/12`
    RefPoint(RefPoint),
    /// `D8 2A`
    HomeXy,
    /// `D8 2C`
    HomeZ,
    /// `D8 2D`
    HomeU,
    /// `D8 2E`
    FocusZ,
    /// `D8 20..27` (pressed) / `D8 30..37` (released)
    Jog { key: JogKey, pressed: bool },
    /// `DA 00 addr`: read a memory register (reply `DA 01 addr value`).
    GetMemory(MemAddr),
    /// `DA 01 addr v0 v1`: write a memory register (MeerK40t writes the value twice).
    SetMemory { addr: MemAddr, v0: u64, v1: u64 },
    /// `E5 02`: document data end.
    DocumentFileEnd,
    /// `E6 01`: coordinates are absolute.
    SetAbsolute,
    /// `E7 00`: end of a block of moves.
    BlockEnd,
    /// `E7 01 name 00`: the file name of a job being stored on the controller (ASCII).
    SetFilename(String),
    /// `E7 06 v0 v1`
    FeedRepeat { v0: u64, v1: u64 },
    /// `E7 3A`
    UnionBlockProperty,
    /// One point (see [`PointKind`]).
    Point { kind: PointKind, x: i32, y: i32 },
    /// `E7 52/53/61/62 part x y`
    PartPoint { kind: PartPointKind, part: u8, x: i32, y: i32 },
    /// Seven 14-bit values (see [`RepeatKind`]).
    Repeat { kind: RepeatKind, values: [i16; 7] },
    /// One 35-bit value (see [`ValueKind`]).
    Value { kind: ValueKind, value: u64 },
    /// `E7 54/55 axis coord`
    Offset { kind: OffsetKind, axis: u8, value: i32 },
    /// `E8 00 n m`: delete stored document `n` (0 = all).
    DeleteDocument { index: u16, other: u16 },
    /// `E8 02`: file transfer follows.
    FileTransfer,
    /// `E8 01/03/04 n`
    Document { kind: DocKind, index: u16 },
    /// `EA index`
    ArrayStart(u8),
    /// `EB`
    ArrayEnd,
    /// `F0`
    RefPointSet,
    /// Any other command, kept byte for byte (unscrambled, opcode first).
    Unknown(Vec<u8>),
}

fn p2(c: &[u8], at: usize) -> [u8; 2] {
    [c[at], c[at + 1]]
}

fn p5(c: &[u8], at: usize) -> [u8; 5] {
    [c[at], c[at + 1], c[at + 2], c[at + 3], c[at + 4]]
}

fn param(b: u8) -> Result<u8> {
    if b >= 0x80 { Err(RuidaError::NotSevenBit(b)) } else { Ok(b) }
}

impl Command {
    /// Encode into `out` (unscrambled).
    pub fn encode_into(&self, out: &mut Vec<u8>) -> Result<()> {
        use Command as C;
        match self {
            C::MoveAbs { x, y } => abs2(out, &[0x88], *x, *y)?,
            C::MoveRel { dx, dy } => rel2(out, 0x89, *dx, *dy)?,
            C::MoveRelX { dx } => rel1(out, 0x8A, *dx)?,
            C::MoveRelY { dy } => rel1(out, 0x8B, *dy)?,
            C::CutAbs { x, y } => abs2(out, &[0xA8], *x, *y)?,
            C::CutRel { dx, dy } => rel2(out, 0xA9, *dx, *dy)?,
            C::CutRelX { dx } => rel1(out, 0xAA, *dx)?,
            C::CutRelY { dy } => rel1(out, 0xAB, *dy)?,
            C::AxisMove { axis, pos } => {
                let op = AXIS_MOVE_OPS.iter().find(|(a, _)| a == axis).map(|(_, o)| o).unwrap_or(&[0x80, 0x00]);
                out.extend_from_slice(op);
                out.extend(encode_abs(*pos)?);
            }
            C::RapidMove { axis, option, pos } => {
                let sub = RAPID_AXIS_OPS.iter().find(|(a, _)| a == axis).map_or(0, |(_, s)| *s);
                out.extend([0xD9, sub, option.byte()]);
                out.extend(encode_abs(*pos)?);
            }
            C::RapidMoveXy { option, x, y } => {
                out.extend([0xD9, 0x10, option.byte()]);
                out.extend(encode_abs(*x)?);
                out.extend(encode_abs(*y)?);
            }
            C::RapidMoveXyu { option, x, y, u } => {
                out.extend([0xD9, 0x30, option.byte()]);
                out.extend(encode_abs(*x)?);
                out.extend(encode_abs(*y)?);
                out.extend(encode_abs(*u)?);
            }
            C::Power { kind, laser, power } => {
                out.extend_from_slice(power_op(*kind, *laser));
                out.extend(power.encode());
            }
            C::PartPower { max, laser, part, power } => {
                let row = if *max { PART_POWER_OPS[1].1 } else { PART_POWER_OPS[0].1 };
                out.extend([0xC6, row[laser.index()], param(*part)?]);
                out.extend(power.encode());
            }
            C::Delay { kind, us } => {
                let sub = DELAY_OPS.iter().find(|(k, _)| k == kind).map_or(0x10, |(_, s)| *s);
                out.extend([0xC6, sub]);
                out.extend(encode_u32(*us)?);
            }
            C::FrequencyPart { laser, part, freq } => {
                out.extend([0xC6, 0x60, param(*laser)?, param(*part)?]);
                out.extend(encode_u32(*freq)?);
            }
            C::Speed { kind, um_per_s } => {
                let sub = SPEED_OPS.iter().find(|(k, _)| k == kind).map_or(0x02, |(_, s)| *s);
                out.extend([0xC9, sub]);
                out.extend(encode_u32(*um_per_s)?);
            }
            C::SpeedPart { part, um_per_s } => {
                out.extend([0xC9, 0x04, param(*part)?]);
                out.extend(encode_u32(*um_per_s)?);
            }
            C::LayerFlag(f) => {
                let sub = match f {
                    LayerFlag::End => 0x00,
                    LayerFlag::WorkMode(m) => {
                        if !WORK_MODES.contains(m) {
                            return Err(RuidaError::Invalid { what: "work mode", value: u32::from(*m) });
                        }
                        *m
                    }
                    LayerFlag::LaserDevice(b) => 0x10 | u8::from(*b),
                    LayerFlag::AirAssist(b) => 0x12 | u8::from(*b),
                    LayerFlag::DualHead => 0x14,
                    LayerFlag::Laser2Offset(b) => 0x30 | u8::from(*b),
                };
                out.extend([0xCA, 0x01, sub]);
            }
            C::Byte { kind, value } => {
                let op = BYTE_OPS.iter().find(|(k, _)| k == kind).map_or([0xCA, 0x02], |(_, o)| *o);
                out.extend(op);
                out.push(param(*value)?);
            }
            C::LayerColor { bgr } => {
                out.extend([0xCA, 0x05]);
                out.extend(encode_u32(*bgr)?);
            }
            C::LayerColorPart { part, bgr } => {
                out.extend([0xCA, 0x06, param(*part)?]);
                out.extend(encode_u32(*bgr)?);
            }
            C::UFileId(n) => {
                out.extend([0xCA, 0x30]);
                out.extend(encode_u14(*n)?);
            }
            C::WorkModePart { part, mode } => out.extend([0xCA, 0x41, param(*part)?, param(*mode)?]),
            C::Ack => out.push(0xCC),
            C::Err => out.push(0xCD),
            C::Enq => out.push(0xCE),
            C::Nak => out.push(0xCF),
            C::EndOfFile => out.push(0xD7),
            C::StartProcess => out.extend([0xD8, 0x00]),
            C::StopProcess => out.extend([0xD8, 0x01]),
            C::PauseProcess => out.extend([0xD8, 0x02]),
            C::RestoreProcess => out.extend([0xD8, 0x03]),
            C::RefPoint(r) => out.extend([
                0xD8,
                match r {
                    RefPoint::MachineZero => 0x10,
                    RefPoint::Anchor => 0x11,
                    RefPoint::Current => 0x12,
                },
            ]),
            C::HomeXy => out.extend([0xD8, 0x2A]),
            C::HomeZ => out.extend([0xD8, 0x2C]),
            C::HomeU => out.extend([0xD8, 0x2D]),
            C::FocusZ => out.extend([0xD8, 0x2E]),
            C::Jog { key, pressed } => {
                let i = JOG_KEYS.iter().position(|k| k == key).unwrap_or(0);
                let base: u8 = if *pressed { 0x20 } else { 0x30 };
                out.extend([0xD8, base + u8::try_from(i).unwrap_or(0)]);
            }
            C::GetMemory(addr) => {
                out.extend([0xDA, 0x00]);
                out.extend(addr.encode());
            }
            C::SetMemory { addr, v0, v1 } => {
                out.extend([0xDA, 0x01]);
                out.extend(addr.encode());
                out.extend(encode_u35(*v0)?);
                out.extend(encode_u35(*v1)?);
            }
            C::DocumentFileEnd => out.extend([0xE5, 0x02]),
            C::SetAbsolute => out.extend([0xE6, 0x01]),
            C::BlockEnd => out.extend([0xE7, 0x00]),
            C::SetFilename(name) => {
                if name.bytes().any(|b| b == 0 || b >= 0x80) {
                    return Err(RuidaError::BadName(name.clone()));
                }
                out.extend([0xE7, 0x01]);
                out.extend_from_slice(name.as_bytes());
                out.push(0);
            }
            C::FeedRepeat { v0, v1 } => {
                out.extend([0xE7, 0x06]);
                out.extend(encode_u35(*v0)?);
                out.extend(encode_u35(*v1)?);
            }
            C::UnionBlockProperty => out.extend([0xE7, 0x3A]),
            C::Point { kind, x, y } => {
                let op = POINT_OPS.iter().find(|(k, _)| k == kind).map_or([0xE7, 0x03], |(_, o)| *o);
                abs2(out, &op, *x, *y)?;
            }
            C::PartPoint { kind, part, x, y } => {
                let sub = PART_POINT_OPS.iter().find(|(k, _)| k == kind).map_or(0x52, |(_, s)| *s);
                abs2(out, &[0xE7, sub, param(*part)?], *x, *y)?;
            }
            C::Repeat { kind, values } => {
                let op = REPEAT_OPS.iter().find(|(k, _)| k == kind).map_or([0xE7, 0x04], |(_, o)| *o);
                out.extend(op);
                for v in values {
                    out.extend(encode_i14(*v)?);
                }
            }
            C::Value { kind, value } => {
                let op = VALUE_OPS.iter().find(|(k, _)| k == kind).map_or([0xE7, 0x09], |(_, o)| *o);
                out.extend(op);
                out.extend(encode_u35(*value)?);
            }
            C::Offset { kind, axis, value } => {
                let sub = match kind {
                    OffsetKind::Pen => 0x54,
                    OffsetKind::Layer => 0x55,
                };
                out.extend([0xE7, sub, param(*axis)?]);
                out.extend(encode_abs(*value)?);
            }
            C::DeleteDocument { index, other } => {
                out.extend([0xE8, 0x00]);
                out.extend(encode_u14(*index)?);
                out.extend(encode_u14(*other)?);
            }
            C::FileTransfer => out.extend([0xE8, 0x02]),
            C::Document { kind, index } => {
                let sub = DOC_OPS.iter().find(|(k, _)| k == kind).map_or(0x01, |(_, s)| *s);
                out.extend([0xE8, sub]);
                out.extend(encode_u14(*index)?);
            }
            C::ArrayStart(i) => out.extend([0xEA, param(*i)?]),
            C::ArrayEnd => out.push(0xEB),
            C::RefPointSet => out.push(0xF0),
            C::Unknown(bytes) => {
                if bytes.first().is_none_or(|&b| b < 0x80) || bytes[1..].iter().any(|&b| b >= 0x80) {
                    return Err(RuidaError::Malformed { offset: 0, bytes: bytes.clone() });
                }
                out.extend_from_slice(bytes);
            }
        }
        Ok(())
    }

    /// Encode to a new vector (unscrambled).
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut v = Vec::new();
        self.encode_into(&mut v)?;
        Ok(v)
    }

    /// Decode one command (one chunk from [`split`]). Unknown opcodes give [`Command::Unknown`]; a known opcode with
    /// the wrong length is an error.
    pub fn decode(c: &[u8]) -> Result<Command> {
        use Command as C;
        let Some(&op) = c.first() else {
            return Err(RuidaError::Malformed { offset: 0, bytes: Vec::new() });
        };
        if op < 0x80 || c[1..].iter().any(|&b| b >= 0x80) {
            return Err(RuidaError::Malformed { offset: 0, bytes: c.to_vec() });
        }
        let sub = c.get(1).copied();
        let want = |n: usize| -> Result<()> {
            if c.len() == n { Ok(()) } else { Err(RuidaError::Length { bytes: c.to_vec(), expected: n }) }
        };
        let unknown = || Ok(C::Unknown(c.to_vec()));

        // Power commands (one- and two-byte opcodes).
        for (kind, row) in &POWER_OPS {
            for (laser, opb) in Laser::ALL.iter().zip(row.iter()) {
                if c.starts_with(opb) {
                    want(opb.len() + 2)?;
                    return Ok(C::Power { kind: *kind, laser: *laser, power: Power::decode(p2(c, opb.len()))? });
                }
            }
        }
        match (op, sub) {
            (0x88, _) => {
                want(11)?;
                Ok(C::MoveAbs { x: decode_abs(p5(c, 1))?, y: decode_abs(p5(c, 6))? })
            }
            (0x89, _) => {
                want(5)?;
                Ok(C::MoveRel { dx: decode_i14(p2(c, 1))?, dy: decode_i14(p2(c, 3))? })
            }
            (0x8A, _) => {
                want(3)?;
                Ok(C::MoveRelX { dx: decode_i14(p2(c, 1))? })
            }
            (0x8B, _) => {
                want(3)?;
                Ok(C::MoveRelY { dy: decode_i14(p2(c, 1))? })
            }
            (0xA8, _) => {
                want(11)?;
                Ok(C::CutAbs { x: decode_abs(p5(c, 1))?, y: decode_abs(p5(c, 6))? })
            }
            (0xA9, _) => {
                want(5)?;
                Ok(C::CutRel { dx: decode_i14(p2(c, 1))?, dy: decode_i14(p2(c, 3))? })
            }
            (0xAA, _) => {
                want(3)?;
                Ok(C::CutRelX { dx: decode_i14(p2(c, 1))? })
            }
            (0xAB, _) => {
                want(3)?;
                Ok(C::CutRelY { dy: decode_i14(p2(c, 1))? })
            }
            (0x80 | 0xA0, Some(s)) => match AXIS_MOVE_OPS.iter().find(|(_, o)| o[0] == op && o[1] == s) {
                Some((axis, _)) => {
                    want(7)?;
                    Ok(C::AxisMove { axis: *axis, pos: decode_abs(p5(c, 2))? })
                }
                None => unknown(),
            },
            (0xC6, Some(s)) => {
                for (max, row) in &PART_POWER_OPS {
                    if let Some(i) = row.iter().position(|&x| x == s) {
                        want(5)?;
                        return Ok(C::PartPower {
                            max: *max,
                            laser: Laser::ALL[i],
                            part: c[2],
                            power: Power::decode(p2(c, 3))?,
                        });
                    }
                }
                if let Some((kind, _)) = DELAY_OPS.iter().find(|(_, x)| *x == s) {
                    want(7)?;
                    return Ok(C::Delay { kind: *kind, us: decode_u32(p5(c, 2), "delay")? });
                }
                if s == 0x60 {
                    want(9)?;
                    return Ok(C::FrequencyPart { laser: c[2], part: c[3], freq: decode_u32(p5(c, 4), "frequency")? });
                }
                unknown()
            }
            (0xC9, Some(0x04)) => {
                want(8)?;
                Ok(C::SpeedPart { part: c[2], um_per_s: decode_u32(p5(c, 3), "speed")? })
            }
            (0xC9, Some(s)) => match SPEED_OPS.iter().find(|(_, x)| *x == s) {
                Some((kind, _)) => {
                    want(7)?;
                    Ok(C::Speed { kind: *kind, um_per_s: decode_u32(p5(c, 2), "speed")? })
                }
                None => unknown(),
            },
            (0xCA, Some(0x01)) => {
                want(3)?;
                let f = match c[2] {
                    0x00 => LayerFlag::End,
                    m if WORK_MODES.contains(&m) => LayerFlag::WorkMode(m),
                    0x10 | 0x11 => LayerFlag::LaserDevice(c[2] == 0x11),
                    0x12 | 0x13 => LayerFlag::AirAssist(c[2] == 0x13),
                    0x14 => LayerFlag::DualHead,
                    0x30 | 0x31 => LayerFlag::Laser2Offset(c[2] == 0x31),
                    _ => return unknown(),
                };
                Ok(C::LayerFlag(f))
            }
            (0xCA, Some(0x05)) => {
                want(7)?;
                Ok(C::LayerColor { bgr: decode_u32(p5(c, 2), "colour")? })
            }
            (0xCA, Some(0x06)) => {
                want(8)?;
                Ok(C::LayerColorPart { part: c[2], bgr: decode_u32(p5(c, 3), "colour")? })
            }
            (0xCA, Some(0x30)) => {
                want(4)?;
                Ok(C::UFileId(decode_u14(p2(c, 2))?))
            }
            (0xCA, Some(0x41)) => {
                want(4)?;
                Ok(C::WorkModePart { part: c[2], mode: c[3] })
            }
            (0xCC, _) => want(1).map(|()| C::Ack),
            (0xCD, _) => want(1).map(|()| C::Err),
            (0xCE, _) => want(1).map(|()| C::Enq),
            (0xCF, _) => want(1).map(|()| C::Nak),
            (0xD7, _) => want(1).map(|()| C::EndOfFile),
            (0xD8, Some(s)) => {
                let cmd = match s {
                    0x00 => C::StartProcess,
                    0x01 => C::StopProcess,
                    0x02 => C::PauseProcess,
                    0x03 => C::RestoreProcess,
                    0x10 => C::RefPoint(RefPoint::MachineZero),
                    0x11 => C::RefPoint(RefPoint::Anchor),
                    0x12 => C::RefPoint(RefPoint::Current),
                    0x2A => C::HomeXy,
                    0x2C => C::HomeZ,
                    0x2D => C::HomeU,
                    0x2E => C::FocusZ,
                    0x20..=0x27 => C::Jog { key: JOG_KEYS[usize::from(s - 0x20)], pressed: true },
                    0x30..=0x37 => C::Jog { key: JOG_KEYS[usize::from(s - 0x30)], pressed: false },
                    _ => return unknown(),
                };
                want(2)?;
                Ok(cmd)
            }
            (0xD9, Some(s)) => {
                if let Some((axis, _)) = RAPID_AXIS_OPS.iter().find(|(_, x)| *x == s) {
                    want(8)?;
                    let Some(option) = RapidOption::from_byte(c[2]) else { return unknown() };
                    return Ok(C::RapidMove { axis: *axis, option, pos: decode_abs(p5(c, 3))? });
                }
                match s {
                    0x10 => {
                        want(13)?;
                        let Some(option) = RapidOption::from_byte(c[2]) else { return unknown() };
                        Ok(C::RapidMoveXy { option, x: decode_abs(p5(c, 3))?, y: decode_abs(p5(c, 8))? })
                    }
                    0x30 => {
                        want(18)?;
                        let Some(option) = RapidOption::from_byte(c[2]) else { return unknown() };
                        Ok(C::RapidMoveXyu {
                            option,
                            x: decode_abs(p5(c, 3))?,
                            y: decode_abs(p5(c, 8))?,
                            u: decode_abs(p5(c, 13))?,
                        })
                    }
                    _ => unknown(),
                }
            }
            (0xDA, Some(0x00)) => {
                want(4)?;
                Ok(C::GetMemory(MemAddr::decode(p2(c, 2))?))
            }
            (0xDA, Some(0x01)) => {
                want(14)?;
                Ok(C::SetMemory {
                    addr: MemAddr::decode(p2(c, 2))?,
                    v0: decode_u35(p5(c, 4))?,
                    v1: decode_u35(p5(c, 9))?,
                })
            }
            (0xE5, Some(0x02)) => want(2).map(|()| C::DocumentFileEnd),
            (0xE6, Some(0x01)) => want(2).map(|()| C::SetAbsolute),
            (0xE7, Some(0x00)) => want(2).map(|()| C::BlockEnd),
            (0xE7, Some(0x01)) => {
                // Name bytes then a NUL terminator.
                match c[2..].iter().position(|&b| b == 0) {
                    Some(n) if n + 3 == c.len() => {
                        Ok(C::SetFilename(String::from_utf8_lossy(&c[2..2 + n]).into_owned()))
                    }
                    _ => Err(RuidaError::Length { bytes: c.to_vec(), expected: c.len() }),
                }
            }
            (0xE7, Some(0x06)) => {
                want(12)?;
                Ok(C::FeedRepeat { v0: decode_u35(p5(c, 2))?, v1: decode_u35(p5(c, 7))? })
            }
            (0xE7, Some(0x3A)) => want(2).map(|()| C::UnionBlockProperty),
            (0xE7, Some(0x54 | 0x55)) => {
                want(8)?;
                let kind = if sub == Some(0x54) { OffsetKind::Pen } else { OffsetKind::Layer };
                Ok(C::Offset { kind, axis: c[2], value: decode_abs(p5(c, 3))? })
            }
            (0xE7, Some(s)) if PART_POINT_OPS.iter().any(|(_, x)| *x == s) => {
                want(13)?;
                let kind = PART_POINT_OPS.iter().find(|(_, x)| *x == s).map_or(PartPointKind::Min, |(k, _)| *k);
                Ok(C::PartPoint { kind, part: c[2], x: decode_abs(p5(c, 3))?, y: decode_abs(p5(c, 8))? })
            }
            (0xE8, Some(0x00)) => {
                want(6)?;
                Ok(C::DeleteDocument { index: decode_u14(p2(c, 2))?, other: decode_u14(p2(c, 4))? })
            }
            (0xE8, Some(0x02)) => want(2).map(|()| C::FileTransfer),
            (0xE8, Some(s)) if DOC_OPS.iter().any(|(_, x)| *x == s) => {
                want(4)?;
                let kind = DOC_OPS.iter().find(|(_, x)| *x == s).map_or(DocKind::Name, |(k, _)| *k);
                Ok(C::Document { kind, index: decode_u14(p2(c, 2))? })
            }
            (0xEA, _) => {
                want(2)?;
                Ok(C::ArrayStart(c[1]))
            }
            (0xEB, _) => want(1).map(|()| C::ArrayEnd),
            (0xF0, _) => want(1).map(|()| C::RefPointSet),
            (_, Some(s)) => {
                if let Some((kind, _)) = BYTE_OPS.iter().find(|(_, o)| o[0] == op && o[1] == s) {
                    want(3)?;
                    return Ok(C::Byte { kind: *kind, value: c[2] });
                }
                if let Some((kind, _)) = POINT_OPS.iter().find(|(_, o)| o[0] == op && o[1] == s) {
                    want(12)?;
                    return Ok(C::Point { kind: *kind, x: decode_abs(p5(c, 2))?, y: decode_abs(p5(c, 7))? });
                }
                if let Some((kind, _)) = REPEAT_OPS.iter().find(|(_, o)| o[0] == op && o[1] == s) {
                    want(16)?;
                    let mut values = [0i16; 7];
                    for (i, v) in values.iter_mut().enumerate() {
                        *v = decode_i14(p2(c, 2 + 2 * i))?;
                    }
                    return Ok(C::Repeat { kind: *kind, values });
                }
                if let Some((kind, _)) = VALUE_OPS.iter().find(|(_, o)| o[0] == op && o[1] == s) {
                    want(7)?;
                    return Ok(C::Value { kind: *kind, value: decode_u35(p5(c, 2))? });
                }
                unknown()
            }
            _ => unknown(),
        }
    }

    /// True for the commands that fire the laser (`A8`..`AB`).
    pub fn is_cut(&self) -> bool {
        matches!(self, Command::CutAbs { .. } | Command::CutRel { .. } | Command::CutRelX { .. } | Command::CutRelY { .. })
    }
}

fn abs2(out: &mut Vec<u8>, op: &[u8], x: i32, y: i32) -> Result<()> {
    out.extend_from_slice(op);
    out.extend(encode_abs(x)?);
    out.extend(encode_abs(y)?);
    Ok(())
}

fn rel1(out: &mut Vec<u8>, op: u8, d: i16) -> Result<()> {
    out.push(op);
    out.extend(encode_i14(d)?);
    Ok(())
}

fn rel2(out: &mut Vec<u8>, op: u8, dx: i16, dy: i16) -> Result<()> {
    out.push(op);
    out.extend(encode_i14(dx)?);
    out.extend(encode_i14(dy)?);
    Ok(())
}

/// Split an unscrambled stream into commands at every byte with bit 7 set: `(offset, bytes)`. Leading bytes before
/// the first opcode come out as a chunk starting below `0x80` (which [`Command::decode`] refuses).
pub fn split(data: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut starts: Vec<usize> = data.iter().enumerate().filter(|(i, b)| *i == 0 || **b >= 0x80).map(|(i, _)| i).collect();
    starts.push(data.len());
    let mut chunks = Vec::with_capacity(starts.len());
    for w in starts.windows(2) {
        if w[1] > w[0] {
            chunks.push((w[0], &data[w[0]..w[1]]));
        }
    }
    chunks.into_iter()
}

/// Parse a whole unscrambled stream. Errors carry the byte offset.
pub fn parse(data: &[u8]) -> Result<Vec<Command>> {
    split(data)
        .map(|(off, c)| {
            Command::decode(c).map_err(|e| match e {
                RuidaError::Malformed { bytes, .. } => RuidaError::Malformed { offset: off, bytes },
                RuidaError::Length { bytes, expected } => RuidaError::At { offset: off, source: Box::new(RuidaError::Length { bytes, expected }) },
                other => RuidaError::At { offset: off, source: Box::new(other) },
            })
        })
        .collect()
}

/// Parse a stream, keeping anything that does not decode as [`Command::Unknown`] (stray leading bytes included).
pub fn parse_lenient(data: &[u8]) -> Vec<Command> {
    split(data).map(|(_, c)| Command::decode(c).unwrap_or_else(|_| Command::Unknown(c.to_vec()))).collect()
}

/// Encode a list of commands (unscrambled).
pub fn encode_all(cmds: &[Command]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for c in cmds {
        c.encode_into(&mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One instance of every typed command (and every table row), for round-trip tests.
    pub(crate) fn every_command() -> Vec<Command> {
        use Command as C;
        let pw = Power::from_raw(0x1234).unwrap();
        let mut v = vec![
            C::MoveAbs { x: 123_456, y: -7 },
            C::MoveAbs { x: i32::MIN, y: i32::MAX },
            C::MoveRel { dx: -8192, dy: 8191 },
            C::MoveRelX { dx: -1 },
            C::MoveRelY { dy: 1 },
            C::CutAbs { x: 0, y: 600_000 },
            C::CutRel { dx: 5, dy: -5 },
            C::CutRelX { dx: 8191 },
            C::CutRelY { dy: -8192 },
            C::RapidMoveXy { option: RapidOption::Light, x: 1, y: 2 },
            C::RapidMoveXyu { option: RapidOption::Origin, x: -1, y: -2, u: 3 },
            C::FrequencyPart { laser: 0, part: 3, freq: 20_000 },
            C::SpeedPart { part: 2, um_per_s: 100_000 },
            C::LayerColor { bgr: 0x00FF_00FF },
            C::LayerColorPart { part: 1, bgr: 0x0012_3456 },
            C::UFileId(77),
            C::WorkModePart { part: 0, mode: 0 },
            C::Ack,
            C::Err,
            C::Enq,
            C::Nak,
            C::EndOfFile,
            C::StartProcess,
            C::StopProcess,
            C::PauseProcess,
            C::RestoreProcess,
            C::RefPoint(RefPoint::MachineZero),
            C::RefPoint(RefPoint::Anchor),
            C::RefPoint(RefPoint::Current),
            C::HomeXy,
            C::HomeZ,
            C::HomeU,
            C::FocusZ,
            C::GetMemory(MemAddr::new(0x02FE).unwrap()),
            C::SetMemory { addr: MemAddr::new(0x0026).unwrap(), v0: 320_000, v1: 320_000 },
            C::DocumentFileEnd,
            C::SetAbsolute,
            C::BlockEnd,
            C::SetFilename("JOB1".into()),
            C::SetFilename(String::new()),
            C::FeedRepeat { v0: 0, v1: 1 },
            C::UnionBlockProperty,
            C::Offset { kind: OffsetKind::Pen, axis: 1, value: -3 },
            C::Offset { kind: OffsetKind::Layer, axis: 0, value: 3 },
            C::DeleteDocument { index: 0, other: 0 },
            C::FileTransfer,
            C::ArrayStart(0),
            C::ArrayEnd,
            C::RefPointSet,
            C::Unknown(vec![0xE7, 0x46]),
            C::Unknown(vec![0xA5, 0x50, 0x02]),
        ];
        for axis in [Axis::X, Axis::Y, Axis::Z, Axis::U] {
            v.push(C::AxisMove { axis, pos: -42 });
            for option in [RapidOption::Origin, RapidOption::LightOrigin, RapidOption::None, RapidOption::Light] {
                v.push(C::RapidMove { axis, option, pos: 99 });
            }
        }
        for laser in Laser::ALL {
            for (kind, _) in POWER_OPS {
                v.push(C::Power { kind, laser, power: pw });
            }
            for max in [false, true] {
                v.push(C::PartPower { max, laser, part: 7, power: Power::FULL });
            }
        }
        for (kind, _) in DELAY_OPS {
            v.push(C::Delay { kind, us: 1500 });
        }
        for (kind, _) in SPEED_OPS {
            v.push(C::Speed { kind, um_per_s: u32::MAX });
        }
        for f in [
            LayerFlag::End,
            LayerFlag::LaserDevice(false),
            LayerFlag::LaserDevice(true),
            LayerFlag::AirAssist(false),
            LayerFlag::AirAssist(true),
            LayerFlag::DualHead,
            LayerFlag::Laser2Offset(false),
            LayerFlag::Laser2Offset(true),
        ] {
            v.push(C::LayerFlag(f));
        }
        for m in WORK_MODES {
            v.push(C::LayerFlag(LayerFlag::WorkMode(m)));
        }
        for (kind, _) in BYTE_OPS {
            v.push(C::Byte { kind, value: 0x7F });
        }
        for (kind, _) in POINT_OPS {
            v.push(C::Point { kind, x: -100, y: 100 });
        }
        for (kind, _) in PART_POINT_OPS {
            v.push(C::PartPoint { kind, part: 4, x: 1, y: -1 });
        }
        for (kind, _) in REPEAT_OPS {
            v.push(C::Repeat { kind, values: [1, 1, 0, 1123, -3328, 4, 3480] });
        }
        for (kind, _) in VALUE_OPS {
            v.push(C::Value { kind, value: crate::num::U35_MAX });
        }
        for (kind, _) in DOC_OPS {
            v.push(C::Document { kind, index: 3 });
        }
        for key in JOG_KEYS {
            v.push(C::Jog { key, pressed: true });
            v.push(C::Jog { key, pressed: false });
        }
        v
    }

    #[test]
    fn every_command_round_trips() {
        let all = every_command();
        let mut seen = std::collections::HashSet::new();
        for c in &all {
            let bytes = c.encode().unwrap();
            assert!(bytes[0] >= 0x80 && bytes[1..].iter().all(|&b| b < 0x80), "{c:?} -> {bytes:02X?}");
            assert_eq!(&Command::decode(&bytes).unwrap(), c, "{bytes:02X?}");
            // No two commands share an encoding.
            assert!(seen.insert(bytes.clone()), "duplicate encoding {bytes:02X?}");
        }
        let stream = encode_all(&all).unwrap();
        assert_eq!(parse(&stream).unwrap(), all);
        assert_eq!(parse_lenient(&stream), all);
    }

    #[test]
    fn known_byte_layouts() {
        // Layouts as in rdjob.py: 0x88 + 2 x 5 bytes, 0x89 + 2 x 2, power 14-bit after the opcode.
        assert_eq!(Command::MoveAbs { x: 1000, y: 0 }.encode().unwrap(), [0x88, 0, 0, 0, 0x07, 0x68, 0, 0, 0, 0, 0]);
        assert_eq!(Command::MoveRel { dx: -1, dy: 1 }.encode().unwrap(), [0x89, 0x7F, 0x7F, 0x00, 0x01]);
        assert_eq!(
            Command::Power { kind: PowerKind::Max, laser: Laser::L1, power: Power::FULL }.encode().unwrap(),
            [0xC6, 0x02, 0x7F, 0x7F]
        );
        assert_eq!(
            Command::Power { kind: PowerKind::Min, laser: Laser::L2, power: Power::ZERO }.encode().unwrap(),
            [0xC6, 0x21, 0, 0]
        );
        assert_eq!(
            Command::Speed { kind: SpeedKind::Laser1, um_per_s: 100_000 }.encode().unwrap(),
            [0xC9, 0x02, 0, 0, 0x06, 0x0D, 0x20]
        );
        assert_eq!(Command::GetMemory(MemAddr::new(0x0221).unwrap()).encode().unwrap(), [0xDA, 0x00, 0x04, 0x21]);
        assert_eq!(Command::SetFilename("AB".into()).encode().unwrap(), [0xE7, 0x01, b'A', b'B', 0]);
        assert_eq!(Command::Byte { kind: ByteKind::LayerNumber, value: 2 }.encode().unwrap(), [0xCA, 0x02, 2]);
    }

    #[test]
    fn refusals() {
        assert!(Command::MoveRel { dx: 8192, dy: 0 }.encode().is_err());
        assert!(Command::CutRelY { dy: -8193 }.encode().is_err());
        assert!(Command::Byte { kind: ByteKind::LayerNumber, value: 0x80 }.encode().is_err());
        assert!(Command::SetFilename("caf\u{e9}".into()).encode().is_err());
        assert!(Command::LayerFlag(LayerFlag::WorkMode(0x07)).encode().is_err());
        assert!(Command::Unknown(vec![0x01]).encode().is_err());
        assert!(Command::Unknown(vec![0xE7, 0x80]).encode().is_err());
        assert!(Command::Value { kind: ValueKind::FileSum, value: 1 << 35 }.encode().is_err());
        // Wrong lengths of known opcodes are errors, not Unknown.
        assert!(matches!(Command::decode(&[0x88, 0, 0]), Err(RuidaError::Length { .. })));
        assert!(matches!(Command::decode(&[0xD7, 0]), Err(RuidaError::Length { .. })));
        assert!(Command::decode(&[]).is_err());
        assert!(Command::decode(&[0x10]).is_err());
        let err = parse(&[0x00, 0xD7]).unwrap_err();
        assert!(matches!(err, RuidaError::Malformed { offset: 0, .. }), "{err:?}");
        let err = parse(&[0xD7, 0x88, 0x01]).unwrap_err();
        assert!(matches!(err, RuidaError::At { offset: 1, .. }), "{err:?}");
        assert_eq!(parse_lenient(&[0x00, 0xD7]), vec![Command::Unknown(vec![0]), Command::EndOfFile]);
    }

    #[test]
    fn split_offsets() {
        let s: Vec<_> = split(&[0xD7, 0x88, 1, 2, 0xCC]).collect();
        assert_eq!(s, vec![(0, &[0xD7][..]), (1, &[0x88, 1, 2][..]), (4, &[0xCC][..])]);
        assert_eq!(split(&[]).count(), 0);
    }

    #[test]
    fn serde_round_trip() {
        let all = every_command();
        let json = serde_json::to_string(&all).unwrap();
        let back: Vec<Command> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, all);
    }
}
