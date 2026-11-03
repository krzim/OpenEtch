//! [`AppState`]: the live session, the machine, pending runs and the event channel, with the operations both the
//! HTTP API and the MCP server call. Locks are std mutexes held only for short, non-awaiting sections; planning
//! and rendering run on blocking threads.
//!
//! Projects: the document is saved to and opened from `<projects-dir>/<name>.openetch` ([`ProjectDir`]); the
//! current project name and the revision it was saved at live in [`ProjectState`] (cleared by `doc.new`,
//! `doc.load` and a replacing `doc.import_lbrn2`, set by save and open; only `doc.new` leaves a clean document, a
//! loaded or imported file is unsaved until it is saved as a project) and are updated under the session lock,
//! before `document_changed` is sent. Project names that differ only in case are the same project (Windows and
//! macOS file names ignore case), so saves refuse to replace one unless asked to and opens find it.
//! The document's `field_mm` comes from the machine profile when the engine starts with a document that has none,
//! and `doc.new` always starts at the active device's workspace (a gantry's bed and origin corner). `doc.import_lbrn2`
//! without a size gets the workspace of the device the file names (registry devices, then LightBurn's prefs), else
//! the active device's, else the document's, else the profile's; `doc.export_lbrn2` gets the profile's when neither
//! the caller nor the document gives one.
//!
//! Devices ([`crate::devices`]): the active device's workspace drives new documents and imports; planning, runs and
//! framing are refused while it is design-only. Activating another device ([`AppState::activate_device`]) is refused
//! while a run is approved, waiting for its confirmation or running, or while framing; it cancels pending runs,
//! swaps the machine profile (re-initialising the board) and emits `device_changed`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use commands::{CommandSpec, HistoryStep, HistoryView, Invocation, Origin, Outcome, Risk, Session};
use doc::Document;
use doc::file::try_to_openetch_json;
use plan::{FillRegions, PlanError, Preview};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::devices::{DeviceRegistry, ImportFilter, ImportReport};
use crate::frame::{self, FrameRequest, FrameRun, Pointer};
use crate::job::JobTracker;
use crate::hw::{AbortOutcome, Machine};
use crate::lightburn::LightBurn;
use crate::pin::{ApprovalConfig, ApprovalMode};
use crate::projects::{self, LBRN2_EXT, PROJECT_EXT, ProjectDir, ProjectInfo, ProjectState};
use crate::render::{self, RenderOptions};
use crate::runs::{self, RunOptions, RunStore, RunView};
use crate::security::SecurityConfig;
use crate::{EngineError, Event};

/// The message every pending-run answer carries.
pub const APPROVAL_MESSAGE: &str = "The run is pending. Nothing fires until a person reviews it and approves it in \
    the OpenEtch UI (Laser workspace) with the operator PIN or, when the engine runs without a PIN, with Start in the \
    app (the machine's foot switch never starts a pending run: it frames and burns the person's current document). \
    Agents cannot and must not approve runs, and must never ask for, handle or set the PIN.";

/// A planned preview and the document it was planned from.
pub type PreviewResult = Arc<(Document, Result<Preview, PlanError>)>;

/// Everything a preview is planned from besides the document's content: two previews with equal keys are equal.
#[derive(Debug, Clone, PartialEq)]
struct PreviewKey {
    /// Unique across open documents (`Session::raise_revision`), so it stands for the document's content.
    revision: u64,
    /// The job counter (auto-rotating fills).
    run_index: u64,
    /// The learned time correction factor's bits.
    correction: u64,
    target: plan::Target,
    /// Why the active device cannot plan, if it cannot.
    design_only: Option<String>,
}

/// A finished binary preview body and its `Content-Encoding` (module `preview_bin`).
#[derive(Debug, Clone)]
pub struct EncodedBody {
    pub bytes: axum::body::Bytes,
    pub encoding: Option<&'static str>,
}

/// A planned preview with what it was planned from, and its binary encodings, made on demand.
struct CachedPreview {
    key: PreviewKey,
    result: PreviewResult,
    /// Encoded answers by request options and content encoding (the last [`MAX_BIN_VARIANTS`] asked for).
    bins: Mutex<Vec<(String, Arc<std::sync::OnceLock<EncodedBody>>)>>,
}

/// Encoded binary previews kept per cached preview (layer filters, level of detail, content encodings).
const MAX_BIN_VARIANTS: usize = 6;

/// Coalesces and caches preview planning: at most one computation runs at a time, requests that queued behind it
/// share one computation of the newest document instead of each planning the revision they arrived at, and the
/// newest result is kept, so asking again at the same revision (the JSON, the binary form, the stats, the SVG, a
/// second browser) plans nothing.
#[derive(Default)]
struct PreviewGate {
    /// Held (owned, inside the blocking task) while a preview is being planned, so a dropped request cannot let a
    /// second computation start while the first is still running.
    running: Arc<tokio::sync::Mutex<()>>,
    /// The newest result.
    latest: Mutex<Option<Arc<CachedPreview>>>,
    /// Plans computed (tests).
    computed: AtomicUsize,
}

/// Shared engine state. See the module docs.
pub struct AppState {
    /// The active document's session (module `documents`: the other open documents wait in [`AppState::documents`]
    /// and are swapped in when activated).
    pub(crate) session: Mutex<Session>,
    pub machine: Arc<Machine>,
    runs: Mutex<RunStore>,
    events: broadcast::Sender<Event>,
    pub security: SecurityConfig,
    /// Operator PIN and the optional physical confirmation input.
    pub approval: ApprovalConfig,
    /// How runs are approved (`--approval`): set once at startup ([`AppState::set_approval_mode`]), else the PIN.
    approval_mode: std::sync::OnceLock<ApprovalMode>,
    previews: Arc<PreviewGate>,
    /// Where named projects are saved.
    pub projects: ProjectDir,
    /// The current project. Locked only while the session lock is held or with no other lock, never the reverse.
    pub(crate) project: Mutex<ProjectState>,
    /// The open documents other than the active one, the active one's id and the revision high-water mark (module
    /// `documents`). Locked only while the session lock (and possibly the project lock) is held, or with no other
    /// lock; never the reverse.
    pub(crate) documents: Mutex<crate::documents::DocSet>,
    /// The Art Library store under the projects directory (module `artlib`).
    pub art: crate::artlib::ArtStore,
    /// LightBurn's devices (field size of LightBurn imports, `GET /api/lightburn`). None until configured
    /// ([`LightBurn::configure`]); never locked while the session lock is held.
    pub lightburn: LightBurn,
    /// OpenEtch's devices and the active one ([`AppState::install_devices`]; until then only the built-in device for
    /// the machine's profile). Never locked while the session or runs lock is held.
    devices: Mutex<DeviceRegistry>,
    /// Job counter, Stop At and the progress of the job holding the machine (module `job`).
    pub job: JobTracker,
    /// Material libraries, preferences, job log, framing record, camera (module `libs`).
    pub libs: crate::libs::Libs,
    /// Autosave, backups, templates, preference export and bundles (module `userfiles`).
    pub files: crate::userfiles::UserFiles,
    /// The graceful shutdown's state (module `shutdown`).
    pub shutdown: crate::shutdown::ShutdownState,
    /// Change sets of the active document for clients (module `docsync`). Locked only while the session lock (and
    /// possibly the documents and project locks) is held, or with no other lock; never the reverse.
    doc_log: Mutex<crate::docsync::DocLog>,
    /// This engine run's id for change sets (module `docsync`): revisions start again after a restart.
    pub epoch: Arc<str>,
}

/// How an invocation changes which project the document belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replaces {
    /// It edits the current document (or does not edit).
    No,
    /// `doc.new`: a new, empty document that counts as clean.
    Clean,
    /// `doc.load` (an uploaded `.openetch`, unless it is a project being opened) or a replacing LightBurn import:
    /// a new document no project holds, so its content is unsaved.
    Unsaved,
}

fn replaces(inv: &Invocation) -> Replaces {
    match inv.id.as_str() {
        "doc.new" => return Replaces::Clean,
        "doc.load" => return Replaces::Unsaved,
        _ => {}
    }
    // `mode` defaults to replace; an invalid mode fails the command, so it never matters here.
    let mode = inv.params.get("mode").and_then(Value::as_str);
    if inv.id == "doc.import_lbrn2" && matches!(mode, None | Some("replace")) {
        return Replaces::Unsaved;
    }
    Replaces::No
}

/// `{"content": content}` without copying the content (`json!` would clone it).
fn content_params(content: String) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("content".into(), Value::String(content));
    Value::Object(params)
}

/// Commands that convert with (or start at) a field size and get one filled in when the caller gives none (see
/// [`AppState::batch_opening`]).
const NEEDS_FIELD: [&str; 3] = ["doc.new", "doc.import_lbrn2", "doc.export_lbrn2"];

/// File formats of the download and upload routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// The native `.openetch` JSON project.
    OpenEtch,
    /// A LightBurn `.lbrn2` project.
    Lbrn2,
}

impl FileFormat {
    pub fn parse(s: &str) -> Result<Self, EngineError> {
        match s {
            "openetch" => Ok(FileFormat::OpenEtch),
            "lbrn2" => Ok(FileFormat::Lbrn2),
            other => Err(EngineError::BadRequest(format!("unknown format '{other}' (expected openetch or lbrn2)"))),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            FileFormat::OpenEtch => PROJECT_EXT,
            FileFormat::Lbrn2 => LBRN2_EXT,
        }
    }
}

