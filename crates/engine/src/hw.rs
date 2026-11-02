//! The engine's side of the laser: [`Machine`] owns the machine driver (`machine::Driver`: the galvo's LMC
//! `Controller`, or a GRBL gantry's `GrblController`) behind its own mutex and drives it from blocking tasks, so a
//! slow USB or serial transfer never stalls the async runtime. Also the links the engine can use
//! ([`CountingSimLink`], [`OfflineLink`], the GRBL simulator) and the profile -> `plan::Target` mapping.
//!
//! The engine holds no link of its own: the driver owns it, and the engine reaches the board only through the
//! driver interface (`machine::MachineDriver`) and the driver's own methods. Swapping in a freshly opened USB link
//! uses `Controller::replace_link` ([`Machine::ensure_ready`]) and the physical run confirmation reads inputs with
//! `Driver::read_port` ([`Machine::read_inputs`]).
//!
//! The profile, correction table, GRBL profile and planning target form one [`MachineConfig`] that a device change
//! swaps as a whole ([`Machine::swap_config`] for a galvo, [`Machine::swap_grbl`] for a GRBL gantry, under the
//! controller lock, re-initialising the board and changing the driver when the kind changes); its `generation`
//! tells runs planned for an earlier device apart. The link kind (simulator or real hardware) never changes: a GRBL
//! device on the simulator runs on `machine::grbl::SimGrbl`, on real hardware on the profile's serial port.

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, TryLockError};

use corfile::CorTable;
use lmc::{LASER_ENABLE_BIT, Packet, Single};
use machine::grbl::{
    GrblController, GrblPoll, GrblProfile, GrblSnapshot, OfflineSerial, SerialLink, SimGrbl, SimGrblConfig, SimGrblHandle, SimGrblReport,
};
use machine::{
    AbortHandle, AxisRole, BoardInfo, Controller, ControllerError, ControllerState, Driver, DriverKind, FiberKind, InitSettings,
    InputSamples, Link, LinkError, MachineDriver, MachineProfile, PollConfig, Reply, RunControl, RunProgress, SimLink,
};
use plan::{GcodeSettings, Target};
use serde_json::{Value, json};

use crate::EngineError;

/// The driver the engine drives: the galvo's controller or a GRBL controller, on any link, boxed.
pub type Ctl = Driver;


/// Which board the engine talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// [`CountingSimLink`]: nothing leaves the process.
    Simulated,
    /// The real board over USB (feature `usb`).
    Usb,
}

/// The planning target for a machine profile. Hard limits come only from the profile; so do the open-MO delay,
/// the lens correction, whether pulse widths are sent (Q-pulse enabled on a MOPA) and which extension axis the board
/// drives.
pub fn target_from_profile(p: &MachineProfile) -> Target {
    Target {
        field_mm: p.field_mm,
        field_height_mm: p.field_height_mm,
        curve_tolerance_mm: p.curve_tolerance_mm,
        swap_xy: p.swap_xy,
        negate_x: p.negate_x,
        negate_y: p.negate_y,
        max_power_pct: p.max_power_pct,
        pulse_widths_ns: p.pulse_widths_ns.clone(),
        timing: doc::GalvoTiming {
            laser_on_delay_us: p.timing.laser_on_delay_us,
            laser_off_delay_us: p.timing.laser_off_delay_us,
            polygon_delay_us: p.timing.polygon_delay_us,
            end_delay_us: p.timing.end_delay_us,
            jump_speed_mm_s: p.timing.jump_speed_mm_s,
            min_jump_delay_us: p.timing.jump_delay_min_us,
            max_jump_delay_us: p.timing.jump_delay_max_us,
            jump_distance_limit_mm: p.timing.jump_distance_limit_mm,
        },
        open_mo_delay_us: p.open_mo_delay_us,
        motion_time_scale: p.motion_time_scale,
        calibration: p.calibration,
        pulse_width_enabled: p.pulse_width_enabled && p.fiber_kind == FiberKind::Mopa,
        z_axis: p.axis.as_ref().is_some_and(|a| a.role == AxisRole::Z),
        rotary_axis: p.axis.as_ref().is_some_and(|a| a.role == AxisRole::Rotary),
        port_outputs: p.io.run_outputs(),
        gantry: false,
    }
}

