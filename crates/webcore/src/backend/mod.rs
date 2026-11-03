//! The browser build's backend: the engine's document API answered in memory, in a Web Worker (the design-only
//! build, docs/design-only/plan.md). [`Backend::handle`] takes one request (method, path, query, body) and returns a
//! [`Reply`]: the HTTP answer the engine would give, the events it would broadcast and the store records to persist.
//! It is synchronous and runs natively too (`cargo test -p webcore`; `tests/parity.rs` sends the same requests to
//! the engine and compares the JSON).
//!
//! State: the open documents ([`appcore::docset`]), each with its timeline and project state; the design devices
//! ([`appcore::devices::DesignDevices`]); preferences, autosave settings, font favourites and presets, restored from
//! the store records the worker shell gives [`Backend::new`]. Nothing here reaches a machine: machine routes answer
//! "not available" (`GET /api/machine` and `GET /api/job` with idle stubs).
//!
//! Projects, backups and templates live in the shell's storage (IndexedDB): it opens one by passing its text to a
//! shell route (`/core/...`, module `shell`) and saves the text `GET /api/document/download?format=openetch` gives.

mod docs;
mod libraries;
mod reply;
mod shell;
mod stores;

use std::collections::HashMap;
use std::sync::Arc;

use appcore::AppError;
use appcore::devices::DesignDevices;
use appcore::docset::DocSet;
use appcore::docsync::DocLog;
use appcore::event::Event;
use appcore::prefs::Preferences;
use appcore::presets::ImagePreset;
use appcore::project::ProjectState;
use appcore::stores::{FileSettings, StoreWrite};
use commands::testgen_presets::TestPreset;
use commands::{Invocation, Origin, Outcome, Session};
use doc::Document;
use machine::MachineProfile;
use plan::{PlanError, Preview, Target};
use serde::Deserialize;
use serde_json::{Value, json};

pub use reply::{Answer, Reply, parse_query, url_decode};

/// The built-in B6 MOPA profile (the engine's default device).
const B6_TOML: &str = include_str!("../../../../fixtures/b6-mopa-60w.toml");

/// The built-in design devices: the B6 MOPA (the base of LightBurn imports), then generic galvo fields.
pub fn builtin_profiles() -> Vec<MachineProfile> {
    let mut out = vec![MachineProfile::from_toml(B6_TOML).expect("the B6 fixture is a valid profile")];
    for field in [110.0, 150.0, 200.0, 300.0] {
        let mut p = MachineProfile::generic_jcz_galvo();
        p.name = format!("Generic galvo {field} mm");
        p.field_mm = field;
        p.field_height_mm = None;
        p.cor_file = None;
        out.push(p);
    }
    out
}

/// One stored record handed to [`Backend::new`] (what an earlier [`StoreWrite`] saved).
#[derive(Debug, Clone, Deserialize)]
pub struct Record {
    pub store: String,
    pub key: String,
    pub value: String,
}

/// What [`Backend::new`] takes (JSON): the stored records, and this session's id for document change sets.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Init {
    pub stores: Vec<Record>,
    /// A random id for this worker's run (document change sets name it); `"design"` when none is given.
    pub epoch: Option<String>,
}

/// A preview planned for a revision and device generation.
struct CachedPreview {
    revision: u64,
    generation: u64,
    doc: Document,
    result: Result<Preview, PlanError>,
}

/// The backend. See the module docs.
pub struct Backend {
    session: Session,
    project: ProjectState,
    docs: DocSet,
    doc_log: DocLog,
    epoch: Arc<str>,
    devices: DesignDevices,
    /// Changes with the active device (as the engine's machine configuration generation).
    generation: u64,
    prefs: Preferences,
    files: FileSettings,
    fonts: appcore::fonts::FontPrefs,
    test_presets: Vec<TestPreset>,
    image_presets: Vec<ImagePreset>,
    libraries: appcore::library::MemLibraries,
    preview: Option<CachedPreview>,
    events: Vec<Event>,
    writes: Vec<StoreWrite>,
    /// Problems with stored records found at start (unusable records are skipped).
    pub notes: Vec<String>,
}

impl Backend {
    /// A backend restored from `init` (see [`Init`]), with a new empty document laid out for the active device.
    pub fn new(init: Init) -> Backend {
        Backend::with_profiles(init, builtin_profiles())
    }

