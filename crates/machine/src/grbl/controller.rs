//! [`GrblController`]: init, status, light-only framing, armed runs, Stop, homing, jogging and the console of one
//! GRBL 1.1 laser over a [`SerialLink`].
//!
//! Safety rules this module enforces (the galvo [`crate::Controller`]'s, carried over):
//!
//! - **Only an armed run fires.** [`GrblController::run_program`] needs an [`Armed`] token (made only by the
//!   engine's approval module) and checks the program against the profile first ([`check_program`]): power within
//!   `max_power_pct` of `$30`, feeds within the limit, every point on the bed, the laser off at the end. Before it
//!   streams it re-reads the board's settings and refuses unless laser mode is on (`$32=1`, so rapids never burn)
//!   and `$30`/`$31` match the profile (so S means the power it was planned for), the board is idle and unlocked, the
//!   safety door is closed and no start-up block can turn the laser on.
//! - **Framing is light-only**: the laser off (`M5`) and jog moves (`$J=`), never `M3`/`M4` or an `S` word. There is
//!   no low-power frame option.
//! - **The console** ([`GrblController::console`]) sends only lines the console policy accepts
//!   ([`check_console_line`]: never `M3`/`M4`, never `S > 0`), and a line that moves or dwells only while the
//!   board's parser state reports the spindle off (`M5`). Homing and jogging need the same (a jog with `M3`/`M4` and
//!   `S > 0` still active would fire); unlocking moves nothing.
//! - **Stop always wins** ([`GrblController::abort`]): realtime feed hold `!` then soft reset `0x18`, written at once
//!   and never queued behind buffered lines (realtime bytes bypass GRBL's line buffer). A soft reset clears GRBL's
//!   buffers and turns the laser off. An [`crate::AbortHandle`] request is noticed within one read timeout of a
//!   streaming run, frame, homing cycle or command; any error during a run, and a run left early or by a panic (a
//!   drop guard), sends the same sequence. A reset while moving leaves GRBL in alarm 3 (position may be lost): it
//!   must then be homed or unlocked by a person.
//! - **Flow control**: character counting. GRBL's receive buffer holds [`RX_BUFFER`] bytes; a line is written only
//!   when it fits beside the bytes of the lines not yet answered (`ok`/`error`), so the buffer never overflows.
//! - **Pause** sends feed hold `!`, resume cycle start `~`; an open safety door (GRBL's `Door` state or the `D` pin)
//!   aborts a run like the galvo's door interlock.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::console::{ConsoleKind, check_console_line};
use super::profile::GrblProfile;
use super::program::{GcodeProgram, check_program};
use super::protocol::{CYCLE_START, FEED_HOLD, GrblState, JOG_CANCEL, RX_BUFFER, Response, SOFT_RESET, STATUS, StatusReport, parse_line};
use super::{GrblError, SerialLink};
use crate::armed::Armed;
use crate::controller::{AbortHandle, ControllerError, ControllerState, FrameStyle, MAX_FRAME_LOOPS, RunControl, RunReport};

type Result<T, E = ControllerError> = std::result::Result<T, E>;

/// Most lines one frame call may send.
pub const MAX_FRAME_LINES: usize = 20_000;
/// A frame ended by its cancel flag (internal).
const FRAME_CANCELLED: &str = "frame cancelled";
/// Board messages kept for the status view.
const KEEP_MESSAGES: usize = 50;

/// Timing of the GRBL driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrblPoll {
    /// How often a streaming run or frame asks for a status report.
    pub status_interval: Duration,
    /// Longest single read; also how soon a stop request is noticed while streaming.
    pub read_timeout: Duration,
    /// Longest wait for an answer (`ok` to a command, a report to `?`).
    pub response_timeout: Duration,
    /// Longest wait for the start-up message after a reset (an Arduino-based board reboots when the port opens).
    pub reset_timeout: Duration,
    /// Longest homing cycle.
    pub homing_timeout: Duration,
    /// Longest run after its last line was accepted; `None` waits while the board reports motion.
    pub finish_timeout: Option<Duration>,
}

impl Default for GrblPoll {
    fn default() -> Self {
        GrblPoll {
            status_interval: Duration::from_millis(200),
            read_timeout: Duration::from_millis(10),
            response_timeout: Duration::from_secs(5),
            reset_timeout: Duration::from_secs(5),
            homing_timeout: Duration::from_secs(120),
            finish_timeout: None,
        }
    }
}

impl GrblPoll {
    /// No waiting and short limits, for [`super::SimGrbl`].
    pub fn immediate() -> Self {
        GrblPoll {
            status_interval: Duration::ZERO,
            read_timeout: Duration::ZERO,
            response_timeout: Duration::from_secs(2),
            reset_timeout: Duration::from_secs(2),
            homing_timeout: Duration::from_secs(2),
            finish_timeout: Some(Duration::from_secs(5)),
        }
    }
}

