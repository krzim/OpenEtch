//! Running a batch of document commands as one undo step, as `POST /api/batch` does (moved from the engine's
//! `AppState::batch_opening`): the Query/Edit risk gate, the field and layout fill of `doc.new` and
//! `doc.import_lbrn2`, the effect of a replacing command on [`ProjectState`] and the `field_source` annotation of a
//! LightBurn import. Nothing here broadcasts: the caller sends [`crate::event::announce`]'s events.
//!
//! The steps are separate functions because the engine runs them under different locks: [`fill_fields`] before the
//! session lock (its LightBurn lookup may read a file), [`run`] under it.

use commands::{CommandError, Invocation, Origin, Outcome, Risk, Session};
use serde_json::Value;

use crate::error::AppError;
use crate::layout::{fill_field_mm, fill_layout, lacks, lacks_field, lightburn_layout, new_layout};
use crate::project::{ProjectState, display_name};

/// How an invocation changes which project the document belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replaces {
    /// It edits the current document (or does not edit).
    No,
    /// `doc.new`: a new, empty document that counts as clean.
    Clean,
    /// `doc.load` (an uploaded `.openetch`, unless it is a project being opened) or a replacing LightBurn import:
    /// a new document no project holds, so its content is unsaved.
    Unsaved,
}

/// See [`Replaces`].
pub fn replaces(inv: &Invocation) -> Replaces {
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
pub fn content_params(content: String) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("content".into(), Value::String(content));
    Value::Object(params)
}

/// Where the field of a `doc.import_lbrn2` came from: `(index in the batch, source, device id)`.
pub type FieldSource = (usize, String, Option<String>);

/// What [`fill_fields`] looks devices up in.
pub struct FieldContext<'a> {
    /// The active device.
    pub active: &'a machine::Device,
    /// The machine profile's field width (the field of a controllable active device).
    pub profile_field: f64,
    /// A device of the registry by id or name (the one a LightBurn file names).
    pub find_device: &'a dyn Fn(&str) -> Option<machine::Device>,
    /// LightBurn's own device list: the field of the device a file names, and where it came from.
    pub lightburn_field: &'a dyn Fn(&str) -> Option<(f64, String)>,
}

/// Fill in the field and layout of `doc.new` (the active device's) and `doc.import_lbrn2` (the device the file names:
/// a registry device, else LightBurn's, else the active device) when the caller gives none. Returns where each
/// import's field came from.
pub fn fill_fields(invs: &mut [Invocation], ctx: &FieldContext<'_>) -> Vec<FieldSource> {
    let mut field_sources = Vec::new();
    if !invs.iter().any(|i| matches!(i.id.as_str(), "doc.new" | "doc.import_lbrn2")) {
        return field_sources;
    }
    let active = ctx.active;
    for (i, inv) in invs.iter_mut().enumerate() {
        match inv.id.as_str() {
            "doc.new" if lacks_field(inv) && lacks(inv, "workspace_mm") => {
                let field = if active.controllable { ctx.profile_field } else { active.field_mm() };
                fill_layout(inv, &new_layout(active, field));
            }
            "doc.import_lbrn2" if lacks_field(inv) && lacks(inv, "workspace_mm") => {
                let content = inv.params.get("content").and_then(Value::as_str).unwrap_or_default();
                let named = commands::lbrn2_device_name(content);
                let device = named.as_deref().and_then(|n| (ctx.find_device)(n));
                if let Some(d) = device {
                    fill_layout(inv, &lightburn_layout(&d));
                    field_sources.push((i, format!("file device {}", d.display_name), Some(d.id)));
                } else if let Some((f, source)) = (ctx.lightburn_field)(content) {
                    fill_field_mm(inv, f);
                    field_sources.push((i, source, None));
                } else {
                    fill_layout(inv, &lightburn_layout(active));
                    field_sources.push((i, format!("active device {}", active.display_name), Some(active.id.clone())));
                }
            }
            _ => {}
        }
    }
    field_sources
}

