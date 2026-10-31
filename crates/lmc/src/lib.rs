//! LMC wire protocol: opcodes, commands, packets, unit conversions, list builders, decoder. Pure, no I/O.
//!
//! The BJJCZ LMC board (EZCAD2 family, USB 9588:9899) takes two kinds of traffic:
//!
//! - **Single commands** ([`Single`]): six little-endian u16 words (12 bytes), executed immediately. All but
//!   `WriteCorLine` answer with an 8-byte reply whose word 3 is the [`StatusWord`].
//! - **List packets** ([`Packet`]): exactly 3072 bytes = 256 list commands of 12 bytes, queued and run by the board.
//!   Unused slots are padded with `listEndOfList` (`0x8002`), as galvoplotter does.
//!
//! Lists are built with [`MarkingList`] (everything, including emission) or [`LightList`] (red-pointer framing,
//! which cannot express emission at all). [`decode`], [`disassemble`], [`trace`] and [`emits_light_only`] read
//! packets back.
//!
//! Galvo units: `0x0000..=0xFFFF` across `field_mm`, centre [`CENTER`]. Public conversions take mm, mm/s, kHz,
//! percent and µs (see [`units`]) and refuse values they cannot represent instead of clamping or wrapping.
//!
//! Protocol facts and several encoders are ported from openlaser-rs @ 9d81a15 and meerk40t/galvoplotter @ fc35136
//! (both MIT); each ported file carries the notice.
#![forbid(unsafe_code)]
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]

mod command;
mod decode;
mod light;
mod list;
pub mod op;
mod packet;
mod single;
pub mod units;

pub use command::ListCommand;
pub use decode::{TraceKind, TraceSegment, TraceState, decode, decode_packet, disassemble, emits_light_only, trace, trace_from};
pub use light::{LightList, RED_POINTER_BIT};
pub use list::MarkingList;
pub use packet::{COMMAND_LEN, PACKET_LEN, Packet, SLOTS};
pub use single::{REPLY_LEN, Single, StatusWord, parse_reply};
pub use units::{CENTER, DelayWord, FieldTransform, JPT_M7_PULSE_WIDTHS_NS, PulseDecision};

/// Port bit 0: the laser-enable output. [`LightList`] can never write it.
pub const LASER_ENABLE_BIT: u16 = 0x0001;

/// Everything this crate refuses.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LmcError {
    #[error("{what} is not finite")]
    NonFinite { what: &'static str },
    /// The value cannot be represented in its word (never clamped or wrapped).
    #[error("{what} = {value} is out of range ({allowed})")]
    OutOfRange { what: &'static str, value: f64, allowed: &'static str },
    /// A motion speed or Q-switch period of 0 would stall or divide by zero on the board.
    #[error("{what} of 0 is refused")]
    Zero { what: &'static str },
    #[error("pulse width {requested} ns is not in the source's table (nearest is {nearest} ns)")]
    PulseWidth { requested: f64, nearest: u16 },
    #[error("an LMC list packet is exactly {PACKET_LEN} bytes, got {0}")]
    PacketLength(usize),
    #[error("an LMC list packet holds at most {SLOTS} commands, got {0}")]
    TooManyCommands(usize),
    #[error("an LMC reply is exactly {REPLY_LEN} bytes, got {0}")]
    ReplyLength(usize),
}

pub type Result<T, E = LmcError> = std::result::Result<T, E>;
