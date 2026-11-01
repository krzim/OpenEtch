//! [`Controller`]: init, correction upload, red-light framing, armed runs and abort over any [`Link`].
//!
//! Safety rules this module enforces:
//!
//! - [`Controller::frame`] builds its packets only with [`lmc::LightList`] and re-checks them with
//!   [`lmc::emits_light_only`] before sending. Its single commands never open the MO or set the laser-enable bit.
//! - [`Controller::run`] is the only path that opens the MO or sets the laser-enable bit. It needs an [`Armed`]
//!   token (see its docs) and checks the packets against the profile's hard limits ([`check_job`]) first.
//! - Any error during a run or frame triggers [`Controller::abort`]. `abort` needs no init, tries every step even
//!   when one fails, and can be requested from another thread with an [`AbortHandle`]. A run that ends any other
//!   way than normally (an error, an early return, or a panic unwinding out of it) sends the abort sequence from a
//!   drop guard.
//! - The link is private: nothing outside this crate can send a single command or packet past these rules.
//!   [`Controller::replace_link`] swaps the link and forgets the init.
//! - Pause ([`RunControl::request_pause`]) only ever stops the list (`StopList`); resume (`RestartList`) continues
//!   the run that was armed; an abort request always wins over a pause. A run reports its progress (packets sent of
//!   the total, paused or not) through the same [`RunControl`].
//! - The extension axis ([`Controller::move_axis`]) and a run's axis moves between parts never emit: during a run's
//!   axis move the MO is closed and opened again afterwards (each part's list waits the open-MO delay again).
//!
//! Where the init, run and abort sequences differ from the references is listed in one place, the
//! "Deviations from the reference sequences" section of [`Controller`].

// Init order, list streaming (READY flow control, `SetEndOfList` after each packet, `ExecuteList` once more than two
// packets are queued), the marking prelude/postlude and the abort order follow meerk40t/galvoplotter @ fc35136
// `galvo/controller.py` (`init_laser`, `_list_end`, `marking_configuration`, `initial_configuration`, `abort`), MIT.
// Written from scratch in Rust.
//
// MIT License
//
// Copyright (c) 2023 MeerK40t
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use corfile::CorTable;
use lmc::{CENTER, LightList, Packet, PointerBits, Single, StatusWord, units};
use serde::{Deserialize, Serialize};

use crate::armed::Armed;
use crate::check::{JobCheckError, check_job};
use crate::geometry::{FieldError, field_to_galvo};
use crate::init::{InitSettings, correction_upload};
use crate::link::{Link, LinkError, Reply};
use crate::profile::{MachineProfile, ProfileError};

/// Most repetitions one [`Controller::frame`] call accepts.
pub const MAX_FRAME_LOOPS: u32 = 10_000;
/// Most packets (256 commands each) one frame may produce (12 MiB).
pub const MAX_FRAME_PACKETS: usize = 4096;

/// What went wrong. After any error from [`Controller::run`] or [`Controller::frame`], `abort` has already been
/// attempted.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ControllerError {
    #[error(transparent)]
    Link(#[from] LinkError),
    #[error("the board has not been initialised (call init first)")]
    NotInitialized,
    #[error("the controller is {0:?}")]
    Busy(ControllerState),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error(transparent)]
    Lmc(#[from] lmc::LmcError),
    #[error(transparent)]
    Job(#[from] JobCheckError),
    /// The request itself is refused (empty outline, too many loops, ...).
    #[error("refused: {0}")]
    Refused(String),
    #[error("the board did not report READY within {0:?}")]
    NotReady(Duration),
    #[error("the board did not finish within {0:?}")]
    NotFinished(Duration),
    /// An [`AbortHandle`] was used; the abort sequence has been sent.
    #[error("aborted")]
    Aborted,
    /// The profile's stop-marking input was seen set while a run streamed; the abort sequence has been sent.
    #[error("stopped by the stop-marking input (bit {0})")]
    StopInput(u8),
    /// The extension axis did not stop moving within the limit.
    #[error("the extension axis did not stop within {0:?}")]
    AxisTimeout(Duration),
    /// The profile's Door Protect input is active (the enclosure is open): the run did not start, or was aborted.
    #[error("the door interlock (input {0}) is open")]
    DoorOpen(u8),
    /// A GRBL board's error, alarm or refusal (the GRBL driver).
    #[error(transparent)]
    Grbl(#[from] crate::grbl::GrblError),
}

type Result<T, E = ControllerError> = std::result::Result<T, E>;

/// What the controller is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerState {
    /// `init` has not succeeded yet.
    Uninitialized,
    Idle,
    /// A red-light frame list was sent and may still be running.
    Framing,
    /// Inside [`Controller::run`].
    Running,
}

/// The board's identification replies from `init` (raw words; their meaning is not decoded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardInfo {
    pub serial: Reply,
    pub version: Reply,
}

/// Longest wait for the extension axis to stop moving.
pub const AXIS_TIMEOUT: Duration = Duration::from_secs(60);

/// One part of a run: an optional extension-axis move (rotary step or Z position, as the axis's step word), then
/// a list. A plain job is one part without a move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPart {
    /// Move the extension axis here (`MoveAxisTo`) before this part's list, with the MO closed.
    pub axis_position: Option<u16>,
    pub packets: Vec<Packet>,
}

impl RunPart {
    /// An axis move with no list (a rotary's return to its start); a job needs at least one part with packets.
    pub fn axis_only(position: u16) -> Self {
        RunPart { axis_position: Some(position), packets: Vec::new() }
    }

    /// A part without an axis move.
    pub fn list(packets: Vec<Packet>) -> Self {
        RunPart { axis_position: None, packets }
    }
}

/// Where a run is, as [`RunControl::progress`] reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProgress {
    /// Packets sent to the board so far in this run.
    pub packets_sent: usize,
    /// Packets of the whole run.
    pub packets_total: usize,
    /// The part being run (0-based) and how many there are.
    pub part: usize,
    pub parts: usize,
    /// The list is paused (`StopList` sent, `RestartList` not yet).
    pub paused: bool,
}

#[derive(Debug, Default)]
struct RunShared {
    pause_requested: AtomicBool,
    paused: AtomicBool,
    packets_sent: AtomicUsize,
    packets_total: AtomicUsize,
    part: AtomicUsize,
    parts: AtomicUsize,
}

/// Pause, resume and progress of the controller's runs, usable from another thread while [`Controller::run`]
/// holds the controller (like [`AbortHandle`]). Pausing sends `StopList` at the next packet or status poll and
/// resuming `RestartList` (galvoplotter's and meerk40t's `pause()` / `resume()`); an abort request ends a paused run
/// too. A pause requested before a run streams is kept: that run pauses before its first packet (so a pause pressed
/// between two runs of a repeating job, or while the next run is being started, is never lost). [`Controller::abort`]
/// and [`RunControl::clear_pause`] drop a pending request.
#[derive(Debug, Clone, Default)]
pub struct RunControl(Arc<RunShared>);

impl RunControl {
    pub fn request_pause(&self) {
        self.0.pause_requested.store(true, Ordering::SeqCst);
    }

    pub fn request_resume(&self) {
        self.0.pause_requested.store(false, Ordering::SeqCst);
    }

    /// Drop a pending pause request (a new job is approved, or a stop ended the last one).
    pub fn clear_pause(&self) {
        self.0.pause_requested.store(false, Ordering::SeqCst);
    }

    /// Whether a pause is asked for (it takes effect at the next packet or poll).
    pub fn pause_requested(&self) -> bool {
        self.0.pause_requested.load(Ordering::SeqCst)
    }

    /// Whether the board's list is paused right now.
    pub fn is_paused(&self) -> bool {
        self.0.paused.load(Ordering::SeqCst)
    }

    pub fn progress(&self) -> RunProgress {
        RunProgress {
            packets_sent: self.0.packets_sent.load(Ordering::SeqCst),
            packets_total: self.0.packets_total.load(Ordering::SeqCst),
            part: self.0.part.load(Ordering::SeqCst),
            parts: self.0.parts.load(Ordering::SeqCst),
            paused: self.is_paused(),
        }
    }

    /// A run of `total` lines (or other units) in one part starts (the GRBL driver).
    pub(crate) fn start_count(&self, total: usize) {
        self.0.paused.store(false, Ordering::SeqCst);
        self.0.packets_sent.store(0, Ordering::SeqCst);
        self.0.packets_total.store(total, Ordering::SeqCst);
        self.0.part.store(0, Ordering::SeqCst);
        self.0.parts.store(1, Ordering::SeqCst);
    }

    pub(crate) fn set_sent(&self, sent: usize) {
        self.0.packets_sent.store(sent, Ordering::SeqCst);
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.0.paused.store(paused, Ordering::SeqCst);
    }

