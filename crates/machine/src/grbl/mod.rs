//! GRBL 1.1 lasers over a serial port: the second machine driver (see [`crate::driver`]).
//!
//! - [`GrblProfile`]: port, baud, `$30`/`$31` power range, power and feed limits, bed and origin corner.
//! - [`GcodeProgram`] and [`check_program`]: what a run streams, checked against the profile before anything is
//!   sent. The program itself is made by `plan::gcode` from the planner's polylines.
//! - [`GrblController`]: init, status, light-only framing, armed runs with character-counting flow control, pause
//!   and resume, Stop, homing, jogging and the console. See its docs for the safety rules.
//! - [`SimGrbl`]: a GRBL simulator for tests and the engine's simulator mode.
//! - [`SerialLink`]: the byte transport; `SerialPortLink` (feature `serial`, the `serial2` crate) is the real port.
//!
//! GRBL is GPL-licensed: only its documented protocol is used here (see [`protocol`]), never its code.

mod console;
mod controller;
mod profile;
mod program;
pub mod protocol;
#[cfg(feature = "serial")]
mod serial;
mod sim;

use std::time::Duration;

pub use console::{ConsoleKind, MAX_CONSOLE_LINE, check_console_line};
pub use controller::{ConsoleReply, GrblController, GrblInfo, GrblPoll, GrblSnapshot, MAX_FRAME_LINES};
pub use profile::{GrblProfile, MAX_FEED_MM_MIN, MAX_S_VALUE};
pub use program::{GcodeProgram, MAX_PROGRAM_LINES, ProgramError, Word, check_program, parse_words, strip_comments};
pub use protocol::{GrblState, RX_BUFFER, StatusReport, alarm_text, error_text};
#[cfg(feature = "serial")]
pub use serial::{SerialPortLink, available_ports};
pub use sim::{Fire, FireKind, PLANNER_BLOCKS, RealtimeEvent, SimGrbl, SimGrblConfig, SimGrblHandle, SimGrblReport};

use crate::link::LinkError;

/// A byte stream to a GRBL board. Like [`crate::Link`] it only moves bytes; flow control and safety live in
/// [`GrblController`].
pub trait SerialLink: Send {
    /// Write all of `bytes`.
    fn write(&mut self, bytes: &[u8]) -> Result<(), LinkError>;

    /// Read what has arrived, waiting at most `timeout` for the first byte; `Ok(0)` when nothing came.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, LinkError>;
}

impl<S: SerialLink + ?Sized> SerialLink for Box<S> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        (**self).write(bytes)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, LinkError> {
        (**self).read(buf, timeout)
    }
}

/// No port opened: every transfer fails with [`LinkError::Disconnected`].
#[derive(Debug, Default)]
pub struct OfflineSerial;

impl SerialLink for OfflineSerial {
    fn write(&mut self, _bytes: &[u8]) -> Result<(), LinkError> {
        Err(LinkError::Disconnected)
    }

    fn read(&mut self, _buf: &mut [u8], _timeout: Duration) -> Result<usize, LinkError> {
        Err(LinkError::Disconnected)
    }
}

/// What went wrong on a GRBL board.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GrblError {
    /// The board refused a line (`error:N`).
    #[error("GRBL refused \"{line}\": error {code} ({})", error_text(*code))]
    Rejected { code: u8, line: String },
    /// The board raised an alarm (`ALARM:N`); it is locked until unlocked or homed.
    #[error("GRBL alarm {code}: {text}", code = .0, text = alarm_text(*.0))]
    Alarm(u8),
    /// The board is in the alarm state (locked) and the request needs it unlocked.
    #[error("GRBL is locked (alarm{}); home it or unlock it ($X) first", .0.map(|c| format!(" {c}: {}", alarm_text(c))).unwrap_or_default())]
    Locked(Option<u8>),
    /// The safety door is open.
    #[error("the safety door is open")]
    DoorOpen,
    /// The board is busy with something else (`Run`, `Hold`, `Home`, ...).
    #[error("GRBL is {0}, not idle")]
    NotIdle(String),
    /// No answer in time.
    #[error("GRBL did not answer within {0:?}")]
    NoResponse(Duration),
    /// No start-up message after a reset: not a GRBL board, the wrong port or baud rate.
    #[error("no GRBL start-up message within {0:?} (wrong port or baud rate, or not a GRBL board)")]
    NoWelcome(Duration),
    /// The board reset while a run or frame was streaming.
    #[error("GRBL reset unexpectedly")]
    Reset,
    /// The board's settings or start-up blocks do not allow the request.
    #[error("{0}")]
    Settings(String),
    /// The program failed [`check_program`].
    #[error(transparent)]
    Program(#[from] ProgramError),
    /// A console line was refused by the console policy ([`check_console_line`]).
    #[error("console: {0}")]
    Console(String),
}