/// The display name of a batch: `display` if given, else the `name` of its last replacing `doc.import_lbrn2`.
pub fn batch_display(invs: &[Invocation], display: Option<String>) -> Option<String> {
    display.or_else(|| {
        invs.iter()
            .rev()
            .find(|i| i.id == "doc.import_lbrn2" && replaces(i) == Replaces::Unsaved)
            .and_then(|i| i.params.get("name").and_then(Value::as_str).and_then(display_name))
    })
}

/// The risk gate: only `Query` and `Edit` commands run here (machine actions have their own endpoints). Every edit
/// passes `check_edit` (the engine's production design lock). Returns how the last edit replaces the document.
pub fn gate<E: From<AppError>>(
    session: &Session,
    invs: &[Invocation],
    mut check_edit: impl FnMut(&str) -> Result<(), E>,
) -> Result<Replaces, E> {
    let mut last_edit = Replaces::No;
    for inv in invs {
        if let Some(spec) = session.registry().spec(&inv.id) {
            if !matches!(spec.risk, Risk::Query | Risk::Edit) {
                return Err(AppError::Forbidden(format!("'{}' is a machine action; use the machine endpoints/tools instead", inv.id)).into());
            }
            if spec.risk == Risk::Edit {
                check_edit(&inv.id)?;
                last_edit = replaces(inv);
            }
        }
    }
    Ok(last_edit)
}

/// A batch's result: the outcomes, the revisions before and after, the timeline's `(marker, steps)` and whether a
/// command replaced the document.
#[derive(Debug)]
pub struct Ran {
    pub outcomes: Vec<Outcome>,
    pub before: u64,
    pub after: u64,
    pub hist: (usize, usize),
    pub replaced: bool,
}

/// Run `invs` (already gated and filled) on `session`. A document without a field gets `profile_field` (recorded
/// as a `machine` field source of a LightBurn import). Then [`settle_project`].
pub fn run(
    session: &mut Session,
    mut invs: Vec<Invocation>,
    origin: Origin,
    profile_field: f64,
    field_sources: &mut Vec<FieldSource>,
) -> Result<Ran, CommandError> {
    if session.document().field_mm.is_none() {
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
    let before = session.document().revision;
    let outcomes = session.batch(invs, origin)?;
    let after = session.document().revision;
    Ok(Ran { outcomes, before, after, hist: (session.marker(), session.history().len()), replaced })
}

/// After a batch that changed the revision: with `opened`, the document is project `opened` saved at the new
/// revision; a replacing batch with no project leaves an unsaved document named `display` (clean after `doc.new` /
/// `doc.load` with nothing edited after it in the same batch).
pub fn settle_project(project: &mut ProjectState, ran: &Ran, opened: Option<String>, display: Option<String>, last_edit: Replaces) {
    if ran.after == ran.before {
        return;
    }
    if let Some(name) = opened {
        *project = ProjectState::saved(name, ran.after);
    } else if ran.replaced {
        let clean = last_edit == Replaces::Clean;
        *project = ProjectState {
            name: None,
            saved_revision: None,
            clean_revision: clean.then_some(ran.after),
            display_name: if clean { None } else { display },
        };
    }
}

/// Put each LightBurn import's `field_source` (and `device`) into its result.
pub fn annotate(outcomes: &mut [Outcome], field_sources: Vec<FieldSource>) {
    for (i, source, device) in field_sources {
        if let Some(Value::Object(r)) = outcomes.get_mut(i).map(|o| &mut o.result) {
            r.insert("field_source".into(), Value::String(source));
            if let Some(id) = device {
                r.insert("device".into(), Value::String(id));
            }
        }
    }
}

/// Whether a batch is refused before anything runs: an empty batch.
pub fn check_not_empty(invs: &[Invocation]) -> Result<(), AppError> {
    if invs.is_empty() { Err(AppError::BadRequest("batch has no commands".into())) } else { Ok(()) }
}
