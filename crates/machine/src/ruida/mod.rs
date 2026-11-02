//! Ruida DSP controllers (RDC644x / 634x CO2 gantries) over Ethernet: the third machine driver (see
//! [`crate::driver`]), on the `ruida` crate's protocol layer.
//!
//! - [`RuidaProfile`]: controller address, scrambling key, power and speed limits, bed and origin corner.
//! - [`RuidaProgram`] and [`check_program`]: what a run streams or Send stores, checked against the profile before
//!   anything is sent. Made from `ruida::rd::RdJob` (the engine converts the planner's polylines).
//! - [`RuidaController`]: init, status, light-only framing, armed runs, the controller's file list (Send, list, start
//!   a stored file with an armed token, delete) and Stop. See its docs for the safety rules.
//! - [`RuidaLink`]: one payload, stop-and-wait; [`UdpRuidaLink`] (the network), [`SimRuida`] (`ruida::sim`) and
//!   [`OfflineRuida`].
//!
//! The protocol facts come from MeerK40t's Ruida module (MIT; see the `ruida` crate). **Nothing here has run against a
//! Ruida controller.**

mod controller;
mod link;
mod profile;
mod program;

pub use controller::{MAX_FILES, MAX_FRAME_MOVES, RuidaController, RuidaInfo, RuidaPoll, RuidaSnapshot, StoredFile};
pub use link::{OfflineRuida, RuidaLink, SimRuida, SimRuidaHandle, SimRuidaReport, UdpRuidaLink};
pub use profile::{MAX_SPEED_MM_S, RuidaProfile, mirror_of};
pub use program::{MAX_FILE_NAME, MAX_PROGRAM_BYTES, ProgramError, Purpose, RuidaProgram, check_name, check_program};

/// What went wrong on a Ruida controller.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RuidaDriverError {
    /// The program failed [`check_program`] (or a file name [`check_name`]).
    #[error(transparent)]
    Program(#[from] ProgramError),
    /// The controller runs a job of its own (from its panel or another program).
    #[error("the Ruida controller is running a job; wait for it or stop it first")]
    JobRunning,
    /// The status register showed a running job while a file was being sent: the upload was stopped.
    #[error("the controller started the job while it was being sent instead of storing it; it was stopped (D8 01)")]
    RanWhileStoring,
    /// The file list changed since it was read.
    #[error("{0}")]
    FileChanged(String),
    /// A Send failed part way and its half file could not be removed from the controller.
    #[error("{0}")]
    PartialFile(String),
    /// The upload ended but the file list does not show the file.
    #[error("{0}")]
    NotStored(String),
    /// The controller answered something unexpected.
    #[error("Ruida: {0}")]
    Reply(String),
}
