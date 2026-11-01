//! Several open documents (LightBurn's File > New Window: one window per project, all sharing the same settings and
//! devices). The engine keeps one live [`Session`] per open document; exactly one is **active**, and everything that
//! names no document (every `/api/*` document route, MCP's document tools, previews, runs, framing, autosave) uses the
//! active one, so a single-document client keeps working unchanged on the `default` document.
//!
//! - `GET /api/documents` → `{active, documents: [{id, name, project, display_name, document_name, revision, dirty,
//!   objects, layers, active}]}` in the order they were opened (the first one is `default`).
//! - `POST /api/documents` `{project?, activate? (default true), origin?}` → **201** `{ok, id, active, existing,
//!   result?}`: a new, empty document laid out for the active device (New Window), or project `project` opened into a
//!   new document (a project already open in a document activates that one instead, `existing: true`). At most
//!   [`MAX_DOCUMENTS`] (409).
//! - `POST /api/documents/{id}/activate` `{origin?}` → `{ok, active, revision}`.
//! - `POST /api/documents/{id}/close` `{discard?}` → `{ok, closed, active}`: unsaved changes need `discard: true`
//!   (409); the last open document cannot be closed (409); closing the active one activates the one opened before it
//!   (else the next).
//! - `GET /api/layers/unsupported` (the active document): see [`unsupported_settings`].
//!
//! Each document has its own timeline (undo/redo, history) and project state. Activation swaps the sessions under the
//! session, project and documents locks (in that order) and raises the activated document's revision above every
//! revision any open document has had ([`Session::raise_revision`]), so revisions stay unique across documents:
//! a pending run of another document is stale and cannot be approved, the preview cache never serves another
//! document's plan, and Require Framing Before Start asks for a new frame. Live framing follows the active document.
//! An activation emits `documents_changed`, then `document_changed` and `history_changed` tagged with the new active
//! document, so a client that only refetches on those sees the switch. Opening and closing an inactive document only
//! emits `documents_changed`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use commands::{Origin, Session};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::projects::{self, ProjectState};
use crate::state::{lock, parse_origin};
use crate::{AppState, EngineError, Event};

/// The id of the document the engine starts with.
pub const DEFAULT_DOCUMENT: &str = "default";
/// Most documents open at once.
pub const MAX_DOCUMENTS: usize = 16;

/// An open document that is not the active one.
pub(crate) struct OpenDoc {
    pub id: String,
    pub session: Session,
    pub project: ProjectState,
}

/// The open documents (see the module docs).
pub(crate) struct DocSet {
    /// The active document's id.
    pub active: String,
    /// Every open document's id, in the order they were opened.
    pub order: Vec<String>,
    /// The documents other than the active one.
    pub others: Vec<OpenDoc>,
    /// The number the next new document's id gets (`doc-N`).
    next: u64,
    /// The highest revision any document had when it stopped being active.
    max_revision: u64,
}

impl Default for DocSet {
    fn default() -> Self {
        DocSet { active: DEFAULT_DOCUMENT.into(), order: vec![DEFAULT_DOCUMENT.into()], others: Vec::new(), next: 2, max_revision: 0 }
    }
}

impl DocSet {
    fn new_id(&mut self) -> String {
        loop {
            let id = format!("doc-{}", self.next);
            self.next += 1;
            if !self.order.contains(&id) {
                return id;
            }
        }
    }
}

/// One row of `GET /api/documents`.
fn row(id: &str, session: &Session, project: &ProjectState, active: bool) -> Value {
    let doc = session.document();
    let name = project.name.clone().or_else(|| project.display_name.clone()).unwrap_or_else(|| doc.name.clone());
    json!({
        "id": id,
        "name": name,
        "project": project.name,
        "display_name": project.display_name,
        "document_name": doc.name,
        "revision": doc.revision,
        "dirty": project.dirty(doc.revision),
        "objects": doc.objects.len(),
        "layers": doc.layers.len(),
        "active": active,
    })
}