/// Formats `POST /api/document/upload` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadFormat {
    /// A native `.openetch` project: replaces the document (`doc.load`).
    OpenEtch,
    /// A LightBurn `.lbrn2` project: `doc.import_lbrn2`, replace or merge.
    Lbrn2,
    /// SVG art, added to the document (`art.import_svg`).
    Svg,
    /// DXF drawing, added to the document (`art.import_dxf`: one object per entity, in one group).
    Dxf,
    /// A bitmap (PNG, JPEG, ...), added to the document (`art.import_image`).
    Image,
}

impl UploadFormat {
    pub fn parse(s: &str) -> Result<Self, EngineError> {
        match s {
            "openetch" => Ok(UploadFormat::OpenEtch),
            "lbrn2" => Ok(UploadFormat::Lbrn2),
            "svg" => Ok(UploadFormat::Svg),
            "dxf" => Ok(UploadFormat::Dxf),
            "image" => Ok(UploadFormat::Image),
            other => Err(EngineError::BadRequest(format!(
                "unknown format '{other}' (expected openetch, lbrn2, svg, dxf or image)"
            ))),
        }
    }

    fn name(self) -> &'static str {
        match self {
            UploadFormat::OpenEtch => "openetch",
            UploadFormat::Lbrn2 => "lbrn2",
            UploadFormat::Svg => "svg",
            UploadFormat::Dxf => "dxf",
            UploadFormat::Image => "image",
        }
    }
}

/// The query parameters of an upload, checked against its format by [`AppState::upload`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UploadOptions {
    /// `mode`: `Some(true)` merge, `Some(false)` replace, `None` not given.
    pub merge: Option<bool>,
    /// LightBurn only: the field the file was laid out for.
    pub field_mm: Option<f64>,
    /// SVG, DXF and images: the layer to put the art on (default the first layer).
    pub layer: Option<String>,
    /// SVG, DXF and images: the new object's name.
    pub name: Option<String>,
    /// SVG, DXF and images: scale to this width in mm.
    pub width_mm: Option<f64>,
    /// Images only: the media type (default: sniffed from the file's first bytes).
    pub mime: Option<String>,
}

/// The media type of a bitmap from its first bytes, for the formats browsers and LightBurn commonly use.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if starts(b"BM") {
        Some("image/bmp")
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("image/tiff")
    } else {
        None
    }
}

/// UTF-8 text of an uploaded file without a byte-order mark (LightBurn and some editors write one). Moves the
/// bytes, never copies them.
fn upload_text(bytes: Vec<u8>, what: &str) -> Result<String, EngineError> {
    let mut text = String::from_utf8(bytes)
        .map_err(|_| EngineError::BadRequest(format!("the file is not UTF-8 text (expected {what})")))?;
    if text.starts_with('\u{feff}') {
        text.drain(..'\u{feff}'.len_utf8());
    }
    Ok(text)
}

/// The document as a file to download.
#[derive(Debug, Clone)]
pub struct DownloadFile {
    /// File name without extension: the project name, else the document name.
    pub stem: String,
    pub format: FileFormat,
    pub content: String,
    /// Conversion warnings (LightBurn export only).
    pub warnings: Vec<String>,
}

