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
//! and `doc.import_lbrn2` / `doc.export_lbrn2` get the profile's field size when neither the caller nor the
//! document gives one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use commands::{CommandSpec, HistoryStep, HistoryView, Invocation, Origin, Outcome, Risk, Session};
use doc::Document;
use doc::file::try_to_openetch_json;
use plan::{FillRegions, PlanError, Preview};
use base64::Engine as _;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::frame::{self, FrameRequest};
use crate::hw::{AbortOutcome, Machine};
use crate::pin::ApprovalConfig;
use crate::projects::{self, LBRN2_EXT, PROJECT_EXT, ProjectDir, ProjectInfo, ProjectState};
use crate::render::{self, RenderOptions};
use crate::runs::{self, RunStore, RunView};
use crate::security::SecurityConfig;
use crate::{EngineError, Event};

/// The message every pending-run answer carries.
pub const APPROVAL_MESSAGE: &str = "The run is pending. Nothing fires until a person reviews it and approves it in \
    the OpenEtch UI (Laser workspace) with the operator PIN. Agents cannot and must not approve runs, and must never \
    ask for, handle or set the PIN.";

/// A planned preview and the document it was planned from.
pub type PreviewResult = Arc<(Document, Result<Preview, PlanError>)>;

/// Coalesces preview planning: at most one computation runs at a time, and requests that queued behind it share
/// one computation of the newest document instead of each planning the revision they arrived at.
#[derive(Default)]
struct PreviewGate {
    /// Held (owned, inside the blocking task) while a preview is being planned, so a dropped request cannot let a
    /// second computation start while the first is still running.
    running: Arc<tokio::sync::Mutex<()>>,
    /// The newest result, kept only while some request is waiting (memory is released when the last one leaves).
    latest: Mutex<Option<PreviewResult>>,
    /// Requests inside [`AppState::preview`].
    waiting: AtomicUsize,
    /// Plans computed (tests).
    computed: AtomicUsize,
}

/// Counts a request in [`PreviewGate::waiting`]; the last one out drops the cached result.
struct PreviewTicket<'a>(&'a PreviewGate);

impl<'a> PreviewTicket<'a> {
    fn new(gate: &'a PreviewGate) -> Self {
        gate.waiting.fetch_add(1, Ordering::SeqCst);
        PreviewTicket(gate)
    }
}

impl Drop for PreviewTicket<'_> {
    fn drop(&mut self) {
        let mut latest = lock(&self.0.latest);
        if self.0.waiting.fetch_sub(1, Ordering::SeqCst) == 1 {
            *latest = None;
        }
    }
}

/// Shared engine state. See the module docs.
pub struct AppState {
    session: Mutex<Session>,
    pub machine: Arc<Machine>,
    runs: Mutex<RunStore>,
    events: broadcast::Sender<Event>,
    pub security: SecurityConfig,
    /// Operator PIN and the optional physical confirmation input.
    pub approval: ApprovalConfig,
    previews: Arc<PreviewGate>,
    /// Where named projects are saved.
    pub projects: ProjectDir,
    /// The current project. Locked only while the session lock is held or with no other lock, never the reverse.
    project: Mutex<ProjectState>,
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

/// Commands that convert with a field size and get the profile's `field_mm` when nothing else gives one.
const NEEDS_FIELD: [&str; 2] = ["doc.import_lbrn2", "doc.export_lbrn2"];

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
    /// DXF drawing, added to the document (`art.import_dxf`).
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

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
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
        Arc::new(AppState {
            session: Mutex::new(session),
            machine,
            runs: Mutex::new(RunStore::default()),
            events: crate::events::channel(),
            security,
            approval,
            previews: Arc::default(),
            projects,
            project: Mutex::new(project),
        })
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

    /// Every step on the timeline (file contents in parameters shortened).
    pub fn history(&self) -> Vec<HistoryStep> {
        self.history_view().steps
    }

    /// `GET /api/history`: the marker and every step.
    pub fn history_view(&self) -> HistoryView {
        lock(&self.session).history_view()
    }