impl AppState {
    /// `GET /api/documents`.
    pub fn documents_view(&self) -> Value {
        let s = lock(&self.session);
        let p = lock(&self.project);
        let d = lock(&self.documents);
        let rows: Vec<Value> = d
            .order
            .iter()
            .filter_map(|id| {
                if *id == d.active {
                    Some(row(id, &s, &p, true))
                } else {
                    d.others.iter().find(|o| o.id == *id).map(|o| row(id, &o.session, &o.project, false))
                }
            })
            .collect();
        json!({ "active": d.active, "documents": rows })
    }

    /// The active document's revision and id, read together (what a pending run is checked against).
    pub(crate) fn active_revision_and_document(&self) -> (u64, String) {
        let s = lock(&self.session);
        (s.document().revision, self.active_document_id())
    }

    /// Set the project state of open document `id` (the active one or a waiting one). Used by a save that finished
    /// after the active document changed.
    pub(crate) fn set_project_of(&self, id: &str, state: ProjectState) {
        let _s = lock(&self.session);
        let mut p = lock(&self.project);
        let mut d = lock(&self.documents);
        if d.active == id {
            *p = state;
        } else if let Some(o) = d.others.iter_mut().find(|o| o.id == id) {
            o.project = state;
        }
    }

    /// Make open document `id` the active one (see the module docs). Returns `{ok, active, revision}`.
    pub fn activate_document(&self, id: &str, origin: Origin) -> Result<Value, EngineError> {
        let (revision, hist) = {
            let mut s = lock(&self.session);
            let mut p = lock(&self.project);
            let mut d = lock(&self.documents);
            if d.active == id {
                return Ok(json!({ "ok": true, "active": id, "revision": s.document().revision, "changed": false }));
            }
            let pos = d
                .others
                .iter()
                .position(|o| o.id == id)
                .ok_or_else(|| EngineError::NotFound(format!("no open document with id '{id}'")))?;
            let mut incoming = d.others.remove(pos);
            d.max_revision = d.max_revision.max(s.document().revision);
            let old = incoming.session.document().revision;
            let new = d.max_revision + 1;
            incoming.session.raise_revision(new);
            d.max_revision = new;
            // The content did not change: a clean or saved state stays clean at the raised revision.
            if incoming.project.clean_revision == Some(old) {
                incoming.project.clean_revision = Some(new);
            }
            if incoming.project.saved_revision == Some(old) {
                incoming.project.saved_revision = Some(new);
            }
            std::mem::swap(&mut *s, &mut incoming.session);
            std::mem::swap(&mut *p, &mut incoming.project);
            let previous = std::mem::replace(&mut d.active, id.to_owned());
            d.others.push(OpenDoc { id: previous, session: incoming.session, project: incoming.project });
            (new, (s.marker(), s.history().len()))
        };
        tracing::info!(document = id, revision, "document activated");
        self.emit(Event::DocumentsChanged { active: id.to_owned() });
        self.announce(revision.wrapping_sub(1), revision, origin, hist, id.to_owned());
        Ok(json!({ "ok": true, "active": id, "revision": revision, "changed": true }))
    }

    /// Open a new, empty document laid out for the active device (New Window). Returns its id; it is not activated.
    pub fn new_document(&self) -> Result<String, EngineError> {
        let device = self.active_device();
        let field = self.machine.profile().field_mm;
        let doc = crate::state::new_document_for(&device, field);
        let session = Session::new(doc);
        let project = ProjectState::clean_at(session.document().revision);
        let id = {
            let mut d = lock(&self.documents);
            if d.order.len() >= MAX_DOCUMENTS {
                return Err(EngineError::Conflict(format!("{MAX_DOCUMENTS} documents are open; close one first")));
            }
            let id = d.new_id();
            d.order.push(id.clone());
            d.others.push(OpenDoc { id: id.clone(), session, project });
            id
        };
        self.emit(Event::DocumentsChanged { active: self.active_document_id() });
        Ok(id)
    }