/// G-code output settings for a GRBL profile.
pub fn gcode_settings(g: &GrblProfile) -> GcodeSettings {
    let (mirror_x, mirror_y) = g.mirror();
    GcodeSettings {
        work_area_mm: g.work_area_mm,
        mirror_x,
        mirror_y,
        s_max: g.s_max,
        max_power_pct: g.max_power_pct,
        max_feed_mm_min: g.max_feed_mm_min,
        dynamic_power: g.dynamic_power,
    }
}

/// The planning target for a GRBL profile: the bed, its power limit, no galvo transform.
pub fn target_from_grbl(g: &GrblProfile) -> Target {
    gcode_settings(g).target()
}

/// What the simulated board has been sent, for the status page and for tests that prove nothing fired.
#[derive(Debug, Default)]
pub struct SimCounters {
    singles: AtomicU64,
    packets: AtomicU64,
    /// Singles that open the MO or set the laser-enable bit.
    laser_singles: AtomicU64,
    /// Packets that are not light-only (they can fire).
    emitting_packets: AtomicU64,
    /// The simulated input port: word 1 of every `ReadPort` reply.
    inputs: AtomicU16,
    /// Every byte sent to the board (singles' 12 bytes, packets' 3072) while recording ([`SimCounters::record`]):
    /// the wire, for tests that compare it byte for byte.
    wire: Mutex<Option<Vec<u8>>>,
    /// The simulated GRBL board of the current GRBL device (none while a galvo is active).
    grbl: Mutex<Option<SimGrblHandle>>,
}

impl SimCounters {
    pub fn singles(&self) -> u64 {
        self.singles.load(Ordering::SeqCst)
    }

    pub fn packets(&self) -> u64 {
        self.packets.load(Ordering::SeqCst)
    }

    /// Single commands that open the MO or set the laser-enable port bit.
    pub fn laser_singles(&self) -> u64 {
        self.laser_singles.load(Ordering::SeqCst)
    }

    /// List packets that fail `lmc::emits_light_only`.
    pub fn emitting_packets(&self) -> u64 {
        self.emitting_packets.load(Ordering::SeqCst)
    }

    /// Set the simulated input port (e.g. press a foot switch on bit 14).
    pub fn set_inputs(&self, bits: u16) {
        self.inputs.store(bits, Ordering::SeqCst);
    }

    pub fn inputs(&self) -> u16 {
        self.inputs.load(Ordering::SeqCst)
    }

    /// Start recording the wire (dropping anything recorded before).
    pub fn record(&self) {
        *self.wire.lock().unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
    }

    /// Stop recording and return the bytes sent since [`SimCounters::record`] (empty when not recording).
    pub fn take_wire(&self) -> Vec<u8> {
        self.wire.lock().unwrap_or_else(PoisonError::into_inner).take().unwrap_or_default()
    }

    fn wire(&self, bytes: &[u8]) {
        if let Some(w) = self.wire.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
            w.extend_from_slice(bytes);
        }
    }

    /// The simulated GRBL board: what it received and whether the laser would have been on.
    pub fn grbl_report(&self) -> Option<SimGrblReport> {
        self.grbl.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(SimGrblHandle::report)
    }

    /// The simulated GRBL board, to open its door or change its settings in tests.
    pub fn grbl_handle(&self) -> Option<SimGrblHandle> {
        self.grbl.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn json(&self) -> Value {
        let mut v = json!({
            "singles": self.singles(),
            "packets": self.packets(),
            "laser_singles": self.laser_singles(),
            "emitting_packets": self.emitting_packets(),
        });
        if let Some(r) = self.grbl_report() {
            v["grbl"] = json!({
                "lines": r.lines_received,
                "fires": r.fires.len(),
                "blocks_executed": r.blocks_executed,
                "max_rx_bytes": r.max_rx_bytes,
                "overflowed": r.overflowed,
                "state": r.state,
            });
        }
        v
    }
}

/// A [`SimLink`] that counts what it is sent and forgets the traffic log after every transfer, so a long-running
/// engine (e.g. continuous framing) does not grow without bound.
pub struct CountingSimLink {
    inner: SimLink,
    counters: Arc<SimCounters>,
}

impl CountingSimLink {
    pub fn new(counters: Arc<SimCounters>) -> Self {
        CountingSimLink { inner: SimLink::new(), counters }
    }
}

/// True when a single command opens the MO or sets the laser-enable bit.
pub fn single_can_fire(cmd: Single) -> bool {
    cmd == Single::fiber_set_mo(true)
        || (cmd.opcode() == Single::write_port(0).opcode() && cmd.0[1] & LASER_ENABLE_BIT != 0)
        || cmd == Single::enable_laser()
}