/// A random id for this engine run (module `docsync`); the time if no random source answers.
fn new_epoch() -> String {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        b = (t as u64).to_le_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Parse a request's `origin`: `"ui"`, `"script"`, `"agent"` / `"agent:<name>"`, or the serde form of
/// [`Origin`] (`{"kind": "agent", "name": "..."}`). Missing means [`Origin::Ui`].
pub fn parse_origin(v: Option<&Value>) -> Result<Origin, EngineError> {
    match v {
        None | Some(Value::Null) => Ok(Origin::Ui),
        Some(Value::String(s)) => match s.as_str() {
            "ui" => Ok(Origin::Ui),
            "script" => Ok(Origin::Script),
            "agent" => Ok(Origin::Agent("agent".into())),
            other => match other.strip_prefix("agent:") {
                Some(name) if !name.is_empty() => Ok(Origin::Agent(name.to_owned())),
                _ => Err(EngineError::BadRequest(format!(
                    "unknown origin '{other}' (expected \"ui\", \"script\" or \"agent:<name>\")"
                ))),
            },
        },
        Some(obj) => {
            serde_json::from_value(obj.clone()).map_err(|e| EngineError::BadRequest(format!("bad origin: {e}")))
        }
    }
}

impl AppState {
    pub fn new(
        session: Session,
        machine: Arc<Machine>,
        security: SecurityConfig,
        approval: ApprovalConfig,
        projects: ProjectDir,
    ) -> Arc<Self> {
        // A fresh session's document is laid out for this machine's field unless it says otherwise.
        let session = if session.document().field_mm.is_none() && session.history().is_empty() {
            Session::new(Document { field_mm: Some(machine.profile().field_mm), ..session.document().clone() })
        } else {
            session
        };
        let project = ProjectState::clean_at(session.document().revision);
        let devices = DeviceRegistry::in_memory(machine.profile(), machine.config().cor.clone());
        Arc::new(AppState {
            session: Mutex::new(session),
            machine,
            runs: Mutex::new(RunStore::default()),
            events: crate::events::channel(),
            security,
            approval,
            approval_mode: std::sync::OnceLock::new(),
            previews: Arc::default(),
            projects,
            project: Mutex::new(project),
            documents: Mutex::new(crate::documents::DocSet::default()),
            art: crate::artlib::ArtStore::default(),
            lightburn: LightBurn::default(),
            devices: Mutex::new(devices),
            job: JobTracker::default(),
            libs: crate::libs::Libs::default(),
            files: crate::userfiles::UserFiles::default(),
            shutdown: crate::shutdown::ShutdownState::default(),
            doc_log: Mutex::default(),
            epoch: new_epoch().into(),
        })
    }

    /// Use `registry` (opened from the config directory) for the devices. The machine must already be driven with
    /// the registry's active device's profile when that device is controllable.
    pub fn install_devices(&self, registry: DeviceRegistry) {
        *lock(&self.devices) = registry;
    }

    /// The device registry (do not hold it across other locks).
    pub fn devices(&self) -> MutexGuard<'_, DeviceRegistry> {
        lock(&self.devices)
    }

    /// The active device (a copy).
    pub fn active_device(&self) -> machine::Device {
        self.devices().active().clone()
    }

    /// Refused (422) while the active device is design-only.
    fn require_controllable(&self) -> Result<(), EngineError> {
        match self.active_device().not_controllable_reason() {
            Some(reason) => Err(EngineError::NotControllable(format!("{reason}. Make a JCZ galvo active in Devices to run or frame."))),
            None => Ok(()),
        }
    }

    /// Refused (409) unless the active machine is a Ruida gantry.
    pub(crate) fn require_ruida(&self) -> Result<(), EngineError> {
        self.require_controllable()?;
        if self.machine.is_ruida() { Ok(()) } else { Err(EngineError::Conflict("the active machine is not a Ruida gantry".into())) }
    }

    /// Refused (409) unless the active machine is a GRBL gantry.
    pub(crate) fn require_grbl(&self) -> Result<(), EngineError> {
        self.require_controllable()?;
        if self.machine.is_grbl() { Ok(()) } else { Err(EngineError::Conflict("the active machine is not a GRBL gantry".into())) }
    }

    /// `GET /api/devices`.
    pub fn devices_view(&self) -> Value {
        self.devices().view()
    }

    /// `GET /api/devices/{id}`.
    pub fn device_details(&self, id: &str) -> Result<Value, EngineError> {
        self.devices().details(id)
    }

    /// `POST /api/devices/import`: a `prefs.ini` or `.lbdev` as text. Emits `devices_changed`.
    pub fn import_devices(&self, text: &str, filter: &ImportFilter) -> Result<ImportReport, EngineError> {
        let (prefs, source) = crate::devices::parse_device_file(text).map_err(EngineError::BadRequest)?;
        let report = self.devices().import(&prefs, source, filter).map_err(|e| {
            if e.starts_with("the file has no device") { EngineError::NotFound(e) } else { EngineError::Internal(e) }
        })?;
        tracing::info!(added = report.added.len(), updated = report.updated.len(), skipped = report.skipped.len(), "imported devices");
        self.emit(Event::DevicesChanged);
        Ok(report)
    }

    /// `POST /api/devices/import?from=lightburn`: the devices of LightBurn's `prefs.ini` on the engine's computer (or
    /// the uploaded copy, see [`LightBurn`]). Emits `devices_changed`.
    pub fn import_devices_from_lightburn(&self, filter: &ImportFilter) -> Result<ImportReport, EngineError> {
        let (_, prefs) = self.lightburn.current().ok_or_else(|| {
            EngineError::NotFound("no LightBurn prefs.ini found on the engine's computer; import a .lbdev or prefs.ini file".into())
        })?;
        if prefs.devices.is_empty() {
            return Err(EngineError::NotFound("LightBurn's prefs.ini lists no devices".into()));
        }
        let report = self.devices().import(&prefs, machine::DeviceSource::LightburnPrefs, filter).map_err(|e| {
            if e.starts_with("the file has no device") { EngineError::NotFound(e) } else { EngineError::Internal(e) }
        })?;
        self.emit(Event::DevicesChanged);
        Ok(report)
    }

    /// `DELETE /api/devices/{id}`: not the active one, not the built-in one; its preferences, camera calibration and
    /// recorded run times (the estimate's correction) go with it. Emits `devices_changed`.
    pub fn remove_device(&self, id: &str) -> Result<(), EngineError> {
        self.devices().remove(id)?;
        self.libs.forget_device(id);
        self.job.times.forget(|d| d == id);
        self.emit(Event::DevicesChanged);
        Ok(())
    }

    /// Make device `id` active (see the module docs). Returns `{ok, active, warnings}`.
    pub async fn activate_device(self: &Arc<Self>, id: &str) -> Result<Value, EngineError> {
        self.refuse_if_shutting_down("changing the device")?;
        let device = self.devices().get(id).cloned().ok_or_else(|| EngineError::NotFound(format!("no device with id '{id}'")))?;
        // Claim the machine as a run would, so no run starts and no frame traces while the profile changes.
        {
            let mut runs = self.runs();
            if let Some(state) = runs.busy_state() {
                return Err(EngineError::Conflict(format!("a run is {state}; stop it or let it finish before changing the device")));
            }
            if self.machine.is_framing() {
                return Err(EngineError::Conflict("framing is on; stop framing before changing the device".into()));
            }
            if !self.machine.try_begin_run() {
                self.refuse_if_shutting_down("changing the device")?;
                return Err(EngineError::Conflict("a run is in progress; let it finish before changing the device".into()));
            }
            runs.cancel_pending("cancelled: the device changed");
        }
        struct Claim<'a>(&'a Machine);
        impl Drop for Claim<'_> {
            fn drop(&mut self) {
                self.0.end_run();
            }
        }
        let _claim = Claim(&self.machine);
        let mut warnings = Vec::new();
        // File I/O (the correction file, the index) on a blocking thread.
        let chosen = device.clone();
        let (cor, cor_warning, previous) = self
            .blocking(move |st| {
                let mut devices = st.devices();
                let (cor, warning) = devices.correction_for(&chosen);
                let previous = devices.active().id.clone();
                devices.set_active(&chosen.id)?;
                Ok((cor, warning, previous))
            })
            .await?;
        warnings.extend(cor_warning);
        /// What the machine becomes: a galvo profile (with its correction) or a GRBL gantry.
        enum Swap {
            Galvo(Box<machine::MachineProfile>, Option<corfile::CorTable>),
            Grbl(machine::grbl::GrblProfile),
            Ruida(machine::ruida::RuidaProfile),
        }
        let swap = match (device.profile.clone(), device.grbl.clone()) {
            _ if device.controllable && device.ruida.is_some() => device.ruida.clone().map(Swap::Ruida),
            (_, Some(grbl)) if device.controllable => Some(Swap::Grbl(grbl)),
            (Some(profile), _) if device.controllable => Some(Swap::Galvo(Box::new(profile), cor)),
            _ => None,
        };
        if let Some(swap) = swap {
            let before = self.machine.generation();
            let result = self
                .machine
                .blocking(move |m, ctl| {
                    m.bump_frame_gen();
                    match swap {
                        Swap::Galvo(profile, cor) => m.swap_config(ctl, *profile, cor),
                        Swap::Grbl(grbl) => m.swap_grbl(ctl, grbl),
                        Swap::Ruida(ruida) => m.swap_ruida(ctl, ruida),
                    }
                })
                .await;
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(e)) if self.machine.generation() != before => {
                    self.machine.set_error(Some(e.to_string()));
                    warnings.push(format!("the board did not initialise ({e}); framing and runs will retry"));
                }
                // Refused before anything changed (an invalid profile): stay on the previous device.
                Ok(Err(e)) => {
                    let _ = self.devices().set_active(&previous);
                    return Err(EngineError::Machine(e));
                }
                Err(e) => {
                    let _ = self.devices().set_active(&previous);
                    return Err(e);
                }
            }
        }
        // Runs planned while the profile changed belong to the previous device.
        self.runs().cancel_pending("cancelled: the device changed");
        tracing::info!(device = %device.id, controllable = device.controllable, "device activated");
        // The device's material library, when it is loaded (module `libs`).
        crate::libs::select_device_library(self);
        self.emit(Event::DeviceChanged { id: device.id.clone() });
        Ok(json!({ "ok": true, "active": self.devices().active().summary(), "warnings": warnings }))
    }

    /// `PUT /api/devices/{id}`: edit a device's settings (see `DeviceRegistry::edit`). Refused while the device is
    /// active and a run holds the machine or framing is on; when it is active the machine is set up again with the
    /// new profile (as on activation).
    pub async fn edit_device(self: &Arc<Self>, id: &str, patch: Option<Value>, display_name: Option<String>) -> Result<Value, EngineError> {
        self.check_device_editable(id)?;
        let target = id.to_owned();
        let d = self.blocking(move |st| st.devices().edit(&target, patch.as_ref(), display_name.as_deref())).await?;
        let mut v = self.device_settings_changed(id).await?;
        v["device"] = d.summary();
        Ok(v)
    }

    /// Refuse (409) changing the stored settings of the active device while a job holds the machine or framing is
    /// on (503 while the engine shuts down): the change would be stored but the machine could not be set up again
    /// with it. Checked before anything is written (`PUT /api/devices/{id}`, its correction file, an applied lens calibration).
    pub fn check_device_editable(&self, id: &str) -> Result<(), EngineError> {
        if self.active_device().id != id {
            return Ok(());
        }
        // The active device is set up again after the change (the board too): refused before anything is written.
        self.refuse_if_shutting_down("changing the active device's settings")?;
        if let Some(state) = self.runs().busy_state() {
            return Err(EngineError::Conflict(format!("a run is {state}; change the active device's settings when it has ended")));
        }
        if self.machine.is_running() {
            return Err(EngineError::Conflict("a job holds the machine; change the active device's settings when it has ended".into()));
        }
        if self.machine.is_framing() {
            return Err(EngineError::Conflict("framing is on; stop framing before changing the active device's settings".into()));
        }
        Ok(())
    }

    /// After a device's stored settings changed: emit `devices_changed`, and set the machine up again when it is the
    /// active device. Returns `{ok, reactivated, warnings}`.
    pub async fn device_settings_changed(self: &Arc<Self>, id: &str) -> Result<Value, EngineError> {
        self.emit(Event::DevicesChanged);
        if self.active_device().id != id {
            return Ok(json!({ "ok": true, "reactivated": false, "warnings": [] }));
        }
        let r = self.activate_device(id).await?;
        Ok(json!({ "ok": true, "reactivated": true, "warnings": r["warnings"] }))
    }

    /// A receiver for every future event.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Send an event (dropped when nobody listens).
    pub fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    pub(crate) fn runs(&self) -> MutexGuard<'_, RunStore> {
        lock(&self.runs)
    }

    /// Every registered document command.
    pub fn specs(&self) -> Vec<CommandSpec> {
        lock(&self.session).registry().specs().cloned().collect()
    }

    /// A copy of the current document.
    pub fn document(&self) -> Document {
        lock(&self.session).document().clone()
    }

    pub fn revision(&self) -> u64 {
        lock(&self.session).document().revision
    }

    /// `GET /api/document`: a copy of the current document, logged so a client holding it can later ask for the
    /// changes since (module `docsync`).
    pub fn document_for_client(&self) -> Document {
        let s = lock(&self.session);
        let id = self.active_document_id();
        lock(&self.doc_log).observe(s.document(), &id);
        s.document().clone()
    }

    /// `GET /api/document/sync` (and the `sync` member of mutating answers): the changes since `since` (`(epoch,
    /// revision)`) when the log still reaches back to it, else the whole document; with the timeline and the project
    /// status of the same revision (module `docsync`).
    pub fn doc_sync(&self, since: Option<(String, u64)>) -> crate::docsync::SyncPayload {
        let s = lock(&self.session);
        let document_id = self.active_document_id();
        let revision = s.document().revision;
        let changes = {
            let mut log = lock(&self.doc_log);
            log.observe(s.document(), &document_id);
            since.filter(|(epoch, _)| **epoch == *self.epoch).and_then(|(_, r)| log.since(r))
        };
        let document = changes.is_none().then(|| s.document().clone());
        let history = s.history_view();
        let project = {
            let p = lock(&self.project);
            json!({
                "name": p.name,
                "display_name": p.display_name,
                "saved_revision": p.saved_revision,
                "revision": revision,
                "dirty": p.dirty(revision),
            })
        };
        drop(s);
        crate::docsync::SyncPayload { epoch: self.epoch.clone(), document_id, revision, changes, document, history, project }
    }

    /// How runs are approved: the `--approval` mode set at startup, else [`ApprovalMode::Pin`].
    pub fn approval_mode(&self) -> ApprovalMode {
        self.approval_mode.get().copied().unwrap_or_default()
    }

    /// Set the approval mode, once, when the engine starts (`main`); `Err` with the mode already set otherwise. Nothing
    /// reachable from the API or MCP calls this, so a running engine never changes how runs are approved.
    pub fn set_approval_mode(&self, mode: ApprovalMode) -> Result<(), ApprovalMode> {
        self.approval_mode.set(mode).map_err(|_| self.approval_mode())
    }

    /// Every step on the timeline (file contents in parameters shortened).
    pub fn history(&self) -> Vec<HistoryStep> {
        self.history_view().steps
    }

    /// `GET /api/history`: the marker and every step.
    pub fn history_view(&self) -> HistoryView {
        lock(&self.session).history_view()
    }

    /// Announce a document change (when the revision moved) and the timeline (always) of open document `document_id`.
    pub(crate) fn announce(&self, before: u64, after: u64, origin: Origin, (marker, steps): (usize, usize), document_id: String) {
        if after != before {
            self.emit(Event::DocumentChanged { revision: after, origin, document_id: document_id.clone() });
        }
        self.emit(Event::HistoryChanged { marker, steps, revision: after, document_id });
    }

    /// The active document's id (`default` for the first one; see `GET /api/documents`).
    pub fn active_document_id(&self) -> String {
        lock(&self.documents).active.clone()
    }

    /// Roll the timeline to `marker` (0 = before the first step). Returns `{ok, marker, revision}`.
    pub fn history_goto(&self, marker: usize, origin: Origin) -> Result<Value, EngineError> {
        self.timeline_op(origin, |s| s.goto(marker))
    }

    /// Delete timeline step `index` (1-based) and replay the steps after it. Returns `{ok, marker, revision}`.
    pub fn history_delete(&self, index: usize, origin: Origin) -> Result<Value, EngineError> {
        self.timeline_op(origin, |s| s.delete_step(index))
    }

    fn timeline_op(
        &self,
        origin: Origin,
        op: impl FnOnce(&mut Session) -> Result<u64, commands::CommandError>,
    ) -> Result<Value, EngineError> {
        let (before, after, hist, doc_id) = {
            let mut s = lock(&self.session);
            let before = s.document().revision;
            let after = op(&mut s)?;
            (before, after, (s.marker(), s.history().len()), self.active_document_id())
        };
        self.announce(before, after, origin, hist, doc_id);
        Ok(json!({ "ok": true, "marker": hist.0, "revision": after }))
    }

    /// Run one command (see [`AppState::batch`]).
    pub fn execute(&self, inv: Invocation, origin: Origin) -> Result<Outcome, EngineError> {
        self.batch(vec![inv], origin).map(|mut v| v.remove(0))
    }

    /// Run commands as one undo step, all-or-nothing. Only `Query` and `Edit` commands are accepted here: machine
    /// actions have their own endpoints and tools.
    pub fn batch(&self, invs: Vec<Invocation>, origin: Origin) -> Result<Vec<Outcome>, EngineError> {
        self.batch_opening(invs, origin, None, None)
    }

    /// [`AppState::batch`]; with `opened`, a successful edit makes `opened` the current project, saved at the new
    /// revision (project open). A replacing edit with no project records `display` (or the `name` of a replacing
    /// `doc.import_lbrn2`) as the name downloads get. The project state changes before `document_changed` is sent,
    /// so a client that refetches `GET /api/project` on the event sees it.
    ///
    /// Field sizes: `doc.new` without `field_mm` gets the machine profile's; `doc.import_lbrn2` without one gets the
    /// LightBurn device's ([`LightBurn::field_for_import`]), else the document's, else the machine profile's, and
    /// its result's `field_source` says which.
    pub(crate) fn batch_opening(
        &self,
        mut invs: Vec<Invocation>,
        origin: Origin,
        opened: Option<String>,
        display: Option<String>,
    ) -> Result<Vec<Outcome>, EngineError> {
        if invs.is_empty() {
            return Err(EngineError::BadRequest("batch has no commands".into()));
        }
        // Saved layer / optimization defaults of the active device (module `libs`).
        crate::libs::fill_defaults(self, &mut invs);
        let profile_field = self.machine.profile().field_mm;
        // Filled in before the session lock: the LightBurn lookup may read prefs.ini.
        let mut field_sources: Vec<(usize, String, Option<String>)> = Vec::new();
        if invs.iter().any(|i| matches!(i.id.as_str(), "doc.new" | "doc.import_lbrn2")) {
            let active = self.active_device();
            for (i, inv) in invs.iter_mut().enumerate() {
                match inv.id.as_str() {
                    "doc.new" if lacks_field(inv) && lacks(inv, "workspace_mm") => {
                        let field = if active.controllable { profile_field } else { active.field_mm() };
                        fill_layout(inv, &new_layout(&active, field));
                    }
                    "doc.import_lbrn2" if lacks_field(inv) && lacks(inv, "workspace_mm") => {
                        let content = inv.params.get("content").and_then(Value::as_str).unwrap_or_default();
                        let named = commands::lbrn2_device_name(content);
                        let device = named.as_deref().and_then(|n| self.devices().find(n).cloned());
                        if let Some(d) = device {
                            fill_layout(inv, &lightburn_layout(&d));
                            field_sources.push((i, format!("file device {}", d.display_name), Some(d.id)));
                        } else if let Some((f, source)) = self.lightburn.field_for_file_device(content) {
                            fill_field_mm(inv, f);
                            field_sources.push((i, source, None));
                        } else {
                            fill_layout(inv, &lightburn_layout(&active));
                            field_sources.push((i, format!("active device {}", active.display_name), Some(active.id.clone())));
                        }
                    }
                    _ => {}
                }
            }
        }
        let display = display.or_else(|| {
            invs.iter()
                .rev()
                .find(|i| i.id == "doc.import_lbrn2" && replaces(i) == Replaces::Unsaved)
                .and_then(|i| i.params.get("name").and_then(Value::as_str).and_then(projects::display_name))
        });
        let (mut outcomes, before, after, hist, doc_id) = {
            let mut s = lock(&self.session);
            let mut last_edit = Replaces::No;
            for inv in &invs {
                if let Some(spec) = s.registry().spec(&inv.id) {
                    if !matches!(spec.risk, Risk::Query | Risk::Edit) {
                        return Err(EngineError::Forbidden(format!(
                            "'{}' is a machine action; use the machine endpoints/tools instead",
                            inv.id
                        )));
                    }
                    if spec.risk == Risk::Edit {
                        last_edit = replaces(inv);
                    }
                }
            }
            if s.document().field_mm.is_none() {
                for (i, inv) in invs.iter_mut().enumerate() {
                    if lacks(inv, "workspace_mm") {
                        if inv.id == "doc.import_lbrn2" && lacks_field(inv) {
                            field_sources.push((i, "machine".into(), None));
                        }
                        fill_field_mm(inv, profile_field);
                    }
                }
            }
            let replaced = invs.iter().any(|i| replaces(i) != Replaces::No);
            let before = s.document().revision;
            let outcomes = s.batch(invs, origin.clone())?;
            let after = s.document().revision;
            if after != before {
                let mut p = lock(&self.project);
                if let Some(name) = opened {
                    *p = ProjectState::saved(name, after);
                } else if replaced {
                    // Clean only after doc.new / doc.load with nothing edited after it in the same batch.
                    let clean = last_edit == Replaces::Clean;
                    *p = ProjectState {
                        name: None,
                        saved_revision: None,
                        clean_revision: clean.then_some(after),
                        display_name: if clean { None } else { display },
                    };
                }
            }
            if s.document().field_mm.is_none() {
                // Cannot happen once the engine starts with a field size (doc.new and doc.load keep it, imports set
                // it, and undo only returns to documents that had one), so it is only reported.
                tracing::warn!(revision = after, "the document has no field_mm");
            }
            (outcomes, before, after, (s.marker(), s.history().len()), self.active_document_id())
        };
        for (i, source, device) in field_sources {
            if let Some(Value::Object(r)) = outcomes.get_mut(i).map(|o| &mut o.result) {
                r.insert("field_source".into(), Value::String(source));
                if let Some(id) = device {
                    r.insert("device".into(), Value::String(id));
                }
            }
        }
        if after != before {
            self.announce(before, after, origin, hist, doc_id);
        }
        Ok(outcomes)
    }

    /// `GET /api/project`: `{name, display_name, saved_revision, revision, dirty}`; `display_name` is the opened
    /// file's name while no project holds the document (else null).
    pub fn project_status(&self) -> Value {
        let s = lock(&self.session);
        let revision = s.document().revision;
        let p = lock(&self.project);
        json!({
            "name": p.name,
            "display_name": p.display_name,
            "saved_revision": p.saved_revision,
            "revision": revision,
            "dirty": p.dirty(revision),
        })
    }

    /// The current project name, if any.
    pub fn project_name(&self) -> Option<String> {
        lock(&self.project).name.clone()
    }

    /// Every saved project.
    pub async fn project_list(&self) -> Result<Vec<ProjectInfo>, EngineError> {
        let dir = self.projects.clone();
        tokio::task::spawn_blocking(move || dir.list()).await?
    }

    /// Run `f` with the state on a blocking thread: document commands can take seconds (a 50 MB import), which
    /// must not stall the async runtime.
    pub async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&AppState) -> Result<T, EngineError> + Send + 'static,
    ) -> Result<T, EngineError> {
        let st = self.clone();
        tokio::task::spawn_blocking(move || f(&st)).await?
    }

    /// Save the document as project `name`. Unless `overwrite`, an existing project other than the current one is
    /// refused (409), also when its name differs only in case. Emits `project_saved`. Returns
    /// `{ok, name, path, revision}`.
    pub async fn project_save(&self, name: &str, overwrite: bool) -> Result<Value, EngineError> {
        projects::validate_project_name(name)?;
        // The document, which open document it is and its project, read together: another document may become the
        // active one at any moment, and its project must not decide whether this save may replace a file.
        let (content, revision, doc_id, current) = {
            let s = lock(&self.session);
            let current = lock(&self.project).name.clone();
            (saveable(s.document())?, s.document().revision, self.active_document_id(), current)
        };
        let dir = self.projects.clone();
        let owned = name.to_owned();
        let keep = self.files.settings().backups_keep;
        let path = tokio::task::spawn_blocking(move || {
            if !overwrite {
                refuse_clobber(&dir, &owned, PROJECT_EXT, current.as_deref())?;
            }
            // The file being replaced goes to <projects-dir>/.backups first (module `userfiles`).
            crate::userfiles::backup_before_save(&dir, &owned, keep);
            dir.write(&owned, PROJECT_EXT, &content)
        })
        .await??;
        // The document saved may have stopped being the active one while the file was written.
        self.set_project_of(&doc_id, ProjectState::saved(name.to_owned(), revision));
        tracing::info!(name, revision, path = %path.display(), "project saved");
        self.emit(Event::ProjectSaved { name: name.to_owned(), revision, document_id: doc_id });
        Ok(json!({ "ok": true, "name": name, "path": path.display().to_string(), "revision": revision }))
    }

    /// Open project `name`: replaces the document (`doc.load`, one undo step) and makes it the current project. A
    /// name that differs only in case finds the project when exactly one matches; the project keeps the name it
    /// has on disk. Returns `{ok, name, result, revision}`.
    pub async fn project_open(self: &Arc<Self>, name: &str, origin: Origin) -> Result<Value, EngineError> {
        projects::validate_project_name(name)?;
        let owned = name.to_owned();
        let (name, o) = self
            .blocking(move |st| {
                let (name, content) = st.projects.read(&owned)?;
                let inv = Invocation { id: "doc.load".into(), params: content_params(content) };
                let o = st.batch_opening(vec![inv], origin, Some(name.clone()), None)?.remove(0);
                Ok((name, o))
            })
            .await?;
        Ok(json!({ "ok": true, "name": name, "result": o.result, "revision": o.revision }))
    }

    /// Export the document as LightBurn to `<projects-dir>/<name>.lbrn2`, at the document's field size. Unless
    /// `overwrite`, an existing `.lbrn2` of that name (ignoring case) is refused (409). Returns
    /// `{ok, path, warnings, field_mm}`.
    pub async fn project_export_lbrn2(
        &self,
        name: &str,
        overwrite: bool,
        origin: Origin,
    ) -> Result<Value, EngineError> {
        projects::validate_project_name(name)?;
        let mut o = self.execute(Invocation { id: "doc.export_lbrn2".into(), params: json!({}) }, origin)?;
        let content = match o.result["content"].take() {
            Value::String(s) => s,
            _ => String::new(),
        };
        let dir = self.projects.clone();
        let owned = name.to_owned();
        let path = tokio::task::spawn_blocking(move || {
            if !overwrite {
                refuse_clobber(&dir, &owned, LBRN2_EXT, None)?;
            }
            dir.write(&owned, LBRN2_EXT, &content)
        })
        .await??;
        Ok(json!({
            "ok": true,
            "path": path.display().to_string(),
            "warnings": o.result["warnings"],
            "field_mm": o.result["field_mm"],
        }))
    }

    /// The document as a file in `format` (LightBurn: `doc.export_lbrn2` at the document's field size).
    pub fn download(&self, format: FileFormat) -> Result<DownloadFile, EngineError> {
        let (content, warnings, name) = match format {
            FileFormat::OpenEtch => {
                let s = lock(&self.session);
                (saveable(s.document())?, Vec::new(), s.document().name.clone())
            }
            FileFormat::Lbrn2 => {
                let o = self.execute(Invocation { id: "doc.export_lbrn2".into(), params: json!({}) }, Origin::Ui)?;
                let warnings = serde_json::from_value(o.result["warnings"].clone()).unwrap_or_default();
                let content = o.result["content"].as_str().unwrap_or_default().to_owned();
                (content, warnings, lock(&self.session).document().name.clone())
            }
        };
        let stem = {
            let p = lock(&self.project);
            p.name.clone().or_else(|| p.display_name.clone()).unwrap_or(name)
        };
        Ok(DownloadFile { stem, format, content, warnings })
    }

    /// Run an uploaded file (the raw bytes): `.openetch` through `doc.load` (replace only), `.lbrn2` through
    /// `doc.import_lbrn2` (`replace` or `merge`, optional `field_mm`), SVG through `art.import_svg`, DXF through
    /// `art.import_dxf` and bitmaps through `art.import_image` (merge only, optional `layer`, `name`, `width_mm`;
    /// images also `mime`, else sniffed). Options that do not apply to the format are refused (400). `name` on an
    /// `.openetch` or replacing `.lbrn2` upload is the file's name, used for later downloads until the document is
    /// saved as a project (a merging `.lbrn2` names its group after it).
    pub fn upload(
        &self,
        format: UploadFormat,
        opts: UploadOptions,
        bytes: Vec<u8>,
        origin: Origin,
    ) -> Result<Outcome, EngineError> {
        let refuse = |what: &str, only: &str| {
            Err(EngineError::BadRequest(format!("{what} is only for {only} uploads, not {}", format.name())))
        };
        let art = matches!(format, UploadFormat::Svg | UploadFormat::Dxf | UploadFormat::Image);
        let display = match format {
            UploadFormat::OpenEtch | UploadFormat::Lbrn2 => opts.name.as_deref().and_then(projects::display_name),
            _ => None,
        };
        if !art {
            if opts.layer.is_some() {
                return refuse("layer", "svg, dxf and image");
            }
            if opts.width_mm.is_some() {
                return refuse("width_mm", "svg, dxf and image");
            }
        }
        if format != UploadFormat::Image && opts.mime.is_some() {
            return refuse("mime", "image");
        }
        if format != UploadFormat::Lbrn2 && opts.field_mm.is_some() {
            return refuse("field_mm", "LightBurn");
        }
        let inv = match format {
            UploadFormat::OpenEtch => {
                if opts.merge == Some(true) {
                    return Err(EngineError::BadRequest(
                        "mode=merge is only for LightBurn imports; an .openetch project replaces the document".into(),
                    ));
                }
                let text = upload_text(bytes, ".openetch JSON or .lbrn2 XML")?;
                Invocation { id: "doc.load".into(), params: content_params(text) }
            }
            UploadFormat::Lbrn2 => {
                let mut params = content_params(upload_text(bytes, ".openetch JSON or .lbrn2 XML")?);
                params["mode"] = json!(if opts.merge == Some(true) { "merge" } else { "replace" });
                if let Some(f) = opts.field_mm {
                    params["field_mm"] = json!(f);
                }
                if let Some(n) = &opts.name {
                    params["name"] = json!(n);
                }
                Invocation { id: "doc.import_lbrn2".into(), params }
            }
            UploadFormat::Svg | UploadFormat::Dxf | UploadFormat::Image => {
                if opts.merge == Some(false) {
                    return Err(EngineError::BadRequest(format!(
                        "{} uploads add to the document; use mode=merge or leave mode out",
                        format.name()
                    )));
                }
                let mut params = serde_json::Map::new();
                let id = match format {
                    UploadFormat::Svg => {
                        params.insert("svg".into(), Value::String(upload_text(bytes, "SVG")?));
                        "art.import_svg"
                    }
                    UploadFormat::Dxf => {
                        params.insert("content".into(), Value::String(upload_text(bytes, "DXF")?));
                        "art.import_dxf"
                    }
                    _ => {
                        let mime = match opts.mime {
                            Some(m) => m,
                            None => sniff_image(&bytes)
                                .ok_or_else(|| {
                                    EngineError::BadRequest(
                                        "not an image the engine recognises (PNG, JPEG, GIF, BMP, WebP or TIFF); pass \
                                         mime=image/... if it is one"
                                            .into(),
                                    )
                                })?
                                .to_owned(),
                        };
                        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        drop(bytes);
                        params.insert("data_base64".into(), Value::String(data));
                        params.insert("mime".into(), Value::String(mime));
                        "art.import_image"
                    }
                };
                if let Some(l) = opts.layer {
                    params.insert("layer".into(), Value::String(l));
                }
                if let Some(n) = opts.name {
                    params.insert("name".into(), Value::String(n));
                }
                if let Some(w) = opts.width_mm {
                    params.insert("width_mm".into(), json!(w));
                }
                Invocation { id: id.into(), params: Value::Object(params) }
            }
        };
        self.batch_opening(vec![inv], origin, None, display).map(|mut v| v.remove(0))
    }

    /// Undo: move the timeline marker back over the last applied edit (by anyone); returns the new revision.
    /// `origin` is who asked, for the event.
    pub fn undo(&self, origin: Origin) -> Result<u64, EngineError> {
        self.timeline_op(origin, Session::undo).map(|v| v["revision"].as_u64().unwrap_or_default())
    }

    /// Redo: move the marker forward over the next rolled-back edit; returns the new revision.
    pub fn redo(&self, origin: Origin) -> Result<u64, EngineError> {
        self.timeline_op(origin, Session::redo).map(|v| v["revision"].as_u64().unwrap_or_default())
    }

    /// Plan the current document (no encoding). An empty document previews as nothing rather than an error.
    ///
    /// Coalesced and cached ([`PreviewGate`]): a result planned from the same revision, job counter, time correction
    /// and machine target is reused; otherwise at most one plan runs at a time, and a request that waited for a
    /// running plan reuses its result if it is current, else plans the current document once for every request
    /// queued with it, so a burst of edits never queues one plan per edit (the newest revision wins). The result can
    /// therefore be of a newer revision than the one current when the request arrived (never an older one).
    pub async fn preview(&self) -> Result<PreviewResult, EngineError> {
        Ok(self.preview_cached().await?.result.clone())
    }

    /// What a preview of revision `revision` would be planned with now.
    fn preview_key(&self, revision: u64) -> PreviewKey {
        let device = self.active_device();
        PreviewKey {
            revision,
            // The next run's index (the job counter), as `request_run_with` plans it: auto-rotating fills preview at
            // the angle they will be marked at, and Start Here segment numbers match the run.
            run_index: self.job.count(&device.id),
            // The device's learned estimate correction (module `marktime`), applied to every time of the preview.
            correction: self.job.times.correction(&device.id).factor.to_bits(),
            target: self.machine.target(),
            design_only: device.not_controllable_reason(),
        }
    }

    /// The cached preview, if it is of the current document and settings.
    fn current_preview(&self) -> Option<Arc<CachedPreview>> {
        let key = self.preview_key(self.revision());
        lock(&self.previews.latest).as_ref().filter(|c| c.key == key).cloned()
    }

    async fn preview_cached(&self) -> Result<Arc<CachedPreview>, EngineError> {
        if let Some(c) = self.current_preview() {
            return Ok(c);
        }
        let running = self.previews.running.clone().lock_owned().await;
        if let Some(c) = self.current_preview() {
            return Ok(c);
        }
        let doc = self.document();
        let key = self.preview_key(doc.revision);
        let target = key.target.clone();
        let (design_only, run_index) = (key.design_only.clone(), key.run_index);
        let correction = f64::from_bits(key.correction);
        let shared = self.previews.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let preview = match design_only {
                // Still a result, so the SVG preview renders the document.
                Some(reason) => Err(PlanError::Target(reason)),
                None => match plan::plan_job_with(&doc, &target, &plan::JobOptions { preview: true, run_index, ..Default::default() }) {
                    Ok(job) => {
                        let mut p = plan::preview_document(&doc, &job);
                        plan::scale_times(&mut p, correction);
                        Ok(p)
                    }
                    Err(PlanError::Empty) => Ok(Preview::default()),
                    Err(e) => Err(e),
                },
            };
            let cached = Arc::new(CachedPreview { key, result: Arc::new((doc, preview)), bins: Mutex::default() });
            shared.computed.fetch_add(1, Ordering::SeqCst);
            // Published before the slot is released, so the next queued request finds it.
            *lock(&shared.latest) = Some(cached.clone());
            drop(running);
            cached
        })
        .await?)
    }

    /// `GET /api/preview.bin`: the preview encoded (module `preview_bin`) and compressed with `encoding` (`br`,
    /// `gzip` or none), made once per revision and request options. Returns the body and the revision it is of.
    pub async fn preview_bin(
        &self,
        opts: crate::preview_bin::BinOptions,
        encoding: Option<&'static str>,
    ) -> Result<(EncodedBody, u64), EngineError> {
        let cached = self.preview_cached().await?;
        if let Err(e) = &cached.result.1 {
            return Err(EngineError::Plan(e.clone()));
        }
        let key = format!("{}|{}", opts.key(), encoding.unwrap_or("identity"));
        let cell = {
            let mut bins = lock(&cached.bins);
            match bins.iter().find(|(k, _)| *k == key) {
                Some((_, c)) => c.clone(),
                None => {
                    let c = Arc::new(std::sync::OnceLock::new());
                    bins.push((key, c.clone()));
                    if bins.len() > MAX_BIN_VARIANTS {
                        bins.remove(0);
                    }
                    c
                }
            }
        };
        let revision = cached.key.revision;
        if let Some(body) = cell.get() {
            return Ok((body.clone(), revision));
        }
        let body = tokio::task::spawn_blocking(move || {
            cell.get_or_init(|| {
                let (doc, preview) = &*cached.result;
                let p = preview.as_ref().expect("a failed plan returned above");
                let raw = crate::preview_bin::encode(doc, p, revision, &opts);
                let (bytes, encoding) = crate::preview_bin::compress(raw, encoding);
                EncodedBody { bytes: bytes.into(), encoding }
            })
            .clone()
        })
        .await?;
        Ok((body, revision))
    }

    /// `GET /api/preview/stats`: the preview without its segments (stats, passes, layers, warnings), its segment count
    /// and the revision it is of.
    pub async fn preview_stats(&self) -> Result<Value, EngineError> {
        let cached = self.preview_cached().await?;
        let (doc, preview) = &*cached.result;
        let p = preview.as_ref().map_err(|e| EngineError::Plan(e.clone()))?;
        let mut v = crate::preview_bin::summary(doc, p, None);
        v["revision"] = json!(cached.key.revision);
        v["segments"] = json!(p.segments.len());
        Ok(v)
    }

    /// The even-odd fill regions of every fill layer of the current document (display only).
    pub async fn fill_regions(&self) -> Result<FillRegions, EngineError> {
        let doc = self.document();
        Ok(tokio::task::spawn_blocking(move || plan::fill_regions(&doc)).await?)
    }

    /// Hold the preview slot as a running plan would (tests).
    #[cfg(test)]
    pub(crate) async fn hold_preview_slot(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.previews.running.clone().lock_owned().await
    }

    /// How many previews have been planned (tests).
    #[cfg(test)]
    pub(crate) fn previews_computed(&self) -> usize {
        self.previews.computed.load(Ordering::SeqCst)
    }

    /// The preview as SVG. A plan error still renders the document (without toolpath); the error is returned
    /// alongside.
    pub async fn preview_svg(&self, opts: RenderOptions) -> Result<(String, PreviewResult), EngineError> {
        let result = self.preview().await?;
        let (doc, preview) = &*result;
        let field = self.machine.profile().field_mm;
        let svg = render::preview_svg(doc, field, preview.as_ref().ok(), opts);
        Ok((svg, result))
    }

    /// After a run finished: advance the serial number / CSV row when the document's auto-advance is on and some
    /// variable text uses it (LightBurn advances on every Start). One undo step from origin `script`; a failure is
    /// logged, never fatal to the run. Called when a run ends in `finished`.
    pub fn advance_variables_after_run(&self) {
        if !commands::auto_advance_due(lock(&self.session).document()) {
            return;
        }
        let inv = Invocation { id: "var.advance".into(), params: json!({ "auto": true }) };
        if let Err(e) = self.execute(inv, Origin::Script) {
            tracing::warn!(error = %e, "could not advance variable text after the run");
        }
    }

    /// Plan, encode and check the current document and store it as a pending run. Never touches the machine.
    pub async fn request_run(&self, origin: Origin) -> Result<RunView, EngineError> {
        self.request_run_with(origin, RunOptions::default()).await
    }

    /// [`AppState::request_run`] with [`RunOptions`] (repeat, continuous, start input, selection, start here). The
    /// active device's job count is the run index for auto-rotating fills.
    pub async fn request_run_with(&self, origin: Origin, options: RunOptions) -> Result<RunView, EngineError> {
        self.request_run_at(origin, options, None).await
    }

    /// [`AppState::request_run_with`] of the document at revision `shown` (the one the app shows): at another
    /// revision nothing is planned (409), so a Start Here segment counted on an older preview never starts a run of a
    /// newer document.
    pub async fn request_run_at(&self, origin: Origin, options: RunOptions, shown: Option<u64>) -> Result<RunView, EngineError> {
        self.refuse_if_shutting_down("requesting a run")?;
        self.require_controllable()?;
        // The document and which open document it is, read together (module `documents`).
        let (doc, document_id) = {
            let s = lock(&self.session);
            (s.document().clone(), self.active_document_id())
        };
        let revision = doc.revision;
        if let Some(shown) = shown
            && shown != revision
        {
            return Err(EngineError::Conflict(format!(
                "the document changed since the run was requested (revision {shown} -> {revision}); nothing was requested: check the job and request it again"
            )));
        }
        let config = self.machine.config();
        let generation = config.generation;
        let run_index = self.job.count(&self.active_device().id);
        // The active device's keep-out zones go with every run planned on it (module `zones`).
        let mut options = options;
        options.device_keep_out = crate::zones::device_zones(self);
        let correction = self.job.times.correction(&self.active_device().id).factor;
        // Batch: the queued projects are read now and planned on their own (module `runs`).
        let queue = match &options.queue {
            Some(names) => Some(self.read_queue(names).await?),
            None => None,
        };
        let mut prepared = tokio::task::spawn_blocking(move || {
            if let Some(ruida) = &config.ruida {
                return crate::ruida_dsp::prepare_ruida_job(&doc, &config.target, ruida, &options, run_index);
            }
            if let Some(grbl) = &config.grbl {
                if queue.is_some() {
                    return Err(EngineError::BadRequest("queue (batch of projects) is not available on a GRBL machine yet".into()));
                }
                let doc = commands::prepare_output(&doc).map_err(|e| EngineError::BadRequest(format!("text: {e}")))?;
                return runs::prepare_gcode_job(&doc, &config.target, grbl, &options, run_index);
            }
            if let Some(docs) = queue {
                return runs::prepare_queue(&docs, &config.target, &config.profile, &options, run_index);
            }
            // Variable text (serial numbers, CSV rows, dates, cut settings) and text on paths as of now.
            let doc = commands::prepare_output(&doc).map_err(|e| EngineError::BadRequest(format!("text: {e}")))?;
            runs::prepare_job_with(&doc, &config.target, &config.profile, &options, run_index)
        })
        .await??
        .with_correction(correction);
        prepared.generation = generation;
        let view = {
            let mut store = self.runs();
            // Under the runs lock the shutdown cancels pending runs under: none is added after that.
            self.refuse_if_shutting_down("requesting a run")?;
            if self.machine.generation() != generation {
                return Err(EngineError::Conflict("the device changed while the run was planned; request it again".into()));
            }
            let run = store.add(origin.clone(), revision, prepared)?;
            run.document_id = document_id;
            run.view(revision)
        };
        tracing::info!(run_id = %view.id, ?origin, "run requested; waiting for approval in the UI");
        self.emit(Event::RunPending { run_id: view.id.clone(), origin, summary: view.summary.clone() });
        Ok(view)
    }

    /// The saved projects of a queued run, read and parsed (unknown names 404, unusable files 400).
    async fn read_queue(&self, names: &[String]) -> Result<Vec<(String, Document)>, EngineError> {
        let dir = self.projects.clone();
        let names = names.to_vec();
        tokio::task::spawn_blocking(move || {
            names
                .iter()
                .map(|n| {
                    let (name, text) = dir.read(n)?;
                    let doc = doc::file::from_openetch_json(&text)
                        .map_err(|e| EngineError::BadRequest(format!("project \"{name}\": {e}")))?;
                    Ok((name, doc))
                })
                .collect()
        })
        .await?
    }

    /// `GET /api/machine`: machine status, how runs are approved, and pending and recent runs.
    pub fn machine_status(&self) -> Value {
        let rev = self.revision();
        let mut v = self.machine.status_json();
        v["device"] = self.devices().active().summary();
        v["approval"] = self.approval.status_json();
        // `mode`: "pin" or "click" (no PIN: Start in the app or the foot switch); `foot_switch`: the input that
        // drives the foot switch cycle in the click mode, or null.
        v["approval"]["mode"] = json!(self.approval_mode().as_str());
        v["approval"]["foot_switch"] = json!(
            crate::approve::foot_switch_port(self).map(|p| json!({ "input_bit": p.bit, "active_low": p.active_low }))
        );
        // The foot switch cycle (frame, burn what was framed, frame again; module `cycle`).
        v["foot_cycle"] = crate::cycle::status_json(self);
        // A graceful shutdown (module `shutdown`): new work is refused while a running job is waited for.
        v["shutting_down"] = json!(self.shutting_down());
        v["shutdown"] = self.shutdown.status_json(self.job.active_run());
        // Whether framing can light a focus pointer (`pointer: focus | both`): a controllable active device whose
        // profile names a usable focus light output.
        v["focus_light"] = json!(
            self.devices().active().not_controllable_reason().is_none()
                && self.machine.profile().focus_light_port_bit.is_some_and(|b| lmc::PointerBits::bit(b).is_ok())
        );
        v["lightburn"] = self.lightburn.machine_json(&self.machine.profile());
        let store = self.runs();
        v["pending_runs"] = json!(store.pending(rev));
        v["runs"] = json!(store.views(rev));
        drop(store);
        v["job"] = self.job_status();
        v
    }

    /// `GET /api/job`: the job counter of the active device, Stop At, and the progress of the job holding the
    /// machine.
    pub fn job_status(&self) -> Value {
        self.job.status(&self.machine, &self.active_device().id)
    }

    /// Start red-pointer framing of the selection (or the whole job, or the tool layers). Light only. With `live`,
    /// the frame follows document edits.
    pub async fn frame_start(self: &Arc<Self>, req: FrameRequest) -> Result<Value, EngineError> {
        self.refuse_if_shutting_down("framing")?;
        self.require_controllable()?;
        let generation = self.machine.generation();
        let doc = self.document();
        let outline = frame::outline_for(&doc, &req)?;
        let live: Option<(u64, frame::LiveSource)> = req.live.then(|| {
            let weak = Arc::downgrade(self);
            let req = req.clone();
            let source: frame::LiveSource = Arc::new(move |since: u64| {
                let st = weak.upgrade()?;
                let doc = st.document();
                if doc.revision == since {
                    return None;
                }
                frame::outline_for(&doc, &req).ok().map(|o| (doc.revision, o))
            });
            (doc.revision, source)
        });
        let run = self.frame_run(&req, live)?;
        let mut started = frame::start(&self.machine, &self.events, outline, run).await?;
        // A shutdown that began while the frame started: the pointer goes off again.
        if self.shutting_down() {
            self.frame_stop().await?;
            return Err(crate::shutdown::shutting_down_error("framing"));
        }
        // Require Framing Before Start: this revision has been framed.
        self.libs.framed(generation, doc.revision, &req);
        // What this frame shows (the foot switch cycle burns exactly this, module `cycle`).
        started["revision"] = json!(doc.revision);
        started["generation"] = json!(generation);
        Ok(started)
    }

    /// How a frame request runs: pointer outputs, blanking, speed, once or looping, time limit.
    fn frame_run(&self, req: &FrameRequest, live: Option<(u64, frame::LiveSource)>) -> Result<FrameRun, EngineError> {
        let profile = self.machine.profile();
        let grbl = self.machine.is_gantry();
        let focus = || {
            profile
                .focus_light_port_bit
                .and_then(|b| lmc::PointerBits::bit(b).ok())
                .ok_or_else(|| EngineError::BadRequest("the machine profile has no focus light port (focus_light_port_bit)".into()))
        };
        // A gantry (GRBL, Ruida) frames by moving the head with the laser off: there is no pointer to choose.
        let pointer = match req.pointer {
            _ if grbl => lmc::PointerBits::RED,
            Pointer::Red => lmc::PointerBits::RED,
            Pointer::Focus => focus()?,
            Pointer::Both => lmc::PointerBits::RED.with(focus()?),
        };
        let max_speed = self.machine.max_frame_speed_mm_s();
        if let Some(v) = req.speed_mm_s
            && !(v.is_finite() && v > 0.0 && v <= max_speed)
        {
            return Err(EngineError::BadRequest(format!(
                "speed_mm_s must be within 0..={max_speed} mm/s (the profile's {} limit)",
                if grbl { "speed" } else { "jump speed" }
            )));
        }
        let max_secs = req.max_seconds.unwrap_or(frame::FRAME_MAX_SECS);
        if !(1..=frame::FRAME_LIMIT_SECS).contains(&max_secs) {
            return Err(EngineError::BadRequest(format!("max_seconds must be within 1..={}", frame::FRAME_LIMIT_SECS)));
        }
        Ok(FrameRun {
            style: machine::FrameStyle { pointer, blanking: req.blanking.unwrap_or(profile.blanking), speed_mm_s: req.speed_mm_s },
            continuous: req.continuous.unwrap_or(true),
            max_secs,
            live,
        })
    }

    /// Red-dot jog: hold the pointer at `(x, y)` field mm (after the red-dot alignment) until frame stop, stop, a
    /// run or `max_seconds` (default 120). Light only.
    pub async fn pointer_to(&self, x: f64, y: f64, max_seconds: Option<u64>, pointer: Pointer) -> Result<Value, EngineError> {
        self.refuse_if_shutting_down("the red dot")?;
        self.require_controllable()?;
        let req = FrameRequest { pointer, max_seconds, ..FrameRequest::default() };
        let run = self.frame_run(&req, None)?;
        let started = frame::start(&self.machine, &self.events, frame::point_outline(x, y), run).await?;
        if self.shutting_down() {
            self.frame_stop().await?;
            return Err(crate::shutdown::shutting_down_error("the red dot"));
        }
        Ok(started)
    }

    /// End framing only. Pending and approved runs are left alone (that is [`AppState::stop`]). `{ok: true}`, or
    /// `{ok: true, connected: false}` when no board is connected.
    pub async fn frame_stop(&self) -> Result<Value, EngineError> {
        Ok(if frame::stop(&self.machine, &self.events).await? {
            json!({ "ok": true })
        } else {
            json!({ "ok": true, "connected": false })
        })
    }

    /// Emergency stop: abort any run or frame, cancel every run not yet streaming (pending, approved, awaiting
    /// confirmation; `cancelled_runs` counts them). Never gated and never waits behind a run: the stop epoch is
    /// bumped (under the runs lock, see the `approve` module) before anything waits for the controller. With no
    /// board connected it answers `{ok: true, connected: false}`.
    pub async fn stop(&self) -> Result<Value, EngineError> {
        let cancelled = {
            let mut runs = self.runs();
            self.machine.request_stop();
            runs.cancel_not_streaming("cancelled by stop before streaming")
        };
        // Stop ends the foot switch cycle too (after the stop epoch moved on, see module `cycle`).
        crate::cycle::end(self, None, None);
        let result = self.machine.abort().await;
        let detail = match &result {
            Ok(AbortOutcome::Sent) => None,
            Ok(AbortOutcome::NotConnected) => Some("no board connected".to_owned()),
            Err(e) => Some(e.to_string()),
        };
        self.emit(Event::machine("stopped", detail));
        match result {
            Ok(AbortOutcome::Sent) => Ok(json!({ "ok": true, "connected": true, "cancelled_runs": cancelled })),
            Ok(AbortOutcome::NotConnected) => {
                Ok(json!({ "ok": true, "connected": false, "cancelled_runs": cancelled }))
            }
            Err(e) => {
                self.machine.set_error(Some(e.to_string()));
                Err(EngineError::Internal(format!(
                    "the abort sequence failed ({e}); use the hardware E-stop or key switch"
                )))
            }
        }
    }
}

