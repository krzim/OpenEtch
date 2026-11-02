//! The machine-driver interface: what every laser controller OpenEtch drives does ([`MachineDriver`]), and the
//! engine's choice of driver at run time ([`Driver`]).
//!
//! **Why a trait plus an enum.** The trait is the contract every driver keeps, with the job type each one runs:
//! state, light-only framing and its stop, armed runs, the never-gated abort and the handles a stop and pause
//! button use from another thread. It lets the contract be written once and tests be run against any driver. The
//! engine holds a [`Driver`] (enum dispatch) rather than a `Box<dyn MachineDriver>` because the drivers' jobs differ
//! (LMC list packets in parts, G-code lines, Ruida programs and stored files), as do their extra abilities (the
//! galvo's input port and extension axis; GRBL's homing, jogging and console; Ruida's file list): an enum keeps each job and ability typed, a mismatched job is refused
//! in one place ([`Driver::run_job`]), and adding a driver fails to compile until every safety path (abort, frame,
//! run) handles it. The set of drivers is closed and in-tree, so nothing is lost by not being open.
//!
//! The galvo's [`Controller`] is one implementation, unchanged: [`Driver::Lmc`] calls straight through to it, so the
//! bytes on the wire are those of the controller alone.

use std::sync::Arc;

use crate::armed::Armed;
use crate::controller::{AbortHandle, BoardInfo, Controller, ControllerError, ControllerState, FrameStyle, RunControl, RunPart, RunReport};
use crate::grbl::{GcodeProgram, GrblController, SerialLink};
use crate::link::Link;
use crate::ruida::{RuidaController, RuidaLink, RuidaProgram};

type Result<T, E = ControllerError> = std::result::Result<T, E>;

/// Which protocol a driver speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverKind {
    /// BJJCZ LMC galvo boards (EzCad2 family) over USB.
    Lmc,
    /// GRBL 1.1 gantries over a serial port.
    Grbl,
    /// Ruida DSP controllers over UDP.
    Ruida,
}

impl DriverKind {
    pub fn name(self) -> &'static str {
        match self {
            DriverKind::Lmc => "lmc",
            DriverKind::Grbl => "grbl",
            DriverKind::Ruida => "ruida",
        }
    }
}

/// What every machine driver does. The safety rules (see ARCHITECTURE.md, "Safety model") hold for every
/// implementation:
///
/// - [`MachineDriver::run_job`] is the only method that can fire the laser, and it takes an [`Armed`] token by
///   value (made only by the engine's approval module); it checks the job against the profile's hard limits before
///   anything is sent.
/// - [`MachineDriver::frame_styled`] never fires: a red pointer on a galvo, the laser off on a gantry.
/// - [`MachineDriver::abort`] is never gated, needs no init and tries every step; an [`AbortHandle`] request is
///   noticed by a running job or frame at its next step and wins over a pause.
pub trait MachineDriver: Send {
    /// What a run of this driver streams.
    type Job: ?Sized;

    fn kind(&self) -> DriverKind;

    fn state(&self) -> ControllerState;

    /// Requests a stop from another thread while a run or frame holds the driver.
    fn abort_handle(&self) -> AbortHandle;

    /// Pause, resume and progress of runs, from another thread.
    fn run_control(&self) -> RunControl;

    /// Poll the board: whether it is idle (a finished frame returns the driver to idle).
    fn poll_idle(&mut self) -> Result<bool>;

    /// Trace `outline` (polylines in field mm) `loops` times without firing.
    fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize>;

    /// End a frame.
    fn stop_frame(&mut self) -> Result<()>;

    /// Run a job; needs an [`Armed`] token.
    fn run_job(&mut self, job: &Self::Job, armed: Armed) -> Result<RunReport>;

    /// Stop everything, always.
    fn abort(&mut self) -> Result<()>;
}

impl<L: Link> MachineDriver for Controller<L> {
    type Job = [RunPart];

    fn kind(&self) -> DriverKind {
        DriverKind::Lmc
    }

    fn state(&self) -> ControllerState {
        Controller::state(self)
    }

    fn abort_handle(&self) -> AbortHandle {
        Controller::abort_handle(self)
    }

    fn run_control(&self) -> RunControl {
        Controller::run_control(self)
    }

    fn poll_idle(&mut self) -> Result<bool> {
        Controller::status(self).map(|s| s.idle())
    }

    fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        Controller::frame_styled(self, outline, loops, style)
    }

    fn stop_frame(&mut self) -> Result<()> {
        Controller::stop_frame(self)
    }

    fn run_job(&mut self, job: &[RunPart], armed: Armed) -> Result<RunReport> {
        Controller::run_parts(self, job, armed)
    }

    fn abort(&mut self) -> Result<()> {
        Controller::abort(self)
    }
}