impl Link for CountingSimLink {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        self.counters.singles.fetch_add(1, Ordering::SeqCst);
        self.counters.wire(&cmd.to_bytes());
        // EnableLaser is part of init and does not fire on its own; count only MO/port changes.
        if cmd != Single::enable_laser() && single_can_fire(cmd) {
            self.counters.laser_singles.fetch_add(1, Ordering::SeqCst);
        }
        let mut reply = self.inner.send_single(cmd);
        self.inner.clear_traffic();
        if cmd == Single::read_port()
            && let Ok(Some(words)) = &mut reply
        {
            words[1] = self.counters.inputs();
        }
        reply
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        self.counters.packets.fetch_add(1, Ordering::SeqCst);
        self.counters.wire(packet.as_bytes());
        if !lmc::emits_light_only(std::slice::from_ref(packet)) {
            self.counters.emitting_packets.fetch_add(1, Ordering::SeqCst);
        }
        let r = self.inner.send_packet(packet);
        self.inner.clear_traffic();
        r
    }
}

/// A link with no board behind it: every transfer fails with [`LinkError::Disconnected`]. Used until a USB board
/// is opened.
#[derive(Debug, Default)]
pub struct OfflineLink;

impl Link for OfflineLink {
    fn send_single(&mut self, _cmd: Single) -> Result<Option<Reply>, LinkError> {
        Err(LinkError::Disconnected)
    }

    fn send_packet(&mut self, _packet: &Packet) -> Result<(), LinkError> {
        Err(LinkError::Disconnected)
    }
}

/// What [`Machine::abort`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortOutcome {
    /// The abort sequence went to the board.
    Sent,
    /// No board is connected (USB not opened or not initialised), so there was nothing to send.
    NotConnected,
}

/// The profile the machine is driven with, its correction table and planning target, swapped as one by a device
/// change.
#[derive(Debug)]
pub struct MachineConfig {
    pub profile: MachineProfile,
    pub cor: Option<CorTable>,
    pub target: Target,
    /// Bumped by every swap; a run planned with another generation is never approved.
    pub generation: u64,
    /// The GRBL profile when a GRBL gantry is active (the driver is then GRBL and `target` is its bed); `None` for a
    /// galvo. `profile` and `cor` keep the last galvo's, unused while a GRBL device is active.
    pub grbl: Option<GrblProfile>,
}

