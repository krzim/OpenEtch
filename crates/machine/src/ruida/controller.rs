//! [`RuidaController`]: init, status, light-only framing, armed runs, the controller's file list (Send, list, start,
//! delete) and Stop of one Ruida DSP controller over a [`RuidaLink`].
//!
//! Safety rules this module enforces (the galvo [`crate::Controller`]'s, carried over):
//!
//! - **Only an armed run fires.** [`RuidaController::run_program`] and [`RuidaController::start_file`] (a file stored on
//!   the controller) take an [`Armed`] token, made only by the engine's approval module. A program is checked against
//!   the profile first ([`check_program`]: only a plain job's commands, powers within `max_power_pct`, speeds within
//!   `max_speed_mm_s`, every point on the bed). Before either starts, the controller's status register must show no
//!   job running. A stored file cannot be read back from a controller, so starting one is checked only as far as
//!   its name at that index (the file list may have changed since it was shown); OpenEtch's own Send checked it when
//!   it was stored.
//! - **Send stores, never runs** ([`RuidaController::upload_file`]): the program starts with its file name (`E7 01`),
//!   which makes a controller store it (MeerK40t's emulator; unverified on hardware). Because a controller that ran the
//!   upload instead would fire without an approval, the status register is read after every datagram and the
//!   upload is stopped (`D8 01`) the moment it shows a running job.
//! - **Framing is light-only**: rapid moves (`D9 10`) with the option byte that asks for nothing (`02`), never a cut
//!   and never the red-light options (their meaning is unverified).
//! - **Stop always wins** ([`RuidaController::abort`]): stop process (`D8 01`), sent at once; an [`AbortHandle`]
//!   request is noticed between datagrams and between status polls. Any error during a run, and a run left early or by
//!   a panic (a drop guard), sends the same.
//! - **Pause** is pause process (`D8 02`), resume restore process (`D8 03`).
//! - The controller's own door and water protection stay the controller's: OpenEtch cannot read them (no status bit
//!   for them is known).
//!
//! Nothing here has run against a Ruida controller: it is tested on `ruida::sim` only.

use std::time::{Duration, Instant};

use ruida::command::{Command, DocKind, RapidOption};
use ruida::frame::{MAX_UDP_PAYLOAD, Reply, packetize};
use ruida::mem::{self, MemAddr};
use serde::Serialize;

use super::link::RuidaLink;
use super::profile::RuidaProfile;
use super::program::{Purpose, RuidaProgram, check_name, check_program};
use super::RuidaDriverError;
use crate::armed::Armed;
use crate::controller::{AbortHandle, ControllerError, ControllerState, FrameStyle, MAX_FRAME_LOOPS, RunControl, RunReport};

type Result<T, E = ControllerError> = std::result::Result<T, E>;

/// Most rapid moves one frame call may send.
pub const MAX_FRAME_MOVES: usize = 20_000;
/// Most files read from a controller's list.
pub const MAX_FILES: u64 = 1000;

/// Timing of the Ruida driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuidaPoll {
    /// How often a run waiting for the controller reads its status.
    pub status_interval: Duration,
    /// How long a started job may show no "running" bit before it counts as finished (a controller sets it a moment
    /// after the end of file).
    pub start_grace: Duration,
    /// Longest wait for a job after its last datagram; `None` waits while the controller reports it running.
    pub finish_timeout: Option<Duration>,
}

impl Default for RuidaPoll {
    fn default() -> Self {
        RuidaPoll { status_interval: Duration::from_millis(250), start_grace: Duration::from_secs(2), finish_timeout: None }
    }
}

impl RuidaPoll {
    /// No waiting, for the simulator (it runs a job as it arrives).
    pub fn immediate() -> Self {
        RuidaPoll { status_interval: Duration::ZERO, start_grace: Duration::ZERO, finish_timeout: Some(Duration::from_secs(5)) }
    }
}

/// A file stored on the controller: its 1-based index in the controller's list, and its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredFile {
    pub index: u16,
    pub name: String,
}