/// What `init` learnt about the board.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GrblInfo {
    pub version: String,
    /// `$32`.
    pub laser_mode: bool,
    /// `$30`, `$31`.
    pub s_max: Option<f64>,
    pub s_min: Option<f64>,
    /// `$22`.
    pub homing: bool,
    pub state: GrblState,
}

/// The board as last seen, for the status page (`GET /api/machine/grbl`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GrblSnapshot {
    pub version: Option<String>,
    pub status: Option<StatusReport>,
    pub alarm: Option<u8>,
    pub laser_mode: Option<bool>,
    pub s_max: Option<f64>,
    pub s_min: Option<f64>,
    /// Start-up blocks that could turn the laser on (runs, frames and moves are refused while there are any).
    pub unsafe_startup: Vec<String>,
    /// The parser state reports `M5`.
    pub spindle_off: bool,
    pub messages: Vec<String>,
}

/// The board's answer to a console line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsoleReply {
    pub line: String,
    /// Lines the board sent before its `ok` / `error` (settings, parser state, messages).
    pub lines: Vec<String>,
    pub ok: bool,
    /// The board's `error:N` or `ALARM:N`, explained.
    pub error: Option<String>,
}

/// One answered command.
struct Reply {
    lines: Vec<String>,
    result: std::result::Result<(), u8>,
}

/// Drives one GRBL board. See the module docs.
#[derive(Debug)]
pub struct GrblController<P: SerialLink> {
    port: P,
    poll: GrblPoll,
    profile: Option<GrblProfile>,
    state: ControllerState,
    abort: AbortHandle,
    control: RunControl,
    /// Ends a frame being streamed (set by the engine when framing stops or is replaced, without the driver lock);
    /// cleared when a frame starts.
    frame_cancel: AbortHandle,
    /// Bytes (with the newline) of each line GRBL has not answered yet, oldest first.
    inflight: VecDeque<usize>,
    /// A line still arriving.
    rx: Vec<u8>,
    wco: Option<[f64; 3]>,
    status: Option<StatusReport>,
    alarm: Option<u8>,
    version: Option<String>,
    settings: BTreeMap<u16, f64>,
    unsafe_startup: Vec<String>,
    spindle_off: bool,
    messages: VecDeque<String>,
    /// Replies collected for the command waiting on them.
    collected: Vec<String>,
}

impl<P: SerialLink> GrblController<P> {
    pub fn new(port: P) -> Self {
        Self::with_handles(port, GrblPoll::default(), AbortHandle::default(), RunControl::default())
    }

    /// A controller sharing existing abort and run-control handles (see [`crate::Controller::with_handles`]).
    pub fn with_handles(port: P, poll: GrblPoll, abort: AbortHandle, control: RunControl) -> Self {
        GrblController {
            port,
            poll,
            profile: None,
            state: ControllerState::Uninitialized,
            abort,
            control,
            frame_cancel: AbortHandle::default(),
            inflight: VecDeque::new(),
            rx: Vec::new(),
            wco: None,
            status: None,
            alarm: None,
            version: None,
            settings: BTreeMap::new(),
            unsafe_startup: Vec::new(),
            spindle_off: false,
            messages: VecDeque::new(),
            collected: Vec::new(),
        }
    }

    /// Swap in a new port (a freshly opened one, or [`super::OfflineSerial`]); the board is then uninitialised.
    pub fn replace_port(&mut self, port: P) {
        self.port = port;
        self.profile = None;
        self.state = ControllerState::Uninitialized;
        self.inflight.clear();
        self.rx.clear();
        self.spindle_off = false;
    }