impl<P: SerialLink> MachineDriver for GrblController<P> {
    type Job = GcodeProgram;

    fn kind(&self) -> DriverKind {
        DriverKind::Grbl
    }

    fn state(&self) -> ControllerState {
        GrblController::state(self)
    }

    fn abort_handle(&self) -> AbortHandle {
        GrblController::abort_handle(self)
    }

    fn run_control(&self) -> RunControl {
        GrblController::run_control(self)
    }

    fn poll_idle(&mut self) -> Result<bool> {
        GrblController::poll_idle(self)
    }

    fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        GrblController::frame_styled(self, outline, loops, style)
    }

    fn stop_frame(&mut self) -> Result<()> {
        GrblController::stop_frame(self)
    }

    fn run_job(&mut self, job: &GcodeProgram, armed: Armed) -> Result<RunReport> {
        GrblController::run_program(self, job, armed)
    }

    fn abort(&mut self) -> Result<()> {
        GrblController::abort(self)
    }
}

impl<L: RuidaLink> MachineDriver for RuidaController<L> {
    type Job = RuidaProgram;

    fn kind(&self) -> DriverKind {
        DriverKind::Ruida
    }

    fn state(&self) -> ControllerState {
        RuidaController::state(self)
    }

    fn abort_handle(&self) -> AbortHandle {
        RuidaController::abort_handle(self)
    }

    fn run_control(&self) -> RunControl {
        RuidaController::run_control(self)
    }

    fn poll_idle(&mut self) -> Result<bool> {
        RuidaController::poll_idle(self)
    }

    fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        RuidaController::frame_styled(self, outline, loops, style)
    }

    fn stop_frame(&mut self) -> Result<()> {
        RuidaController::stop_frame(self)
    }

    fn run_job(&mut self, job: &RuidaProgram, armed: Armed) -> Result<RunReport> {
        RuidaController::run_program(self, job, armed)
    }

    fn abort(&mut self) -> Result<()> {
        RuidaController::abort(self)
    }
}

/// The LMC controller as the engine holds it: any link, boxed.
pub type LmcDriver = Controller<Box<dyn Link>>;
/// The GRBL controller as the engine holds it: any serial link, boxed.
pub type GrblDriver = GrblController<Box<dyn SerialLink>>;
/// The Ruida controller as the engine holds it: any link, boxed.
pub type RuidaDriver = RuidaController<Box<dyn RuidaLink>>;

/// The driver the engine holds (see the module docs for why an enum).
pub enum Driver {
    Lmc(LmcDriver),
    Grbl(GrblDriver),
    Ruida(RuidaDriver),
}

/// A run's job for one of the drivers.
#[derive(Debug, Clone)]
pub enum DriverJob {
    /// LMC list packets in parts (rotary strips, Z positions; one part for a plain job).
    Lmc(Arc<Vec<RunPart>>),
    /// A G-code program for GRBL.
    Grbl(Arc<GcodeProgram>),
    /// A Ruida program (an `.rd` job streamed and run).
    Ruida(Arc<RuidaProgram>),
    /// A file stored on the Ruida controller, started by its 1-based index once its name is checked again.
    RuidaFile { index: u16, name: String },
}

impl DriverJob {
    pub fn kind(&self) -> DriverKind {
        match self {
            DriverJob::Lmc(_) => DriverKind::Lmc,
            DriverJob::Grbl(_) => DriverKind::Grbl,
            DriverJob::Ruida(_) | DriverJob::RuidaFile { .. } => DriverKind::Ruida,
        }
    }
}

impl std::fmt::Debug for Driver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Driver").field("kind", &self.kind()).field("state", &self.state()).finish()
    }
}

impl Driver {
    /// The galvo controller, when that is the driver.
    pub fn lmc(&self) -> Option<&LmcDriver> {
        match self {
            Driver::Lmc(c) => Some(c),
            Driver::Grbl(_) | Driver::Ruida(_) => None,
        }
    }

    pub fn lmc_mut(&mut self) -> Option<&mut LmcDriver> {
        match self {
            Driver::Lmc(c) => Some(c),
            Driver::Grbl(_) | Driver::Ruida(_) => None,
        }
    }

    pub fn grbl(&self) -> Option<&GrblDriver> {
        match self {
            Driver::Grbl(g) => Some(g),
            Driver::Lmc(_) | Driver::Ruida(_) => None,
        }
    }

    pub fn grbl_mut(&mut self) -> Option<&mut GrblDriver> {
        match self {
            Driver::Grbl(g) => Some(g),
            Driver::Lmc(_) | Driver::Ruida(_) => None,
        }
    }

    pub fn ruida(&self) -> Option<&RuidaDriver> {
        match self {
            Driver::Ruida(r) => Some(r),
            Driver::Lmc(_) | Driver::Grbl(_) => None,
        }
    }