/// An empty document laid out for `device`: a galvo's field (`machine_field` when it drives the machine; a
/// non-square field as its `workspace_mm` with the centre origin), a gantry's bed with its origin corner (the
/// smaller side as `field_mm`).
pub fn new_document_for(device: &machine::Device, machine_field: f64) -> Document {
    let mut d = Document::default();
    if device.kind.is_galvo() {
        let [w, h] = galvo_field(device, machine_field);
        d.field_mm = Some(w);
        if (w - h).abs() > 1e-9 {
            d.workspace_mm = Some([w, h]);
            d.extras.insert(commands::ORIGIN_KEY.into(), "center".into());
        }
    } else {
        d.field_mm = Some(device.field_mm());
        d.workspace_mm = Some(device.workspace_mm);
        d.extras.insert(commands::ORIGIN_KEY.into(), device.origin.name().into());
    }
    d
}

/// The document as `.openetch` JSON, refusing one that would not load again (never written as `null`s). The
/// session refuses to commit such a document, so this is a backstop.
fn saveable(doc: &Document) -> Result<String, EngineError> {
    try_to_openetch_json(doc).map_err(|e| EngineError::Internal(e.to_string()))
}

/// Refuse (409) writing `<name>.<ext>` when that would replace an existing file other than the current project's,
/// including one whose name differs only in case.
fn refuse_clobber(dir: &ProjectDir, name: &str, ext: &str, current: Option<&str>) -> Result<(), EngineError> {
    let clash = dir.replaced_by(name, ext)?.into_iter().find(|existing| current != Some(existing.as_str()));
    match clash {
        None => Ok(()),
        Some(existing) => {
            let what = if ext == PROJECT_EXT { format!("a project named \"{existing}\"") } else {
                format!("\"{existing}.{ext}\"")
            };
            let case = if existing == name { "" } else { " (names that differ only in case are the same file)" };
            Err(EngineError::Conflict(format!("{what} already exists{case}; pass overwrite: true to replace it")))
        }
    }
}