    pub fn profile(&self) -> Option<&GrblProfile> {
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

    /// Share a frame-cancel flag: requesting it ends the frame being streamed (then jog cancel), without waiting for
    /// the driver. Each frame clears it when it starts.
    pub fn set_frame_cancel(&mut self, cancel: AbortHandle) {
        self.frame_cancel = cancel;
    }

    /// The board as last seen.
    pub fn snapshot(&self) -> GrblSnapshot {
        GrblSnapshot {
            version: self.version.clone(),
            status: self.status.clone(),
            alarm: self.alarm,
            laser_mode: self.settings.get(&32).map(|v| *v != 0.0),
            s_max: self.settings.get(&30).copied(),
            s_min: self.settings.get(&31).copied(),
            unsafe_startup: self.unsafe_startup.clone(),
            spindle_off: self.spindle_off,
            messages: self.messages.iter().cloned().collect(),
        }
    }

    // ----- transport -----

    fn inflight_bytes(&self) -> usize {
        self.inflight.iter().sum()
    }

    fn write_rt(&mut self, b: u8) -> Result<()> {
        Ok(self.port.write(&[b])?)
    }

    /// Write `line` (no newline) and count its bytes in GRBL's buffer. The caller made room.
    fn write_line(&mut self, line: &str) -> Result<()> {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        self.port.write(&bytes)?;
        self.inflight.push_back(bytes.len());
        Ok(())
    }

    /// One read: the complete lines that arrived, classified, with the bookkeeping done (answers free buffer space,
    /// reports and alarms are remembered, a start-up message means the board reset and its buffer is empty).
    fn read_responses(&mut self, timeout: Duration) -> Result<Vec<Response>> {
        let mut buf = [0u8; 512];
        let n = self.port.read(&mut buf, timeout)?;
        self.rx.extend_from_slice(&buf[..n]);
        let mut out = Vec::new();
        while let Some(end) = self.rx.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.rx.drain(..=end).collect();
            let line = String::from_utf8_lossy(&raw).trim().to_owned();
            if line.is_empty() {
                continue;
            }
            let r = parse_line(&line, &mut self.wco);
            match &r {
                Response::Ok | Response::Error(_) => {
                    self.inflight.pop_front();
                }
                Response::Status(s) => {
                    if s.state != GrblState::Alarm {
                        self.alarm = None;
                    }
                    self.status = Some(s.clone());
                }
                Response::Alarm(code) => {
                    self.alarm = Some(*code);
                    self.note(line.clone());
                }
                Response::Welcome(v) => {
                    self.version = Some(v.clone());
                    self.inflight.clear();
                    self.spindle_off = true;
                }
                Response::Other(text) => {
                    if text.starts_with("[MSG:") || text.starts_with('>') {
                        self.note(text.clone());
                    }
                    self.collected.push(text.clone());
                }
            }
            out.push(r);
        }
        if self.rx.len() > 4096 {
            // No line ending for 4 KiB: not GRBL talking.
            self.rx.clear();
            return Err(ControllerError::Refused("the board sends lines longer than 4 KiB (wrong baud rate?)".into()));
        }
        Ok(out)
    }

    fn note(&mut self, m: String) {
        self.messages.push_back(m);
        while self.messages.len() > KEEP_MESSAGES {
            self.messages.pop_front();
        }
    }

    fn check_abort(&self) -> Result<()> {
        if self.abort.is_requested() { Err(ControllerError::Aborted) } else { Ok(()) }
    }

    /// Read until the board's start-up message (after a reset), then briefly more to swallow a second one (the
    /// board may also reboot because the port was opened).
    fn wait_welcome(&mut self, timeout: Duration) -> Result<String> {
        let start = Instant::now();
        loop {
            for r in self.read_responses(self.poll.read_timeout.max(Duration::from_millis(1)).min(timeout))? {
                if let Response::Welcome(v) = r {
                    let settle = (self.poll.reset_timeout / 20).min(Duration::from_millis(250));
                    let until = Instant::now() + settle;
                    while Instant::now() < until {
                        self.read_responses(self.poll.read_timeout)?;
                    }
                    self.inflight.clear();
                    return Ok(v);
                }
            }
            if start.elapsed() >= timeout {
                return Err(GrblError::NoWelcome(timeout).into());
            }
        }
    }

    /// Wait until GRBL answered every line in its buffer (errors of those lines are ignored here).
    fn drain(&mut self, timeout: Duration) -> Result<()> {
        let start = Instant::now();
        while !self.inflight.is_empty() {
            self.check_abort()?;
            self.read_responses(self.poll.read_timeout)?;
            if start.elapsed() >= timeout {
                self.inflight.clear();
                return Err(GrblError::NoResponse(timeout).into());
            }
        }
        Ok(())
    }

    /// Send one line on its own and wait for its answer, collecting the lines before it. A stop request ends the
    /// wait (`Aborted`).
    fn command(&mut self, line: &str, timeout: Duration) -> Result<Reply> {
        self.drain(self.poll.response_timeout)?;
        self.collected.clear();
        self.write_line(line)?;
        let start = Instant::now();
        loop {
            self.check_abort()?;
            for r in self.read_responses(self.poll.read_timeout)? {
                match r {
                    Response::Ok => return Ok(Reply { lines: std::mem::take(&mut self.collected), result: Ok(()) }),
                    Response::Error(code) => return Ok(Reply { lines: std::mem::take(&mut self.collected), result: Err(code) }),
                    Response::Alarm(code) => {
                        self.inflight.clear();
                        return Err(GrblError::Alarm(code).into());
                    }
                    Response::Welcome(_) => return Err(GrblError::Reset.into()),
                    _ => {}
                }
            }
            if start.elapsed() >= timeout {
                self.inflight.clear();
                return Err(GrblError::NoResponse(timeout).into());
            }
        }
    }

    /// [`GrblController::command`] that must succeed.
    fn command_ok(&mut self, line: &str, timeout: Duration) -> Result<Vec<String>> {
        let r = self.command(line, timeout)?;
        r.result.map_err(|code| GrblError::Rejected { code, line: line.to_owned() })?;
        Ok(r.lines)
    }