/// What `init` learnt.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuidaInfo {
    /// The card ID register (`0x02FE`), hex.
    pub card_id: String,
    /// A job was running on the controller.
    pub running: bool,
}

/// The controller as last seen, for the status page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuidaSnapshot {
    pub card_id: Option<String>,
    /// The status register (`0x0200`).
    pub status: Option<u64>,
    pub running: bool,
    pub moving: bool,
    /// Machine-frame head position, mm.
    pub position_mm: Option<[f64; 2]>,
}

/// Drives one Ruida controller. See the module docs.
#[derive(Debug)]
pub struct RuidaController<L: RuidaLink> {
    link: L,
    poll: RuidaPoll,
    profile: Option<RuidaProfile>,
    state: ControllerState,
    abort: AbortHandle,
    control: RunControl,
    /// Ends a frame being sent (set by the engine without the driver lock); cleared when a frame starts.
    frame_cancel: AbortHandle,
    card_id: Option<u64>,
    status: Option<u64>,
    position_um: Option<[i32; 2]>,
}

fn reply_error(what: &str, got: Option<Reply>) -> ControllerError {
    RuidaDriverError::Reply(format!("{what}: unexpected answer {got:?}")).into()
}

impl<L: RuidaLink> RuidaController<L> {
    pub fn new(link: L) -> Self {
        Self::with_handles(link, RuidaPoll::default(), AbortHandle::default(), RunControl::default())
    }

    /// A controller sharing existing abort and run-control handles (see [`crate::Controller::with_handles`]).
    pub fn with_handles(link: L, poll: RuidaPoll, abort: AbortHandle, control: RunControl) -> Self {
        RuidaController {
            link,
            poll,
            profile: None,
            state: ControllerState::Uninitialized,
            abort,
            control,
            frame_cancel: AbortHandle::default(),
            card_id: None,
            status: None,
            position_um: None,
        }
    }

    /// Swap in a new link (a freshly opened one, or [`super::OfflineRuida`]); the controller is then uninitialised.
    pub fn replace_link(&mut self, link: L) {
        self.link = link;
        self.profile = None;
        self.state = ControllerState::Uninitialized;
    }

    pub fn profile(&self) -> Option<&RuidaProfile> {
        self.profile.as_ref()
    }

    pub fn state(&self) -> ControllerState {
        self.state
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    pub fn run_control(&self) -> RunControl {
        self.control.clone()
    }

    /// Share a frame-cancel flag (see [`crate::grbl::GrblController::set_frame_cancel`]).
    pub fn set_frame_cancel(&mut self, cancel: AbortHandle) {
        self.frame_cancel = cancel;
    }

    pub fn snapshot(&self) -> RuidaSnapshot {
        RuidaSnapshot {
            card_id: self.card_id.map(|c| format!("{c:#010x}")),
            status: self.status,
            running: self.status.is_some_and(|s| s & mem::STATUS_JOB_RUNNING != 0),
            moving: self.status.is_some_and(|s| s & mem::STATUS_MOVING != 0),
            position_mm: self.position_um.map(|p| [f64::from(p[0]) / 1000.0, f64::from(p[1]) / 1000.0]),
        }
    }

    // ----- transport -----

    fn command(&mut self, c: &Command) -> Result<Option<Reply>> {
        let bytes = c.encode().map_err(|e| ControllerError::Refused(e.to_string()))?;
        Ok(self.link.send_payload(&bytes)?)
    }

    fn read_memory(&mut self, addr: MemAddr) -> Result<u64> {
        match self.command(&Command::GetMemory(addr))? {
            Some(Reply::Memory { addr: a, value, .. }) if a == addr => Ok(value),
            other => Err(reply_error(&format!("reading register {:#06x}", addr.get()), other)),
        }
    }

    fn read_status(&mut self) -> Result<u64> {
        let s = self.read_memory(mem::MACHINE_STATUS)?;
        self.status = Some(s);
        Ok(s)
    }

    fn check_abort(&self) -> Result<()> {
        if self.abort.is_requested() { Err(ControllerError::Aborted) } else { Ok(()) }
    }

    /// Refuse while the controller runs a job of its own (started from its panel or by another program).
    fn require_no_job(&mut self) -> Result<()> {
        if self.read_status()? & mem::STATUS_JOB_RUNNING != 0 {
            return Err(RuidaDriverError::JobRunning.into());
        }
        Ok(())
    }

    // ----- connect and status -----

    /// Connect-time setup: a keepalive, the card ID and the status register. Nothing here moves or fires.
    pub fn init(&mut self, profile: &RuidaProfile) -> Result<RuidaInfo> {
        profile.validate()?;
        self.profile = None;
        self.state = ControllerState::Uninitialized;
        self.abort.clear();
        self.link.send_payload(&[0xCE])?;
        let card = self.read_memory(mem::CARD_ID)?;
        self.card_id = Some(card);
        let status = self.read_status()?;
        self.profile = Some(profile.clone());
        self.state = ControllerState::Idle;
        Ok(RuidaInfo { card_id: format!("{card:#010x}"), running: status & mem::STATUS_JOB_RUNNING != 0 })
    }

    /// Read the status register and the head position.
    pub fn refresh(&mut self) -> Result<RuidaSnapshot> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        self.read_status()?;
        let x = mem::position_um(self.read_memory(mem::POSITION_X)?);
        let y = mem::position_um(self.read_memory(mem::POSITION_Y)?);
        self.position_um = Some([x, y]);
        Ok(self.snapshot())
    }