/// Whether an invocation's params have no (or a null) `field_mm`.
fn lacks_field(inv: &Invocation) -> bool {
    lacks(inv, "field_mm")
}

/// Whether an invocation's params have no (or a null) `key`.
fn lacks(inv: &Invocation, key: &str) -> bool {
    inv.params.get(key).is_none_or(Value::is_null)
}

/// A galvo device's field `[width, height]`: `machine_field` (the machine profile's width) when it drives the
/// machine, else the device's workspace width, with the device's height (LightBurn's separate field Height; the
/// width again for a square field).
pub(crate) fn galvo_field(device: &machine::Device, machine_field: f64) -> [f64; 2] {
    let [w, h] = device.workspace_mm;
    let width = if device.controllable { machine_field } else { w };
    // A square device keeps a square field even when the machine's width differs from the record's.
    if (w - h).abs() < 1e-9 { [width, width] } else { [width, h] }
}

/// `doc.new` parameters for `device`: a galvo's `field_mm` (`field`) or, for a non-square field, its
/// `workspace_mm` with the centre origin; a gantry's bed and origin corner.
fn new_layout(device: &machine::Device, field: f64) -> Value {
    if device.kind.is_galvo() {
        let [w, h] = galvo_field(device, field);
        if (w - h).abs() > 1e-9 {
            json!({ "workspace_mm": [w, h], "origin": "center" })
        } else {
            json!({ "field_mm": w })
        }
    } else {
        json!({ "workspace_mm": device.workspace_mm, "origin": device.origin.name() })
    }
}