    pub fn ruida_mut(&mut self) -> Option<&mut RuidaDriver> {
        match self {
            Driver::Ruida(r) => Some(r),
            Driver::Lmc(_) | Driver::Grbl(_) => None,
        }
    }

    fn not_on(&self, what: &str) -> ControllerError {
        ControllerError::Refused(format!("{what} is not available on a {} machine", self.kind().name()))
    }

    /// The galvo board's input port (`ReadPort`); GRBL boards have none.
    pub fn read_port(&mut self) -> Result<u16> {
        match self {
            Driver::Lmc(c) => c.read_port(),
            Driver::Grbl(_) | Driver::Ruida(_) => Err(self.not_on("the input port")),
        }
    }

    /// The galvo's extension axis (Z lift, rotary, table).
    pub fn move_axis(&mut self, position: u16) -> Result<()> {
        match self {
            Driver::Lmc(c) => c.move_axis(position),
            Driver::Grbl(_) | Driver::Ruida(_) => Err(self.not_on("the extension axis")),
        }
    }

    /// The galvo board's identification from its last init.
    pub fn board_info(&self) -> Option<&BoardInfo> {
        self.lmc().and_then(Controller::board_info)
    }

    /// Read the galvo board's identification again.
    pub fn refresh_info(&mut self) -> Result<BoardInfo> {
        match self {
            Driver::Lmc(c) => c.refresh_info(),
            Driver::Grbl(_) | Driver::Ruida(_) => Err(self.not_on("board info")),
        }
    }

    /// GRBL's safety door (a status report); `None` on a galvo (its door is an input bit of the profile).
    pub fn door_open(&mut self) -> Option<Result<bool>> {
        self.grbl_mut().map(GrblController::door_open)
    }
}

impl MachineDriver for Driver {
    type Job = DriverJob;

    fn kind(&self) -> DriverKind {
        match self {
            Driver::Lmc(_) => DriverKind::Lmc,
            Driver::Grbl(_) => DriverKind::Grbl,
            Driver::Ruida(_) => DriverKind::Ruida,
        }
    }

    fn state(&self) -> ControllerState {
        match self {
            Driver::Lmc(c) => c.state(),
            Driver::Grbl(g) => g.state(),
            Driver::Ruida(r) => r.state(),
        }
    }

    fn abort_handle(&self) -> AbortHandle {
        match self {
            Driver::Lmc(c) => c.abort_handle(),
            Driver::Grbl(g) => g.abort_handle(),
            Driver::Ruida(r) => r.abort_handle(),
        }
    }

    fn run_control(&self) -> RunControl {
        match self {
            Driver::Lmc(c) => c.run_control(),
            Driver::Grbl(g) => g.run_control(),
            Driver::Ruida(r) => r.run_control(),
        }
    }

    fn poll_idle(&mut self) -> Result<bool> {
        match self {
            Driver::Lmc(c) => MachineDriver::poll_idle(c),
            Driver::Grbl(g) => g.poll_idle(),
            Driver::Ruida(r) => r.poll_idle(),
        }
    }

    fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        match self {
            Driver::Lmc(c) => c.frame_styled(outline, loops, style),
            Driver::Grbl(g) => g.frame_styled(outline, loops, style),
            Driver::Ruida(r) => r.frame_styled(outline, loops, style),
        }
    }

    fn stop_frame(&mut self) -> Result<()> {
        match self {
            Driver::Lmc(c) => c.stop_frame(),
            Driver::Grbl(g) => g.stop_frame(),
            Driver::Ruida(r) => r.stop_frame(),
        }
    }

    /// Run `job` on its own driver; a job made for the other driver is refused before anything is sent (the run was
    /// planned for another device).
    fn run_job(&mut self, job: &DriverJob, armed: Armed) -> Result<RunReport> {
        match (self, job) {
            (Driver::Lmc(c), DriverJob::Lmc(parts)) => c.run_parts(parts, armed),
            (Driver::Grbl(g), DriverJob::Grbl(program)) => g.run_program(program, armed),
            (Driver::Ruida(r), DriverJob::Ruida(program)) => r.run_program(program, armed),
            (Driver::Ruida(r), DriverJob::RuidaFile { index, name }) => r.start_file(*index, name, armed),
            (d, j) => Err(ControllerError::Refused(format!(
                "a {} job cannot run on a {} machine; request the run again",
                j.kind().name(),
                d.kind().name()
            ))),
        }
    }

    fn abort(&mut self) -> Result<()> {
        match self {
            Driver::Lmc(c) => c.abort(),
            Driver::Grbl(g) => g.abort(),
            Driver::Ruida(r) => r.abort(),
        }
    }
}