/// The laser as the engine sees it. See the module docs.
pub struct Machine {
    ctl: Arc<Mutex<Ctl>>,
    abort: AbortHandle,
    /// Pause, resume and progress of the controller's runs (usable while a run holds the controller).
    control: RunControl,
    /// Input port reads taken while a frame streams (the foot switch watcher's reads in between).
    samples: InputSamples,
    /// The current [`MachineConfig`]; replaced only with the controller lock held.
    config: RwLock<Arc<MachineConfig>>,
    kind: LinkKind,
    sim: Option<Arc<SimCounters>>,
    /// The galvo controller's and the GRBL controller's polling (kept for driver changes).
    poll: PollConfig,
    grbl_poll: GrblPoll,
    /// An approved run holds this from approval until it ends.
    running: AtomicBool,
    /// Bumped by every stop. A run captures it when it claims the machine and never starts streaming once it has
    /// changed, so a stop can never be lost between approval and the run taking the controller lock.
    stop_epoch: AtomicU64,
    /// Bumped to end the current frame loop; the loop owns the value it started with.
    frame_gen: AtomicU64,
    /// Set with every frame-generation bump: ends a GRBL frame still being streamed without waiting for the driver.
    frame_cancel: AbortHandle,
    /// The generation of the frame loop that is tracing, or 0. Framing is on while it equals `frame_gen`.
    framing_gen: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Machine {
    /// A machine on the simulated board. `poll` is the controller's status polling ([`PollConfig::immediate`] for
    /// tests).
    pub fn simulated(profile: MachineProfile, cor: Option<CorTable>, poll: PollConfig) -> Self {
        let counters = Arc::new(SimCounters::default());
        let link: Box<dyn Link> = Box::new(CountingSimLink::new(counters.clone()));
        Self::with_link(link, LinkKind::Simulated, Some(counters), profile, cor, poll)
    }

    /// A machine on the USB board. Nothing is opened here; [`Machine::connect`] opens it.
    pub fn usb(profile: MachineProfile, cor: Option<CorTable>) -> Self {
        Self::with_link(Box::new(OfflineLink), LinkKind::Usb, None, profile, cor, PollConfig::default())
    }

    pub(crate) fn with_link(
        link: Box<dyn Link>,
        kind: LinkKind,
        sim: Option<Arc<SimCounters>>,
        profile: MachineProfile,
        cor: Option<CorTable>,
        poll: PollConfig,
    ) -> Self {
        let ctl = Controller::with_settings(link, InitSettings::default(), poll);
        let grbl_poll = if poll.interval.is_zero() { GrblPoll::immediate() } else { GrblPoll::default() };
        Machine {
            abort: ctl.abort_handle(),
            control: ctl.run_control(),
            samples: ctl.input_samples(),
            ctl: Arc::new(Mutex::new(Driver::Lmc(ctl))),
            config: RwLock::new(Arc::new(MachineConfig { target: target_from_profile(&profile), profile, cor, generation: 0, grbl: None })),
            kind,
            sim,
            poll,
            grbl_poll,
            running: AtomicBool::new(false),
            stop_epoch: AtomicU64::new(0),
            frame_gen: AtomicU64::new(0),
            frame_cancel: AbortHandle::default(),
            framing_gen: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    /// The current profile, correction table, target and generation, as one snapshot.
    pub fn config(&self) -> Arc<MachineConfig> {
        self.config.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The current machine profile (a copy).
    pub fn profile(&self) -> MachineProfile {
        self.config().profile.clone()
    }

    /// The planning target derived from the profile (a copy).
    pub fn target(&self) -> Target {
        self.config().target.clone()
    }

    /// The generation of the current [`MachineConfig`].
    pub fn generation(&self) -> u64 {
        self.config().generation
    }

    /// Swap in another galvo profile and correction table (a device change) and re-initialise the board: the
    /// simulator gets a fresh `SimLink` (the counters carry on), a USB board is closed and opened again; a GRBL driver
    /// is replaced by the galvo's controller (sharing the stop and pause handles). Call with the controller lock held
    /// and no run or frame active (the caller has claimed the machine). The swap happens even when the board then
    /// fails to initialise; that error is returned (and the next frame or run retries).
    pub fn swap_config(&self, ctl: &mut Ctl, profile: MachineProfile, cor: Option<CorTable>) -> Result<u64, ControllerError> {
        profile.validate()?;
        let generation = {
            let mut c = self.config.write().unwrap_or_else(PoisonError::into_inner);
            let generation = c.generation + 1;
            *c = Arc::new(MachineConfig { target: target_from_profile(&profile), profile, cor, generation, grbl: None });
            generation
        };
        let link: Box<dyn Link> = match (&self.sim, self.kind) {
            (Some(counters), LinkKind::Simulated) => Box::new(CountingSimLink::new(counters.clone())),
            _ => Box::new(OfflineLink),
        };
        match ctl {
            Driver::Lmc(c) => c.replace_link(link),
            Driver::Grbl(_) => {
                if let Some(counters) = &self.sim {
                    *counters.grbl.lock().unwrap_or_else(PoisonError::into_inner) = None;
                }
                *ctl = Driver::Lmc(Controller::with_handles(
                    link,
                    InitSettings::default(),
                    self.poll,
                    self.abort.clone(),
                    self.control.clone(),
                    self.samples.clone(),
                ));
            }
        }
        self.ensure_ready(ctl).map(|()| generation)
    }

    /// Make a GRBL gantry the machine (a device change): the driver becomes a `GrblController` (sharing the stop and
    /// pause handles) on a fresh simulated GRBL board, or on real hardware on the profile's serial port (opened by
    /// [`Machine::ensure_ready`]). The galvo profile stays in the config, unused. Same rules as
    /// [`Machine::swap_config`].
    pub fn swap_grbl(&self, ctl: &mut Ctl, grbl: GrblProfile) -> Result<u64, ControllerError> {
        grbl.validate()?;
        let generation = {
            let mut c = self.config.write().unwrap_or_else(PoisonError::into_inner);
            let generation = c.generation + 1;
            *c = Arc::new(MachineConfig {
                target: target_from_grbl(&grbl),
                profile: c.profile.clone(),
                cor: c.cor.clone(),
                generation,
                grbl: Some(grbl.clone()),
            });
            generation
        };
        let port: Box<dyn SerialLink> = match (&self.sim, self.kind) {
            (Some(counters), LinkKind::Simulated) => {
                let sim =
                    SimGrbl::with_config(&SimGrblConfig { s_max: grbl.s_max, s_min: grbl.s_min, homing: grbl.homing, ..SimGrblConfig::default() });
                *counters.grbl.lock().unwrap_or_else(PoisonError::into_inner) = Some(sim.handle());
                Box::new(sim)
            }
            _ => Box::new(OfflineSerial),
        };
        let mut g = GrblController::with_handles(port, self.grbl_poll, self.abort.clone(), self.control.clone());
        g.set_frame_cancel(self.frame_cancel.clone());
        *ctl = Driver::Grbl(g);
        self.ensure_ready(ctl).map(|()| generation)
    }

    /// Whether the active machine is a GRBL gantry.
    pub fn is_grbl(&self) -> bool {
        self.config().grbl.is_some()
    }

    /// The GRBL profile of the active machine.
    pub fn grbl_profile(&self) -> Option<GrblProfile> {
        self.config().grbl.clone()
    }

    /// The default frame (pointer or head) speed, mm/s: the galvo's red-light speed, or the GRBL frame feed.
    pub fn frame_speed_mm_s(&self) -> f64 {
        let c = self.config();
        c.grbl.as_ref().map_or(c.profile.red_light_speed_mm_s, |g| g.frame_feed_mm_min / 60.0)
    }

    /// The fastest frame speed allowed, mm/s.
    pub fn max_frame_speed_mm_s(&self) -> f64 {
        let c = self.config();
        c.grbl.as_ref().map_or(c.profile.max_jump_speed_mm_s, |g| g.max_feed_mm_min / 60.0)
    }

    /// The GRBL board as last seen, without waiting for the driver (`None` while a run or frame holds it, or for a
    /// galvo).
    pub fn grbl_snapshot(&self) -> Option<GrblSnapshot> {
        match self.ctl.try_lock() {
            Ok(ctl) => ctl.grbl().map(GrblController::snapshot),
            Err(TryLockError::Poisoned(p)) => p.into_inner().grbl().map(GrblController::snapshot),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    pub fn kind(&self) -> LinkKind {
        self.kind
    }

    /// The simulated board's counters (`None` on USB).
    pub fn sim_counters(&self) -> Option<&Arc<SimCounters>> {
        self.sim.as_ref()
    }

    /// True while an approved run holds the machine.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Claim the machine for a run. False if a run already holds it.
    pub(crate) fn try_begin_run(&self) -> bool {
        self.running.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    }

    pub(crate) fn end_run(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Ask a running job to pause (`StopList` at its next packet or poll). Never waits; harmless without a run.
    pub fn pause(&self) {
        self.control.request_pause();
    }

    /// Ask a paused job to continue (`RestartList`).
    pub fn resume(&self) {
        self.control.request_resume();
    }

    /// Drop a pending pause request (a new job was approved).
    pub fn clear_pause(&self) {
        self.control.clear_pause();
    }

    /// Whether the running job's list is paused.
    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }

    /// Whether a pause is asked for (it may not have reached the board yet).
    pub fn pause_requested(&self) -> bool {
        self.control.pause_requested()
    }

    /// Packets sent of the current (or last) run.
    pub fn progress(&self) -> RunProgress {
        self.control.progress()
    }

    /// The board's identification: read again when the board is idle, else the copy from its init. `None` when no
    /// board has been initialised.
    pub fn board_info(&self, ctl: &mut Ctl) -> Option<BoardInfo> {
        if ctl.state() == ControllerState::Idle
            && let Ok(info) = ctl.refresh_info()
        {
            return Some(info);
        }
        ctl.board_info().cloned()
    }

    /// The stop epoch (see the field docs).
    pub fn stop_epoch(&self) -> u64 {
        self.stop_epoch.load(Ordering::SeqCst)
    }

    /// The synchronous half of a stop: bump the stop epoch, request an abort from whatever holds the controller,
    /// and end framing. Never waits.
    pub fn request_stop(&self) {
        self.stop_epoch.fetch_add(1, Ordering::SeqCst);
        self.abort.request();
        self.bump_frame_gen();
        // A press read before the stop must not act after it (`Controller::abort` clears again under the lock).
        self.samples.clear();
    }

    /// True while a frame loop is tracing (and has not been superseded or stopped).
    pub fn is_framing(&self) -> bool {
        let f = self.framing_gen.load(Ordering::SeqCst);
        f != 0 && f == self.frame_gen.load(Ordering::SeqCst)
    }

    /// Mark frame loop `generation` as tracing (0: none).
    pub(crate) fn set_framing_gen(&self, generation: u64) {
        self.framing_gen.store(generation, Ordering::SeqCst);
    }

    /// The current frame generation.
    pub(crate) fn frame_gen(&self) -> u64 {
        self.frame_gen.load(Ordering::SeqCst)
    }

    /// End any frame loop; returns the new generation (which a new loop then owns).
    pub(crate) fn bump_frame_gen(&self) -> u64 {
        self.frame_cancel.request();
        self.frame_gen.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn set_error(&self, e: Option<String>) {
        *self.last_error.lock().unwrap_or_else(PoisonError::into_inner) = e;
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Run `f` with the controller locked, on a blocking thread. A poisoned lock is recovered: stopping the laser
    /// must keep working after a panic elsewhere.
    pub async fn blocking<R, F>(self: &Arc<Self>, f: F) -> Result<R, EngineError>
    where
        R: Send + 'static,
        F: FnOnce(&Machine, &mut Ctl) -> R + Send + 'static,
    {
        let m = self.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let mut ctl = m.ctl.lock().unwrap_or_else(PoisonError::into_inner);
            f(&m, &mut ctl)
        })
        .await?)
    }

    /// Initialise the board if it is not initialised yet (opening the USB link, or a GRBL board's serial port,
    /// first). Call with the lock held.
    pub fn ensure_ready(&self, ctl: &mut Ctl) -> Result<(), ControllerError> {
        if ctl.state() != ControllerState::Uninitialized {
            return Ok(());
        }
        let config = self.config();
        let ctl = match ctl {
            Driver::Lmc(c) => c,
            Driver::Grbl(g) => {
                let grbl = config.grbl.clone().ok_or_else(|| ControllerError::Refused("a GRBL driver without a GRBL profile".into()))?;
                if self.kind == LinkKind::Usb {
                    g.replace_port(open_serial(&grbl)?);
                }
                return match g.init(&grbl) {
                    Ok(info) => {
                        tracing::info!(version = %info.version, laser_mode = info.laser_mode, state = ?info.state, "GRBL board initialised");
                        self.set_error(None);
                        Ok(())
                    }
                    Err(e) => {
                        if self.kind == LinkKind::Usb {
                            g.replace_port(Box::new(OfflineSerial));
                        }
                        self.set_error(Some(e.to_string()));
                        Err(e)
                    }
                };
            }
        };
        if self.kind == LinkKind::Usb {
            ctl.replace_link(open_usb()?);
        }
        match ctl.init(&config.profile, config.cor.as_ref()) {
            Ok(info) => {
                tracing::info!(serial = ?info.serial, version = ?info.version, "board initialised");
                self.set_error(None);
                Ok(())
            }
            Err(e) => {
                if self.kind == LinkKind::Usb {
                    ctl.replace_link(Box::new(OfflineLink));
                }
                self.set_error(Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// Connect and initialise (see [`Machine::ensure_ready`]).
    pub async fn connect(self: &Arc<Self>) -> Result<(), EngineError> {
        self.blocking(|m, ctl| m.ensure_ready(ctl)).await??;
        Ok(())
    }

    /// Read the board's input port (`ReadPort`, reply word 1, as galvoplotter's `wait_for_input` reads it). Takes
    /// the locked controller so it can never interleave with a list being streamed. Sends nothing that can emit. A
    /// GRBL board has no input port (refused).
    pub fn read_inputs(&self, ctl: &mut Ctl) -> Result<u16, ControllerError> {
        ctl.read_port()
    }

    /// The input port reads taken while frames streamed since the last call, oldest first
    /// ([`machine::InputSamples`]).
    pub(crate) fn take_input_samples(&self) -> Vec<(std::time::Instant, u16)> {
        self.samples.take()
    }

    pub(crate) fn clear_input_samples(&self) {
        self.samples.clear();
    }

    #[cfg(test)]
    pub(crate) fn has_input_samples(&self) -> bool {
        !self.samples.is_empty()
    }

    /// [`Machine::request_stop`] (noticed by a running job at its next packet or poll), then send the abort sequence
    /// once the controller is free. Never gated. With no board connected there is nothing to send:
    /// [`AbortOutcome::NotConnected`].
    pub async fn abort(self: &Arc<Self>) -> Result<AbortOutcome, EngineError> {
        self.request_stop();
        let outcome = self
            .blocking(|_, ctl| {
                let connected = ctl.state() != ControllerState::Uninitialized;
                match ctl.abort() {
                    Ok(()) => Ok(AbortOutcome::Sent),
                    Err(ControllerError::Link(LinkError::Disconnected | LinkError::NotFound(_))) if !connected => {
                        Ok(AbortOutcome::NotConnected)
                    }
                    Err(e) => Err(e),
                }
            })
            .await??;
        Ok(outcome)
    }

    /// Status without waiting for the controller: a held lock reads as busy.
    pub fn status_json(&self) -> Value {
        let running = self.is_running();
        let (state, initialised) = match self.ctl.try_lock() {
            Ok(ctl) => (state_name(ctl.state()), ctl.state() != ControllerState::Uninitialized),
            Err(TryLockError::Poisoned(p)) => {
                let ctl = p.into_inner();
                (state_name(ctl.state()), ctl.state() != ControllerState::Uninitialized)
            }
            Err(TryLockError::WouldBlock) => (if running { "running" } else { "busy" }, true),
        };
        let framing = self.is_framing();
        let simulated = self.kind == LinkKind::Simulated;
        let config = self.config();
        let driver = if config.grbl.is_some() { DriverKind::Grbl } else { DriverKind::Lmc };
        let link = match (simulated, driver) {
            (true, _) => "sim",
            (false, DriverKind::Lmc) => "usb",
            (false, DriverKind::Grbl) => "serial",
        };
        let mut v = json!({
            "connected": initialised,
            "simulated": simulated,
            "sim": simulated,
            "driver": driver.name(),
            "link": link,
            "busy": running || framing || state == "busy" || state == "running" || state == "framing",
            "state": state,
            "running": running,
            "framing": framing,
            "field_mm": config.profile.field_mm,
            "field_height_mm": config.profile.field_height_mm.unwrap_or(config.profile.field_mm),
            "profile": config.profile.name,
            "laser_source": config.profile.laser_source,
            "max_power_pct": config.profile.max_power_pct,
            "last_error": self.last_error(),
            "sim_counters": self.sim.as_ref().map(|c| c.json()),
        });
        if let Some(g) = &config.grbl {
            v["work_area_mm"] = json!(g.work_area_mm);
            v["field_mm"] = json!(g.work_area_mm[0]);
            v["field_height_mm"] = json!(g.work_area_mm[1]);
            v["max_power_pct"] = json!(g.max_power_pct);
            v["grbl"] = json!({ "profile": g, "board": self.grbl_snapshot() });
        }
        v
    }
}

fn state_name(s: ControllerState) -> &'static str {
    match s {
        ControllerState::Uninitialized => "uninitialized",
        ControllerState::Idle => "idle",
        ControllerState::Framing => "framing",
        ControllerState::Running => "running",
    }
}

#[cfg(feature = "usb")]
fn open_usb() -> Result<Box<dyn Link>, ControllerError> {
    let link = machine::UsbLink::open(&machine::UsbOptions::default())?;
    Ok(Box::new(link))
}

#[cfg(not(feature = "usb"))]
fn open_usb() -> Result<Box<dyn Link>, ControllerError> {
    Err(LinkError::Unsupported("this build has no USB support (build with --features usb)".into()).into())
}

#[cfg(feature = "serial")]
fn open_serial(g: &GrblProfile) -> Result<Box<dyn SerialLink>, ControllerError> {
    Ok(Box::new(machine::grbl::SerialPortLink::open(&g.port, g.baud)?))
}

#[cfg(not(feature = "serial"))]
fn open_serial(_g: &GrblProfile) -> Result<Box<dyn SerialLink>, ControllerError> {
    Err(LinkError::Unsupported("this build has no serial port support (build with --features serial)".into()).into())
}

/// The serial ports of this computer (empty without the `serial` feature).
pub fn serial_ports() -> Vec<String> {
    #[cfg(feature = "serial")]
    {
        machine::grbl::available_ports()
    }
    #[cfg(not(feature = "serial"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_takes_limits_from_profile() {
        let p = MachineProfile::generic_lmc_fiber();
        let t = target_from_profile(&p);
        assert_eq!(t.field_mm, p.field_mm);
        assert_eq!(t.max_power_pct, p.max_power_pct);
        assert_eq!(t.timing.max_jump_delay_us, p.timing.jump_delay_max_us);
        assert_eq!(t.open_mo_delay_us, p.open_mo_delay_us);
        assert!(t.pulse_width_enabled && !t.z_axis && !t.rotary_axis);
        let mut q = p.clone();
        q.open_mo_delay_us = 2000.0;
        q.fiber_kind = FiberKind::QSwitched;
        q.calibration.scale_x = 1.01;
        q.axis = Some(machine::ExtensionAxis { role: AxisRole::Rotary, steps_per_mm: 0.0, min_mm: 0.0, max_mm: 0.0, invert: false });
        let t = target_from_profile(&q);
        assert_eq!((t.open_mo_delay_us, t.calibration.scale_x), (2000.0, 1.01));
        assert!(!t.pulse_width_enabled && t.rotary_axis && !t.z_axis);
    }

    #[test]
    fn firing_singles_are_recognised() {
        assert!(single_can_fire(Single::fiber_set_mo(true)));
        assert!(single_can_fire(Single::write_port(LASER_ENABLE_BIT | 0x100)));
        assert!(!single_can_fire(Single::write_port(0x100)));
        assert!(!single_can_fire(Single::fiber_set_mo(false)));
    }

    #[tokio::test]
    async fn read_inputs_returns_the_simulated_port() {
        let m = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
        let r = m.blocking(|m, ctl| m.read_inputs(ctl)).await.unwrap();
        assert!(matches!(r, Err(ControllerError::NotInitialized)));
        m.connect().await.unwrap();
        m.sim_counters().unwrap().set_inputs(1 << 14);
        assert_eq!(m.blocking(|m, ctl| m.read_inputs(ctl)).await.unwrap().unwrap(), 1 << 14);
        assert_eq!(m.sim_counters().unwrap().laser_singles(), 0);
    }

    /// The engine reaches the board only through `Controller`: `Machine` holds no link of its own, and no engine
    /// module sends singles or packets itself.
    #[test]
    fn the_engine_keeps_no_link_of_its_own() {
        let src = include_str!("hw.rs");
        let start = src.find(concat!("pub struct ", "Machine {")).unwrap();
        let fields = &src[start..start + src[start..].find("\n}").unwrap()];
        let link_fields: Vec<&str> =
            fields.lines().filter(|l| l.contains("Link") && !l.contains("LinkKind")).collect();
        assert!(link_fields.is_empty(), "Machine holds a link: {link_fields:?}");
        assert!(!src.contains(concat!("Link", "Slot")));
        for (name, text) in [
            ("approve.rs", include_str!("approve.rs")),
            ("frame.rs", include_str!("frame.rs")),
            ("state.rs", include_str!("state.rs")),
            ("api.rs", include_str!("api.rs")),
            ("mcp.rs", include_str!("mcp.rs")),
        ] {
            for call in [concat!(".send_", "single("), concat!(".send_", "packet(")] {
                assert!(!text.contains(call), "{name} calls {call}");
            }
        }
    }

    #[tokio::test]
    async fn abort_without_a_board_reports_not_connected() {
        // Machine::usb opens nothing until connect/ensure_ready, and abort never calls those.
        let m = Arc::new(Machine::usb(MachineProfile::generic_lmc_fiber(), None));
        let before = m.stop_epoch();
        assert_eq!(m.abort().await.unwrap(), AbortOutcome::NotConnected);
        assert_eq!(m.stop_epoch(), before + 1);
        let sim = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
        sim.connect().await.unwrap();
        assert_eq!(sim.abort().await.unwrap(), AbortOutcome::Sent);
    }

    #[tokio::test]
    async fn swapping_the_config_reinitialises_the_simulator() {
        let m = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
        m.connect().await.unwrap();
        let singles = m.sim_counters().unwrap().singles();
        let generic = MachineProfile::generic_jcz_galvo();
        let r = m.blocking(move |m, ctl| m.swap_config(ctl, generic, None)).await.unwrap();
        assert_eq!(r.unwrap(), 1);
        assert_eq!((m.generation(), m.profile().name.as_str(), m.target().field_mm), (1, "Generic JCZ galvo", 110.0));
        assert_eq!(m.status_json()["connected"], true, "initialised again");
        assert!(m.sim_counters().unwrap().singles() > singles, "the counters carry on");
        assert_eq!(m.sim_counters().unwrap().laser_singles(), 0);
        let bad = MachineProfile { field_mm: 0.0, ..MachineProfile::generic_lmc_fiber() };
        assert!(m.blocking(move |m, ctl| m.swap_config(ctl, bad, None)).await.unwrap().is_err());
        assert_eq!(m.generation(), 1, "an invalid profile is never swapped in");
    }

    #[tokio::test]
    async fn simulated_machine_connects_without_firing() {
        let m = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
        m.connect().await.unwrap();
        assert_eq!(m.status_json()["connected"], true);
        let c = m.sim_counters().unwrap();
        assert!(c.singles() > 0);
        assert_eq!(c.laser_singles(), 0);
        assert_eq!(c.packets(), 0);
    }
}