    /// The open document holding project `name` (ignoring case), if any.
    fn document_with_project(&self, name: &str) -> Option<String> {
        let _s = lock(&self.session);
        let p = lock(&self.project);
        let d = lock(&self.documents);
        if p.name.as_deref().is_some_and(|n| projects::same_name_ignoring_case(n, name)) {
            return Some(d.active.clone());
        }
        d.others
            .iter()
            .find(|o| o.project.name.as_deref().is_some_and(|n| projects::same_name_ignoring_case(n, name)))
            .map(|o| o.id.clone())
    }

    /// `POST /api/documents`: a new document, optionally holding project `project`, activated unless `activate` is
    /// false (not with a project).
    pub async fn open_document(
        self: &Arc<Self>,
        project: Option<String>,
        activate: bool,
        origin: Origin,
    ) -> Result<Value, EngineError> {
        if let Some(name) = &project {
            if !activate {
                return Err(EngineError::BadRequest("a project opens into the active document: leave activate out".into()));
            }
            projects::validate_name(name)?;
            if let Some(id) = self.document_with_project(name) {
                let r = self.activate_document(&id, origin)?;
                return Ok(json!({ "ok": true, "id": id, "active": id, "existing": true, "revision": r["revision"] }));
            }
            // Check the project exists before a document is made for it.
            let dir = self.projects.clone();
            let owned = name.clone();
            tokio::task::spawn_blocking(move || dir.resolve_name(&owned)).await??;
        }
        let id = self.new_document()?;
        if !activate {
            return Ok(json!({ "ok": true, "id": id, "active": self.active_document_id(), "existing": false }));
        }
        let Some(name) = project else {
            let a = self.activate_document(&id, origin)?;
            return Ok(json!({ "ok": true, "id": id, "active": id, "existing": false, "revision": a["revision"] }));
        };
        // The project goes into the new document while it is still inactive, then the document is activated: a
        // switch another client makes meanwhile can never have the project replace a different document.
        let loaded = match self.load_project_into(&id, &name, origin.clone()).await {
            Ok(r) => r,
            Err(e) => {
                let _ = self.close_document(&id, true);
                return Err(e);
            }
        };
        let a = self.activate_document(&id, origin)?;
        let mut result = loaded;
        result["revision"] = a["revision"].clone();
        Ok(json!({ "ok": true, "id": id, "active": id, "existing": false, "revision": a["revision"], "result": result }))
    }

    /// Open project `name` into open document `id` while it is not the active one (`doc.load`, one undo step, the
    /// document then belongs to the project). Returns `{ok, name, result, revision}` as `POST /api/projects/open`.
    pub(crate) async fn load_project_into(self: &Arc<Self>, id: &str, name: &str, origin: Origin) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let (id, owned) = (id.to_owned(), name.to_owned());
        self.blocking(move |st| {
            let (name, content) = st.projects.read(&owned)?;
            let inv = commands::Invocation { id: "doc.load".into(), params: json!({ "content": content }) };
            let _s = lock(&st.session);
            let mut d = lock(&st.documents);
            let o = d.others.iter_mut().find(|o| o.id == id).ok_or_else(|| {
                EngineError::Conflict(format!("document '{id}' was closed or made active while the project was read"))
            })?;
            let out = o.session.execute(inv, origin)?;
            o.project = ProjectState::saved(name.clone(), out.revision);
            tracing::info!(document = %id, project = %name, "project opened into a new document");
            Ok(json!({ "ok": true, "name": name, "result": out.result, "revision": out.revision }))
        })
        .await
    }

    /// `POST /api/documents/{id}/close`. Returns `{ok, closed, active}`.
    pub fn close_document(&self, id: &str, discard: bool) -> Result<Value, EngineError> {
        let active = self.active_document_id();
        // Check first, without holding anything across the activation below.
        {
            let s = lock(&self.session);
            let p = lock(&self.project);
            let d = lock(&self.documents);
            if !d.order.iter().any(|o| o == id) {
                return Err(EngineError::NotFound(format!("no open document with id '{id}'")));
            }
            if d.order.len() == 1 {
                return Err(EngineError::Conflict("the last open document cannot be closed".into()));
            }
            let dirty = if d.active == id {
                p.dirty(s.document().revision)
            } else {
                d.others.iter().find(|o| o.id == id).is_some_and(|o| o.project.dirty(o.session.document().revision))
            };
            if dirty && !discard {
                return Err(EngineError::Conflict(format!(
                    "document '{id}' has unsaved changes; save it or close it with discard: true"
                )));
            }
        }
        if active == id {
            let next = {
                let d = lock(&self.documents);
                let pos = d.order.iter().position(|o| o == id).unwrap_or(0);
                let before = pos.checked_sub(1).and_then(|i| d.order.get(i));
                before.or_else(|| d.order.get(pos + 1)).cloned()
            };
            let next = next.ok_or_else(|| EngineError::Internal("no other document to activate".into()))?;
            self.activate_document(&next, Origin::Ui)?;
        }
        let active = {
            let _s = lock(&self.session);
            let mut d = lock(&self.documents);
            if d.active == id {
                // Activated again by someone else in between: keep it open.
                return Err(EngineError::Conflict(format!("document '{id}' became active again; close it again")));
            }
            // Checked again: an edit may have landed between the first check and the switch above.
            if !discard && d.others.iter().find(|o| o.id == id).is_some_and(|o| o.project.dirty(o.session.document().revision)) {
                return Err(EngineError::Conflict(format!(
                    "document '{id}' has unsaved changes; save it or close it with discard: true"
                )));
            }
            d.others.retain(|o| o.id != id);
            d.order.retain(|o| o != id);
            d.active.clone()
        };
        tracing::info!(document = id, "document closed");
        self.emit(Event::DocumentsChanged { active: active.clone() });
        Ok(json!({ "ok": true, "closed": id, "active": active }))
    }
}