    /// Whether the head stands still with no job running (ends a frame).
    pub fn poll_idle(&mut self) -> Result<bool> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        let s = self.read_status()?;
        let idle = s & (mem::STATUS_MOVING | mem::STATUS_JOB_RUNNING) == 0;
        if idle && self.state == ControllerState::Framing {
            self.state = ControllerState::Idle;
        }
        Ok(idle)
    }

    // ----- framing -----

    /// Trace `outline` (polylines in field mm) `loops` times with rapid moves (`D9 10`, option `02`: no light, no
    /// cut) at the controller's own travel speed (the style's speed, blanking and pointer do not apply). Refused off
    /// the bed. Sends every move, then returns (the controller finishes them on its own).
    pub fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, _style: &FrameStyle) -> Result<usize> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if !(1..=MAX_FRAME_LOOPS).contains(&loops) {
            return Err(ControllerError::Refused(format!("loops must be 1..={MAX_FRAME_LOOPS}, got {loops}")));
        }
        let mut points = Vec::new();
        for poly in outline {
            for &(x, y) in poly {
                let (mx, my) = profile.to_machine(x, y).map_err(ControllerError::Refused)?;
                let um = |v: f64| ruida::num::mm_to_um(v, "frame point").map_err(|e| ControllerError::Refused(e.to_string()));
                points.push((um(mx)?, um(my)?));
            }
        }
        if points.is_empty() {
            return Err(ControllerError::Refused("nothing to frame: the outline is empty".into()));
        }
        if points.len().saturating_mul(loops as usize) > MAX_FRAME_MOVES {
            return Err(ControllerError::Refused(format!("frame too large: {} points x {loops} loops", points.len())));
        }
        let mut moves = Vec::with_capacity(points.len() * loops as usize);
        for _ in 0..loops {
            moves.extend(points.iter().map(|&(x, y)| Command::RapidMoveXy { option: RapidOption::None, x, y }));
        }
        let payloads = ruida::encode_all(&moves)
            .and_then(|b| packetize(&b, MAX_UDP_PAYLOAD))
            .map_err(|e| ControllerError::Refused(e.to_string()))?;
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        self.require_no_job()?;
        self.frame_cancel.clear();
        self.state = ControllerState::Framing;
        for p in &payloads {
            if self.abort.is_requested() {
                self.abort_after(&ControllerError::Aborted);
                return Err(ControllerError::Aborted);
            }
            if self.frame_cancel.is_requested() {
                self.stop_frame()?;
                return Ok(0);
            }
            if let Err(e) = self.link.send_payload(p) {
                let _ = self.stop_frame();
                return Err(e.into());
            }
        }
        Ok(moves.len())
    }

    /// End a frame: stop process (`D8 01`) stops the head.
    pub fn stop_frame(&mut self) -> Result<()> {
        let r = self.command(&Command::StopProcess).map(|_| ());
        if self.state == ControllerState::Framing {
            self.state = ControllerState::Idle;
        }
        r
    }

    // ----- runs -----

    /// Before a run, a start or a Send: initialised, not running, no frame, no stop request, no job on the controller.
    fn ready_for_job(&mut self) -> Result<RuidaProfile> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        self.require_no_job()?;
        Ok(profile)
    }

    /// Run a program. Requires an [`Armed`] token, which only the engine's approval creates.
    ///
    /// Order: [`check_program`] against the profile (nothing is sent if it fails); a frame is stopped; a pending stop
    /// request cancels the run before anything is sent; the controller must show no running job; then the datagrams
    /// go out one at a time (stop-and-wait), the stop request checked before each and pause/resume sent between them;
    /// then the status register is read until the job has finished. Any error aborts (`D8 01`).
    pub fn run_program(&mut self, program: &RuidaProgram, armed: Armed) -> Result<RunReport> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        check_program(&profile, program, Purpose::Run).map_err(RuidaDriverError::Program)?;
        let payloads = program.payloads().map_err(RuidaDriverError::Program)?;
        self.ready_for_job()?;
        tracing::info!(run_id = armed.run_id(), bytes = program.len(), datagrams = payloads.len(), "starting armed Ruida run");
        self.state = ControllerState::Running;
        self.control.start_count(payloads.len());
        let mut guard = RunGuard { ctl: self, done: false };
        let result = guard.ctl.send_payloads(&payloads, false).and_then(|()| guard.ctl.wait_finished());
        guard.ctl.finish(&result);
        guard.done = true;
        result.map(|()| RunReport { run_id: armed.run_id().to_owned(), packets: payloads.len(), mark_time: None })
    }

    /// Start file `index` stored on the controller (the File List's Start). Requires an [`Armed`] token. Refused unless
    /// the file at `index` is still named `name` (the list may have changed since it was read); then like
    /// [`RuidaController::run_program`] from the start command (`E8 03`) on.
    pub fn start_file(&mut self, index: u16, name: &str, armed: Armed) -> Result<RunReport> {
        if self.profile.is_none() {
            return Err(ControllerError::NotInitialized);
        }
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        self.ready_for_job()?;
        self.expect_name(index, name)?;
        tracing::info!(run_id = armed.run_id(), index, name, "starting armed Ruida stored file");
        self.state = ControllerState::Running;
        self.control.start_count(1);
        let mut guard = RunGuard { ctl: self, done: false };
        let result = guard
            .ctl
            .command(&Command::Document { kind: DocKind::Start, index })
            .and_then(|_| {
                guard.ctl.control.set_sent(1);
                guard.ctl.wait_finished()
            });
        guard.ctl.finish(&result);
        guard.done = true;
        result.map(|()| RunReport { run_id: armed.run_id().to_owned(), packets: 1, mark_time: None })
    }

    fn finish(&mut self, result: &Result<()>) {
        self.control.set_paused(false);
        match result {
            Err(e) => self.abort_after(e),
            Ok(()) => self.state = ControllerState::Idle,
        }
    }

    /// Pause or resume as asked; `paused` is the controller's state as last sent.
    fn pause_step(&mut self, paused: &mut bool) -> Result<()> {
        let want = self.control.pause_requested();
        if want && !*paused {
            self.command(&Command::PauseProcess)?;
            *paused = true;
            self.control.set_paused(true);
            tracing::info!("Ruida run paused");
        } else if !want && *paused {
            self.command(&Command::RestoreProcess)?;
            *paused = false;
            self.control.set_paused(false);
            tracing::info!("Ruida run resumed");
        }
        Ok(())
    }

    fn nap(&self) {
        std::thread::sleep(self.poll.status_interval.max(Duration::from_millis(5)));
    }

    /// Send `payloads` in order. `storing`: read the status register after each and stop the moment it shows a job
    /// running (a controller that runs an upload instead of storing it).
    fn send_payloads(&mut self, payloads: &[Vec<u8>], storing: bool) -> Result<()> {
        let mut paused = false;
        for (i, p) in payloads.iter().enumerate() {
            loop {
                self.check_abort()?;
                if !storing {
                    self.pause_step(&mut paused)?;
                }
                if !paused {
                    break;
                }
                self.nap();
            }
            self.link.send_payload(p)?;
            self.control.set_sent(i + 1);
            if storing && self.read_status()? & mem::STATUS_JOB_RUNNING != 0 {
                return Err(RuidaDriverError::RanWhileStoring.into());
            }
        }
        Ok(())
    }

    /// Read the status register until the job has finished.
    fn wait_finished(&mut self) -> Result<()> {
        let start = Instant::now();
        let mut paused = self.control.is_paused();
        let mut seen_running = false;
        let mut held = Duration::ZERO;
        loop {
            self.check_abort()?;
            self.pause_step(&mut paused)?;
            let t = Instant::now();
            let s = self.read_status()?;
            if s & mem::STATUS_JOB_RUNNING != 0 {
                seen_running = true;
            } else if !paused && (seen_running || start.elapsed() >= self.poll.start_grace) {
                return Ok(());
            }
            if let Some(limit) = self.poll.finish_timeout
                && !paused
                && start.elapsed().saturating_sub(held) >= limit
            {
                return Err(ControllerError::NotFinished(limit));
            }
            self.nap();
            if paused {
                held += t.elapsed();
            }
        }
    }

    // ----- the controller's file list -----

    fn expect_name(&mut self, index: u16, name: &str) -> Result<()> {
        match self.command(&Command::Document { kind: DocKind::Name, index })? {
            Some(Reply::DocumentName { index: i, name: n }) if i == index && n == name => Ok(()),
            Some(Reply::DocumentName { name: n, .. }) => Err(RuidaDriverError::FileChanged(format!(
                "file {index} on the controller is now {n:?}, not {name:?}; read the file list again"
            ))
            .into()),
            None => Err(RuidaDriverError::FileChanged(format!("the controller has no file {index}; read the file list again")).into()),
            other => Err(reply_error("reading a file name", other)),
        }
    }

    /// The files stored on the controller (its document count, then each name). Never moves or fires.
    pub fn list_files(&mut self) -> Result<Vec<StoredFile>> {
        if self.profile.is_none() {
            return Err(ControllerError::NotInitialized);
        }
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        let n = self.read_memory(mem::TOTAL_DOC_NUMBER)?;
        if n > MAX_FILES {
            return Err(RuidaDriverError::Reply(format!("the controller reports {n} files")).into());
        }
        let mut files = Vec::new();
        for index in 1..=u16::try_from(n).unwrap_or(0) {
            match self.command(&Command::Document { kind: DocKind::Name, index })? {
                Some(Reply::DocumentName { index: i, name }) if i == index => files.push(StoredFile { index, name }),
                other => return Err(reply_error(&format!("reading the name of file {index}"), other)),
            }
        }
        Ok(files)
    }

    /// Delete file `index` if it is still named `name`. Never moves or fires.
    pub fn delete_file(&mut self, index: u16, name: &str) -> Result<()> {
        if self.profile.is_none() {
            return Err(ControllerError::NotInitialized);
        }
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if index == 0 {
            return Err(ControllerError::Refused("file numbers start at 1 (0 would delete every file)".into()));
        }
        self.expect_name(index, name)?;
        self.command(&Command::DeleteDocument { index, other: 0 })?;
        Ok(())
    }

    /// Send: store `program` on the controller under its name (no [`Armed`] token: storing does not fire). The program
    /// is checked like a run's ([`check_program`], `Purpose::Store`); the controller must show no running job before
    /// and after every datagram (see the module docs). Returns the stored file as the list then shows it.
    pub fn upload_file(&mut self, program: &RuidaProgram) -> Result<StoredFile> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        check_program(&profile, program, Purpose::Store).map_err(RuidaDriverError::Program)?;
        let name = program.name().unwrap_or_default().to_owned();
        check_name(&name).map_err(RuidaDriverError::Program)?;
        let payloads = program.payloads().map_err(RuidaDriverError::Program)?;
        self.ready_for_job()?;
        let before = self.read_memory(mem::TOTAL_DOC_NUMBER)?;
        tracing::info!(name = %name, bytes = program.len(), datagrams = payloads.len(), "sending a file to the Ruida controller");
        self.state = ControllerState::Running;
        let mut guard = RunGuard { ctl: self, done: false };
        let result = guard.ctl.send_payloads(&payloads, true);
        guard.ctl.finish(&result);
        guard.done = true;
        drop(guard);
        result?;
        let files = self.list_files()?;
        let after = files.len() as u64;
        match files.iter().rev().find(|f| f.name == name) {
            Some(f) if after > before => Ok(f.clone()),
            _ => Err(RuidaDriverError::NotStored(format!(
                "the controller's file list did not gain {name:?} ({before} -> {after} files); it may not have stored it"
            ))
            .into()),
        }
    }

    // ----- stop -----

    /// Stop everything, always: stop process (`D8 01`). Needs no init (it is sent on whatever link there is).
    pub fn abort(&mut self) -> Result<()> {
        let r = self.command(&Command::StopProcess).map(|_| ());
        self.abort.clear();
        self.control.clear_pause();
        self.control.set_paused(false);
        self.state = if self.profile.is_some() { ControllerState::Idle } else { ControllerState::Uninitialized };
        r
    }

    fn abort_after(&mut self, cause: &ControllerError) {
        tracing::warn!(%cause, "Ruida: stopping (stop process)");
        if let Err(e) = self.abort() {
            tracing::error!(error = %e, "Ruida stop failed; use the machine's emergency stop");
        }
    }
}