    fn start(&self, parts: &[RunPart]) {
        // `pause_requested` is kept (see the type docs).
        self.0.paused.store(false, Ordering::SeqCst);
        self.0.packets_sent.store(0, Ordering::SeqCst);
        self.0.packets_total.store(parts.iter().map(|p| p.packets.len()).sum(), Ordering::SeqCst);
        self.0.part.store(0, Ordering::SeqCst);
        self.0.parts.store(parts.len(), Ordering::SeqCst);
    }
}

/// How a frame is traced: which pointer outputs light, whether the pointer is off between shapes (blanking), and the
/// travel speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStyle {
    pub pointer: PointerBits,
    pub blanking: bool,
    /// Pointer travel speed, mm/s; `None` uses the profile's `red_light_speed_mm_s`. Refused above the profile's
    /// `max_jump_speed_mm_s`.
    pub speed_mm_s: Option<f64>,
}

impl FrameStyle {
    /// The red pointer, with the profile's blanking and frame speed.
    pub fn for_profile(profile: &MachineProfile) -> Self {
        FrameStyle { pointer: PointerBits::RED, blanking: profile.blanking, speed_mm_s: None }
    }
}

/// The result of a completed run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunReport {
    pub run_id: String,
    pub packets: usize,
    /// Raw `GetMarkTime(3)` reply (meaning not decoded).
    pub mark_time: Option<Reply>,
}

/// Status polling. galvoplotter polls every 10 ms with no timeout; here waiting for READY is bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollConfig {
    /// Pause between status polls.
    pub interval: Duration,
    /// Longest wait for READY before sending a packet.
    pub ready_timeout: Duration,
    /// Longest wait for a run to finish; `None` waits as long as the board reports BUSY.
    pub finish_timeout: Option<Duration>,
}

impl Default for PollConfig {
    fn default() -> Self {
        PollConfig { interval: Duration::from_millis(10), ready_timeout: Duration::from_secs(5), finish_timeout: None }
    }
}

impl PollConfig {
    /// No pauses and short timeouts, for [`crate::SimLink`].
    pub fn immediate() -> Self {
        PollConfig { interval: Duration::ZERO, ready_timeout: Duration::from_secs(2), finish_timeout: Some(Duration::from_secs(2)) }
    }
}

/// Requests an abort from another thread while the controller is busy in [`Controller::run`] or
/// [`Controller::frame`]. The running call notices it at the next packet or status poll, sends the abort sequence
/// and returns [`ControllerError::Aborted`].
///
/// A request stays pending until [`Controller::abort`] runs, so a stop pressed just before a run starts cancels that
/// run. The engine's stop endpoint should call [`AbortHandle::request`] and then, once it holds the controller,
/// [`Controller::abort`] (which is harmless to repeat and clears the request).
#[derive(Debug, Clone, Default)]
pub struct AbortHandle(Arc<AtomicBool>);

impl AbortHandle {
    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Drop a pending request.
    pub fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Drives one board through a [`Link`]. See the module docs for the safety rules.
///
/// # Deviations from the reference sequences
///
/// The references are meerk40t/galvoplotter @ fc35136 `galvo/controller.py` (`init_laser`, `abort`,
/// `marking_configuration`, `_list_end`) and meerk40t `balormk/controller.py` (same init and abort order, checked
/// 2026-10-31). Every difference is deliberate and listed here:
///
/// - **Init** ([`Controller::init`]): `GetSerialNo`, `GetVersion`, then **`StopExecute` and `WritePort(0)`** (in
///   neither reference), then the reference order (`Reset`, correction upload, [`InitSettings::configuration`]).
///   After an engine crash mid-run the board would otherwise keep executing its buffered list, with the
///   laser-enable bit set, until `configuration()`'s `Fiber_SetMo(0)`.
/// - **Init**: the references' trailing `sleep(0.05)` is not ported; frames and runs poll for READY before their
///   first packet. A .cor whose field differs from the profile's only warns.
/// - **Abort** ([`Controller::abort`]): `StopExecute`, `Fiber_SetMo(0)`, `ResetList`, `WritePort(0)`,
///   `GotoXY(centre)`. The references send `StopExecute`, `Fiber_SetMo(0)`, `ResetList`, then optionally a dummy
///   packet (an empty list ending in `EndOfList`) plus `ExecuteList`, then `Fiber_SetMo(0)` again and the port with
///   bit 0 cleared. Here: no dummy packet or `ExecuteList` (the list was just stopped and reset); `Fiber_SetMo(0)`
///   once; `WritePort(0)` clears the whole port, red pointer included, not only bit 0; `GotoXY(centre)` is added to
///   park the galvos; every step is tried even when one fails; it needs no init.
/// - **Run** ([`Controller::run`]): the prelude (`ResetList`, `WritePort(laser enable)`, `Fiber_SetMo(1)`) and
///   postlude (`Fiber_SetMo(0)`, `WritePort(0)`, `GetMarkTime(3)`) are galvoplotter's. The list never uses the
///   in-list `FiberOpenMO` (0x8021): the MO is opened and closed only by these singles. Unlike the references, an
///   abort request is checked before the prelude and before every packet, waits for READY and for the end of the
///   run are bounded by [`PollConfig`], and a run that does not finish normally (including a panic) sends the abort
///   sequence from a drop guard.
/// - **Frame stop** ([`Controller::stop_frame`]): `StopExecute`, `ResetList`, `WritePort(0)`; the references have no
///   separate frame stop.
#[derive(Debug)]
pub struct Controller<L: Link> {
    link: L,
    settings: InitSettings,
    poll: PollConfig,
    profile: Option<MachineProfile>,
    state: ControllerState,
    abort: AbortHandle,
    control: RunControl,
    info: Option<BoardInfo>,
}

impl<L: Link> Controller<L> {
    /// A controller with galvoplotter's [`InitSettings`] and the default [`PollConfig`].
    pub fn new(link: L) -> Self {
        Self::with_settings(link, InitSettings::default(), PollConfig::default())
    }

    pub fn with_settings(link: L, settings: InitSettings, poll: PollConfig) -> Self {
        Self::with_handles(link, settings, poll, AbortHandle::default(), RunControl::default())
    }

    /// [`Controller::with_settings`] sharing existing abort and run-control handles, so a machine that changes
    /// drivers keeps the handles its stop and pause buttons hold.
    pub fn with_handles(link: L, settings: InitSettings, poll: PollConfig, abort: AbortHandle, control: RunControl) -> Self {
        Controller { link, settings, poll, profile: None, state: ControllerState::Uninitialized, abort, control, info: None }
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    /// Tests only: a `&mut L` would let a caller send `Fiber_SetMo(1)` or any packet past [`Armed`] and
    /// [`check_job`]. Use [`Controller::replace_link`] to swap the link.
    #[cfg(test)]
    pub(crate) fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }

    /// Swap in a new link (a freshly opened USB connection, or an offline placeholder after a failed connect). The
    /// old link is dropped here rather than returned, so no caller ever holds a raw link outside [`Armed`] and
    /// [`check_job`]. The board behind the new link is treated as uninitialised: the profile is forgotten and
    /// [`Controller::init`] must succeed again before framing or running.
    pub fn replace_link(&mut self, link: L) {
        self.profile = None;
        self.state = ControllerState::Uninitialized;
        self.link = link;
    }

    /// The profile `init` was given.
    pub fn profile(&self) -> Option<&MachineProfile> {
        self.profile.as_ref()
    }

    pub fn state(&self) -> ControllerState {
        self.state
    }

    /// A handle that can request an abort from another thread.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// A handle that pauses and resumes runs and reads their progress from another thread.
    pub fn run_control(&self) -> RunControl {
        self.control.clone()
    }

    /// The board's identification as read by the last successful `init` (or [`Controller::refresh_info`]).
    pub fn board_info(&self) -> Option<&BoardInfo> {
        self.info.as_ref()
    }

    /// Read `GetSerialNo` and `GetVersion` again (LightBurn's Get Controller Info). Refused while running or
    /// framing; sends nothing that can emit.
    pub fn refresh_info(&mut self) -> Result<BoardInfo> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        if self.state != ControllerState::Idle {
            return Err(ControllerError::Busy(self.state));
        }
        let info = BoardInfo { serial: self.reply(Single::get_serial_no())?, version: self.reply(Single::get_version())? };
        self.info = Some(info.clone());
        Ok(info)
    }

