//! [`Controller`]: init, correction upload, red-light framing, armed runs and abort over any [`Link`].
//!
//! Safety rules this module enforces:
//!
//! - [`Controller::frame`] builds its packets only with [`lmc::LightList`] and re-checks them with
//!   [`lmc::emits_light_only`] before sending. Its single commands never open the MO or set the laser-enable bit.
//! - [`Controller::run`] is the only path that opens the MO or sets the laser-enable bit. It needs an [`Armed`]
//!   token (see its docs) and checks the packets against the profile's hard limits ([`check_job`]) first.
//! - Any error during a run or frame triggers [`Controller::abort`]. `abort` needs no init, tries every step even
//!   when one fails, and can be requested from another thread with an [`AbortHandle`].

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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use corfile::CorTable;
use lmc::{CENTER, LightList, Packet, Single, StatusWord, units};
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

    fn clear(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Drives one board through a [`Link`]. See the module docs for the safety rules.
#[derive(Debug)]
pub struct Controller<L: Link> {
    link: L,
    settings: InitSettings,
    poll: PollConfig,
    profile: Option<MachineProfile>,
    state: ControllerState,
    abort: AbortHandle,
}

impl<L: Link> Controller<L> {
    /// A controller with galvoplotter's [`InitSettings`] and the default [`PollConfig`].
    pub fn new(link: L) -> Self {
        Self::with_settings(link, InitSettings::default(), PollConfig::default())
    }

    pub fn with_settings(link: L, settings: InitSettings, poll: PollConfig) -> Self {
        Controller { link, settings, poll, profile: None, state: ControllerState::Uninitialized, abort: AbortHandle::default() }
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    pub fn link_mut(&mut self) -> &mut L {
        &mut self.link
    }

    pub fn into_link(self) -> L {
        self.link
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

    /// Connect-time setup, in galvoplotter's `init_laser` order: `GetSerialNo`, `GetVersion`, `Reset`, the correction
    /// upload (`WriteCorTable(1)` + one `WriteCorLine` per node, or `WriteCorTable(0)` without a table), then
    /// [`InitSettings::configuration`] (which closes the MO). Nothing here can emit.
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
        self.single(Single::reset())?;
        for cmd in correction_upload(cor) {
            self.single(cmd)?;
        }
        for cmd in self.settings.configuration() {
            self.single(cmd)?;
        }
        self.profile = Some(profile.clone());
        self.state = ControllerState::Idle;
        Ok(BoardInfo { serial, version })
    }

    /// Poll the board status (`GetVersion`, word 3). A finished frame moves the state back to idle.
    pub fn status(&mut self) -> Result<StatusWord> {
        let status = self.poll_status()?;
        if self.state == ControllerState::Framing && status.idle() {
            self.state = ControllerState::Idle;
        }
        Ok(status)
    }

    /// Trace `outline` (polylines in field mm) with the red pointer `loops` times, using only
    /// [`lmc::LightList`]. Each polyline is visited in order; the pointer is switched off while travelling between
    /// polylines. A running frame is stopped first. Returns the number of packets sent.
    ///
    /// Every point is converted with [`field_to_galvo`], so anything outside the field is refused before anything
    /// is sent.
    pub fn frame(&mut self, outline: &[Vec<(f64, f64)>], loops: u32) -> Result<usize> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        let packets = light_packets(&profile, outline, loops)?;
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

    /// Stop a running frame: `StopExecute`, `ResetList`, `WritePort(0)` (pointer off). Every step is tried.
    pub fn stop_frame(&mut self) -> Result<()> {
        let result = self.send_all(&[Single::stop_execute(), Single::reset_list(), Single::write_port(0)]);
        if self.state == ControllerState::Framing {
            self.state = ControllerState::Idle;
        }
        result
    }

    /// Run a job's packets. Requires an [`Armed`] token, which only the engine's UI approval creates.
    ///
    /// Order: [`check_job`] against the profile (nothing is sent if it fails); a running frame is stopped; prelude
    /// `ResetList`, `WritePort(laser enable)`, `Fiber_SetMo(1)`; the packets with READY flow control, `SetEndOfList`
    /// after each and `ExecuteList` once more than two are queued (or after the last); wait until idle; postlude
    /// `Fiber_SetMo(0)`, `WritePort(0)`, `GetMarkTime(3)`. Any error or abort request runs [`Controller::abort`].
    pub fn run(&mut self, packets: &[Packet], armed: Armed) -> Result<RunReport> {
        let profile = self.profile.clone().ok_or(ControllerError::NotInitialized)?;
        if self.state == ControllerState::Running {
            return Err(ControllerError::Busy(self.state));
        }
        check_job(&profile, packets)?;
        if self.state == ControllerState::Framing {
            self.stop_frame()?;
        }
        if self.abort.is_requested() {
            self.abort_after(&ControllerError::Aborted);
            return Err(ControllerError::Aborted);
        }
        tracing::info!(run_id = armed.run_id(), packets = packets.len(), "starting armed run");
        self.state = ControllerState::Running;
        match self.run_inner(&profile, packets) {
            Ok(mark_time) => {
                self.state = ControllerState::Idle;
                Ok(RunReport { run_id: armed.run_id().to_owned(), packets: packets.len(), mark_time })
            }
            Err(e) => {
                self.abort_after(&e);
                Err(e)
            }
        }
    }

    fn run_inner(&mut self, profile: &MachineProfile, packets: &[Packet]) -> Result<Option<Reply>> {
        self.single(Single::reset_list())?;
        self.single(Single::write_port(profile.laser_enable_mask()))?;
        self.single(Single::fiber_set_mo(true))?;
        self.stream(packets)?;
        self.wait_idle()?;
        self.single(Single::fiber_set_mo(false))?;
        self.single(Single::write_port(0))?;
        self.single(Single::get_mark_time())
    }

    /// Emergency stop, always callable (no init needed): `StopExecute`, `Fiber_SetMo(0)`, `ResetList`,
    /// `WritePort(0)` (laser enable and pointer off), `GotoXY(centre)`. Every step is tried even if an earlier one
    /// fails; the first error is returned. Clears any pending [`AbortHandle`] request.
    pub fn abort(&mut self) -> Result<()> {
        let result = self.send_all(&[
            Single::stop_execute(),
            Single::fiber_set_mo(false),
            Single::reset_list(),
            Single::write_port(0),
            Single::goto_xy(CENTER, CENTER, 0),
        ]);
        self.abort.clear();
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

    fn pause(&self) {
        if !self.poll.interval.is_zero() {
            std::thread::sleep(self.poll.interval);
        }
    }

    fn wait_ready(&mut self) -> Result<()> {
        let start = Instant::now();
        loop {
            self.check_abort()?;
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
        let start = Instant::now();
        loop {
            self.check_abort()?;
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

/// The frame list for `outline`, `loops` times, as packets. See [`Controller::frame`].
fn light_packets(profile: &MachineProfile, outline: &[Vec<(f64, f64)>], loops: u32) -> Result<Vec<Packet>> {
    if !(1..=MAX_FRAME_LOOPS).contains(&loops) {
        return Err(ControllerError::Refused(format!("loops must be 1..={MAX_FRAME_LOOPS}, got {loops}")));
    }
    let mut polys: Vec<Vec<(u16, u16)>> = Vec::new();
    for poly in outline.iter().filter(|p| !p.is_empty()) {
        polys.push(poly.iter().map(|&(x, y)| field_to_galvo(profile, x, y)).collect::<Result<_, _>>()?);
    }
    let points: usize = polys.iter().map(|p| p.len() + 2).sum();
    if points == 0 {
        return Err(ControllerError::Refused("nothing to frame: the outline is empty".into()));
    }
    let budget = MAX_FRAME_PACKETS * lmc::SLOTS;
    if points.saturating_mul(loops as usize) > budget.saturating_sub(8) {
        return Err(ControllerError::Refused(format!("frame too large: {points} points x {loops} loops")));
    }
    let mut list = LightList::new();
    list.jump_speed(units::speed_to_units(profile.red_light_speed_mm_s, profile.field_mm)?)?;
    list.jump_delay(units::delay_word(profile.timing.jump_delay_max_us)?);
    let mut lit = false;
    for _ in 0..loops {
        for poly in &polys {
            let (&(x0, y0), rest) = poly.split_first().ok_or_else(|| ControllerError::Refused("empty polyline".into()))?;
            if lit && polys.len() > 1 {
                list.red_pointer_off();
                lit = false;
            }
            list.jump_to(x0, y0);
            if !lit {
                list.red_pointer_on();
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
        assert_eq!(singles.len(), 3 + 1 + corfile::NODE_COUNT + config.len());
        assert_eq!(opcodes(&singles[..4]), [op::GET_SERIAL_NO, op::GET_VERSION, op::RESET, op::WRITE_COR_TABLE]);
        assert_eq!(singles[3], Single::write_cor_table(true));
        let lines = &singles[4..4 + corfile::NODE_COUNT];
        assert!(lines.iter().all(|s| s.opcode() == op::WRITE_COR_LINE));
        assert_eq!(lines[0], Single::write_cor_line(0x89D8, 0x8159, true));
        assert!(lines[1..].iter().all(|s| s.0[3] == 1));
        assert_eq!(singles.iter().filter(|s| s.opcode() == op::WRITE_COR_LINE).count(), 4225);
        assert_eq!(&singles[4 + corfile::NODE_COUNT..], config.as_slice());
        assert!(!singles_can_emit(&singles));
        assert!(!link.mo_open());
    }

    #[test]
    fn init_without_cor_writes_blank_table() {
        let mut c = controller(SimBoard::default());
        c.init(&b6(), None).unwrap();
        let singles = c.link().singles();
        assert_eq!(singles[3], Single::write_cor_table(false));
        assert_eq!(singles[4], Single::enable_laser());
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
        let singles = link.singles_without_polls();
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
}
