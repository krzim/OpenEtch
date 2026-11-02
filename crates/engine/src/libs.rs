//! Engine state for libraries, preferences and the remaining job-control options (one field of `AppState`):
//! the material library store, the preferences (layer and optimization defaults, Require Framing Before Start, the
//! Job Checklist, Play Sound on Job Finished, debug log, update check), the job log, the framing record that
//! Require Framing checks, the framing switch input watcher, the camera, and the last update check.
//!
//! Approval hooks (called by `approve`): [`check_before_approve`] refuses a run while Require Framing is on and the
//! job was not framed since the last edit, while the Job Checklist is on and not every item is ticked, and while
//! the Door Protect input reads open; [`after_run`] appends the job log line and sends the finished-sound event.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::camera::{Alignment, Lens};
use crate::frame::FrameRequest;
use crate::joblog::{JobLog, JobLogEntry};
use crate::library::LibraryStore;
use crate::prefs::PrefStore;
use crate::runs::RunState;
use crate::updates::UpdateCheck;
use crate::{AppState, EngineError, Event};

/// How often the framing switch input is read while the board is idle.
pub const IO_POLL: Duration = Duration::from_millis(150);
pub const CAMERA_FILE: &str = "camera.json";

/// One device's camera calibration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraCalibration {
    pub lens: Option<Lens>,
    pub alignment: Option<Alignment>,
}

/// The camera: the newest frame and the calibrations by device id.
#[derive(Debug, Default)]
pub struct CameraState {
    pub frame: Option<image::RgbImage>,
    pub frame_unix_s: u64,
    pub calibrations: std::collections::BTreeMap<String, CameraCalibration>,
    path: Option<PathBuf>,
}