    /// Ask for a status report and wait for it.
    fn request_status(&mut self) -> Result<StatusReport> {
        self.write_rt(STATUS)?;
        let start = Instant::now();
        loop {
            for r in self.read_responses(self.poll.read_timeout)? {
                if let Response::Status(s) = r {
                    return Ok(s);
                }
            }
            if start.elapsed() >= self.poll.response_timeout {
                return Err(GrblError::NoResponse(self.poll.response_timeout).into());
            }
        }
    }

    fn read_settings(&mut self) -> Result<()> {
        let lines = self.command_ok("$$", self.poll.response_timeout)?;
        self.settings = lines
            .iter()
            .filter_map(|l| {
                let (k, v) = l.strip_prefix('$')?.split_once('=')?;
                Some((k.parse().ok()?, v.trim().parse().ok()?))
            })
            .collect();
        Ok(())
    }

    fn read_startup(&mut self) -> Result<()> {
        let lines = self.command_ok("$N", self.poll.response_timeout)?;
        self.unsafe_startup = lines
            .iter()
            .filter_map(|l| {
                let block = l.strip_prefix("$N")?.split_once('=')?.1.trim();
                (!block.is_empty() && check_console_line(block).is_err()).then(|| l.clone())
            })
            .collect();
        Ok(())
    }

    fn read_parser_state(&mut self) -> Result<()> {
        let lines = self.command_ok("$G", self.poll.response_timeout)?;
        self.spindle_off = lines.iter().any(|l| l.starts_with("[GC:") && l.split([' ', ']']).any(|w| w == "M5"));
        Ok(())
    }

    /// Before an unarmed line that moves or dwells (a jog, homing, a console move or `G4`): the board's parser state
    /// must say the spindle is off (`M5`), read again (`$G`) when that is not known. A start-up block cleared after
    /// the reset that ran it, or a stop whose reset was never confirmed, can leave `M3`/`M4` with `S > 0` active, and
    /// a jog or a constant-power dwell would then fire.
    fn require_spindle_off(&mut self) -> std::result::Result<(), GrblError> {
        if !self.spindle_off {
            self.read_parser_state().map_err(|e| GrblError::Settings(format!("could not read the parser state ($G): {e}")))?;
        }
        if self.spindle_off {
            Ok(())
        } else {
            Err(GrblError::Settings("the board's spindle state is not M5 (the laser could be on); send M5 in the console first".into()))
        }
    }

    // ----- connect and status -----

    /// Connect-time setup: soft reset and wait for GRBL's start-up message, then read the settings (`$$`), the
    /// start-up blocks (`$N`), the parser state (`$G`) and a status report. Nothing here moves or fires. A board
    /// that starts locked (homing enabled, or an alarm) stays locked: unlocking and homing are a person's call.
    pub fn init(&mut self, profile: &GrblProfile) -> Result<GrblInfo> {
        profile.validate()?;
        self.profile = None;
        self.state = ControllerState::Uninitialized;
        self.inflight.clear();
        self.rx.clear();
        self.abort.clear();
        self.write_rt(SOFT_RESET)?;
        let version = self.wait_welcome(self.poll.reset_timeout)?;
        self.read_settings()?;
        self.read_startup()?;
        self.read_parser_state()?;
        let status = self.request_status()?;
        if !self.unsafe_startup.is_empty() {
            tracing::warn!(blocks = ?self.unsafe_startup, "GRBL start-up blocks can turn the laser on; motion is refused until they are cleared");
        }
        self.profile = Some(profile.clone());
        self.state = ControllerState::Idle;
        Ok(GrblInfo {
            version,
            laser_mode: self.settings.get(&32).is_some_and(|v| *v != 0.0),
            s_max: self.settings.get(&30).copied(),
            s_min: self.settings.get(&31).copied(),
            homing: self.settings.get(&22).is_some_and(|v| *v != 0.0),
            state: status.state,
        })
    }

