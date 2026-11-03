//! Several open documents (LightBurn's File > New Window), moved from the engine's `documents` module, which keeps
//! the locking and the routes. Exactly one document is **active**: its [`Session`] and [`ProjectState`] are the
//! caller's own (the engine's session and project locks, webcore's fields); the others wait here.
//!
//! Activation swaps the sessions and raises the activated document's revision above every revision any open
//! document has had ([`Session::raise_revision`]), so revisions stay unique across documents.

use commands::Session;
use serde_json::{Value, json};

use crate::error::AppError;
use crate::project::{ProjectState, same_name_ignoring_case};

/// The id of the document the engine starts with.
pub const DEFAULT_DOCUMENT: &str = "default";
/// Most documents open at once.
pub const MAX_DOCUMENTS: usize = 16;

/// An open document that is not the active one.
pub struct OpenDoc {
    pub id: String,
    pub session: Session,
    pub project: ProjectState,
}

/// The open documents (see the module docs).
pub struct DocSet {
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

/// What [`DocSet::activate`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activated {
    /// False when the document was already the active one.
    pub changed: bool,
    /// The active document's revision.
    pub revision: u64,
    /// Its timeline's `(marker, steps)`.
    pub hist: (usize, usize),
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

fn no_document(id: &str) -> AppError {
    AppError::NotFound(format!("no open document with id '{id}'"))
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

    /// `GET /api/documents` (`session` and `project` are the active document's).
    pub fn view(&self, session: &Session, project: &ProjectState) -> Value {
        let rows: Vec<Value> = self
            .order
            .iter()
            .filter_map(|id| {
                if *id == self.active {
                    Some(row(id, session, project, true))
                } else {
                    self.others.iter().find(|o| o.id == *id).map(|o| row(id, &o.session, &o.project, false))
                }
            })
            .collect();
        json!({ "active": self.active, "documents": rows })
    }

    /// Make open document `id` the active one: its session and project are swapped into `session` and `project`,
    /// the previous ones wait here.
    pub fn activate(&mut self, session: &mut Session, project: &mut ProjectState, id: &str) -> Result<Activated, AppError> {
        if self.active == id {
            return Ok(Activated { changed: false, revision: session.document().revision, hist: (session.marker(), session.history().len()) });
        }
        let pos = self.others.iter().position(|o| o.id == id).ok_or_else(|| no_document(id))?;
        let mut incoming = self.others.remove(pos);
        self.max_revision = self.max_revision.max(session.document().revision);
        let old = incoming.session.document().revision;
        let new = self.max_revision + 1;
        incoming.session.raise_revision(new);
        self.max_revision = new;
        // The content did not change: a clean or saved state stays clean at the raised revision.
        if incoming.project.clean_revision == Some(old) {
            incoming.project.clean_revision = Some(new);
        }
        if incoming.project.saved_revision == Some(old) {
            incoming.project.saved_revision = Some(new);
        }
        std::mem::swap(session, &mut incoming.session);
        std::mem::swap(project, &mut incoming.project);
        let previous = std::mem::replace(&mut self.active, id.to_owned());
        self.others.push(OpenDoc { id: previous, session: incoming.session, project: incoming.project });
        Ok(Activated { changed: true, revision: new, hist: (session.marker(), session.history().len()) })
    }

    /// Add an open, inactive document (New Window). Returns its id.
    pub fn add(&mut self, session: Session, project: ProjectState) -> Result<String, AppError> {
        if self.order.len() >= MAX_DOCUMENTS {
            return Err(AppError::Conflict(format!("{MAX_DOCUMENTS} documents are open; close one first")));
        }
        let id = self.new_id();
        self.order.push(id.clone());
        self.others.push(OpenDoc { id: id.clone(), session, project });
        Ok(id)
    }

    /// The open document holding project `name` (ignoring case), if any (`project` is the active document's).
    pub fn with_project(&self, project: &ProjectState, name: &str) -> Option<String> {
        if project.name.as_deref().is_some_and(|n| same_name_ignoring_case(n, name)) {
            return Some(self.active.clone());
        }
        self.others
            .iter()
            .find(|o| o.project.name.as_deref().is_some_and(|n| same_name_ignoring_case(n, name)))
            .map(|o| o.id.clone())
    }

    /// Set the project state of open document `id` (`project` is the active document's).
    pub fn set_project_of(&mut self, project: &mut ProjectState, id: &str, state: ProjectState) {
        if self.active == id {
            *project = state;
        } else if let Some(o) = self.others.iter_mut().find(|o| o.id == id) {
            o.project = state;
        }
    }

    /// Refuse closing `id` (404 unknown, 409 the last one or unsaved without `discard`).
    pub fn check_close(&self, session: &Session, project: &ProjectState, id: &str, discard: bool) -> Result<(), AppError> {
        if !self.order.iter().any(|o| o == id) {
            return Err(no_document(id));
        }
        if self.order.len() == 1 {
            return Err(AppError::Conflict("the last open document cannot be closed".into()));
        }
        let dirty = if self.active == id {
            project.dirty(session.document().revision)
        } else {
            self.others.iter().find(|o| o.id == id).is_some_and(|o| o.project.dirty(o.session.document().revision))
        };
        if dirty && !discard {
            return Err(unsaved(id));
        }
        Ok(())
    }

    /// The document to activate when the active one, `id`, closes: the one opened before it, else the next.
    pub fn successor(&self, id: &str) -> Result<String, AppError> {
        let pos = self.order.iter().position(|o| o == id).unwrap_or(0);
        let before = pos.checked_sub(1).and_then(|i| self.order.get(i));
        before.or_else(|| self.order.get(pos + 1)).cloned().ok_or_else(|| AppError::Internal("no other document to activate".into()))
    }

    /// Close inactive document `id`. Returns the active document's id.
    pub fn remove(&mut self, id: &str, discard: bool) -> Result<String, AppError> {
        if self.active == id {
            // Activated again by someone else in between: keep it open.
            return Err(AppError::Conflict(format!("document '{id}' became active again; close it again")));
        }
        // Checked again: an edit may have landed between the first check and the switch.
        if !discard && self.others.iter().find(|o| o.id == id).is_some_and(|o| o.project.dirty(o.session.document().revision)) {
            return Err(unsaved(id));
        }
        self.others.retain(|o| o.id != id);
        self.order.retain(|o| o != id);
        Ok(self.active.clone())
    }
}

fn unsaved(id: &str) -> AppError {
    AppError::Conflict(format!("document '{id}' has unsaved changes; save it or close it with discard: true"))
}

/// Per layer of a document, the LightBurn settings it keeps that the job will not execute (LAY-034,
/// `lbrn::unexecuted_settings`): `{layers: [{layer_id, name, unsupported: [...]}]}`, only layers that have some
/// (`GET /api/layers/unsupported`).
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
