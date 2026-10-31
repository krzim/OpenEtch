//! The engine's side of the laser: [`Machine`] owns the `machine::Controller` behind its own mutex and drives it
//! from blocking tasks, so a slow USB transfer never stalls the async runtime. Also the links the engine can use
//! ([`CountingSimLink`], [`OfflineLink`]) and the profile -> `plan::Target` mapping.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};

use corfile::CorTable;
use lmc::{LASER_ENABLE_BIT, Packet, Single};
use machine::{
    AbortHandle, Controller, ControllerError, ControllerState, InitSettings, Link, LinkError, MachineProfile,
    PollConfig, Reply, SimLink,
};
use plan::Target;
use serde_json::{Value, json};

use crate::EngineError;

/// The controller type the engine drives: any link, boxed.
pub type Ctl = Controller<Box<dyn Link>>;

/// galvoplotter's `delay_open_mo` (8 ms), the wait after opening the MO inside a list.
pub const OPEN_MO_DELAY_US: f64 = 8000.0;

/// Which board the engine talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// [`CountingSimLink`]: nothing leaves the process.
    Simulated,
    /// The real board over USB (feature `usb`).
    Usb,
}

/// The planning target for a machine profile. Hard limits come only from the profile.
pub fn target_from_profile(p: &MachineProfile) -> Target {
    Target {
        field_mm: p.field_mm,
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
        open_mo_delay_us: OPEN_MO_DELAY_US,
    }
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

    fn json(&self) -> Value {
        json!({
            "singles": self.singles(),
            "packets": self.packets(),
            "laser_singles": self.laser_singles(),
            "emitting_packets": self.emitting_packets(),
        })
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
        // EnableLaser is part of init and does not fire on its own; count only MO/port changes.
        if cmd != Single::enable_laser() && single_can_fire(cmd) {
            self.counters.laser_singles.fetch_add(1, Ordering::SeqCst);
        }
        let reply = self.inner.send_single(cmd);
        self.inner.clear_traffic();
        reply
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        self.counters.packets.fetch_add(1, Ordering::SeqCst);
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

/// The laser as the engine sees it. See the module docs.
pub struct Machine {
    ctl: Arc<Mutex<Ctl>>,
    abort: AbortHandle,
    profile: MachineProfile,
    cor: Option<CorTable>,
    target: Target,
    kind: LinkKind,
    sim: Option<Arc<SimCounters>>,
    /// An approved run holds this from approval until it ends.
    running: AtomicBool,
    /// Bumped to end the current frame loop; the loop owns the value it started with.
    frame_gen: AtomicU64,
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

    fn with_link(
        link: Box<dyn Link>,
        kind: LinkKind,
        sim: Option<Arc<SimCounters>>,
        profile: MachineProfile,
        cor: Option<CorTable>,
        poll: PollConfig,
    ) -> Self {
        let ctl = Controller::with_settings(link, InitSettings::default(), poll);
        Machine {
            abort: ctl.abort_handle(),
            ctl: Arc::new(Mutex::new(ctl)),
            target: target_from_profile(&profile),
            profile,
            cor,
            kind,
            sim,
            running: AtomicBool::new(false),
            frame_gen: AtomicU64::new(0),
            framing_gen: AtomicU64::new(0),
            last_error: Mutex::new(None),
        }
    }

    pub fn profile(&self) -> &MachineProfile {
        &self.profile
    }

    /// The planning target derived from the profile.
    pub fn target(&self) -> &Target {
        &self.target
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

    /// Initialise the board if it is not initialised yet (opening the USB link first). Call with the lock held.
    pub fn ensure_ready(&self, ctl: &mut Ctl) -> Result<(), ControllerError> {
        if ctl.state() != ControllerState::Uninitialized {
            return Ok(());
        }
        if self.kind == LinkKind::Usb {
            *ctl.link_mut() = open_usb()?;
        }
        match ctl.init(&self.profile, self.cor.as_ref()) {
            Ok(info) => {
                tracing::info!(serial = ?info.serial, version = ?info.version, "board initialised");
                self.set_error(None);
                Ok(())
            }
            Err(e) => {
                if self.kind == LinkKind::Usb {
                    *ctl.link_mut() = Box::new(OfflineLink);
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

    /// Request an abort (noticed by a running job at its next packet or poll), end framing, then send the abort
    /// sequence once the controller is free. Never gated.
    pub async fn abort(self: &Arc<Self>) -> Result<(), EngineError> {
        self.abort.request();
        self.bump_frame_gen();
        self.blocking(|_, ctl| ctl.abort()).await??;
        Ok(())
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
        json!({
            "connected": initialised,
            "simulated": simulated,
            "sim": simulated,
            "link": if simulated { "sim" } else { "usb" },
            "busy": running || framing || state == "busy" || state == "running" || state == "framing",
            "state": state,
            "running": running,
            "framing": framing,
            "field_mm": self.profile.field_mm,
            "profile": self.profile.name,
            "laser_source": self.profile.laser_source,
            "max_power_pct": self.profile.max_power_pct,
            "last_error": self.last_error(),
            "sim_counters": self.sim.as_ref().map(|c| c.json()),
        })
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
    }

    #[test]
    fn firing_singles_are_recognised() {
        assert!(single_can_fire(Single::fiber_set_mo(true)));
        assert!(single_can_fire(Single::write_port(LASER_ENABLE_BIT | 0x100)));
        assert!(!single_can_fire(Single::write_port(0x100)));
        assert!(!single_can_fire(Single::fiber_set_mo(false)));
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
