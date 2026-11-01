//! Ruida DSP controller protocol (RDC644x/634x families, as driven by RDWorks and LightBurn). Pure, no I/O except the
//! optional UDP channel (feature `udp`).
//!
//! - [`swizzle`]: the one-byte-key scrambling of every byte (default key `0x88`).
//! - [`num`]: 7-bit packed numbers (14-bit and 35-bit), power steps, unit conversions.
//! - [`command`]: typed encode/decode of the command stream.
//! - [`mem`]: memory registers (status, position, card ID, bed size, settings).
//! - [`frame`]: UDP datagrams (checksum, ACK/ERR/NAK, ports 50200/40200) and USB serial framing.
//! - [`rd`]: `.rd` job files: read to commands and to polylines per layer, write from polylines.
//! - [`sim`] (feature `sim`): a controller simulator for driver tests.
//! - [`udp`] (feature `udp`): a blocking stop-and-wait UDP channel.
//!
//! **Provenance.** Everything here comes from MeerK40t's Ruida module (MIT; meerk40t/meerk40t @ `d511944`,
//! `meerk40t/ruida/` and `test/test_ruida.py`), re-written in Rust. GPL Ruida projects (jnweiger/ruida-laser) and
//! projects under other licences (kkaempf/ruida, Ruby licence) were not used. **Nothing has been run against Ruida
//! hardware**: the simulator is OpenEtch's own reading of MeerK40t's emulator and client, and MeerK40t's own notes
//! say its Ruida support was tested on one RDC6442S only.
//!
//! # Command reference
//!
//! Bytes are unscrambled. `abs` = 5-byte absolute coordinate (µm), `rel` = 2-byte signed relative coordinate (µm),
//! `pw` = 2-byte power (`0x3FFF` = 100 %), `u35` = 5-byte unsigned, `u14` = 2-byte unsigned, `b` = one 7-bit byte.
//! "rdjob" is `meerk40t/ruida/rdjob.py` (its constants, `process()` decoder and encoder methods), "emulator" is
//! `meerk40t/ruida/emulator.py`, "udp" is `meerk40t/ruida/udp_connection.py`. Status: **cons.** = MeerK40t's encoder
//! and decoder agree; **dec.** = MeerK40t only decodes (or only describes) it; **conflict** = MeerK40t disagrees with
//! itself and this crate follows the decoder.
//!
//! | Bytes | [`Command`] | Parameters | Source | Status |
//! |---|---|---|---|---|
//! | `88` | `MoveAbs` | abs x, abs y | rdjob `MOVE_ABS_XY` | cons. |
//! | `89` | `MoveRel` | rel dx, rel dy | rdjob `MOVE_REL_XY` | cons. |
//! | `8A` / `8B` | `MoveRelX` / `MoveRelY` | rel | rdjob `MOVE_REL_X/Y` | cons. |
//! | `A8` | `CutAbs` | abs x, abs y | rdjob `CUT_ABS_XY` | cons. |
//! | `A9` | `CutRel` | rel dx, rel dy | rdjob `CUT_REL_XY` | cons. |
//! | `AA` / `AB` | `CutRelX` / `CutRelY` | rel | rdjob `CUT_REL_X/Y` | cons. |
//! | `80 00` / `A0 00` / `80 08` / `A0 08` | `AxisMove` X / Y / Z / U | abs | rdjob `process()` | conflict (constants say `80 01` Z, `A0 00` "A") |
//! | `D9 00..03` | `RapidMove` X/Y/Z/U | option b, abs | rdjob `RAPID_MOVE_*` | cons. |
//! | `D9 10` | `RapidMoveXy` | option b, abs, abs | rdjob `RAPID_MOVE_XY` | cons. |
//! | `D9 30` | `RapidMoveXyu` | option b, abs ×3 | rdjob `RAPID_MOVE_XYU` | cons. (decoder slices x wrongly) |
//! | `C7` `C0` `C2` `C3` | `Power` Immediate, lasers 1-4 | pw | rdjob `IMD_POWER_*` | cons. |
//! | `C8` `C1` `C4` `C5` | `Power` End, lasers 1-4 | pw | rdjob `END_POWER_*` | cons. |
//! | `C6 01/21/05/07` | `Power` Min, lasers 1-4 | pw | rdjob `MIN_POWER_*` | cons. |
//! | `C6 02/22/06/08` | `Power` Max, lasers 1-4 | pw | rdjob `MAX_POWER_*` | cons. |
//! | `C6 50/51/55/56` | `Power` Through, lasers 1-4 | pw | rdjob `THROUGH_POWER_*` | cons. |
//! | `C6 31/41/35/37` | `PartPower` min, lasers 1-4 | part b, pw | rdjob `MIN_POWER_*_PART` | cons. |
//! | `C6 32/42/36/38` | `PartPower` max, lasers 1-4 | part b, pw | rdjob `MAX_POWER_*_PART` | cons. |
//! | `C6 10/11/12/13/15/16` | `Delay` (interval, add, on, off, on 2, off 2) | u35 µs | rdjob `LASER_*_DELAY` etc. | cons. |
//! | `C6 60` | `FrequencyPart` | laser b, part b, u35 | rdjob `FREQUENCY_PART` | conflict (encoder ×1000 and disabled; decoder raw) |
//! | `C9 02/03/05/06` | `Speed` (laser 1, axis, force engraving, axis move) | u35 µm/s | rdjob `SPEED_*` | cons. (decoder divides 05/06 by 1000 again) |
//! | `C9 04` | `SpeedPart` | part b, u35 µm/s | rdjob `SPEED_LASER_1_PART` | cons. |
//! | `CA 01 00` | `LayerFlag(End)` | | rdjob `LAYER_END` | cons. |
//! | `CA 01 01..05, 55` | `LayerFlag(WorkMode)` | | rdjob `WORK_MODE_*` | cons. |
//! | `CA 01 10/11` | `LayerFlag(LaserDevice)` | | rdjob `LASER_DEVICE_*` | cons. |
//! | `CA 01 12/13` | `LayerFlag(AirAssist)` | | rdjob `AIR_ASSIST_*` | cons. |
//! | `CA 01 14` | `LayerFlag(DualHead)` | | rdjob `DB_HEAD` | cons. |
//! | `CA 01 30/31` | `LayerFlag(Laser2Offset)` | | rdjob `EN_LASER_2_OFFSET_*` | cons. |
//! | `CA 02` / `03` / `04` / `10` / `22` / `40` | `Byte` LayerNumber / EnLaserTubeStart / XSignMap / EnExIo / MaxLayer / ZuMap | b | rdjob | cons. |
//! | `CA 05` | `LayerColor` | u35 `0xBBGGRR` | rdjob `LAYER_COLOR` | cons. |
//! | `CA 06` | `LayerColorPart` | part b, u35 | rdjob `LAYER_COLOR_PART` | cons. |
//! | `CA 30` | `UFileId` | u14 | rdjob `U_FILE_ID` | dec. (its encoder writes `CA 22`) |
//! | `CA 41` | `WorkModePart` | part b, mode b | rdjob `WORK_MODE_PART` | cons. |
//! | `CC` / `CD` / `CE` / `CF` | `Ack` / `Err` / `Enq` (keepalive) / `Nak` | | rdjob, udp | cons. |
//! | `D7` | `EndOfFile` | | rdjob `END_OF_FILE` | cons. |
//! | `D8 00/01/02/03` | `StartProcess` / `StopProcess` / `PauseProcess` / `RestoreProcess` | | rdjob | cons. |
//! | `D8 10/11/12` | `RefPoint` machine zero / anchor / current | | rdjob `REF_POINT_*` | cons. |
//! | `D8 2A/2C/2D/2E` | `HomeXy` / `HomeZ` / `HomeU` / `FocusZ` | | rdjob | cons. |
//! | `D8 20..27` / `D8 30..37` | `Jog` pressed / released | | rdjob `KEYDOWN_*` / `KEYUP_*` | cons. |
//! | `DA 00` | `GetMemory` | u14 addr | rdjob `GET_SETTING`, emulator | cons. |
//! | `DA 01` | `SetMemory` | u14 addr, u35, u35 | rdjob `SET_SETTING` | cons. |
//! | `E5 02` | `DocumentFileEnd` | | rdjob `DOCUMENT_FILE_END` | cons. |
//! | `E5 05` | `Value(FileSum)` | u35 | rdjob `SET_FILE_SUM`, `write_tail` | cons. |
//! | `E6 01` | `SetAbsolute` | | rdjob `SET_ABSOLUTE` | cons. |
//! | `E7 00` | `BlockEnd` | | rdjob `BLOCK_END` | cons. |
//! | `E7 01` | `SetFilename` | ASCII, `00` | rdjob `SET_FILENAME`, emulator | cons. |
//! | `E7 03/07/13/17/23/37/50/51` | `Point` process top-left / bottom-right, array min / max / add / even distance, document min / max | abs, abs | rdjob | cons. (decoder slices `E7 50` wrongly) |
//! | `E7 04` / `E7 08` / `F2 05` | `Repeat` process / array / element array | 7 × rel | rdjob | cons. |
//! | `E7 05/0B/24/38/60` | `Byte` array direction / en mirror cut / mirror, feed auto pause, current element index | b | rdjob | cons. |
//! | `E7 06` | `FeedRepeat` | u35, u35 | rdjob `FEED_REPEAT` | cons. |
//! | `E7 09` / `E7 0A` | `Value` FeedLength / FeedInfo | u35 | rdjob | cons. (decoder reads no value for `0A`) |
//! | `E7 3A` | `UnionBlockProperty` | | rdjob | cons. |
//! | `E7 52/53/61/62` | `PartPoint` min / max / min ex / max ex | part b, abs, abs | rdjob `PART_*_POINT*` | cons. |
//! | `E7 54` / `E7 55` | `Offset` pen / layer | axis b, abs | rdjob `PEN_OFFSET`, `LAYER_OFFSET` | cons. |
//! | `E8 00` | `DeleteDocument` | u14 (0 = all), u14 | emulator | dec. |
//! | `E8 01` | `Document(Name)` | u14; reply `E8 01 n name 00` | emulator | dec. |
//! | `E8 02` | `FileTransfer` | | emulator | dec. |
//! | `E8 03` / `E8 04` | `Document(Start)` / `Document(CalcTime)` | u14 | emulator | dec. |
//! | `EA` / `EB` | `ArrayStart` / `ArrayEnd` | b / – | rdjob | cons. |
//! | `F0` | `RefPointSet` | | rdjob `REF_POINT_SET` | cons. |
//! | `F1 00/01/02/04` | `Byte` element max index / element name max index / enable block cutting / feed auto calc | b | rdjob | cons. |
//! | `F1 03` / `F2 03/04/06` | `Point` display offset / element array min / max / add | abs, abs | rdjob | cons. |
//! | `F2 00` / `F2 07` | `Byte` element index / element array mirror | b | rdjob | cons. |
//! | anything else | `Unknown` | kept byte for byte | – | – |
//!
//! Memory registers are listed in [`mem`]; framing facts in [`frame`].
#![forbid(unsafe_code)]
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]