impl CameraState {
    pub fn save(&self) -> Result<(), EngineError> {
        let Some(p) = &self.path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&self.calibrations).map_err(|e| EngineError::Internal(e.to_string()))?;
        crate::prefs::write_atomic(p, text.as_bytes())
    }
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct Libs {
    pub prefs: Mutex<PrefStore>,
    pub library: Mutex<LibraryStore>,
    job_log: RwLock<JobLog>,
    /// The machine generation (device and profile) and document revision of the last frame started (Require
    /// Framing Before Start): a frame on another device, or of another revision, does not count.
    framed: Mutex<Option<(u64, u64)>>,
    /// The last frame request (the framing switch repeats it).
    pub last_frame: Mutex<Option<FrameRequest>>,
    pub camera: Mutex<CameraState>,
    pub updates: Mutex<Option<UpdateCheck>>,
    /// The foot switch cycle (module `cycle`).
    pub foot_cycle: Mutex<crate::cycle::FootCycle>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Libs {
    /// Use `<config-dir>` for preferences, libraries, the job log and camera calibrations. Returns notes for the
    /// console (broken files that were skipped).
    pub fn open(&self, dir: &Path) -> Vec<String> {
        let (prefs, note) = PrefStore::open(dir);
        crate::diagnostics::set_debug(prefs.prefs.debug_log);
        *lock(&self.prefs) = prefs;
        let (lib, mut notes) = LibraryStore::open(dir);
        notes.extend(note);
        *lock(&self.library) = lib;
        *self.job_log.write().unwrap_or_else(PoisonError::into_inner) = JobLog::open(dir);
        let path = dir.join(CAMERA_FILE);
        let calibrations = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        *lock(&self.camera) = CameraState { calibrations, path: Some(path), ..CameraState::default() };
        notes
    }

    pub fn read_job_log(&self, limit: usize) -> Result<Vec<JobLogEntry>, EngineError> {
        self.job_log.read().unwrap_or_else(PoisonError::into_inner).read(limit)
    }

    pub fn append_job_log(&self, e: &JobLogEntry) {
        self.job_log.read().unwrap_or_else(PoisonError::into_inner).append(e);
    }

    /// Record a frame of document revision `revision` on machine generation `generation` (and the request, for the
    /// framing switch).
    pub fn framed(&self, generation: u64, revision: u64, req: &FrameRequest) {
        *lock(&self.framed) = Some((generation, revision));
        *lock(&self.last_frame) = Some(req.clone());
    }

    /// `(machine generation, document revision)` of the last frame started.
    pub fn framed_at(&self) -> Option<(u64, u64)> {
        *lock(&self.framed)
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Refuse an approval the device's options do not allow yet (see the module docs). `ticked`: the Job Checklist
/// items the operator ticked, in order.
pub(crate) async fn check_before_approve(st: &Arc<AppState>, run_revision: u64, ticked: Option<&[bool]>) -> Result<(), EngineError> {
    let device = st.active_device().id;
    let p = lock(&st.libs.prefs).device(&device);
    if p.require_framing && st.libs.framed_at() != Some((st.machine.generation(), run_revision)) {
        return Err(EngineError::Conflict(
            "Require Framing Before Start is on: frame the job (since its last edit) before approving it".into(),
        ));
    }
    if p.checklist.enabled {
        let n = p.checklist.items.len();
        let ok = ticked.is_some_and(|t| t.len() == n && t.iter().all(|b| *b));
        if !ok {
            return Err(EngineError::Conflict(format!(
                "the Job Checklist has {n} item(s); tick every one (`checklist`: [true, ...]) to approve"
            )));
        }
    }
    let door = st.machine.profile().io.door_protect;
    if let Some(d) = door {
        // Reading the input waits for the controller, which a running job holds until it ends: the approval would
        // hang for the whole job and then start this run with nobody having pressed anything since. Refused as a
        // second approval is (`Controller::run_parts` checks the door again before anything can emit).
        if st.machine.is_running() {
            return Err(EngineError::Conflict("another run is in progress".into()));
        }
        let read = st
            .machine
            .blocking(|m, ctl| {
                if m.is_running() {
                    return None;
                }
                Some(m.ensure_ready(ctl).and_then(|()| m.read_inputs(ctl)))
            })
            .await?;
        match read {
            None => return Err(EngineError::Conflict("another run is in progress".into())),
            Some(Ok(bits)) if d.active(bits) => {
                return Err(EngineError::Conflict(format!("the door interlock (input {}) is open; close the enclosure", d.bit)));
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(EngineError::Machine(e)),
        }
    }
    Ok(())
}

/// After a run ended: append the job log line and, for a finished run with Play Sound on, send the
/// `job_finished_sound` machine event (the UI plays the sound).
pub(crate) fn after_run(st: &Arc<AppState>, run_id: &str, device: &str, state: RunState, started: Instant, error: Option<String>) {
    let (origin, summary, completed) = match st.runs().get(run_id) {
        Some(r) => (r.origin.clone(), r.summary.clone(), r.completed),
        None => (commands::Origin::Ui, String::new(), 0),
    };
    let origin = match origin {
        commands::Origin::Ui => "ui".to_string(),
        commands::Origin::Script => "script".to_string(),
        commands::Origin::Agent(name) => format!("agent:{name}"),
    };
    let state_name = serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    st.libs.append_job_log(&JobLogEntry {
        time_unix_s: now_unix(),
        run_id: run_id.to_string(),
        state: state_name,
        origin,
        device: device.to_string(),
        project: st.project_name(),
        summary,
        duration_s: (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        completed,
        error,
    });
    if state == RunState::Finished && lock(&st.libs.prefs).device(device).play_sound {
        st.emit(Event::machine("job_finished_sound", Some(run_id.to_string())));
    }
}

/// Release-then-press detection for an input read by [`spawn_io_watch`].
#[derive(Debug, Default)]
struct Edge {
    released: bool,
    pressed: u32,
}

impl Edge {
    /// Forget what was seen (the input is not watched, or the board is busy or unreadable): the next press counts
    /// only after a release.
    fn reset(&mut self) {
        *self = Edge::default();
    }

    /// One read: true once the input, seen released, has read pressed `reads` times in a row (then a release is
    /// needed again).
    fn update(&mut self, active: bool, reads: u32) -> bool {
        if !active {
            *self = Edge { released: true, pressed: 0 };
            return false;
        }
        if !self.released {
            return false;
        }
        self.pressed += 1;
        if self.pressed < reads {
            return false;
        }
        self.reset();
        true
    }
}

/// The foot switch as [`spawn_io_watch`] sees it: its release-then-press and the port it was seen on.
#[derive(Debug, Default)]
struct FootWatch {
    edge: Edge,
    port: Option<machine::InputPort>,
}

impl FootWatch {
    /// The port to watch now (`None`: not watched). A different bit or polarity (a device edit or switch) forgets
    /// what was seen: after a polarity flip a released pedal reads pressed, which must not count as a press.
    fn watch(&mut self, port: Option<machine::InputPort>) {
        if port != self.port {
            self.port = port;
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.edge.reset();
    }

    /// One read of the port: true once a press is complete (`pin::CONFIRM_READS` pressed reads after a release).
    fn update(&mut self, bits: u16) -> bool {
        let Some(port) = self.port else { return false };
        self.edge.update(port.active(bits), crate::pin::CONFIRM_READS)
    }
}

/// Watch the profile's Start Framing input and, in the `click` approval mode, the foot switch (the Start Marking
/// input, `approve::foot_switch_port`). Start Framing starts framing (the last frame request, else the whole job's
/// bounds) when it is pressed after being seen released, while the board is idle. The foot switch drives the foot
/// switch cycle (module `cycle`: frame, then burn what was framed, then frame again) when it is seen released and then
/// pressed for `pin::CONFIRM_READS` reads in a row; a changed foot switch port (bit or polarity) needs a fresh
/// release. It is read every `pin::CONFIRM_POLL` while watched, else every [`IO_POLL`]; the reads a frame took while
/// it streamed (`machine::InputSamples`, when the controller was held too long for these) count first, in order.
/// Reads only while the board is initialised and not running (a run claimed by an approval, including one waiting
/// for its start input or physical confirmation, owns the input; a press then is not seen at all), and while
/// framing only when the foot switch is watched (the cycle's second press comes while it frames); sends nothing but
/// `ReadPort` until it starts a frame or the cycle acts.
pub fn spawn_io_watch(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut framing, mut foot) = (Edge::default(), FootWatch::default());
        loop {
            let poll = if crate::approve::foot_switch_port(&st).is_some() { crate::pin::CONFIRM_POLL } else { IO_POLL };
            tokio::time::sleep(poll).await;
            let frame_port = st.machine.profile().io.start_framing;
            let foot_port = crate::approve::foot_switch_port(&st);
            crate::cycle::sync(&st, foot_port.is_some());
            if frame_port.is_none() {
                framing.reset();
            }
            foot.watch(foot_port);
            let frame_now = st.machine.is_framing();
            if (frame_port.is_none() && foot_port.is_none()) || st.machine.is_running() || (frame_now && foot_port.is_none()) {
                framing.reset();
                foot.reset();
                st.machine.clear_input_samples();
                continue;
            }
            let read = st
                .machine
                .blocking(|m, ctl| {
                    // Checked again with the controller held: an approval may have claimed the machine meanwhile.
                    // The reads a streaming frame took come first (taken with the controller held, so none is lost
                    // or out of order), then this one.
                    let streamed = m.take_input_samples();
                    (!m.is_running() && ctl.state() != machine::ControllerState::Uninitialized)
                        .then(|| ctl.read_port().ok())
                        .flatten()
                        .map(|bits| (streamed, bits))
                })
                .await;
            let Ok(Some((streamed, bits))) = read else {
                framing.reset();
                foot.reset();
                continue;
            };
            if streamed.into_iter().chain([bits]).any(|b| foot.update(b)) {
                framing.reset();
                crate::cycle::pressed(&st).await;
                continue;
            }
            if frame_now {
                // The Start Framing input starts a frame only while the board is idle.
                framing.reset();
                continue;
            }
            if let Some(port) = frame_port
                && framing.update(port.active(bits), 1)
            {
                let req = lock(&st.libs.last_frame).clone().unwrap_or_default();
                match st.frame_start(req).await {
                    Ok(_) => st.emit(Event::machine("framing", Some("framing switch".into()))),
                    Err(e) => tracing::warn!(error = %e, "framing switch: could not start framing"),
                }
            }
        }
    })
}

/// Fill in the saved defaults of the active device (LightBurn's layer defaults and optimization defaults), so the
/// recorded invocations replay without the preferences: `layer.add` without `defaults` gets the per-colour layer
/// defaults; `doc.new` without `layer_settings` gets the default for the new layer's colour when "load defaults on
/// New" is on, and without `optimization` the saved optimization defaults.
pub(crate) fn fill_defaults(st: &AppState, invs: &mut [commands::Invocation]) {
    if !invs.iter().any(|i| matches!(i.id.as_str(), "layer.add" | "doc.new")) {
        return;
    }
    let device = st.active_device().id;
    let p = lock(&st.libs.prefs).device(&device);
    for inv in invs.iter_mut() {
        if inv.params.is_null() && inv.id == "doc.new" {
            inv.params = serde_json::json!({});
        }
        if let Some(params) = inv.params.as_object_mut() {
            fill_one(&inv.id, params, &p);
        }
    }
}

fn fill_one(id: &str, params: &mut serde_json::Map<String, serde_json::Value>, p: &crate::prefs::DevicePrefs) {
    match id {
        "layer.add" if !params.contains_key("defaults") && !p.layer_defaults.is_empty() => {
            let map: std::collections::BTreeMap<&String, &doc::CutSettings> = p.layer_defaults.iter().map(|(k, v)| (k, &v.settings)).collect();
            if let Ok(v) = serde_json::to_value(map) {
                params.insert("defaults".into(), v);
            }
        }
        "doc.new" => {
            if p.load_defaults_on_new
                && !params.contains_key("layer_settings")
                && let Some(d) = p.layer_default(NEW_LAYER_COLOR)
                && let Ok(v) = serde_json::to_value(&d.settings)
            {
                params.insert("layer_settings".into(), v);
            }
            if !params.contains_key("optimization")
                && let Some(o) = &p.optimization_defaults
                && let Ok(v) = serde_json::to_value(o)
            {
                params.insert("optimization".into(), v);
            }
        }
        _ => {}
    }
}

/// The colour of a new document's layer (`doc::Document::default`).
pub const NEW_LAYER_COLOR: &str = "#1f77b4";

/// Select the active device's library when it is loaded (LightBurn loads the library saved with a device when the
/// device is selected).
pub fn select_device_library(st: &AppState) {
    let device = st.active_device().id;
    let Some(name) = lock(&st.libs.prefs).device(&device).library else { return };
    let mut store = lock(&st.libs.library);
    if store.find(&name).is_some()
        && let Err(e) = store.select(&name)
    {
        tracing::warn!(library = %name, error = %e, "cannot select the device's library");
    }
}

/// At start: with "load default layer settings on New / restart" on, the first document starts from the saved
/// defaults (as `doc.new` would make it).
pub fn apply_startup_defaults(st: &AppState) {
    let device = st.active_device().id;
    let p = lock(&st.libs.prefs).device(&device);
    if !p.load_defaults_on_new && p.optimization_defaults.is_none() {
        return;
    }
    let doc = st.document();
    let mut params = serde_json::json!({ "name": doc.name, "field_mm": doc.field_mm });
    if let Some(w) = doc.workspace_mm {
        params["workspace_mm"] = serde_json::json!(w);
    }
    if let Some(o) = doc.extras.get(commands::ORIGIN_KEY) {
        params["origin"] = serde_json::json!(o);
    }
    let inv = commands::Invocation { id: "doc.new".into(), params };
    if let Err(e) = st.execute(inv, commands::Origin::Script) {
        tracing::warn!(error = %e, "could not apply the saved defaults to the first document");
    }
}

#[cfg(test)]
mod tests {
    use super::{Edge, FootWatch};
    use machine::InputPort;

    const HIGH: Option<InputPort> = Some(InputPort { bit: 14, active_low: false });
    const LOW: Option<InputPort> = Some(InputPort { bit: 14, active_low: true });
    const PEDAL: u16 = 1 << 14;

    #[test]
    fn a_foot_press_needs_a_release_then_three_reads() {
        let mut f = FootWatch::default();
        f.watch(HIGH);
        assert!(!f.update(PEDAL), "held since the start: never");
        assert!(!f.update(0));
        assert!(!f.update(PEDAL) && !f.update(PEDAL));
        assert!(f.update(PEDAL), "third pressed read");
        assert!(!f.update(PEDAL), "held: no new press");
        assert!(!f.update(0) && !f.update(PEDAL) && !f.update(PEDAL));
        assert!(f.update(PEDAL));
    }

    #[test]
    fn a_changed_foot_switch_port_needs_a_fresh_release() {
        let mut f = FootWatch::default();
        f.watch(HIGH);
        assert!(!f.update(0), "released");
        // The polarity is flipped (a device edit): the released pedal now reads pressed. Not a press.
        f.watch(LOW);
        for _ in 0..5 {
            assert!(!f.update(0));
        }
        // Real press (reads high, inactive for active-low) and release (low, active): a press after the release.
        assert!(!f.update(PEDAL));
        assert!(!f.update(0) && !f.update(0));
        assert!(f.update(0));
        // Same port again: nothing forgotten; no port: never.
        f.watch(LOW);
        f.watch(None);
        assert!(!f.update(PEDAL) && !f.update(0) && !f.update(0) && !f.update(0));
    }

    #[test]
    fn an_edge_needs_a_release_then_enough_presses() {
        let mut e = Edge::default();
        assert!(!e.update(true, 3), "held since the start: never");
        assert!(!e.update(true, 3));
        assert!(!e.update(false, 3));
        assert!(!e.update(true, 3) && !e.update(true, 3));
        assert!(e.update(true, 3), "third read in a row");
        assert!(!e.update(true, 3), "still held: needs a release again");
        assert!(!e.update(false, 3) && !e.update(true, 3) && !e.update(false, 3), "a bounce restarts the count");
        assert!(!e.update(true, 3) && !e.update(true, 3));
        e.reset();
        assert!(!e.update(true, 3), "after a reset a release comes first");
        assert!(!e.update(false, 1));
        assert!(e.update(true, 1), "one read for the framing switch");
    }
}