    /// Move the extension axis to step `position` (a Z lift, or a rotary outside a run) and wait until the board
    /// stops reporting AXIS (at most [`AXIS_TIMEOUT`]). Needs an initialised, idle board (a running frame is stopped
    /// first) and an axis in the profile; the MO is closed first. Nothing here can emit. An abort request ends the
    /// wait (and sends the abort sequence).
    pub fn move_axis(&mut self, position: u16) -> Result<()> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        let axis = profile.axis.clone().ok_or_else(|| ControllerError::Refused("the profile has no extension axis".into()))?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        let r = self.single(Single::fiber_set_mo(false)).and_then(|_| self.axis_to(position, axis.invert));
        if let Err(e) = &r {
            self.abort_after(e);
        }
        r
    }

    /// `MoveAxisTo` and wait for the AXIS status bit to clear.
    fn axis_to(&mut self, position: u16, invert: bool) -> Result<()> {
        self.single(Single::move_axis_to(position, invert))?;
        let start = Instant::now();
        loop {
            self.check_abort()?;
            if !self.poll_status()?.axis() {
                return Ok(());
            }
            if start.elapsed() >= AXIS_TIMEOUT {
                return Err(ControllerError::AxisTimeout(AXIS_TIMEOUT));
            }
            self.pause();
        }
    }

    /// Connect-time setup, in galvoplotter's `init_laser` order: `GetSerialNo`, `GetVersion`, then `StopExecute` and
    /// `WritePort(0)` (a deviation, see the type docs: stops a list left running and drops the laser-enable bit),
    /// `Reset`, the correction upload (`WriteCorTable(1)` + one `WriteCorLine` per node, or `WriteCorTable(0)` without
    /// a table), then [`InitSettings::configuration`] (which closes the MO). Nothing here can emit.
    pub fn init(&mut self, profile: &MachineProfile, cor: Option<&CorTable>) -> Result<BoardInfo> {
        profile.validate()?;
        if let Some(field) = cor.and_then(CorTable::field_mm)
            && (field - profile.field_mm).abs() > 0.01
        {
            tracing::warn!(cor_field_mm = field, profile_field_mm = profile.field_mm, "correction file and profile field sizes differ");
        }
        self.state = ControllerState::Uninitialized;
        let serial = self.reply(Single::get_serial_no())?;
        let version = self.reply(Single::get_version())?;
        self.single(Single::stop_execute())?;
        self.single(Single::write_port(0))?;
        self.single(Single::reset())?;
        for cmd in correction_upload(cor) {
            self.single(cmd)?;
        }
        for cmd in self.settings.configuration() {
            self.single(cmd)?;
        }
        self.profile = Some(profile.clone());
        self.state = ControllerState::Idle;
        let idle = profile.io.idle_outputs();
        if idle != 0 {
            self.single(Single::write_port(idle))?;
        }
        let info = BoardInfo { serial, version };
        self.info = Some(info.clone());
        Ok(info)
    }

    /// Poll the board status (`GetVersion`, word 3). A finished frame moves the state back to idle.
    pub fn status(&mut self) -> Result<StatusWord> {
        let status = self.poll_status()?;
        if self.state == ControllerState::Framing && status.idle() {
            self.state = ControllerState::Idle;
        }
        Ok(status)
    }

    /// The output word while idle (ready light, Red Dot Always On; never the laser-enable bit): 0 without a profile.
    pub fn idle_outputs(&self) -> u16 {
        self.profile.as_ref().map_or(0, |p| p.io.idle_outputs())
    }

    /// Write the idle output word (after a stop, or when Red Dot Always On changes). Never sets the laser-enable bit.
    /// Refused while a job runs or a frame is on (they own the port).
    pub fn apply_idle_outputs(&mut self) -> Result<()> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        if matches!(self.state, ControllerState::Running | ControllerState::Framing) {
            return Err(ControllerError::Busy(self.state));
        }
        let idle = self.idle_outputs();
        debug_assert_eq!(idle & lmc::LASER_ENABLE_BIT, 0);
        self.single(Single::write_port(idle & !lmc::LASER_ENABLE_BIT)).map(|_| ())
    }

    /// Read the board's input port: `ReadPort`, reply word 1 (as galvoplotter's `wait_for_input` reads it). Used for
    /// a physical confirmation input. Needs an initialised board; sends nothing that can emit and changes no state.
    pub fn read_port(&mut self) -> Result<u16> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        Ok(self.reply(Single::read_port())?[1])
    }

    /// Trace `outline` (polylines in field mm) with the red pointer `loops` times, using only
    /// [`lmc::LightList`]. Each polyline is visited in order; the pointer is switched off while travelling between
    /// polylines. A running frame is stopped first. Returns the number of packets sent.
    ///
    /// Every point is converted with [`field_to_galvo`], so anything outside the field is refused before anything
    /// is sent.
    pub fn frame(&mut self, outline: &[Vec<(f64, f64)>], loops: u32) -> Result<usize> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        self.frame_styled(outline, loops, &FrameStyle::for_profile(&profile))
    }

    /// [`Controller::frame`] with a [`FrameStyle`]: which pointer outputs light (the red pointer, the profile's focus
    /// light, or both), blanking between shapes, and the travel speed. Every point first goes through the profile's
    /// red-dot alignment ([`crate::RedDot`]), then the shared field transform (lens correction included).
    pub fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        let packets = light_packets(&profile, outline, loops, style)?;
        if !lmc::emits_light_only(&packets) {
            // Unreachable by LightList's invariant; checked anyway.
            return Err(ControllerError::Refused("frame packets failed the light-only check".into()));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        self.state = ControllerState::Framing;
        let sent = self.single(Single::fiber_set_mo(false)).and_then(|_| self.single(Single::reset_list())).and_then(|_| self.stream(&packets));
        if let Err(e) = sent {
            self.abort_after(&e);
            return Err(e);
        }
        Ok(packets.len())
    }

    /// Stop a running frame: `StopExecute`, `ResetList`, `WritePort(0)` (pointer off), then `GotoXY` to the red
    /// dot's rest position when the profile sets one. Every step is tried.
    pub fn stop_frame(&mut self) -> Result<()> {
        let mut cmds = vec![Single::stop_execute(), Single::reset_list(), Single::write_port(self.idle_outputs())];
        if let Some(p) = self.profile.as_ref().filter(|p| p.red_dot.rest_x_mm != 0.0 || p.red_dot.rest_y_mm != 0.0)
            && let Ok((x, y)) = field_to_galvo(p, p.red_dot.rest_x_mm, p.red_dot.rest_y_mm)
        {
            cmds.push(Single::goto_xy(x, y, 0));
        }
        let result = self.send_all(&cmds);
        if self.state == ControllerState::Framing {
            self.state = ControllerState::Idle;
        }
        result
    }

    /// Run a job's packets. Requires an [`Armed`] token, which only the engine's UI approval creates.
    ///
    /// Order: [`check_job`] against the profile (nothing is sent if it fails); a running frame is stopped; the abort
    /// request is checked (a pending one cancels the run before the prelude, so nothing emits); prelude
    /// `ResetList`, `WritePort(laser enable)`, `Fiber_SetMo(1)`; the packets with READY flow control, the abort
    /// request checked again before each, `SetEndOfList` after each and `ExecuteList` once more than two are queued
    /// (or after the last); wait until idle; postlude `Fiber_SetMo(0)`, `WritePort(0)`, `GetMarkTime(3)`.
    ///
    /// Any error or abort request runs [`Controller::abort`]. From the prelude on, a drop guard also sends the
    /// abort sequence if this call is left any other way, including a panic unwinding through it (should the link
    /// panic again inside that abort, the process aborts, which also closes the USB handle).
    pub fn run(&mut self, packets: &[Packet], armed: Armed) -> Result<RunReport> {
        self.run_parts(&[RunPart::list(packets.to_vec())], armed)
    }

    /// [`Controller::run`] for a job of several parts (rotary strips, Z steps): every part is checked before
    /// anything is sent; between parts the board is waited idle, the MO closed, the extension axis moved
    /// (`MoveAxisTo`, waiting for AXIS to clear), the list reset and the MO opened again. A part with an axis move
    /// needs an axis in the profile.
    pub fn run_parts(&mut self, parts: &[RunPart], armed: Armed) -> Result<RunReport> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if parts.iter().all(|p| p.packets.is_empty()) {
            return Err(ControllerError::Job(JobCheckError::Empty));
        }
        for part in parts.iter().filter(|p| !p.packets.is_empty() || p.axis_position.is_none()) {
            check_job(&profile, &part.packets)?;
        }
        if parts.iter().any(|p| p.axis_position.is_some()) && profile.axis.is_none() {
            return Err(ControllerError::Refused("the job moves the extension axis but the profile has none".into()));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        // Door Protect: an open enclosure refuses the run before anything that can emit is sent.
        if let Some(door) = profile.io.door_protect
            && door.active(self.read_port()?)
        {
            return Err(ControllerError::DoorOpen(door.bit));
        }
        let total: usize = parts.iter().map(|p| p.packets.len()).sum();
        tracing::info!(run_id = armed.run_id(), packets = total, parts = parts.len(), "starting armed run");
        self.state = ControllerState::Running;
        self.control.start(parts);
        let mut guard = RunGuard { ctl: self, done: false };
        let result = guard.ctl.run_inner(&profile, parts);
        guard.ctl.control.0.paused.store(false, Ordering::SeqCst);
        if let Err(e) = &result {
            guard.ctl.abort_after(e);
        } else {
            guard.ctl.state = ControllerState::Idle;
        }
        guard.done = true;
        result.map(|mark_time| RunReport { run_id: armed.run_id().to_owned(), packets: total, mark_time })
    }

    fn run_inner(&mut self, profile: &MachineProfile, parts: &[RunPart]) -> Result<Option<Reply>> {
        let invert = profile.axis.as_ref().is_some_and(|a| a.invert);
        // Laser enable plus the busy / ready lights while the job runs.
        let enable = profile.laser_enable_mask() | profile.io.run_outputs();
        let mut mo_open = false;
        for (i, part) in parts.iter().enumerate() {
            self.control.0.part.store(i, Ordering::SeqCst);
            if let Some(position) = part.axis_position {
                if mo_open {
                    self.single(Single::fiber_set_mo(false))?;
                    mo_open = false;
                }
                self.check_abort()?;
                self.axis_to(position, invert)?;
            }
            if part.packets.is_empty() {
                // An axis move only (e.g. the rotary's return to its start).
                continue;
            }
            self.single(Single::reset_list())?;
            if i == 0 {
                self.single(Single::write_port(enable))?;
            }
            if !mo_open {
                if i > 0 {
                    self.single(Single::write_port(enable))?;
                }
                self.single(Single::fiber_set_mo(true))?;
                mo_open = true;
            }
            self.stream(&part.packets)?;
            self.wait_idle()?;
        }
        self.single(Single::fiber_set_mo(false))?;
        let idle = profile.io.idle_outputs();
        self.single(Single::write_port(idle))?;
        let mark_time = self.single(Single::get_mark_time())?;
        // Done Marking: pulse the output (never with the laser-enable bit; the MO is closed).
        let done = profile.io.done_output();
        if done != 0 {
            self.single(Single::write_port(idle | done))?;
            std::thread::sleep(Duration::from_millis(u64::from(profile.io.done_pulse_ms)));
            self.single(Single::write_port(idle))?;
        }
        Ok(mark_time)
    }

    /// Emergency stop, always callable (no init needed): `StopExecute`, `Fiber_SetMo(0)`, `ResetList`,
    /// `WritePort(0)` (laser enable and pointer off), `GotoXY(centre)`. Every step is tried even if an earlier one
    /// fails; the first error is returned. Clears any pending [`AbortHandle`] request and leaves the controller idle
    /// (no run state survives it: an [`Armed`] token is consumed by the run it was given to). Not gated by anything;
    /// the engine's stop epoch, which keeps an approved run from starting after a stop, lives in the engine.
    pub fn abort(&mut self) -> Result<()> {
        let mut cmds = vec![
            Single::stop_execute(),
            Single::fiber_set_mo(false),
            Single::reset_list(),
            Single::write_port(0),
            Single::goto_xy(CENTER, CENTER, 0),
        ];
        // Ready light / Red Dot Always On come back after the port was cleared (never the laser-enable bit).
        let idle = self.idle_outputs() & !lmc::LASER_ENABLE_BIT;
        if idle != 0 {
            cmds.push(Single::write_port(idle));
        }
        let result = self.send_all(&cmds);
        self.abort.clear();
        // A stop ends a pause too; a request left over would pause the next run.
        self.control.clear_pause();
        self.state = if self.profile.is_some() { ControllerState::Idle } else { ControllerState::Uninitialized };
        result
    }

    fn abort_after(&mut self, cause: &ControllerError) {
        tracing::warn!(%cause, "aborting");
        if let Err(e) = self.abort() {
            tracing::error!(error = %e, "abort sequence failed");
        }
    }

    /// Send each command, continuing after failures; return the first error.
    fn send_all(&mut self, cmds: &[Single]) -> Result<()> {
        let mut first = None;
        for cmd in cmds {
            if let Err(e) = self.link.send_single(*cmd) {
                tracing::warn!(command = %cmd, error = %e, "single command failed");
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), |e| Err(e.into()))
    }

    fn single(&mut self, cmd: Single) -> Result<Option<Reply>> {
        Ok(self.link.send_single(cmd)?)
    }

    fn reply(&mut self, cmd: Single) -> Result<Reply> {
        self.single(cmd)?.ok_or_else(|| LinkError::Io(format!("{cmd} returned no reply")).into())
    }

    /// One `GetVersion` status poll, without state changes (used while streaming).
    fn poll_status(&mut self) -> Result<StatusWord> {
        Ok(StatusWord::from_reply(self.reply(Single::get_version())?))
    }

    fn check_abort(&self) -> Result<()> {
        if self.abort.is_requested() { Err(ControllerError::Aborted) } else { Ok(()) }
    }

    /// While running: the stop-marking input (when the profile names one) and a requested pause. A pause sends
    /// `StopList` and then waits (polling, abort and stop input checked) until resumed, then sends `RestartList`.
    /// Returns how long it was paused.
    fn service_run(&mut self) -> Result<Duration> {
        self.check_abort()?;
        if self.state != ControllerState::Running {
            return Ok(Duration::ZERO);
        }
        self.check_stop_input()?;
        if !self.control.pause_requested() {
            return Ok(Duration::ZERO);
        }
        let start = Instant::now();
        self.single(Single::stop_list())?;
        self.control.0.paused.store(true, Ordering::SeqCst);
        tracing::info!("run paused");
        while self.control.pause_requested() {
            self.check_abort()?;
            self.check_stop_input()?;
            self.poll_status()?;
            if self.poll.interval.is_zero() {
                std::thread::sleep(Duration::from_millis(1));
            } else {
                self.pause();
            }
        }
        self.check_abort()?;
        self.single(Single::restart_list())?;
        self.control.0.paused.store(false, Ordering::SeqCst);
        tracing::info!("run resumed");
        Ok(start.elapsed())
    }

    fn check_stop_input(&mut self) -> Result<()> {
        let stop = self.profile.as_ref().and_then(|p| p.stop_mark_input_bit);
        let door = self.profile.as_ref().and_then(|p| p.io.door_protect);
        if stop.is_none() && door.is_none() {
            return Ok(());
        }
        let inputs = self.reply(Single::read_port())?[1];
        if let Some(bit) = stop
            && inputs & (1u16 << (bit & 15)) != 0
        {
            return Err(ControllerError::StopInput(bit));
        }
        match door {
            Some(d) if d.active(inputs) => Err(ControllerError::DoorOpen(d.bit)),
            _ => Ok(()),
        }
    }

    fn pause(&self) {
        if !self.poll.interval.is_zero() {
            std::thread::sleep(self.poll.interval);
        }
    }

    fn wait_ready(&mut self) -> Result<()> {
        let mut start = Instant::now();
        loop {
            start += self.service_run()?;
            if self.poll_status()?.ready() {
                return Ok(());
            }
            if start.elapsed() >= self.poll.ready_timeout {
                return Err(ControllerError::NotReady(self.poll.ready_timeout));
            }
            self.pause();
        }
    }

    fn wait_idle(&mut self) -> Result<()> {
        let mut start = Instant::now();
        loop {
            start += self.service_run()?;
            if self.poll_status()?.idle() {
                return Ok(());
            }
            if let Some(limit) = self.poll.finish_timeout
                && start.elapsed() >= limit
            {
                return Err(ControllerError::NotFinished(limit));
            }
            self.pause();
        }
    }

    /// galvoplotter's `_list_end` loop: wait READY, send the packet, `SetEndOfList(0)`, and `ExecuteList` once more
    /// than two packets are queued; `ExecuteList` after the last packet if it has not been sent yet.
    fn stream(&mut self, packets: &[Packet]) -> Result<()> {
        let mut executing = false;
        for (i, packet) in packets.iter().enumerate() {
            self.wait_ready()?;
            self.check_abort()?;
            self.link.send_packet(packet)?;
            if self.state == ControllerState::Running {
                self.control.0.packets_sent.fetch_add(1, Ordering::SeqCst);
            }
            self.single(Single::set_end_of_list())?;
            if i + 1 > 2 && !executing {
                self.single(Single::execute_list())?;
                executing = true;
            }
        }
        if !executing {
            self.single(Single::execute_list())?;
        }
        Ok(())
    }
}