    /// [`Backend::new`] with other built-in design profiles (the first is the built-in device; tests).
    pub fn with_profiles(init: Init, profiles: Vec<MachineProfile>) -> Backend {
        let mut devices = DesignDevices::new(profiles);
        let mut notes = Vec::new();
        let mut prefs = Preferences::default();
        let mut files = FileSettings::default();
        let mut fonts = appcore::fonts::FontPrefs::default();
        let mut test_presets = Vec::new();
        let mut image_presets = Vec::new();
        let mut libraries = appcore::library::MemLibraries::default();
        // The device records first (the index names them), then the rest.
        let mut records = init.stores;
        records.sort_by_key(|r| (r.store != appcore::stores::names::DEVICES, r.key == "index"));
        for r in &records {
            let note = |e: String| format!("{}/{}: {e}; skipped", r.store, r.key);
            let result: Result<(), String> = match (r.store.as_str(), r.key.as_str()) {
                (appcore::stores::names::DEVICES, key) => devices.restore(key, &r.value),
                (appcore::stores::names::PREFS, "preferences") => serde_json::from_str(&r.value).map(|p| prefs = p).map_err(|e| e.to_string()),
                (appcore::stores::names::FILES, "settings") => serde_json::from_str(&r.value).map(|f| files = f).map_err(|e| e.to_string()),
                (appcore::stores::names::FONTS, "prefs") => serde_json::from_str(&r.value).map(|f| fonts = f).map_err(|e| e.to_string()),
                (appcore::stores::names::PRESETS, "material-test") => {
                    serde_json::from_str(&r.value).map(|p| test_presets = p).map_err(|e| e.to_string())
                }
                (appcore::stores::names::PRESETS, "image-adjust") => {
                    serde_json::from_str(&r.value).map(|p| image_presets = p).map_err(|e| e.to_string())
                }
                (appcore::library::STORE, key) => libraries.restore(key, &r.value),
                _ => Err("not a store this build keeps".into()),
            };
            if let Err(e) = result {
                notes.push(note(e));
            }
        }
        notes.extend(libraries.restored());
        let active = devices.active().clone();
        let session = Session::new(appcore::layout::new_document_for(&active, profile_field(&active)));
        let project = ProjectState::clean_at(session.document().revision);
        let mut b = Backend {
            session,
            project,
            docs: DocSet::default(),
            doc_log: DocLog::default(),
            epoch: init.epoch.unwrap_or_else(|| "design".into()).into(),
            devices,
            generation: 0,
            prefs,
            files,
            fonts,
            test_presets,
            image_presets,
            libraries,
            preview: None,
            events: Vec::new(),
            writes: Vec::new(),
            notes,
        };
        b.apply_startup_defaults();
        b
    }

    /// With "load default layer settings on New / restart" (or saved optimization defaults), the first document
    /// starts from them, as the engine's `libs::apply_startup_defaults` does (one `doc.new` step from `script`).
    fn apply_startup_defaults(&mut self) {
        let p = self.device_prefs();
        if !p.load_defaults_on_new && p.optimization_defaults.is_none() {
            return;
        }
        let d = self.session.document();
        let mut params = json!({ "name": d.name, "field_mm": d.field_mm });
        if let Some(w) = d.workspace_mm {
            params["workspace_mm"] = json!(w);
        }
        if let Some(o) = d.extras.get(commands::ORIGIN_KEY) {
            params["origin"] = json!(o);
        }
        if let Err(e) = self.execute(Invocation { id: "doc.new".into(), params }, Origin::Script) {
            self.notes.push(format!("could not apply the saved defaults to the first document: {e}"));
        }
        self.events.clear();
    }

    /// Answer one request: `method` (`GET`, `POST`, ...), `path` (`/api/...` or a shell route `/core/...`),
    /// `query` (without `?`), the body, and `since` (the `X-OpenEtch-Since` header: `<epoch>:<revision>`; a JSON answer
    /// to a `POST` then gets the `sync` member with the document's changes, as the engine adds it).
    pub fn handle(&mut self, method: &str, path: &str, query: &str, body: Vec<u8>, since: Option<&str>) -> Reply {
        let method = method.to_ascii_uppercase();
        let q = parse_query(query);
        let segs = reply::segments(path);
        let segs: Vec<&str> = segs.iter().map(String::as_str).collect();
        let answer = match segs.split_first() {
            Some((&"api", rest)) => self.route_api(&method, rest, q, body),
            Some((&"core", rest)) => shell::route(self, &method, rest, q, body),
            _ => Err(AppError::NotFound("no such API route".into())),
        };
        let mut answer = answer.unwrap_or_else(|e| Answer::error(&e));
        if method == "POST"
            && let Some(since) = since.and_then(appcore::docsync::parse_since)
            && answer.headers.iter().any(|(k, v)| k == "content-type" && v.starts_with("application/json"))
        {
            let sync = serde_json::to_vec(&self.doc_sync(Some(since))).unwrap_or_default();
            if let Some(out) = appcore::http::splice_member(&answer.body, "sync", &sync) {
                answer.body = out;
            }
        }
        Reply {
            status: answer.status,
            headers: answer.headers,
            body: answer.body,
            events: self.events.drain(..).filter_map(|e| serde_json::to_string(&e).ok()).collect(),
            writes: std::mem::take(&mut self.writes),
        }
    }