/// The document routes (merged into `/api`).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/documents", get(list).post(create))
        .route("/documents/{id}/activate", post(activate))
        .route("/documents/{id}/close", post(close))
        .route("/layers/unsupported", get(unsupported))
}

/// Per layer of the active document, the LightBurn settings it keeps that the job will not execute (LAY-034,
/// `lbrn::unexecuted_settings`): `{layers: [{layer_id, name, unsupported: [{tag, setting, value, sub_layer?, active,
/// note}]}]}`, only layers that have some.
pub fn unsupported_settings(doc: &doc::Document) -> Value {
    let layers: Vec<Value> = doc
        .layers
        .iter()
        .filter_map(|l| {
            let u = lbrn::unexecuted_settings(l);
            (!u.is_empty()).then(|| json!({ "layer_id": l.id, "name": l.name, "unsupported": u }))
        })
        .collect();
    json!({ "layers": layers })
}

async fn unsupported(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    axum::Json(unsupported_settings(&st.document()))
}

async fn list(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    axum::Json(st.documents_view())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    project: Option<String>,
    activate: Option<bool>,
    origin: Option<Value>,
}

/// An optional JSON body (`{}` when empty).
fn body<T: for<'de> Deserialize<'de> + Default>(bytes: &[u8]) -> Result<T, EngineError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes).map_err(|e| EngineError::BadRequest(format!("bad body: {e}")))
}

async fn create(State(st): State<Arc<AppState>>, bytes: axum::body::Bytes) -> Result<Response, EngineError> {
    let b: CreateBody = body(&bytes)?;
    let origin = parse_origin(b.origin.as_ref())?;
    let v = st.open_document(b.project, b.activate.unwrap_or(true), origin).await?;
    let status = if v["existing"] == true { StatusCode::OK } else { StatusCode::CREATED };
    Ok((status, axum::Json(v)).into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateBody {
    origin: Option<Value>,
}

async fn activate(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    bytes: axum::body::Bytes,
) -> Result<axum::Json<Value>, EngineError> {
    let b: ActivateBody = body(&bytes)?;
    let origin = parse_origin(b.origin.as_ref())?;
    Ok(axum::Json(st.activate_document(&id, origin)?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloseBody {
    discard: Option<bool>,
}

async fn close(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    bytes: axum::body::Bytes,
) -> Result<axum::Json<Value>, EngineError> {
    let b: CloseBody = body(&bytes)?;
    Ok(axum::Json(st.close_document(&id, b.discard.unwrap_or(false))?))
}