    /// A status report (`?`). A finished frame moves the state back to idle.
    pub fn status(&mut self) -> Result<StatusReport> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        let s = self.request_status()?;
        if self.state == ControllerState::Framing && s.state == GrblState::Idle && self.inflight.is_empty() {
            self.state = ControllerState::Idle;
        }
        Ok(s)
    }

    /// Whether the board is idle with nothing left in its buffer (ends a frame).
    pub fn poll_idle(&mut self) -> Result<bool> {
        let s = self.status()?;
        Ok(s.state == GrblState::Idle && self.inflight.is_empty())
    }

    /// Whether the safety door is open (a status report).
    pub fn door_open(&mut self) -> Result<bool> {
        Ok(self.status()?.door_open())
    }

    /// Before anything that moves: initialised, not running, unlocked, no start-up block that can fire, and (for a
    /// run) idle with the door closed. Returns the profile.
    fn ready_to_move(&mut self, run: bool) -> Result<GrblProfile> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if !self.unsafe_startup.is_empty() {
            return Err(GrblError::Settings(format!(
                "start-up blocks can turn the laser on after every reset ({}); clear them in the console ($N0= / $N1=)",
                self.unsafe_startup.join(", ")
            ))
            .into());
        }
        let s = self.request_status()?;
        match s.state {
            GrblState::Alarm => return Err(GrblError::Locked(self.alarm).into()),
            GrblState::Door => return Err(GrblError::DoorOpen.into()),
            _ if s.door_open() && run => return Err(GrblError::DoorOpen.into()),
            GrblState::Check if run => {
                return Err(GrblError::NotIdle("in check mode ($C): nothing would be marked; send $C again to leave it".into()).into());
            }
            GrblState::Idle | GrblState::Check => {}
            GrblState::Jog if !run => {}
            other => return Err(GrblError::NotIdle(other.name().to_owned()).into()),
        }
        Ok(profile)
    }

    // ----- framing -----

    /// Trace `outline` (polylines in field mm) `loops` times with the laser off: `M5`, then one absolute jog
    /// (`$J=G90 G21 X.. Y.. F..`) per point at the style's speed (mm/s, default the profile's frame feed). Blanking and
    /// the pointer choice do not apply (there is no pointer: nothing lights). Refused outside the bed and above the
    /// feed limit. Streams every line, then returns (the board finishes the last moves on its own).
    pub fn frame_styled(&mut self, outline: &[Vec<(f64, f64)>], loops: u32, style: &FrameStyle) -> Result<usize> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if !(1..=MAX_FRAME_LOOPS).contains(&loops) {
            return Err(ControllerError::Refused(format!("loops must be 1..={MAX_FRAME_LOOPS}, got {loops}")));
        }
        let feed = style.speed_mm_s.map_or(profile.frame_feed_mm_min, |v| v * 60.0);
        if !(feed.is_finite() && feed > 0.0 && feed <= profile.max_feed_mm_min + 1e-9) {
            return Err(ControllerError::Refused(format!(
                "frame speed {feed:.0} mm/min must be positive and at most the profile's {} mm/min",
                profile.max_feed_mm_min
            )));
        }
        let mut points = Vec::new();
        for poly in outline {
            for &(x, y) in poly {
                points.push(profile.to_machine(x, y).map_err(ControllerError::Refused)?);
            }
        }
        if points.is_empty() {
            return Err(ControllerError::Refused("nothing to frame: the outline is empty".into()));
        }
        if points.len().saturating_mul(loops as usize) > MAX_FRAME_LINES {
            return Err(ControllerError::Refused(format!("frame too large: {} points x {loops} loops", points.len())));
        }
        let mut lines = vec!["M5".to_owned()];
        for _ in 0..loops {
            lines.extend(points.iter().map(|&(x, y)| format!("$J=G90G21X{}Y{}F{}", num(x), num(y), num(feed))));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        self.ready_to_move(false)?;
        self.frame_cancel.clear();
        self.state = ControllerState::Framing;
        match self.stream_simple(&lines) {
            Ok(()) => Ok(lines.len()),
            Err(ControllerError::Refused(why)) if why == FRAME_CANCELLED => {
                self.stop_frame()?;
                Ok(0)
            }
            Err(ControllerError::Aborted) => {
                self.abort_after(&ControllerError::Aborted);
                Err(ControllerError::Aborted)
            }
            Err(e) => {
                let _ = self.stop_frame();
                Err(e)
            }
        }
    }

    /// Stream `lines` with character counting until all are written; any `error`/`ALARM` or a stop request ends it.
    fn stream_simple(&mut self, lines: &[String]) -> Result<()> {
        let mut next = 0;
        let start = Instant::now();
        let mut last_answer = Instant::now();
        while next < lines.len() {
            self.check_abort()?;
            if self.frame_cancel.is_requested() {
                return Err(ControllerError::Refused(FRAME_CANCELLED.into()));
            }
            let len = lines[next].len() + 1;
            if self.inflight_bytes() + len <= RX_BUFFER {
                self.write_line(&lines[next])?;
                next += 1;
                continue;
            }
            for r in self.read_responses(self.poll.read_timeout)? {
                last_answer = Instant::now();
                match r {
                    Response::Error(code) => return Err(GrblError::Rejected { code, line: lines[next.saturating_sub(1)].clone() }.into()),
                    Response::Alarm(code) => return Err(GrblError::Alarm(code).into()),
                    Response::Welcome(_) => return Err(GrblError::Reset.into()),
                    _ => {}
                }
            }
            // A frame's jog lines are accepted as fast as the planner frees room; no answer for this long is a dead
            // board.
            if last_answer.elapsed() > self.poll.response_timeout.max(Duration::from_secs(1)) * 6 {
                return Err(GrblError::NoResponse(start.elapsed()).into());
            }
        }
        Ok(())
    }

    /// Stop a frame: jog cancel (`0x85`: GRBL stops and drops the jog motion without an alarm), wait for the lines
    /// still in its buffer to be answered, and cancel again (they may have started new jogs).
    pub fn stop_frame(&mut self) -> Result<()> {
        let r = self.write_rt(JOG_CANCEL);
        if r.is_ok() {
            let _ = self.drain(self.poll.response_timeout);
            let _ = self.write_rt(JOG_CANCEL);
        }
        if self.state == ControllerState::Framing {
            self.state = ControllerState::Idle;
        }
        r
    }

    // ----- runs -----

    /// Run a G-code program. Requires an [`Armed`] token, which only the engine's approval creates.
    ///
    /// Order: [`check_program`] against the profile (nothing is sent if it fails); a frame is stopped; a pending stop
    /// request cancels the run before anything is sent; the board must be unlocked, idle, its door closed, laser mode
    /// on and `$30`/`$31` equal to the profile's (settings read again now) with no start-up block that can fire;
    /// then the lines stream with character counting, a status report every `status_interval`, the stop request
    /// checked at every step, pause/resume as feed hold / cycle start; the run ends when every line was answered
    /// and the board reports `Idle`. Any `error:N`, alarm, open door, reset or timeout aborts (`!` then `0x18`).
    pub fn run_program(&mut self, program: &GcodeProgram, armed: Armed) -> Result<RunReport> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        check_program(&profile, program).map_err(|e| ControllerError::Grbl(GrblError::Program(e)))?;
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        self.ready_to_move(true)?;
        // Settings and start-up blocks may have changed since init (the console, another program): read them again.
        self.read_settings()?;
        self.check_settings(&profile)?;
        self.read_startup()?;
        if !self.unsafe_startup.is_empty() {
            return Err(GrblError::Settings(format!("start-up blocks can turn the laser on ({})", self.unsafe_startup.join(", "))).into());
        }
        tracing::info!(run_id = armed.run_id(), lines = program.len(), "starting armed GRBL run");
        self.state = ControllerState::Running;
        self.control.start_count(program.len());
        let mut guard = RunGuard { ctl: self, done: false };
        let result = guard.ctl.stream_run(program);
        guard.ctl.control.set_paused(false);
        match &result {
            Err(e) => guard.ctl.abort_after(e),
            Ok(()) => {
                guard.ctl.state = ControllerState::Idle;
                guard.ctl.spindle_off = true;
            }
        }
        guard.done = true;
        result.map(|()| RunReport { run_id: armed.run_id().to_owned(), packets: program.len(), mark_time: None })
    }

    fn check_settings(&self, profile: &GrblProfile) -> Result<()> {
        let get = |n: u16| self.settings.get(&n).copied();
        if get(32) != Some(1.0) {
            return Err(GrblError::Settings(
                "laser mode is off ($32=0): GRBL would keep the laser on through rapid moves; set $32=1".into(),
            )
            .into());
        }
        for (n, want, name) in [(30u16, profile.s_max, "s_max"), (31, profile.s_min, "s_min")] {
            match get(n) {
                Some(v) if (v - want).abs() < 1e-6 => {}
                v => {
                    return Err(GrblError::Settings(format!(
                        "the board's ${n} is {} but the profile's {name} is {want}: S values would mean another power; fix the profile or the board",
                        v.map_or("unknown".to_owned(), |v| v.to_string())
                    ))
                    .into());
                }
            }
        }
        Ok(())
    }

    fn stream_run(&mut self, program: &GcodeProgram) -> Result<()> {
        let lines = &program.lines;
        // Index of the line each in-flight entry belongs to (for error messages).
        let mut sent_index: VecDeque<usize> = VecDeque::new();
        let (mut next, mut acked) = (0usize, 0usize);
        let mut paused = false;
        let mut paused_for = Duration::ZERO;
        let mut pause_began: Option<Instant> = None;
        let mut last_poll: Option<Instant> = None;
        let mut status_wait: Option<Instant> = None;
        let mut finished_at: Option<Instant> = None;
        loop {
            self.check_abort()?;
            // Pause / resume.
            let want_pause = self.control.pause_requested();
            if want_pause && !paused {
                self.write_rt(FEED_HOLD)?;
                paused = true;
                pause_began = Some(Instant::now());
                self.control.set_paused(true);
                tracing::info!("GRBL run paused (feed hold)");
            } else if !want_pause && paused {
                self.write_rt(CYCLE_START)?;
                paused = false;
                if let Some(b) = pause_began.take() {
                    paused_for += b.elapsed();
                }
                self.control.set_paused(false);
                tracing::info!("GRBL run resumed (cycle start)");
            }
            // Fill GRBL's buffer.
            if !paused && next < lines.len() {
                let len = lines[next].len() + 1;
                if self.inflight_bytes() + len <= RX_BUFFER {
                    self.write_line(&lines[next])?;
                    sent_index.push_back(next);
                    next += 1;
                    continue;
                }
            }
            // Status reports: one outstanding at a time.
            let due = last_poll.is_none_or(|t| t.elapsed() >= self.poll.status_interval);
            if status_wait.is_none() && due {
                self.write_rt(STATUS)?;
                status_wait = Some(Instant::now());
                last_poll = Some(Instant::now());
            }
            let mut idle_seen = false;
            for r in self.read_responses(self.poll.read_timeout)? {
                match r {
                    Response::Ok => {
                        sent_index.pop_front();
                        acked += 1;
                        self.control.set_sent(acked);
                    }
                    Response::Error(code) => {
                        let i = sent_index.pop_front().unwrap_or(0);
                        return Err(GrblError::Rejected { code, line: lines.get(i).cloned().unwrap_or_default() }.into());
                    }
                    Response::Alarm(code) => return Err(GrblError::Alarm(code).into()),
                    Response::Welcome(_) => return Err(GrblError::Reset.into()),
                    Response::Status(s) => {
                        status_wait = None;
                        if s.door_open() {
                            return Err(GrblError::DoorOpen.into());
                        }
                        idle_seen = s.state == GrblState::Idle;
                    }
                    Response::Other(_) => {}
                }
            }
            if let Some(t) = status_wait
                && t.elapsed() >= self.poll.response_timeout
                && !paused
            {
                return Err(GrblError::NoResponse(self.poll.response_timeout).into());
            }
            if next == lines.len() && acked == lines.len() {
                let since = *finished_at.get_or_insert_with(Instant::now);
                if idle_seen && !paused {
                    return Ok(());
                }
                if let Some(limit) = self.poll.finish_timeout {
                    let held = pause_began.map_or(Duration::ZERO, |b| b.elapsed());
                    if since.elapsed().saturating_sub(paused_for).saturating_sub(held) >= limit {
                        return Err(ControllerError::NotFinished(limit));
                    }
                }
            }
        }
    }

    /// Stop, always callable (no init needed): feed hold `!` and soft reset `0x18` at once, then wait (bounded) for
    /// the board's start-up message. GRBL's buffers are cleared and the laser is off after the reset. Clears any
    /// pending stop request and pause. Every step is tried; the first error is returned.
    pub fn abort(&mut self) -> Result<()> {
        let hold = self.port.write(&[FEED_HOLD]);
        let reset = self.port.write(&[SOFT_RESET]);
        self.inflight.clear();
        self.rx.clear();
        // Unknown until the board's start-up message confirms the reset (`read_responses` sets it then): a reset byte
        // that never arrived leaves the parser state (`M3`/`M4`, `S`) as it was.
        self.spindle_off = false;
        let welcome = if reset.is_ok() { self.wait_welcome(self.poll.reset_timeout).map(|_| ()) } else { Ok(()) };
        self.abort.clear();
        self.control.clear_pause();
        self.control.set_paused(false);
        self.state = if self.profile.is_some() { ControllerState::Idle } else { ControllerState::Uninitialized };
        hold.map_err(ControllerError::from).and(reset.map_err(ControllerError::from)).and(welcome)
    }

    fn abort_after(&mut self, cause: &ControllerError) {
        tracing::warn!(%cause, "GRBL: stopping (feed hold, soft reset)");
        if let Err(e) = self.abort() {
            tracing::error!(error = %e, "GRBL stop sequence failed");
        }
    }

    // ----- homing, jogging, console -----

    /// Run the homing cycle (`$H`). Moves, never fires. Needs homing in the profile and on the board (`$22=1`); works
    /// from the alarm state (that is what it is for). A stop request ends the wait with a soft reset.
    pub fn home(&mut self) -> Result<()> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if !profile.homing {
            return Err(ControllerError::Refused("homing is off in this device's profile".into()));
        }
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if !self.unsafe_startup.is_empty() {
            return Err(GrblError::Settings("start-up blocks can turn the laser on; clear them first".into()).into());
        }
        self.require_spindle_off()?;
        match self.command_ok("$H", self.poll.homing_timeout) {
            Ok(_) => {
                self.alarm = None;
                Ok(())
            }
            Err(ControllerError::Aborted) => {
                self.abort_after(&ControllerError::Aborted);
                Err(ControllerError::Aborted)
            }
            Err(e) => Err(e),
        }
    }

    /// Unlock after an alarm (`$X`). Moves nothing; the position may be wrong until homed.
    pub fn unlock(&mut self) -> Result<()> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        self.command_ok("$X", self.poll.response_timeout)?;
        self.alarm = None;
        Ok(())
    }

    /// Jog by `(dx, dy)` mm in the field frame's directions (+X right, +Y away from the operator) at `feed` mm/min
    /// (default the frame feed): `$J=G91 G21 ...`. The laser stays off. Each step at most the bed's size.
    pub fn jog(&mut self, dx: f64, dy: f64, feed: Option<f64>) -> Result<()> {
        let profile = self.ready_to_move(false)?;
        let [w, h] = profile.work_area_mm;
        if !(dx.is_finite() && dy.is_finite() && dx.abs() <= w && dy.abs() <= h) || (dx == 0.0 && dy == 0.0) {
            return Err(ControllerError::Refused(format!("a jog step must be non-zero and at most the bed ({w} x {h} mm)")));
        }
        let feed = self.jog_feed(&profile, feed)?;
        self.require_spindle_off()?;
        let (mx, my) = profile.mirror();
        let (dx, dy) = (if mx { -dx } else { dx }, if my { -dy } else { dy });
        let line = format!("$J=G91G21X{}Y{}F{}", num(dx), num(dy), num(feed));
        self.command_ok(&line, self.poll.response_timeout).map(|_| ())
    }

    /// Jog to `(x, y)` in the field frame (refused outside the bed): `$J=G90 G21 ...`.
    pub fn jog_to(&mut self, x: f64, y: f64, feed: Option<f64>) -> Result<()> {
        let profile = self.ready_to_move(false)?;
        let (mx, my) = profile.to_machine(x, y).map_err(ControllerError::Refused)?;
        let feed = self.jog_feed(&profile, feed)?;
        self.require_spindle_off()?;
        let line = format!("$J=G90G21X{}Y{}F{}", num(mx), num(my), num(feed));
        self.command_ok(&line, self.poll.response_timeout).map(|_| ())
    }

    fn jog_feed(&self, profile: &GrblProfile, feed: Option<f64>) -> Result<f64> {
        let feed = feed.unwrap_or(profile.frame_feed_mm_min);
        if !(feed.is_finite() && feed > 0.0 && feed <= profile.max_feed_mm_min + 1e-9) {
            return Err(ControllerError::Refused(format!("jog feed must be within 0..={} mm/min", profile.max_feed_mm_min)));
        }
        Ok(feed)
    }

    /// Cancel a jog (`0x85`).
    pub fn jog_cancel(&mut self) -> Result<()> {
        self.write_rt(JOG_CANCEL)
    }

    /// Send one console line (see [`check_console_line`] for what is accepted) and return the board's answer.
    /// Refused while a run streams or a frame is on. A line that moves is sent only while the board's parser state
    /// says the spindle is off (`$G` is read again when that is not known).
    pub fn console(&mut self, line: &str) -> Result<ConsoleReply> {
        if self.state == ControllerState::Uninitialized {
            return Err(ControllerError::NotInitialized);
        }
        if matches!(self.state, ControllerState::Running | ControllerState::Framing) {
            return Err(ControllerError::Busy(self.state));
        }
        let (line, kind) = check_console_line(line).map_err(GrblError::Console)?;
        let moves = matches!(kind, ConsoleKind::Gcode { moves: true } | ConsoleKind::Jog | ConsoleKind::Home);
        if moves && !self.unsafe_startup.is_empty() {
            return Err(GrblError::Console("start-up blocks can turn the laser on; clear them ($N0= / $N1=) before moving".into()).into());
        }
        if moves {
            self.require_spindle_off().map_err(|e| GrblError::Console(e.to_string()))?;
        }
        let timeout = if kind == ConsoleKind::Home { self.poll.homing_timeout } else { self.poll.response_timeout };
        let reply = match self.command(&line, timeout) {
            Ok(r) => r,
            Err(ControllerError::Grbl(GrblError::Alarm(code))) => {
                return Ok(ConsoleReply { line, lines: Vec::new(), ok: false, error: Some(GrblError::Alarm(code).to_string()) });
            }
            Err(ControllerError::Aborted) => {
                self.abort_after(&ControllerError::Aborted);
                return Err(ControllerError::Aborted);
            }
            Err(e) => return Err(e),
        };
        if reply.result.is_ok() {
            match kind {
                ConsoleKind::Unlock | ConsoleKind::Home => self.alarm = None,
                ConsoleKind::System if line.to_ascii_uppercase().starts_with("$N") && line.contains('=') => {
                    self.read_startup()?;
                }
                _ => {}
            }
        }
        let error = reply.result.err().map(|code| GrblError::Rejected { code, line: line.clone() }.to_string());
        Ok(ConsoleReply { line, lines: reply.lines, ok: error.is_none(), error })
    }
}

/// Held by [`GrblController::run_program`] while it streams. Unless the run finished normally, dropping it (an
/// early return, or a panic unwinding through the run) sends the stop sequence.
struct RunGuard<'a, P: SerialLink> {
    ctl: &'a mut GrblController<P>,
    done: bool,
}

impl<P: SerialLink> Drop for RunGuard<'_, P> {
    fn drop(&mut self) {
        if !self.done {
            let cause = if std::thread::panicking() { "panic during run" } else { "run left early" };
            tracing::error!(cause, "GRBL: stopping");
            if let Err(e) = self.ctl.abort() {
                tracing::error!(error = %e, "GRBL stop sequence failed");
            }
        }
    }
}

/// A number for G-code: up to 3 decimals, no trailing zeros, no `-0`.
pub(crate) fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}