    fn route_api(&mut self, method: &str, segs: &[&str], q: HashMap<String, String>, body: Vec<u8>) -> reply::Routed {
        if let Some(a) = docs::route(self, method, segs, &q, &body)? {
            return Ok(a);
        }
        if let Some(a) = stores::route(self, method, segs, &q, &body)? {
            return Ok(a);
        }
        if let Some(a) = libraries::route(self, method, segs, &q, &body)? {
            return Ok(a);
        }
        if let Some(a) = machine_stub(self, method, segs) {
            return Ok(a);
        }
        Err(AppError::NotFound("no such API route".into()))
    }

    /// The active document (tests, the preview worker).
    pub fn document(&self) -> &Document {
        self.session.document()
    }

    /// The active document's id.
    pub fn active_document_id(&self) -> &str {
        &self.docs.active
    }

    fn emit(&mut self, ev: Event) {
        self.events.push(ev);
    }

    fn announce(&mut self, before: u64, after: u64, origin: Origin, hist: (usize, usize)) {
        let id = self.docs.active.clone();
        self.events.extend(appcore::event::announce(before, after, origin, hist, id));
    }

    /// The machine field new documents and LightBurn imports get: the active device's profile field, else its
    /// workspace's.
    fn profile_field(&self) -> f64 {
        profile_field(self.devices.active())
    }

    /// Run commands as one undo step (see the engine's `AppState::batch_opening`).
    fn batch_opening(&mut self, mut invs: Vec<Invocation>, origin: Origin, opened: Option<String>, display: Option<String>) -> Result<Vec<Outcome>, AppError> {
        appcore::batch::check_not_empty(&invs)?;
        let active = self.devices.active().clone();
        if appcore::prefs::wants_defaults(&invs) {
            let p = self.prefs.devices.get(&active.id).cloned().unwrap_or_default();
            appcore::prefs::fill_defaults(&p, &mut invs);
        }
        let profile_field = self.profile_field();
        let devices = &self.devices;
        let find_device = |n: &str| devices.find(n).cloned();
        let lightburn_field = |_: &str| None;
        let ctx = appcore::batch::FieldContext { active: &active, profile_field, find_device: &find_device, lightburn_field: &lightburn_field };
        let mut field_sources = appcore::batch::fill_fields(&mut invs, &ctx);
        let display = appcore::batch::batch_display(&invs, display);
        let last_edit = appcore::batch::gate(&self.session, &invs, |_| Ok::<(), AppError>(()))?;
        let mut ran = appcore::batch::run(&mut self.session, invs, origin.clone(), profile_field, &mut field_sources)?;
        appcore::batch::settle_project(&mut self.project, &ran, opened, display, last_edit);
        appcore::batch::annotate(&mut ran.outcomes, field_sources);
        if ran.after != ran.before {
            self.announce(ran.before, ran.after, origin, ran.hist);
        }
        Ok(ran.outcomes)
    }

    fn execute(&mut self, inv: Invocation, origin: Origin) -> Result<Outcome, AppError> {
        self.batch_opening(vec![inv], origin, None, None).map(|mut v| v.remove(0))
    }

    /// Undo, redo, goto and delete: the marker and revision after it.
    fn timeline_op(&mut self, origin: Origin, op: impl FnOnce(&mut Session) -> Result<u64, commands::CommandError>) -> Result<Value, AppError> {
        let before = self.session.document().revision;
        let after = op(&mut self.session)?;
        let hist = (self.session.marker(), self.session.history().len());
        self.announce(before, after, origin, hist);
        Ok(json!({ "ok": true, "marker": hist.0, "revision": after }))
    }

    /// `GET /api/document/sync`'s payload.
    fn doc_sync(&mut self, since: Option<(String, u64)>) -> appcore::docsync::SyncPayload {
        let document_id = self.docs.active.clone();
        let revision = self.session.document().revision;
        self.doc_log.observe(self.session.document(), &document_id);
        let changes = since.filter(|(epoch, _)| **epoch == *self.epoch).and_then(|(_, r)| self.doc_log.since(r));
        let document = changes.is_none().then(|| self.session.document().clone());
        appcore::docsync::SyncPayload {
            epoch: self.epoch.clone(),
            document_id,
            revision,
            changes,
            document,
            history: self.session.history_view(),
            project: self.project.status(revision),
        }
    }