/// Stops the controller if a run is left without finishing (an early return or a panic).
struct RunGuard<'a, L: RuidaLink> {
    ctl: &'a mut RuidaController<L>,
    done: bool,
}

impl<L: RuidaLink> Drop for RunGuard<'_, L> {
    fn drop(&mut self) {
        if !self.done {
            self.ctl.abort_after(&ControllerError::Refused("the run ended unexpectedly".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use ruida::rd::{RdJob, RdLayer};

    use super::super::{SimRuida, SimRuidaHandle};
    use super::*;

    fn ctl() -> (RuidaController<SimRuida>, SimRuidaHandle) {
        let sim = SimRuida::new(0x88);
        let h = sim.handle();
        let mut c = RuidaController::with_handles(sim, RuidaPoll::immediate(), AbortHandle::default(), RunControl::default());
        c.init(&RuidaProfile::default()).unwrap();
        (c, h)
    }

    fn square(power: f64) -> RdJob {
        RdJob {
            layers: vec![RdLayer {
                color: [0, 0, 0],
                speed_mm_s: 100.0,
                min_power: [power, 0.0],
                max_power: [power, 0.0],
                frequency: None,
                polylines: vec![vec![[10.0, 10.0], [30.0, 10.0], [30.0, 30.0], [10.0, 30.0], [10.0, 10.0]]],
            }],
        }
    }

    fn armed() -> Armed {
        Armed::approved_in_ui("r1".into())
    }

    fn style() -> FrameStyle {
        FrameStyle { pointer: lmc::PointerBits::RED, blanking: false, speed_mm_s: None }
    }

    #[test]
    fn init_reads_the_card() {
        let (c, h) = ctl();
        assert_eq!(c.state(), ControllerState::Idle);
        assert_eq!(c.snapshot().card_id.as_deref(), Some("0x65106510"));
        assert_eq!(h.report().burns, 0);
    }

    #[test]
    fn an_armed_run_burns_the_program() {
        let (mut c, h) = ctl();
        let p = RuidaProgram::from_job(&square(30.0), None).unwrap();
        let r = c.run_program(&p, armed()).unwrap();
        assert_eq!(r.run_id, "r1");
        let rep = h.report();
        assert_eq!((rep.burns, rep.jobs_finished, rep.running), (4, 1, false));
        assert_eq!(c.state(), ControllerState::Idle);
        // A stored-file program is not a run.
        let named = RuidaProgram::from_job(&square(30.0), Some("A")).unwrap();
        assert!(c.run_program(&named, armed()).is_err());
        assert_eq!(h.report().burns, 4);
    }

    #[test]
    fn limits_and_a_stop_request_send_nothing_that_fires() {
        let (mut c, h) = ctl();
        let limited = RuidaProfile { max_power_pct: 20.0, ..RuidaProfile::default() };
        c.init(&limited).unwrap();
        let p = RuidaProgram::from_job(&square(30.0), None).unwrap();
        assert!(matches!(c.run_program(&p, armed()), Err(ControllerError::Ruida(RuidaDriverError::Program(_)))));
        c.init(&RuidaProfile::default()).unwrap();
        let before = h.report().datagrams;
        c.abort_handle().request();
        assert!(matches!(c.run_program(&p, armed()), Err(ControllerError::Aborted)));
        assert_eq!(h.report().burns, 0);
        // Only the stop (D8 01) went out.
        assert_eq!(h.report().datagrams, before + 1);
        assert!(!c.abort_handle().is_requested(), "the abort cleared the request");
    }

    #[test]
    fn a_job_running_on_the_controller_blocks_runs_frames_and_sends() {
        let (mut c, h) = ctl();
        h.with(|s| s.running = true);
        let p = RuidaProgram::from_job(&square(30.0), None).unwrap();
        assert!(matches!(c.run_program(&p, armed()), Err(ControllerError::Ruida(RuidaDriverError::JobRunning))));
        assert!(c.frame_styled(&[vec![(0.0, 0.0), (1.0, 1.0)]], 1, &style()).is_err());
        let named = RuidaProgram::from_job(&square(30.0), Some("A")).unwrap();
        assert!(c.upload_file(&named).is_err());
        assert_eq!(h.report().files.len(), 0);
    }

    #[test]
    fn frames_move_without_cutting() {
        let (mut c, h) = ctl();
        let n = c.frame_styled(&[vec![(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0)]], 2, &style()).unwrap();
        assert_eq!(n, 6);
        assert_eq!(c.state(), ControllerState::Framing);
        let rep = h.report();
        assert_eq!((rep.segments, rep.burns), (6, 0));
        // Rear right origin on a 600 x 400 bed: field (10, 10) is machine (290, 190) mm.
        assert_eq!(rep.position_um, [290_000, 190_000]);
        assert!(c.poll_idle().unwrap());
        assert_eq!(c.state(), ControllerState::Idle);
        assert!(c.frame_styled(&[vec![(400.0, 0.0)]], 1, &style()).is_err(), "off the bed");
        assert!(c.frame_styled(&[], 1, &style()).is_err());
    }

    #[test]
    fn send_list_start_and_delete_files() {
        let (mut c, h) = ctl();
        for name in ["ONE", "TWO"] {
            let f = c.upload_file(&RuidaProgram::from_job(&square(25.0), Some(name)).unwrap()).unwrap();
            assert_eq!(f.name, name);
        }
        // Stored, nothing fired.
        assert_eq!(h.report().burns, 0);
        let files = c.list_files().unwrap();
        assert_eq!(files, vec![StoredFile { index: 1, name: "ONE".into() }, StoredFile { index: 2, name: "TWO".into() }]);
        // A run program cannot be sent as a file.
        assert!(c.upload_file(&RuidaProgram::from_job(&square(25.0), None).unwrap()).is_err());
        // Start needs the name the list showed.
        assert!(matches!(c.start_file(2, "ONE", armed()), Err(ControllerError::Ruida(RuidaDriverError::FileChanged(_)))));
        assert!(matches!(c.start_file(9, "X", armed()), Err(ControllerError::Ruida(RuidaDriverError::FileChanged(_)))));
        assert_eq!(h.report().burns, 0);
        c.start_file(2, "TWO", armed()).unwrap();
        assert_eq!((h.report().burns, h.report().jobs_finished), (4, 1));
        assert!(c.delete_file(1, "TWO").is_err());
        assert!(c.delete_file(0, "ONE").is_err());
        c.delete_file(1, "ONE").unwrap();
        assert_eq!(h.report().files, vec!["TWO".to_owned()]);
    }

    #[test]
    fn an_upload_that_starts_a_job_is_stopped() {
        // A controller that runs the upload instead of storing it (not what the simulator does): after the first
        // datagram of the file its status register shows a running job.
        struct Runs(SimRuida, bool);
        impl RuidaLink for Runs {
            fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, crate::LinkError> {
                self.1 |= plain.first() == Some(&0xE7);
                let r = self.0.send_payload(plain)?;
                match r {
                    Some(Reply::Memory { addr, .. }) if self.1 && addr == mem::MACHINE_STATUS => {
                        Ok(Some(Reply::Memory { addr, value: mem::STATUS_JOB_RUNNING, second: None }))
                    }
                    r => Ok(r),
                }
            }
        }
        let sim = SimRuida::new(0x88);
        let h = sim.handle();
        let mut c = RuidaController::with_handles(Runs(sim, false), RuidaPoll::immediate(), AbortHandle::default(), RunControl::default());
        c.init(&RuidaProfile::default()).unwrap();
        let p = RuidaProgram::from_job(&square(25.0), Some("RUNS")).unwrap();
        let e = c.upload_file(&p).unwrap_err();
        assert!(matches!(e, ControllerError::Ruida(RuidaDriverError::RanWhileStoring)), "{e}");
        // The stop went out right after.
        assert_eq!(h.with(|s| s.log.last().cloned()), Some(Command::StopProcess));
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn pause_and_resume_reach_the_controller() {
        let (mut c, h) = ctl();
        let p = RuidaProgram::from_job(&square(30.0), None).unwrap();
        let control = c.run_control();
        control.request_pause();
        let resumer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            control.request_resume();
        });
        c.run_program(&p, armed()).unwrap();
        resumer.join().unwrap();
        let log = h.with(|s| s.log.clone());
        let pause = log.iter().position(|c| *c == Command::PauseProcess).unwrap();
        let resume = log.iter().position(|c| *c == Command::RestoreProcess).unwrap();
        let first_cut = log.iter().position(Command::is_cut).unwrap();
        assert!(pause < resume && resume < first_cut, "paused before the job, resumed, then it ran");
    }

    #[test]
    fn udp_link_to_a_simulator() {
        use std::net::UdpSocket;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let addr = sock.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let t = std::thread::spawn(move || {
            let mut sim = ruida::sim::Simulator::new(0x88);
            let mut buf = [0u8; 2048];
            while !stop2.load(Ordering::Relaxed) {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    for r in sim.handle_udp(&buf[..n]) {
                        sock.send_to(&r, from).unwrap();
                    }
                }
            }
            sim
        });
        let link = super::super::UdpRuidaLink::bind("127.0.0.1:0".parse().unwrap(), addr, 0x88, Duration::from_millis(200)).unwrap();
        let mut c = RuidaController::with_handles(link, RuidaPoll::immediate(), AbortHandle::default(), RunControl::default());
        c.init(&RuidaProfile::default()).unwrap();
        c.upload_file(&RuidaProgram::from_job(&square(10.0), Some("NET")).unwrap()).unwrap();
        c.start_file(1, "NET", armed()).unwrap();
        stop.store(true, Ordering::Relaxed);
        let sim = t.join().unwrap();
        assert_eq!((sim.burns().count(), sim.jobs_finished, sim.documents.len()), (4, 1, 1));
    }
}
