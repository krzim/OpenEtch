//! [`AppState`]: the live session, the machine, pending runs and the event channel, with the operations both the
//! HTTP API and the MCP server call. Locks are std mutexes held only for short, non-awaiting sections; planning
//! and rendering run on blocking threads.
//!
//! Projects: the document is saved to and opened from `<projects-dir>/<name>.openetch` ([`ProjectDir`]); the
//! current project name and the revision it was saved at live in [`ProjectState`] (cleared by `doc.new` and
//! `doc.load`, set by save and open) and are updated under the session lock, before `document_changed` is sent.
//! The document's `field_mm` comes from the machine profile when the engine starts with a document that has none,
//! and `doc.import_lbrn2` / `doc.export_lbrn2` get the profile's field size when neither the caller nor the
//! document gives one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use commands::{CommandSpec, HistoryEntry, Invocation, Origin, Outcome, Risk, Session};
use doc::Document;
use doc::file::to_openetch_json;
use plan::{PlanError, Preview};
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

/// Commands that replace the document with one no project file holds: they clear the current project.
const REPLACING: [&str; 2] = ["doc.new", "doc.load"];
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

    pub fn history(&self) -> Vec<HistoryEntry> {
        lock(&self.session).history().to_vec()
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
        let (outcomes, before, after) = {
            let mut s = lock(&self.session);
            let mut last_edit = None;
            for inv in &invs {
                if let Some(spec) = s.registry().spec(&inv.id) {
                    if !matches!(spec.risk, Risk::Query | Risk::Edit) {
                        return Err(EngineError::Forbidden(format!(
                            "'{}' is a machine action; use the machine endpoints/tools instead",
                            inv.id
                        )));
                    }
                    if spec.risk == Risk::Edit {
                        last_edit = Some(inv.id.clone());
                    }
                }
            }
            if s.document().field_mm.is_none() {
                let field = self.machine.profile().field_mm;
                for inv in &mut invs {
                    fill_field_mm(inv, field);
                }
            }
            let replaced = invs.iter().any(|i| REPLACING.contains(&i.id.as_str()));
            let before = s.document().revision;
            let outcomes = s.batch(invs, origin.clone())?;
            let after = s.document().revision;
            if after != before {
                let mut p = lock(&self.project);
                if let Some(name) = opened {
                    *p = ProjectState::saved(name, after);
                } else if replaced {
                    // Clean only when nothing was edited after the replacement in the same batch.
                    let clean = last_edit.is_some_and(|id| REPLACING.contains(&id.as_str()));
                    *p = ProjectState { name: None, saved_revision: None, clean_revision: clean.then_some(after) };
                }
            }
            if s.document().field_mm.is_none() {
                // Cannot happen once the engine starts with a field size (doc.new and doc.load keep it, imports set
                // it, and undo only returns to documents that had one), so it is only reported.
                tracing::warn!(revision = after, "the document has no field_mm");
            }
            (outcomes, before, after)
        };
        if after != before {
            self.emit(Event::DocumentChanged { revision: after, origin });
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

    /// Save the document as project `name`. Unless `overwrite`, an existing project other than the current one is
    /// refused (409). Emits `project_saved`. Returns `{ok, name, path, revision}`.
    pub async fn project_save(&self, name: &str, overwrite: bool) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let (content, revision) = {
            let s = lock(&self.session);
            (to_openetch_json(s.document()), s.document().revision)
        };
        let dir = self.projects.clone();
        let current = self.project_name();
        let owned = name.to_owned();
        let path = tokio::task::spawn_blocking(move || {
            if !overwrite && current.as_deref() != Some(owned.as_str()) && dir.exists(&owned)? {
                return Err(EngineError::Conflict(format!(
                    "a project named \"{owned}\" already exists; pass overwrite: true to replace it"
                )));
            }
            dir.write(&owned, PROJECT_EXT, &content)
        })
        .await??;
        *lock(&self.project) = ProjectState::saved(name.to_owned(), revision);
        tracing::info!(name, revision, path = %path.display(), "project saved");
        self.emit(Event::ProjectSaved { name: name.to_owned(), revision });
        Ok(json!({ "ok": true, "name": name, "path": path.display().to_string(), "revision": revision }))
    }

    /// Open project `name`: replaces the document (`doc.load`, one undo step) and makes it the current project.
    /// Returns `{ok, name, result, revision}`.
    pub async fn project_open(&self, name: &str, origin: Origin) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let dir = self.projects.clone();
        let owned = name.to_owned();
        let content = tokio::task::spawn_blocking(move || dir.read(&owned)).await??;
        let inv = Invocation { id: "doc.load".into(), params: json!({ "content": content }) };
        let o = self.batch_opening(vec![inv], origin, Some(name.to_owned()))?.remove(0);
        Ok(json!({ "ok": true, "name": name, "result": o.result, "revision": o.revision }))
    }

    /// Export the document as LightBurn to `<projects-dir>/<name>.lbrn2`, at the document's field size. Returns
    /// `{ok, path, warnings, field_mm}`.
    pub async fn project_export_lbrn2(&self, name: &str, origin: Origin) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let o = self.execute(Invocation { id: "doc.export_lbrn2".into(), params: json!({}) }, origin)?;
        let content = o.result["content"].as_str().unwrap_or_default().to_owned();
        let dir = self.projects.clone();
        let owned = name.to_owned();
        let path = tokio::task::spawn_blocking(move || dir.write(&owned, LBRN2_EXT, &content)).await??;
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
                (to_openetch_json(s.document()), Vec::new(), s.document().name.clone())
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

    /// Run an uploaded file: `.openetch` through `doc.load` (replace only), `.lbrn2` through `doc.import_lbrn2`
    /// (`replace` or `merge`, optional `field_mm`).
    pub fn upload(
        &self,
        format: FileFormat,
        merge: bool,
        field_mm: Option<f64>,
        content: String,
        origin: Origin,
    ) -> Result<Outcome, EngineError> {
        let inv = match format {
            FileFormat::OpenEtch => {
                if merge {
                    return Err(EngineError::BadRequest(
                        "mode=merge is only for LightBurn imports; an .openetch project replaces the document".into(),
                    ));
                }
                if field_mm.is_some() {
                    return Err(EngineError::BadRequest("field_mm is only for LightBurn imports".into()));
                }
                Invocation { id: "doc.load".into(), params: json!({ "content": content }) }
            }
            FileFormat::Lbrn2 => {
                let mut params = json!({ "content": content, "mode": if merge { "merge" } else { "replace" } });
                if let Some(f) = field_mm {
                    params["field_mm"] = json!(f);
                }
                Invocation { id: "doc.import_lbrn2".into(), params }
            }
        };
        self.execute(inv, origin)
    }

    /// Undo the last edit (by anyone); returns the new revision. `origin` is who asked, for the event.
    pub fn undo(&self, origin: Origin) -> Result<u64, EngineError> {
        let rev = lock(&self.session).undo()?;
        self.emit(Event::DocumentChanged { revision: rev, origin });
        Ok(rev)
    }

    /// Redo the last undone edit; returns the new revision.
    pub fn redo(&self, origin: Origin) -> Result<u64, EngineError> {
        let rev = lock(&self.session).redo()?;
        self.emit(Event::DocumentChanged { revision: rev, origin });
        Ok(rev)
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
                Ok(job) => Ok(plan::preview_plan(&job)),
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
        assert!(FileFormat::parse("svg").is_err());
    }

    #[test]
    fn origins_parse() {
        assert_eq!(parse_origin(None).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("ui"))).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("agent:claude"))).unwrap(), Origin::Agent("claude".into()));
        assert_eq!(parse_origin(Some(&json!({"kind": "script"}))).unwrap(), Origin::Script);
        assert!(parse_origin(Some(&json!("root"))).is_err());
    }
}