    /// What the preview is planned with: the active design device's target, no learned correction, the first run.
    pub fn preview_context(&self) -> appcore::preview::PreviewContext {
        let device = self.devices.active();
        let target = appcore::preview::target_for_device(device);
        let design_only = match &target {
            Some(_) => None,
            None => Some(device.not_controllable_reason().unwrap_or_else(|| format!("\"{}\" has no profile to plan with", device.display_name))),
        };
        let target = target.unwrap_or_else(|| {
            let mut p = MachineProfile::generic_jcz_galvo();
            p.field_mm = device.field_mm();
            appcore::preview::target_from_profile(&p)
        });
        appcore::preview::PreviewContext { target, correction: 1.0, run_index: 0, design_only, generation: self.generation }
    }

    /// The preview of the active document, planned once per revision and device.
    fn planned(&mut self) -> &CachedPreview {
        let revision = self.session.document().revision;
        let fresh = self.preview.as_ref().is_some_and(|c| c.revision == revision && c.generation == self.generation);
        if !fresh {
            let ctx = self.preview_context();
            let (doc, result) = appcore::preview::plan_preview(self.session.document().clone(), &ctx.target, ctx.run_index, ctx.correction, ctx.design_only);
            self.preview = Some(CachedPreview { revision, generation: self.generation, doc, result });
        }
        self.preview.as_ref().expect("planned above")
    }

    /// The active device's target, for the preview SVG's field.
    fn target(&self) -> Target {
        self.preview_context().target
    }

    /// The name downloads get: the project, else the opened file, else the document's name.
    fn stem(&self) -> String {
        self.project.stem(&self.session.document().name)
    }

    fn write(&mut self, w: StoreWrite) {
        self.writes.push(w);
    }

    /// The active device's preferences.
    fn device_prefs(&self) -> appcore::prefs::DevicePrefs {
        self.prefs.devices.get(&self.devices.active().id).cloned().unwrap_or_default()
    }

    /// Store the preferences.
    fn save_prefs(&mut self) {
        self.prefs.format = "openetch-preferences".into();
        let w = StoreWrite::json(appcore::stores::names::PREFS, "preferences", &self.prefs);
        self.write(w);
    }
}

fn profile_field(d: &machine::Device) -> f64 {
    d.profile.as_ref().map_or_else(|| d.field_mm(), |p| p.field_mm)
}

/// Machine routes: `GET /api/machine` and `GET /api/job` answer "no machine"; the rest are not available.
fn machine_stub(b: &Backend, method: &str, segs: &[&str]) -> Option<Answer> {
    let machine_route = matches!(
        segs.first().copied(),
        Some("machine" | "runs" | "job" | "camera" | "calibration" | "production" | "notifications" | "job-log" | "job-ledger" | "sim" | "export")
    ) || matches!(segs, ["devices", "scan" | "wizard" | "grbl" | "ruida"] | ["devices", "import", "markcfg"] | ["devices", _, "cor" | "duplicate"])
        || matches!(segs, ["lightburn", "prefs"] | ["preview.png"] | ["diagnostics", ..] | ["updates", ..]);
    match (method, segs) {
        ("GET", ["machine"]) => Some(Answer::json(&json!({
            "connected": false,
            "design_only": true,
            "device": b.devices.active().summary(),
            "runs": [],
            "pending_runs": [],
            "job": idle_job(),
        }))),
        ("GET", ["job"]) => Some(Answer::json(&idle_job())),
        _ if machine_route => Some(Answer::with_status(
            404,
            &json!({ "ok": false, "error": "not available in the design-only build", "code": "design_only" }),
        )),
        _ => None,
    }
}

/// `GET /api/job` with no machine: nothing running, nothing counted.
fn idle_job() -> Value {
    json!({
        "active": false, "run_id": null, "state": "idle", "iteration": 0, "iterations": null, "completed": 0,
        "packets_sent": 0, "packets_acknowledged": 0, "packets_total": 0, "part": 0, "parts": 0, "paused": false,
        "elapsed_s": 0.0, "estimated_time_s": 0.0, "remaining_s": 0.0, "finishing": false, "progress": 0.0,
        "progress_source": "time", "count": 0, "stop_at": null, "end_after_current": false, "waiting_layer": null,
    })
}