pub mod command;
pub mod frame;
pub mod mem;
pub mod num;
pub mod rd;
#[cfg(any(test, feature = "sim"))]
pub mod sim;
pub mod swizzle;
#[cfg(feature = "udp")]
pub mod udp;

pub use command::{Command, encode_all, parse, parse_lenient};
pub use frame::Reply;
pub use mem::MemAddr;
pub use num::Power;
pub use rd::{RdJob, RdLayer, RdRead, WriteOptions, read_rd, write_rd};
pub use swizzle::{DEFAULT_MAGIC, Swizzle};

/// Everything this crate refuses.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RuidaError {
    #[error("{what} is not finite")]
    NonFinite { what: &'static str },
    /// The value cannot be represented in its field (never clamped or wrapped).
    #[error("{what} = {value} is out of range ({min}..={max})")]
    OutOfRange { what: &'static str, value: i64, min: i64, max: i64 },
    #[error("{what} = {value} is out of range ({allowed})")]
    OutOfRangeF { what: &'static str, value: f64, allowed: &'static str },
    #[error("{what} of 0 is refused")]
    Zero { what: &'static str },
    #[error("{what} {value:#x} is not a known value")]
    Invalid { what: &'static str, value: u32 },
    #[error("parameter byte {0:#04x} has bit 7 set")]
    NotSevenBit(u8),
    #[error("name {0:?} must be ASCII without NUL")]
    BadName(String),
    #[error("malformed command at byte {offset}: {bytes:02X?}")]
    Malformed { offset: usize, bytes: Vec<u8> },
    #[error("command {bytes:02X?} should be {expected} bytes long")]
    Length { bytes: Vec<u8>, expected: usize },
    #[error("at byte {offset}: {source}")]
    At { offset: usize, source: Box<RuidaError> },
    #[error("payload of {len} bytes (1..={max} allowed)")]
    PayloadSize { len: usize, max: usize },
    #[error("checksum {got:#06x} does not match {expected:#06x}")]
    Checksum { expected: u16, got: u16 },
    #[error("{0} layers (at most {max})", max = rd::MAX_LAYERS)]
    TooManyLayers(usize),
    #[error("could not find the scrambling key")]
    NoMagic,
}

pub type Result<T, E = RuidaError> = std::result::Result<T, E>;