/// `doc.import_lbrn2` parameters for a file laid out on `device`: a galvo's field as LightBurn knows it (its
/// `Width`, e.g. 153.8 for the B6, whose profile keeps EzCad's 153.75), a gantry's bed and origin corner.
fn lightburn_layout(device: &machine::Device) -> Value {
    if let Some(field) = device.lightburn_field_mm() {
        let h = device.workspace_mm[1];
        // A non-square galvo field (LightBurn's Height more than its 0.1 mm display step from the Width).
        if (h - device.workspace_mm[0]).abs() > 0.05 {
            json!({ "workspace_mm": [field, h], "origin": "center" })
        } else {
            json!({ "field_mm": field })
        }
    } else {
        json!({ "workspace_mm": device.workspace_mm, "origin": device.origin.name() })
    }
}

/// Add every key of `layout` the invocation does not set.
fn fill_layout(inv: &mut Invocation, layout: &Value) {
    if inv.params.is_null() {
        inv.params = json!({});
    }
    if let (Some(obj), Some(l)) = (inv.params.as_object_mut(), layout.as_object()) {
        for (k, v) in l {
            if obj.get(k).is_none_or(Value::is_null) {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Give a `doc.new` / `doc.import_lbrn2` / `doc.export_lbrn2` invocation `field_mm` when it has none.
fn fill_field_mm(inv: &mut Invocation, field_mm: f64) {
    if !NEEDS_FIELD.contains(&inv.id.as_str()) {
        return;
    }
    if inv.params.is_null() {
        inv.params = json!({});
    }
    if let Some(obj) = inv.params.as_object_mut()
        && obj.get("field_mm").is_none_or(Value::is_null)
    {
        obj.insert("field_mm".into(), json!(field_mm));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A galvo with a separate field Height lays new documents (and LightBurn imports) out on its whole field, so the
    /// canvas can draw it; a square galvo keeps the plain field_mm.
    #[test]
    fn non_square_galvo_fields_lay_documents_out_on_their_workspace() {
        let profile = machine::MachineProfile { field_height_mm: Some(120.0), ..machine::MachineProfile::generic_jcz_galvo() };
        let w = profile.field_mm;
        let device = machine::Device::builtin(profile);
        let d = new_document_for(&device, w);
        assert_eq!((d.field_mm, d.workspace(), d.extras.get(commands::ORIGIN_KEY).map(String::as_str)), (Some(w), Some([w, 120.0]), Some("center")));
        assert_eq!(new_layout(&device, w), json!({"workspace_mm": [w, 120.0], "origin": "center"}));
        assert_eq!(lightburn_layout(&device), json!({"workspace_mm": [w, 120.0], "origin": "center"}));
        // doc.new with that layout makes the same document.
        let mut s = commands::Session::new(Document::default());
        s.execute(Invocation { id: "doc.new".into(), params: new_layout(&device, w) }, commands::Origin::Ui).unwrap();
        assert_eq!((s.document().field_mm, s.document().workspace()), (Some(w), Some([w, 120.0])));

        let square = machine::Device::builtin(machine::MachineProfile::generic_jcz_galvo());
        let d = new_document_for(&square, 150.0);
        assert_eq!((d.field_mm, d.workspace_mm, d.extras.get(commands::ORIGIN_KEY)), (Some(150.0), None, None));
        assert_eq!(new_layout(&square, 150.0), json!({"field_mm": 150.0}));
    }

    #[test]
    fn field_is_filled_only_when_missing() {
        let mut inv = Invocation { id: "doc.export_lbrn2".into(), params: Value::Null };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params, json!({"field_mm": 150.0}));
        let mut inv = Invocation { id: "doc.import_lbrn2".into(), params: json!({"content": "x", "field_mm": 110}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params["field_mm"], json!(110));
        let mut inv = Invocation { id: "doc.import_lbrn2".into(), params: json!({"content": "x", "field_mm": null}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params["field_mm"], json!(150.0));
        let mut inv = Invocation { id: "shape.rect".into(), params: json!({}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params, json!({}));
        let mut inv = Invocation { id: "doc.new".into(), params: json!({"name": "x"}) };
        fill_field_mm(&mut inv, 153.75);
        assert_eq!(inv.params, json!({"name": "x", "field_mm": 153.75}));
        assert_eq!(FileFormat::parse("lbrn2").unwrap().extension(), "lbrn2");
        assert!(FileFormat::parse("svg").is_err(), "downloads are openetch or lbrn2");
        assert_eq!(UploadFormat::parse("svg").unwrap(), UploadFormat::Svg);
        assert!(UploadFormat::parse("pdf").is_err());
    }

    #[test]
    fn images_are_sniffed() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0"), Some("image/png"));
        assert_eq!(sniff_image(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"II*\0...."), Some("image/tiff"));
        assert_eq!(sniff_image(b"<svg/>"), None);
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn origins_parse() {
        assert_eq!(parse_origin(None).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("ui"))).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("agent:claude"))).unwrap(), Origin::Agent("claude".into()));
        assert_eq!(parse_origin(Some(&json!({"kind": "script"}))).unwrap(), Origin::Script);
        assert!(parse_origin(Some(&json!("root"))).is_err());
    }

    #[test]
    fn saving_refuses_a_document_that_would_not_load_again() {
        let good = Document::default();
        assert!(doc::file::from_openetch_json(&saveable(&good).unwrap()).is_ok());
        let mut bad = Document::default();
        bad.layers[0].settings.power_pct = f64::NAN;
        let e = saveable(&bad).unwrap_err();
        assert_eq!(e.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR, "{e}");
    }
}