    /// Announce a document change (when the revision moved) and the timeline (always).
    fn announce(&self, before: u64, after: u64, origin: Origin, (marker, steps): (usize, usize)) {
        if after != before {
            self.emit(Event::DocumentChanged { revision: after, origin });
        }
        self.emit(Event::HistoryChanged { marker, steps, revision: after });
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
        let (before, after, hist) = {
            let mut s = lock(&self.session);
            let before = s.document().revision;
            let after = op(&mut s)?;
            (before, after, (s.marker(), s.history().len()))
        };
        self.announce(before, after, origin, hist);
        Ok(json!({ "ok": true, "marker": hist.0, "revision": after }))
    }

    /// Run one command (see [`AppState::batch`]).
    pub fn execute(&self, inv: Invocation, origin: Origin) -> Result<Outcome, EngineError> {
        self.batch(vec![inv], origin).map(|mut v| v.remove(0))
    }

    /// Run commands as one undo step, all-or-nothing. Only `Query` and `Edit` commands are accepted here: machine
    /// actions have their own endpoints and tools.
    pub fn batch(&self, invs: Vec<Invocation>, origin: Origin) -> Result<Vec<Outcome>, EngineError> {
        self.batch_opening(invs, origin, None)
    }

    /// [`AppState::batch`]; with `opened`, a successful edit makes `opened` the current project, saved at the new
    /// revision (project open). The project state changes before `document_changed` is sent, so a client that
    /// refetches `GET /api/project` on the event sees it.
    fn batch_opening(
        &self,
        mut invs: Vec<Invocation>,
        origin: Origin,
        opened: Option<String>,
    ) -> Result<Vec<Outcome>, EngineError> {
        if invs.is_empty() {
            return Err(EngineError::BadRequest("batch has no commands".into()));
        }
        let (outcomes, before, after, hist) = {
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
                let field = self.machine.profile().field_mm;
                for inv in &mut invs {
                    fill_field_mm(inv, field);
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
                    *p = ProjectState { name: None, saved_revision: None, clean_revision: clean.then_some(after) };
                }
            }
            if s.document().field_mm.is_none() {
                // Cannot happen once the engine starts with a field size (doc.new and doc.load keep it, imports set
                // it, and undo only returns to documents that had one), so it is only reported.
                tracing::warn!(revision = after, "the document has no field_mm");
            }
            (outcomes, before, after, (s.marker(), s.history().len()))
        };
        if after != before {
            self.announce(before, after, origin, hist);
        }
        Ok(outcomes)
    }

    /// `GET /api/project`: `{name, saved_revision, revision, dirty}`.
    pub fn project_status(&self) -> Value {
        let s = lock(&self.session);
        let revision = s.document().revision;
        let p = lock(&self.project);
        json!({
            "name": p.name,
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
        projects::validate_name(name)?;
        let (content, revision) = {
            let s = lock(&self.session);
            (saveable(s.document())?, s.document().revision)
        };
        let dir = self.projects.clone();
        let current = self.project_name();
        let owned = name.to_owned();
        let path = tokio::task::spawn_blocking(move || {
            if !overwrite {
                refuse_clobber(&dir, &owned, PROJECT_EXT, current.as_deref())?;
            }
            dir.write(&owned, PROJECT_EXT, &content)
        })
        .await??;
        *lock(&self.project) = ProjectState::saved(name.to_owned(), revision);
        tracing::info!(name, revision, path = %path.display(), "project saved");
        self.emit(Event::ProjectSaved { name: name.to_owned(), revision });
        Ok(json!({ "ok": true, "name": name, "path": path.display().to_string(), "revision": revision }))
    }

    /// Open project `name`: replaces the document (`doc.load`, one undo step) and makes it the current project. A
    /// name that differs only in case finds the project when exactly one matches; the project keeps the name it
    /// has on disk. Returns `{ok, name, result, revision}`.
    pub async fn project_open(self: &Arc<Self>, name: &str, origin: Origin) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let owned = name.to_owned();
        let (name, o) = self
            .blocking(move |st| {
                let (name, content) = st.projects.read(&owned)?;
                let inv = Invocation { id: "doc.load".into(), params: content_params(content) };
                let o = st.batch_opening(vec![inv], origin, Some(name.clone()))?.remove(0);
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
        projects::validate_name(name)?;
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
        let stem = self.project_name().unwrap_or(name);
        Ok(DownloadFile { stem, format, content, warnings })
    }

    /// Run an uploaded file (the raw bytes): `.openetch` through `doc.load` (replace only), `.lbrn2` through
    /// `doc.import_lbrn2` (`replace` or `merge`, optional `field_mm`), SVG through `art.import_svg`, DXF through
    /// `art.import_dxf` and bitmaps through `art.import_image` (merge only, optional `layer`, `name`, `width_mm`;
    /// images also `mime`, else sniffed). Options that do not apply to the format are refused (400).
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
        if !art {
            if opts.layer.is_some() {
                return refuse("layer", "svg, dxf and image");
            }
            if opts.name.is_some() {
                return refuse("name", "svg, dxf and image");
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
        self.execute(inv, origin)
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
    /// Coalesced ([`PreviewGate`]): at most one plan runs at a time. A request that waited for a running plan
    /// reuses the newest result if it is of the current revision, and otherwise plans the current document once
    /// for every request queued with it, so a burst of edits never queues one plan per edit. The result can
    /// therefore be of a newer revision than the one current when the request arrived (never an older one).
    pub async fn preview(&self) -> Result<PreviewResult, EngineError> {
        let gate = &*self.previews;
        let _ticket = PreviewTicket::new(gate);
        let running = gate.running.clone().lock_owned().await;
        let revision = self.revision();
        if let Some(r) = lock(&gate.latest).as_ref().filter(|r| r.0.revision == revision) {
            return Ok(r.clone());
        }
        let doc = self.document();
        let target = self.machine.target().clone();
        let shared = self.previews.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let preview = match plan::plan_job(&doc, &target) {
                Ok(job) => Ok(plan::preview_document(&doc, &job)),
                Err(PlanError::Empty) => Ok(Preview::default()),
                Err(e) => Err(e),
            };
            let result: PreviewResult = Arc::new((doc, preview));
            shared.computed.fetch_add(1, Ordering::SeqCst);
            // Published before the slot is released, so the next queued request finds it; kept only while some
            // request still waits (the ticket's drop takes the same lock).
            let mut latest = lock(&shared.latest);
            if shared.waiting.load(Ordering::SeqCst) > 0 {
                *latest = Some(result.clone());
            }
            drop(latest);
            drop(running);
            result
        })
        .await?)
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

    /// Plan, encode and check the current document and store it as a pending run. Never touches the machine.
    pub async fn request_run(&self, origin: Origin) -> Result<RunView, EngineError> {
        let doc = self.document();
        let revision = doc.revision;
        let target = self.machine.target().clone();
        let profile = self.machine.profile().clone();
        let prepared = tokio::task::spawn_blocking(move || runs::prepare_job(&doc, &target, &profile)).await??;
        let view = {
            let mut store = self.runs();
            let run = store.add(origin.clone(), revision, prepared)?;
            run.view(revision)
        };
        tracing::info!(run_id = %view.id, ?origin, "run requested; waiting for approval in the UI");
        self.emit(Event::RunPending { run_id: view.id.clone(), origin, summary: view.summary.clone() });
        Ok(view)
    }

    /// `GET /api/machine`: machine status, how runs are approved, and pending and recent runs.
    pub fn machine_status(&self) -> Value {
        let rev = self.revision();
        let mut v = self.machine.status_json();
        v["approval"] = self.approval.status_json();
        let store = self.runs();
        v["pending_runs"] = json!(store.pending(rev));
        v["runs"] = json!(store.views(rev));
        v
    }

    /// Start red-pointer framing of the selection (or the whole job). Light only.
    pub async fn frame_start(&self, req: FrameRequest) -> Result<Value, EngineError> {
        let outline = frame::outline_for(&self.document(), &req)?;
        frame::start(&self.machine, &self.events, outline).await
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

/// Give a `doc.import_lbrn2` / `doc.export_lbrn2` invocation `field_mm` when it has none.
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
