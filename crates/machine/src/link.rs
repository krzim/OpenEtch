//! [`Link`]: the transport between the [`crate::Controller`] and a board (real USB or simulated).
//!
//! A link only moves bytes: it does no flow control, no retries and no safety checks. Those live in the controller.

use lmc::{Packet, Single};

/// The four words of an 8-byte reply. Word 3 is the board status ([`lmc::StatusWord::from_reply`]).
pub type Reply = [u16; 4];

/// Transport failures. None of them is retried by the link.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    /// A transfer did not complete within the link's timeout.
    #[error("USB transfer timed out")]
    Timeout,
    /// The board went away (cable, power, key switch).
    #[error("the board is disconnected")]
    Disconnected,
    /// The endpoint stalled.
    #[error("USB endpoint stalled")]
    Stall,
    /// No supported board was found.
    #[error("no board: {0}")]
    NotFound(String),
    /// A board was found but cannot be used (e.g. the 9588:9980 loader board).
    #[error("unsupported board: {0}")]
    Unsupported(String),
    /// A reply had the wrong length or another transfer-level problem.
    #[error("USB I/O: {0}")]
    Io(String),
}

/// Sends single commands and list packets to one board.
pub trait Link: Send {
    /// Send one 12-byte single command. Returns the 8-byte reply's words, or `None` for the one command that has no
    /// reply (`WriteCorLine`, see [`Single::reads_reply`]).
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError>;

    /// Send one 3072-byte list packet (no reply).
    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError>;
}

impl<L: Link + ?Sized> Link for Box<L> {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        (**self).send_single(cmd)
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        (**self).send_packet(packet)
    }
}