/// Held by [`Controller::run`] from the prelude on. Unless the run finished normally (`done`), dropping it sends the
/// abort sequence: this covers a panic unwinding out of the link or the run code, which would otherwise leave the MO
/// open and the laser-enable bit set.
struct RunGuard<'a, L: Link> {
    ctl: &'a mut Controller<L>,
    done: bool,
}

impl<L: Link> Drop for RunGuard<'_, L> {
    fn drop(&mut self) {
        if !self.done {
            let cause = if std::thread::panicking() { "panic during run" } else { "run left early" };
            tracing::error!(cause, "aborting");
            if let Err(e) = self.ctl.abort() {
                tracing::error!(error = %e, "abort sequence failed");
            }
        }
    }
}

/// The frame list for `outline`, `loops` times, as packets. See [`Controller::frame_styled`].
fn light_packets(profile: &MachineProfile, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<Vec<Packet>> {
    if !(1..=MAX_FRAME_LOOPS).contains(&loops) {
        return Err(ControllerError::Refused(format!("loops must be 1..={MAX_FRAME_LOOPS}, got {loops}")));
    }
    let mut polys: Vec<Vec<(u16, u16)>> = Vec::new();
    for poly in outline.iter().filter(|p| !p.is_empty()) {
        polys.push(
            poly.iter()
                .map(|&(x, y)| {
                    let (dx, dy) = profile.red_dot.apply(x, y);
                    field_to_galvo(profile, dx, dy)
                })
                .collect::<Result<_, _>>()?,
        );
    }
    let points: usize = polys.iter().map(|p| p.len() + 2).sum();
    if points == 0 {
        return Err(ControllerError::Refused("nothing to frame: the outline is empty".into()));
    }
    let budget = MAX_FRAME_PACKETS * lmc::SLOTS;
    if points.saturating_mul(loops as usize) > budget.saturating_sub(8) {
        return Err(ControllerError::Refused(format!("frame too large: {points} points x {loops} loops")));
    }
    let speed = style.speed_mm_s.unwrap_or(profile.red_light_speed_mm_s);
    if !(speed.is_finite() && speed > 0.0 && speed <= profile.max_jump_speed_mm_s) {
        return Err(ControllerError::Refused(format!(
            "frame speed {speed} mm/s must be positive and at most the profile's {} mm/s",
            profile.max_jump_speed_mm_s
        )));
    }
    let mut list = LightList::new();
    list.jump_speed(units::speed_to_units(speed, profile.field_transform().speed_field_mm())?)?;
    list.jump_delay(units::delay_word(profile.timing.jump_delay_max_us)?);
    let mut lit = false;
    for _ in 0..loops {
        for poly in &polys {
            let (&(x0, y0), rest) = poly.split_first().ok_or_else(|| ControllerError::Refused("empty polyline".into()))?;
            if lit && polys.len() > 1 && style.blanking {
                list.red_pointer_off();
                lit = false;
            }
            list.jump_to(x0, y0);
            if !lit {
                list.pointer_on(style.pointer);
                lit = true;
            }
            for &(x, y) in rest {
                list.jump_to(x, y);
            }
        }
    }
    list.red_pointer_off();
    Ok(list.finish())
}

#[cfg(test)]
mod tests {
    use lmc::{ListCommand, MarkingList, op};

    use super::*;
    use crate::sim::{SimBoard, SimLink, Traffic};

    const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
    const COR: &[u8] = include_bytes!("../../../fixtures/jcz150.cor");

    fn b6() -> MachineProfile {
        MachineProfile::from_toml(B6).unwrap()
    }

    fn controller(board: SimBoard) -> Controller<SimLink> {
        Controller::with_settings(SimLink::with_board(board), InitSettings::default(), PollConfig::immediate())
    }

    fn ready() -> Controller<SimLink> {
        let mut c = controller(SimBoard::default());
        c.init(&b6(), None).unwrap();
        c.link_mut().clear_traffic();
        c
    }

    fn opcodes(singles: &[Single]) -> Vec<u16> {
        singles.iter().map(Single::opcode).collect()
    }

    /// True if any single in `singles` could make the laser emit.
    fn singles_can_emit(singles: &[Single]) -> bool {
        singles.iter().any(|s| {
            (s.opcode() == op::FIBER_SET_MO && s.0[1] != 0) || (s.opcode() == op::WRITE_PORT && s.0[1] & lmc::LASER_ENABLE_BIT != 0)
        })
    }

    fn job() -> Vec<Packet> {
        let mut l = MarkingList::new();
        l.mark_power(units::power_to_mark_current(20.0).unwrap()).unwrap();
        l.jump_to(30000, 30000);
        for i in 0..700u16 {
            l.mark_to(30000 + i, 31000);
        }
        l.finish()
    }

    #[test]
    fn init_sequence_order_with_real_cor() {
        let cor = CorTable::parse(COR).unwrap();
        let mut c = controller(SimBoard::default());
        let info = c.init(&b6(), Some(&cor)).unwrap();
        assert_eq!(info.serial[..3], SimBoard::default().serial);
        assert_eq!(c.state(), ControllerState::Idle);
        let link = c.link();
        assert!(link.packets().is_empty());
        let singles = link.singles();
        let config = InitSettings::default().configuration();
        assert_eq!(singles.len(), 5 + 1 + corfile::NODE_COUNT + config.len());
        assert_eq!(
            opcodes(&singles[..6]),
            [op::GET_SERIAL_NO, op::GET_VERSION, op::STOP_EXECUTE, op::WRITE_PORT, op::RESET, op::WRITE_COR_TABLE]
        );
        // Deviation from galvoplotter: a list left running is stopped and the port cleared right after GetVersion.
        assert_eq!((singles[2], singles[3]), (Single::stop_execute(), Single::write_port(0)));
        assert_eq!(singles[5], Single::write_cor_table(true));
        let lines = &singles[6..6 + corfile::NODE_COUNT];
        assert!(lines.iter().all(|s| s.opcode() == op::WRITE_COR_LINE));
        assert_eq!(lines[0], Single::write_cor_line(0x89D8, 0x8159, true));
        assert!(lines[1..].iter().all(|s| s.0[3] == 1));
        assert_eq!(singles.iter().filter(|s| s.opcode() == op::WRITE_COR_LINE).count(), 4225);
        assert_eq!(&singles[6 + corfile::NODE_COUNT..], config.as_slice());
        assert!(!singles_can_emit(&singles));
        assert!(!link.mo_open());
    }

    #[test]
    fn init_without_cor_writes_blank_table() {
        let mut c = controller(SimBoard::default());
        c.init(&b6(), None).unwrap();
        let singles = c.link().singles();
        assert_eq!(singles[5], Single::write_cor_table(false));
        assert_eq!(singles[6], Single::enable_laser());
    }

    #[test]
    fn init_stops_a_list_left_running() {
        // A board still executing a list with the laser-enable bit set, as after an engine crash mid-run.
        let mut c = controller(SimBoard::default());
        c.link_mut().send_single(Single::write_port(1)).unwrap();
        c.link_mut().send_single(Single::fiber_set_mo(true)).unwrap();
        c.link_mut().send_packet(&job()[0]).unwrap();
        c.link_mut().send_single(Single::execute_list()).unwrap();
        assert!(c.link().executing());
        c.link_mut().clear_traffic();
        c.init(&b6(), None).unwrap();
        assert!(!c.link().executing() && !c.link().mo_open());
        assert_eq!(c.link().port(), 0);
        let singles = c.link().singles();
        let stop = singles.iter().position(|s| *s == Single::stop_execute()).unwrap();
        assert_eq!(stop, 2, "StopExecute comes right after GetVersion");
    }

    #[test]
    fn frame_traffic_is_light_only() {
        let mut c = ready();
        let square = vec![(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0), (-20.0, -20.0)];
        let circle: Vec<(f64, f64)> = (0..64).map(|i| f64::from(i) * std::f64::consts::TAU / 64.0).map(|a| (5.0 * a.cos(), 5.0 * a.sin())).collect();
        let sent = c.frame(&[square.clone(), circle], 40).unwrap();
        assert!(sent > 3);
        assert_eq!(c.state(), ControllerState::Framing);
        let link = c.link();
        let packets = link.packets();
        assert_eq!(packets.len(), sent);
        assert!(lmc::emits_light_only(&packets));
        assert!(!singles_can_emit(&link.singles()));
        assert!(!link.mo_open());
        assert!(!link.singles().contains(&Single::enable_laser()));
        // The first corner is visited at the expected galvo position.
        let corner = field_to_galvo(&b6(), -20.0, -20.0).unwrap();
        assert!(lmc::decode(&packets).iter().any(|cmd| matches!(cmd, ListCommand::JumpTo { x, y, .. } if (*x, *y) == corner)));
        // ExecuteList follows the third packet, once.
        let traffic = link.traffic();
        let third = traffic.iter().enumerate().filter(|(_, t)| matches!(t, Traffic::Packet(_))).nth(2).unwrap().0;
        let exec: Vec<usize> = traffic.iter().enumerate().filter(|(_, t)| **t == Traffic::Single(Single::execute_list())).map(|(i, _)| i).collect();
        assert_eq!(exec.len(), 1);
        assert!(exec[0] > third);
    }

    #[test]
    fn small_frame_executes_after_last_packet() {
        let mut c = ready();
        assert_eq!(c.frame(&[vec![(0.0, 0.0), (1.0, 1.0)]], 1).unwrap(), 1);
        let singles = c.link().singles_without_polls();
        assert_eq!(opcodes(&singles), [op::FIBER_SET_MO, op::RESET_LIST, op::EXECUTE_LIST]);
        assert_eq!(singles[0], Single::fiber_set_mo(false));
    }

    #[test]
    fn frame_refusals_send_nothing() {
        let mut fresh = controller(SimBoard::default());
        assert_eq!(fresh.frame(&[vec![(0.0, 0.0)]], 1), Err(ControllerError::NotInitialized));
        let mut c = ready();
        let half = b6().field_mm / 2.0;
        assert!(matches!(c.frame(&[vec![(0.0, 0.0), (half, 0.0)]], 1), Err(ControllerError::Field(FieldError::OutsideField { .. }))));
        assert!(matches!(c.frame(&[vec![(0.0, -half - 1.0)]], 1), Err(ControllerError::Field(_))));
        assert!(matches!(c.frame(&[], 1), Err(ControllerError::Refused(_))));
        assert!(matches!(c.frame(&[vec![(0.0, 0.0)]], 0), Err(ControllerError::Refused(_))));
        assert!(matches!(c.frame(&[vec![(0.0, 0.0)]], MAX_FRAME_LOOPS + 1), Err(ControllerError::Refused(_))));
        assert!(c.link().traffic().is_empty());
    }

    #[test]
    fn stop_frame_sequence() {
        let mut c = ready();
        c.frame(&[vec![(0.0, 0.0), (1.0, 0.0)]], 1).unwrap();
        c.link_mut().clear_traffic();
        c.stop_frame().unwrap();
        assert_eq!(c.link().singles(), [Single::stop_execute(), Single::reset_list(), Single::write_port(0)]);
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn abort_sequence_works_uninitialised() {
        let mut c = controller(SimBoard::default());
        c.link_mut().send_single(Single::fiber_set_mo(true)).unwrap();
        c.link_mut().send_single(Single::write_port(0x0101)).unwrap();
        c.link_mut().clear_traffic();
        c.abort().unwrap();
        assert_eq!(
            c.link().singles(),
            [Single::stop_execute(), Single::fiber_set_mo(false), Single::reset_list(), Single::write_port(0), Single::goto_xy(CENTER, CENTER, 0)]
        );
        assert!(!c.link().mo_open());
        assert_eq!((c.link().port(), c.link().position()), (0, (CENTER, CENTER)));
        assert_eq!(c.state(), ControllerState::Uninitialized);
    }

    #[test]
    fn abort_reports_failure_on_dead_link() {
        let mut c = ready();
        c.link_mut().disconnect();
        assert_eq!(c.abort(), Err(ControllerError::Link(LinkError::Disconnected)));
    }

    #[test]
    fn armed_run_sequence() {
        let mut c = ready();
        let packets = job();
        assert!(packets.len() >= 3);
        let report = c.run(&packets, Armed::approved_in_ui("run-1".into())).unwrap();
        assert_eq!((report.run_id.as_str(), report.packets), ("run-1", packets.len()));
        let link = c.link();
        assert_eq!(link.packets(), packets);
        // The B6 profile has a door interlock: its input reads (before and during the run) are covered by
        // `door_protect_refuses_and_aborts_runs`, not here.
        let singles: Vec<Single> = link.singles_without_polls().into_iter().filter(|s| *s != Single::read_port()).collect();
        assert_eq!(
            singles,
            [
                Single::reset_list(),
                Single::write_port(1),
                Single::fiber_set_mo(true),
                Single::execute_list(),
                Single::fiber_set_mo(false),
                Single::write_port(0),
                Single::get_mark_time(),
            ]
        );
        assert!(!link.mo_open() && !link.executing());
        assert_eq!(link.port(), 0);
        assert_eq!(c.state(), ControllerState::Idle);
        // READY was polled before every packet.
        let traffic = link.traffic();
        for (i, t) in traffic.iter().enumerate() {
            if matches!(t, Traffic::Packet(_)) {
                assert_eq!(traffic[i - 1], Traffic::Single(Single::get_version()));
            }
        }
    }

    fn io_profile() -> MachineProfile {
        let mut p = b6();
        p.io = crate::IoPorts {
            door_protect: Some(crate::InputPort { bit: 5, active_low: false }),
            busy_light: Some(3),
            ready_light: Some(4),
            done_marking: Some(6),
            done_pulse_ms: 1,
            red_dot_always_on: true,
            ..crate::IoPorts::default()
        };
        p.validate().unwrap();
        p
    }

    #[test]
    fn io_ports_drive_status_lights_and_done_pulse() {
        let mut c = controller(SimBoard::default());
        c.init(&io_profile(), None).unwrap();
        let idle = 0x10 | lmc::RED_POINTER_BIT;
        assert_eq!(c.link().port(), idle, "ready light and red dot on after init");
        c.link_mut().clear_traffic();
        c.run(&job(), Armed::approved_in_ui("run-1".into())).unwrap();
        let ports: Vec<u16> =
            c.link().singles_without_polls().iter().filter(|s| s.opcode() == op::WRITE_PORT).map(|s| s.0[1]).collect();
        assert_eq!(ports, [0x01 | 0x08 | 0x10, idle, idle | 0x40, idle], "enable + busy + ready, idle, done pulse, idle");
        assert!(ports.iter().skip(1).all(|b| b & lmc::LASER_ENABLE_BIT == 0));
        // Abort clears the port, then restores the idle lights (never the laser-enable bit).
        c.abort().unwrap();
        assert_eq!(c.link().port(), idle);
        c.link_mut().clear_traffic();
        c.apply_idle_outputs().unwrap();
        assert_eq!(c.link().singles_without_polls(), [Single::write_port(idle)]);
    }

    #[test]
    fn door_protect_refuses_and_aborts_runs() {
        let mut c = controller(SimBoard::default());
        c.init(&io_profile(), None).unwrap();
        c.link_mut().set_inputs(1 << 5);
        c.link_mut().clear_traffic();
        let e = c.run(&job(), Armed::approved_in_ui("run-1".into())).unwrap_err();
        assert!(matches!(e, ControllerError::DoorOpen(5)), "{e}");
        assert!(!singles_can_emit(&c.link().singles()), "nothing that can emit was sent");
        assert!(c.link().packets().is_empty());
        // Active low: open when the input reads 0.
        let mut low = io_profile();
        low.io.door_protect = Some(crate::InputPort { bit: 5, active_low: true });
        let mut c = controller(SimBoard::default());
        c.init(&low, None).unwrap();
        assert!(matches!(c.run(&job(), Armed::approved_in_ui("run-2".into())), Err(ControllerError::DoorOpen(5))));
        c.link_mut().set_inputs(1 << 5);
        c.run(&job(), Armed::approved_in_ui("run-3".into())).unwrap();
    }

    #[test]
    fn io_port_validation() {
        let mut p = b6();
        for bad in [0u8, 8, 16] {
            p.io.busy_light = Some(bad);
            assert!(p.validate().is_err(), "bit {bad}");
        }
        p.io.busy_light = Some(3);
        p.io.ready_light = Some(3);
        assert!(p.validate().is_err(), "shared bit");
        p.io.ready_light = None;
        p.io.done_pulse_ms = 6000;
        assert!(p.validate().is_err());
        p.io.done_pulse_ms = 500;
        p.io.door_protect = Some(crate::InputPort { bit: 16, active_low: false });
        assert!(p.validate().is_err());
    }

    #[test]
    fn run_over_cap_sends_nothing() {
        let mut c = ready();
        let mut l = MarkingList::new();
        l.mark_power(units::power_to_mark_current(100.0).unwrap()).unwrap();
        l.mark_to(30000, 30000);
        let mut capped = b6();
        capped.max_power_pct = 50.0;
        c.init(&capped, None).unwrap();
        c.link_mut().clear_traffic();
        assert!(matches!(c.run(&l.finish(), Armed::approved_in_ui("r".into())), Err(ControllerError::Job(JobCheckError::PowerAboveCap { .. }))));
        assert!(c.link().traffic().is_empty());
    }

    #[test]
    fn run_failure_aborts() {
        let mut c = ready();
        c.link_mut().fail_packet(1, LinkError::Timeout);
        let err = c.run(&job(), Armed::approved_in_ui("r".into())).unwrap_err();
        assert_eq!(err, ControllerError::Link(LinkError::Timeout));
        let singles = c.link().singles();
        let tail = &singles[singles.len() - 5..];
        assert_eq!(tail[0], Single::stop_execute());
        assert_eq!(tail[1], Single::fiber_set_mo(false));
        assert!(!c.link().mo_open());
        assert_eq!(c.link().port(), 0);
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn abort_request_cancels_run_and_is_cleared() {
        let mut c = ready();
        let handle = c.abort_handle();
        handle.request();
        assert_eq!(c.run(&job(), Armed::approved_in_ui("r".into())), Err(ControllerError::Aborted));
        assert!(c.link().packets().is_empty());
        assert!(!singles_can_emit(&c.link().singles()));
        assert!(!handle.is_requested());
        assert!(c.run(&job(), Armed::approved_in_ui("r2".into())).is_ok());
    }

    /// A link that panics on one packet, like a bug in the transport or the run code would.
    struct PanickyLink {
        inner: SimLink,
        panic_at_packet: usize,
        packets: usize,
    }

    impl Link for PanickyLink {
        fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
            self.inner.send_single(cmd)
        }

        fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
            self.packets += 1;
            if self.packets == self.panic_at_packet {
                panic!("simulated panic in the middle of a run");
            }
            self.inner.send_packet(packet)
        }
    }

    #[test]
    fn panic_mid_run_sends_the_abort_sequence() {
        let link = PanickyLink { inner: SimLink::new(), panic_at_packet: 2, packets: 0 };
        let mut c = Controller::with_settings(link, InitSettings::default(), PollConfig::immediate());
        c.init(&b6(), None).unwrap();
        let packets = job();
        assert!(packets.len() >= 2);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| c.run(&packets, Armed::approved_in_ui("r".into()))));
        assert!(unwound.is_err(), "the panic propagates to the caller");
        let sim = &c.link().inner;
        // The prelude had opened the MO and set the laser-enable bit; the guard closed both.
        assert!(sim.singles().contains(&Single::fiber_set_mo(true)));
        let tail = &sim.singles()[sim.singles().len() - 5..];
        assert_eq!(
            tail,
            [Single::stop_execute(), Single::fiber_set_mo(false), Single::reset_list(), Single::write_port(0), Single::goto_xy(CENTER, CENTER, 0)]
        );
        assert!(!sim.mo_open() && !sim.executing());
        assert_eq!(sim.port(), 0);
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn abort_request_is_checked_before_the_first_packet() {
        // A stop arriving while a frame is being stopped (between the request and the prelude) still cancels the run.
        let mut c = ready();
        c.frame(&[vec![(0.0, 0.0), (1.0, 0.0)]], 1).unwrap();
        c.link_mut().clear_traffic();
        c.abort_handle().request();
        assert_eq!(c.run(&job(), Armed::approved_in_ui("r".into())), Err(ControllerError::Aborted));
        assert!(c.link().packets().is_empty());
        assert!(!singles_can_emit(&c.link().singles()));
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn replace_link_forgets_the_init() {
        let mut c = ready();
        c.replace_link(SimLink::new());
        assert_eq!((c.state(), c.profile()), (ControllerState::Uninitialized, None));
        assert_eq!(c.frame(&[vec![(0.0, 0.0)]], 1), Err(ControllerError::NotInitialized));
        assert_eq!(c.run(&job(), Armed::approved_in_ui("r".into())), Err(ControllerError::NotInitialized));
        assert!(c.link().traffic().is_empty());
        c.init(&b6(), None).unwrap();
        assert_eq!(c.state(), ControllerState::Idle);
    }

    #[test]
    fn read_port_needs_init_and_sends_only_read_port() {
        let mut c = controller(SimBoard::default());
        assert_eq!(c.read_port(), Err(ControllerError::NotInitialized));
        assert!(c.link().traffic().is_empty());
        let mut c = ready();
        assert!(c.read_port().is_ok());
        assert_eq!(c.link().singles(), [Single::read_port()]);
        assert!(c.link().packets().is_empty() && !c.link().mo_open());
        assert_eq!(c.state(), ControllerState::Idle);
    }

    /// Framing and the burn use one transform: for all 8 swap/negate settings, the frame's galvo words for a field
    /// point equal `lmc::units::field_to_galvo`'s (which `plan::Target::to_galvo` also calls; plan has the mirror
    /// test) and follow the contract's order: negate in the field frame, then swap.
    #[test]
    fn frame_words_match_the_shared_transform_for_every_setting() {
        let w = |v: f64| units::mm_to_galvo(v, 153.75).unwrap();
        for bits in 0..8u8 {
            let profile = MachineProfile { swap_xy: bits & 1 != 0, negate_x: bits & 2 != 0, negate_y: bits & 4 != 0, ..b6() };
            let packets = light_packets(&profile, &[vec![(10.0, 5.0), (-20.0, 7.5)]], 1, &FrameStyle::for_profile(&profile)).unwrap();
            let jumps: Vec<(u16, u16)> = lmc::decode(&packets)
                .into_iter()
                .filter_map(|c| if let ListCommand::JumpTo { x, y, .. } = c { Some((x, y)) } else { None })
                .collect();
            let t = lmc::FieldTransform::plain(153.75, profile.swap_xy, profile.negate_x, profile.negate_y);
            assert_eq!(profile.field_transform(), t);
            for (i, (x, y)) in [(10.0, 5.0), (-20.0, 7.5)].into_iter().enumerate() {
                let direct = units::field_to_galvo(x, y, &t).unwrap();
                assert_eq!(jumps[i], direct, "{bits:03b} ({x}, {y})");
                assert_eq!(field_to_galvo(&profile, x, y), Ok(direct));
                let (fx, fy) = (if t.negate_x { -x } else { x }, if t.negate_y { -y } else { y });
                assert_eq!(direct, if t.swap_xy { (w(fy), w(fx)) } else { (w(fx), w(fy)) }, "{bits:03b}");
            }
        }
    }

    #[test]
    fn run_requires_init() {
        let mut c = controller(SimBoard::default());
        assert_eq!(c.run(&job(), Armed::approved_in_ui("r".into())), Err(ControllerError::NotInitialized));
        assert!(c.link().traffic().is_empty());
    }

    #[test]
    fn ready_timeout_aborts_frame() {
        let mut c = controller(SimBoard { not_ready_polls: usize::MAX, ..SimBoard::default() });
        c.poll.ready_timeout = Duration::from_millis(20);
        c.init(&b6(), None).unwrap();
        let outline: Vec<(f64, f64)> = (0..300).map(|i| (f64::from(i) * 0.1, 0.0)).collect();
        assert!(matches!(c.frame(&[outline], 1), Err(ControllerError::NotReady(_))));
        assert_eq!(c.link().singles().last(), Some(&Single::goto_xy(CENTER, CENTER, 0)));
    }

    /// A link that pauses the run (through its [`RunControl`]) when packet `pause_at` is sent and resumes it after
    /// `resume_after` status polls; it can also raise an input bit after some packets.
    struct ScriptedLink {
        inner: SimLink,
        control: Arc<std::sync::Mutex<Option<RunControl>>>,
        pause_at: usize,
        resume_after: usize,
        input_at: Option<(usize, u16)>,
        packets: usize,
        polls_paused: usize,
        stopped: bool,
    }

    impl Link for ScriptedLink {
        fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
            if cmd == Single::stop_list() {
                self.stopped = true;
            }
            if cmd == Single::restart_list() {
                self.stopped = false;
            }
            if cmd == Single::get_version() && self.stopped {
                self.polls_paused += 1;
                if self.polls_paused >= self.resume_after
                    && let Some(c) = self.control.lock().unwrap().as_ref()
                {
                    c.request_resume();
                }
            }
            self.inner.send_single(cmd)
        }

        fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
            self.packets += 1;
            if self.packets == self.pause_at
                && let Some(c) = self.control.lock().unwrap().as_ref()
            {
                c.request_pause();
            }
            if let Some((at, bits)) = self.input_at
                && self.packets == at
            {
                self.inner.set_inputs(bits);
            }
            self.inner.send_packet(packet)
        }
    }

    fn scripted(pause_at: usize, resume_after: usize, input_at: Option<(usize, u16)>, profile: &MachineProfile) -> Controller<ScriptedLink> {
        let control = Arc::new(std::sync::Mutex::new(None));
        let link = ScriptedLink {
            inner: SimLink::new(),
            control: control.clone(),
            pause_at,
            resume_after,
            input_at,
            packets: 0,
            polls_paused: 0,
            stopped: false,
        };
        let mut c = Controller::with_settings(link, InitSettings::default(), PollConfig::immediate());
        *control.lock().unwrap() = Some(c.run_control());
        c.init(profile, None).unwrap();
        c
    }

    #[test]
    fn pause_sends_stop_list_and_resume_restart_list() {
        let mut c = scripted(2, 5, None, &b6());
        let packets = job();
        let control = c.run_control();
        c.run(&packets, Armed::approved_in_ui("run-p".into())).unwrap();
        let singles: Vec<u16> = c.link().inner.singles_without_polls().iter().map(Single::opcode).collect();
        let stop = singles.iter().position(|o| *o == op::STOP_LIST).expect("paused");
        let restart = singles.iter().position(|o| *o == op::RESTART_LIST).expect("resumed");
        assert!(stop < restart);
        assert_eq!(singles.iter().filter(|o| **o == op::STOP_LIST).count(), 1);
        assert!(c.link().polls_paused >= 5);
        let progress = control.progress();
        assert_eq!((progress.packets_sent, progress.packets_total, progress.paused), (packets.len(), packets.len(), false));
        assert_eq!((progress.part, progress.parts), (0, 1));
        assert_eq!(c.link().inner.packets(), packets, "every packet was sent once");
    }

    #[test]
    fn abort_ends_a_paused_run() {
        let mut c = scripted(1, usize::MAX, None, &b6());
        let abort = c.abort_handle();
        let control = c.run_control();
        let handle = std::thread::spawn(move || {
            while !control.is_paused() {
                std::thread::sleep(Duration::from_millis(1));
            }
            abort.request();
        });
        let r = c.run(&job(), Armed::approved_in_ui("run-a".into()));
        handle.join().unwrap();
        assert_eq!(r, Err(ControllerError::Aborted));
        assert!(!c.link().inner.mo_open() && c.link().inner.port() == 0);
        assert!(!c.run_control().is_paused());
    }

    #[test]
    fn a_pause_requested_before_the_run_holds_it_before_the_first_packet() {
        // pause_at 0: the link never pauses by itself; the request is made before `run`.
        let mut c = scripted(0, 3, None, &b6());
        c.run_control().request_pause();
        let packets = job();
        c.run(&packets, Armed::approved_in_ui("run-early".into())).unwrap();
        let traffic = c.link().inner.traffic();
        let stop = traffic.iter().position(|t| matches!(t, Traffic::Single(s) if s.opcode() == op::STOP_LIST)).expect("paused");
        let restart = traffic.iter().position(|t| matches!(t, Traffic::Single(s) if s.opcode() == op::RESTART_LIST)).expect("resumed");
        let first_packet = traffic.iter().position(|t| matches!(t, Traffic::Packet(_))).expect("packets");
        assert!(stop < restart && restart < first_packet, "nothing was sent while paused");
        assert_eq!(c.link().inner.packets(), packets);
        // An abort drops a pending pause, so the next run is not paused by a stale request.
        let control = c.run_control();
        control.request_pause();
        c.abort().unwrap();
        assert!(!control.pause_requested());
    }

    #[test]
    fn stop_input_aborts_the_run() {
        let mut p = b6();
        p.stop_mark_input_bit = Some(13);
        let mut c = scripted(0, 0, Some((2, 1 << 13)), &p);
        let r = c.run(&job(), Armed::approved_in_ui("run-s".into()));
        assert_eq!(r, Err(ControllerError::StopInput(13)));
        assert!(!c.link().inner.mo_open());
        // Without the profile bit the same input does nothing.
        let mut c = scripted(0, 0, Some((2, 1 << 13)), &b6());
        assert!(c.run(&job(), Armed::approved_in_ui("run-t".into())).is_ok());
    }

    fn with_axis() -> MachineProfile {
        let mut p = b6();
        p.axis = Some(crate::ExtensionAxis {
            role: crate::AxisRole::Rotary,
            steps_per_mm: 0.0,
            min_mm: 0.0,
            max_mm: 0.0,
            invert: true,
        });
        p
    }

    #[test]
    fn parts_move_the_axis_with_the_mo_closed() {
        let mut c = controller(SimBoard::default());
        c.init(&with_axis(), None).unwrap();
        c.link_mut().clear_traffic();
        let parts = vec![
            RunPart { axis_position: Some(100), packets: job() },
            RunPart { axis_position: Some(200), packets: job() },
        ];
        let report = c.run_parts(&parts, Armed::approved_in_ui("run-r".into())).unwrap();
        assert_eq!(report.packets, parts[0].packets.len() * 2);
        let singles = c.link().singles_without_polls();
        let moves: Vec<usize> = singles.iter().enumerate().filter(|(_, s)| s.opcode() == op::MOVE_AXIS_TO).map(|(i, _)| i).collect();
        assert_eq!(moves.len(), 2);
        assert_eq!(singles[moves[0]], Single::move_axis_to(100, true));
        assert_eq!(singles[moves[1]], Single::move_axis_to(200, true));
        // Before the second move the MO is closed; after it, it is opened again.
        assert_eq!(singles[moves[1] - 1], Single::fiber_set_mo(false));
        assert!(singles[moves[1]..].contains(&Single::fiber_set_mo(true)));
        assert_eq!(c.link().axis_position(), 200);
        assert!(!c.link().mo_open());
        // A move-only part at the end returns the axis without opening the MO again.
        c.link_mut().clear_traffic();
        let mut with_return = parts.clone();
        with_return.push(RunPart::axis_only(0));
        c.run_parts(&with_return, Armed::approved_in_ui("run-z".into())).unwrap();
        assert_eq!(c.link().axis_position(), 0);
        let singles = c.link().singles_without_polls();
        let last_move = singles.iter().rposition(|s| s.opcode() == op::MOVE_AXIS_TO).unwrap();
        assert!(!singles[last_move..].contains(&Single::fiber_set_mo(true)));
        assert!(matches!(c.run_parts(&[RunPart::axis_only(5)], Armed::approved_in_ui("e".into())), Err(ControllerError::Job(JobCheckError::Empty))));
        // Without an axis in the profile, a part with a move is refused before anything is sent.
        let mut c = ready();
        let r = c.run_parts(&parts, Armed::approved_in_ui("run-x".into()));
        assert!(matches!(r, Err(ControllerError::Refused(_))));
        assert!(c.link().traffic().is_empty());
    }

    #[test]
    fn move_axis_needs_an_axis_and_never_emits() {
        let mut c = ready();
        assert!(matches!(c.move_axis(10), Err(ControllerError::Refused(_))));
        let mut c = controller(SimBoard::default());
        c.init(&with_axis(), None).unwrap();
        c.link_mut().clear_traffic();
        c.move_axis(42).unwrap();
        assert_eq!(c.link().axis_position(), 42);
        assert!(!singles_can_emit(&c.link().singles()));
    }

    #[test]
    fn frame_style_pointer_blanking_speed_and_red_dot() {
        let mut p = b6();
        p.focus_light_port_bit = Some(9);
        p.red_dot.offset_x_mm = 1.0;
        p.red_dot.rest_x_mm = 10.0;
        let mut c = controller(SimBoard::default());
        c.init(&p, None).unwrap();
        c.link_mut().clear_traffic();
        let two = [vec![(0.0, 0.0), (5.0, 0.0)], vec![(10.0, 0.0), (15.0, 0.0)]];
        let focus = PointerBits::bit(9).unwrap();
        let style = FrameStyle { pointer: focus, blanking: false, speed_mm_s: Some(1000.0) };
        c.frame_styled(&two, 1, &style).unwrap();
        let cmds = lmc::decode(&c.link().packets());
        let ports: Vec<u16> = cmds.iter().filter_map(|c| if let ListCommand::WritePort { bits } = c { Some(*bits) } else { None }).collect();
        assert_eq!(ports, [0x0200, 0], "focus light on once (no blanking), off at the end");
        assert!(cmds.contains(&ListCommand::JumpSpeed { units: units::speed_to_units(1000.0, p.field_mm).unwrap() }));
        // The red-dot offset moves every framed point by +1 mm in X.
        let first = cmds.iter().find_map(|c| if let ListCommand::JumpTo { x, y, .. } = c { Some((*x, *y)) } else { None }).unwrap();
        assert_eq!(first, field_to_galvo(&p, 1.0, 0.0).unwrap());
        assert!(lmc::emits_light_only(&c.link().packets()));
        // With blanking the pointer goes off between the two shapes.
        c.link_mut().clear_traffic();
        c.frame_styled(&two, 1, &FrameStyle { blanking: true, ..style }).unwrap();
        let cmds = lmc::decode(&c.link().packets());
        assert_eq!(cmds.iter().filter(|c| matches!(c, ListCommand::WritePort { bits: 0 })).count(), 2);
        // Too fast is refused; stop parks the dot at its rest position.
        let fast = FrameStyle { speed_mm_s: Some(p.max_jump_speed_mm_s + 1.0), ..style };
        assert!(matches!(c.frame_styled(&two, 1, &fast), Err(ControllerError::Refused(_))));
        c.link_mut().clear_traffic();
        c.stop_frame().unwrap();
        let (rx, ry) = field_to_galvo(&p, 10.0, 0.0).unwrap();
        assert_eq!(c.link().singles().last(), Some(&Single::goto_xy(rx, ry, 0)));
    }

    #[test]
    fn controller_info_is_kept_and_refreshed() {
        let mut c = ready();
        assert_eq!(c.board_info().unwrap().serial[..3], SimBoard::default().serial);
        c.link_mut().clear_traffic();
        let info = c.refresh_info().unwrap();
        assert_eq!(info.version[..3], SimBoard::default().version);
        assert_eq!(opcodes(&c.link().singles()), [op::GET_SERIAL_NO, op::GET_VERSION]);
        let mut fresh = controller(SimBoard::default());
        assert_eq!(fresh.refresh_info(), Err(ControllerError::NotInitialized));
    }

    #[test]
    fn calibration_moves_frame_and_burn_alike() {
        let mut p = b6();
        p.calibration.offset_x_mm = 2.0;
        p.calibration.angle_deg = 1.0;
        let mut c = controller(SimBoard::default());
        c.init(&p, None).unwrap();
        c.link_mut().clear_traffic();
        c.frame(&[vec![(10.0, 10.0), (20.0, 10.0)]], 1).unwrap();
        let first = lmc::decode(&c.link().packets())
            .into_iter()
            .find_map(|c| if let ListCommand::JumpTo { x, y, .. } = c { Some((x, y)) } else { None })
            .unwrap();
        assert_eq!(first, lmc::units::field_to_galvo(10.0, 10.0, &p.field_transform()).unwrap());
        assert_ne!(first, lmc::units::field_to_galvo(10.0, 10.0, &b6().field_transform()).unwrap());
    }
}
