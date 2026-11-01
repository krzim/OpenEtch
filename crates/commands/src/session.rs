//! [`Session`]: the live document and its Fusion-style timeline.
//!
//! Every successful edit (one `execute`, or one `batch`) is recorded as a [`HistoryStep`]: the edit invocations as
//! they were run, who ran them, a summary, and a snapshot of the document after it. A **marker** says how many steps
//! are applied: `marker == n` means steps `1..=n` are applied and later steps are rolled back (still kept, greyed out
//! in the UI). Step indices are 1-based, so `marker == 0` is the document before the first step and
//! `marker == steps.len()` is the end of the timeline.
//!
//! - [`Session::goto`] moves the marker anywhere, instantly (snapshots).
//! - [`Session::undo`] / [`Session::redo`] move the marker back or forward over one step (failed steps, which change
//!   nothing, are skipped).
//! - A new edit while rolled back is **inserted** right after the marker, and every later step is **replayed** from
//!   its recorded invocations on top of it so its snapshot is current; the marker stays right after the new step,
//!   so the later steps stay rolled back until the marker is moved past them. A later step that no longer applies
//!   (e.g. it moved an object an inserted or deleted step removed) is kept with state `failed` and its error, and
//!   changes nothing.
//! - [`Session::delete_step`] removes one step and replays the steps after it.
//!
//! Ids stay stable across replays: each step remembers the document's `next_id` when it first ran and is replayed
//! from the same value, and a newly recorded step starts above every id the timeline has ever allocated, so an
//! inserted step never takes an id a later step creates or refers to.

use doc::Document;
use serde::Serialize;
use serde_json::Value;

use crate::{CommandError, Invocation, Origin, Outcome, Registry, Result, Risk};

/// Most steps a timeline keeps. Older applied steps are folded into the starting document (they can no longer be
/// rolled back); with nothing applied, the newest rolled-back steps are dropped instead.
pub const MAX_STEPS: usize = 500;

/// Strings in step parameters longer than this are shortened in [`Session::history_view`] (file contents).
const MAX_VIEW_STRING: usize = 512;

/// Where a step stands relative to the marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// Applied (at or before the marker).
    Active,
    /// After the marker: kept but not applied.
    RolledBack,
    /// Its replay failed: kept, changes nothing, `error` says why. Shown red.
    Failed,
}

/// One recorded edit on the timeline.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryStep {
    /// 1-based position on the timeline (changes when steps are inserted or deleted before it).
    pub index: usize,
    /// The document revision this step produced when it was recorded.
    pub revision: u64,
    /// The edit command ids, comma-separated.
    pub command: String,
    /// The edit invocations as they ran (queries in the same batch are not recorded).
    pub invocations: Vec<Invocation>,
    pub origin: Origin,
    /// One line per command, `; `-separated (from the last time the step was applied).
    pub summary: String,
    pub state: StepState,
    /// Why the last replay failed (state `failed` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The document after this step; `None` when it failed (the document is then the previous step's).
    #[serde(skip)]
    after: Option<Document>,
    /// `next_id` when the step first ran; replays start from it so they allocate the same ids.
    #[serde(skip)]
    next_id_before: u64,
}

/// The old name of [`HistoryStep`].
pub type HistoryEntry = HistoryStep;

/// `GET /api/history`: the marker and every step (large parameters shortened).
#[derive(Debug, Clone, Serialize)]
pub struct HistoryView {
    /// Index of the last applied step; 0 when none is.
    pub marker: usize,
    pub steps: Vec<HistoryStep>,
}

/// A live editing session: the document and its timeline.
pub struct Session {
    registry: Registry,
    /// The document before the first step.
    base: Document,
    steps: Vec<HistoryStep>,
    /// Number of applied steps.
    marker: usize,
    /// The current document: the snapshot at the marker, with the live revision.
    doc: Document,
    /// Highest `next_id` any document on the timeline has reached.
    high_water: u64,
}

/// What [`apply_all`] did.
struct Applied {
    outcomes: Vec<Value>,
    /// The edit invocations, for the timeline.
    edits: Vec<Invocation>,
    /// `(command id, summary)` per edit.
    summaries: Vec<(String, String)>,
}

impl Applied {
    fn command(&self) -> String {
        self.summaries.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")
    }

    fn summary(&self) -> String {
        self.summaries.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>().join("; ")
    }
}

/// Run `invs` on `doc` in order. Before the first edit, `doc.next_id` is set to `start_id`. On error `doc` is left
/// part-way (callers work on a copy). A result holding NaN/infinity is refused.
fn apply_all(registry: &Registry, doc: &mut Document, invs: Vec<Invocation>, start_id: u64) -> Result<Applied> {
    let mut applied = Applied { outcomes: Vec::with_capacity(invs.len()), edits: Vec::new(), summaries: Vec::new() };
    for inv in invs {
        let entry = registry.entry(&inv.id)?;
        let params = if inv.params.is_null() { Value::Object(Default::default()) } else { inv.params };
        let edit = entry.spec.risk != Risk::Query;
        if edit {
            if applied.edits.is_empty() {
                doc.next_id = start_id;
            }
            applied.edits.push(Invocation { id: inv.id.clone(), params: params.clone() });
        }
        let (result, summary) = (entry.apply)(doc, params)?;
        if edit {
            applied.summaries.push((inv.id, summary));
        }
        applied.outcomes.push(result);
    }
    if !applied.edits.is_empty() {
        // Backstop for every command: JSON cannot store NaN or infinity and planning cannot use them, so a
        // document holding one (e.g. from transforms that overflow) is never committed.
        if let Some(problem) = doc.numeric_problem() {
            return Err(CommandError::Failed(format!(
                "refused: the result would contain unusable geometry ({problem}); nothing was changed"
            )));
        }
        if let Some(id) = duplicate_id(doc) {
            return Err(CommandError::Failed(format!("refused: the result would use the id {id} twice")));
        }
    }
    Ok(applied)
}

/// The first id used by more than one object, layer or group (only a replay that went wrong could produce one).
fn duplicate_id(doc: &Document) -> Option<String> {
    let mut seen = std::collections::HashSet::new();
    doc.objects
        .iter()
        .map(|o| &o.id)
        .chain(doc.layers.iter().map(|l| &l.id))
        .chain(doc.groups.iter().map(|g| &g.id))
        .find(|id| !seen.insert(*id))
        .cloned()
}

/// `v` with every string longer than [`MAX_VIEW_STRING`] replaced by a note of its size.
fn elide(v: &Value) -> Value {
    match v {
        Value::String(s) if s.len() > MAX_VIEW_STRING => Value::String(format!("<{} bytes omitted>", s.len())),
        Value::Array(a) => Value::Array(a.iter().map(elide).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), elide(v))).collect()),
        other => other.clone(),
    }
}

impl Session {
    pub fn new(doc: Document) -> Self {
        Session {
            registry: Registry::new(),
            base: doc.clone(),
            high_water: doc.next_id,
            doc,
            steps: Vec::new(),
            marker: 0,
        }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Every step on the timeline, oldest first (applied, rolled back and failed).
    pub fn history(&self) -> &[HistoryStep] {
        &self.steps
    }

    /// Index of the last applied step; 0 when none is.
    pub fn marker(&self) -> usize {
        self.marker
    }

    /// The timeline for clients: marker and steps, with file contents in parameters shortened.
    pub fn history_view(&self) -> HistoryView {
        let steps = self
            .steps
            .iter()
            .map(|s| HistoryStep {
                index: s.index,
                revision: s.revision,
                command: s.command.clone(),
                invocations: s
                    .invocations
                    .iter()
                    .map(|i| Invocation { id: i.id.clone(), params: elide(&i.params) })
                    .collect(),
                origin: s.origin.clone(),
                summary: s.summary.clone(),
                state: s.state,
                error: s.error.clone(),
                after: None,
                next_id_before: s.next_id_before,
            })
            .collect();
        HistoryView { marker: self.marker, steps }
    }

    /// Run one command. `Query` commands do not create a step or bump the revision.
    pub fn execute(&mut self, inv: Invocation, origin: Origin) -> Result<Outcome> {
        self.batch(vec![inv], origin).map(|mut v| v.remove(0))
    }

    /// Run several commands as one step. All-or-nothing: on the first error the document is unchanged. While rolled
    /// back, the step is inserted after the marker and the later steps are replayed (see the module docs).
    pub fn batch(&mut self, invs: Vec<Invocation>, origin: Origin) -> Result<Vec<Outcome>> {
        let mut work = self.doc.clone();
        let start_id = self.doc.next_id.max(self.high_water);
        let applied = apply_all(&self.registry, &mut work, invs, start_id)?;
        if applied.edits.is_empty() {
            let revision = self.doc.revision;
            return Ok(applied.outcomes.into_iter().map(|result| Outcome { result, revision }).collect());
        }
        let revision = self.doc.revision + 1;
        work.revision = revision;
        self.high_water = self.high_water.max(work.next_id);
        let step = HistoryStep {
            index: 0,
            revision,
            command: applied.command(),
            summary: applied.summary(),
            invocations: applied.edits,
            origin,
            state: StepState::Active,
            error: None,
            after: Some(work.clone()),
            next_id_before: start_id,
        };
        let pos = self.marker;
        self.steps.insert(pos, step);
        self.marker += 1;
        self.replay_from(pos + 1);
        self.doc = work;
        self.trim();
        self.refresh();
        Ok(applied.outcomes.into_iter().map(|result| Outcome { result, revision }).collect())
    }

    /// Move the marker back over the last applied step that changed something; returns the new revision.
    pub fn undo(&mut self) -> Result<u64> {
        let j = (0..self.marker).rev().find(|&j| self.steps[j].error.is_none()).ok_or(CommandError::NothingTo("undo"))?;
        self.marker = j;
        Ok(self.restore())
    }

    /// Move the marker forward over the next step that changes something; returns the new revision.
    pub fn redo(&mut self) -> Result<u64> {
        let j = (self.marker..self.steps.len())
            .find(|&j| self.steps[j].error.is_none())
            .ok_or(CommandError::NothingTo("redo"))?;
        self.marker = j + 1;
        Ok(self.restore())
    }

    /// Roll the timeline to `marker` (0 = before the first step, `history().len()` = the end). Returns the
    /// revision; moving to where the marker already is changes nothing and keeps the revision.
    pub fn goto(&mut self, marker: usize) -> Result<u64> {
        if marker > self.steps.len() {
            return Err(CommandError::Failed(format!(
                "marker {marker} is past the end of the timeline ({} steps)",
                self.steps.len()
            )));
        }
        if marker == self.marker {
            return Ok(self.doc.revision);
        }
        self.marker = marker;
        Ok(self.restore())
    }

    /// Remove step `index` (1-based) and replay the steps after it. A marker after it moves back by one. Returns
    /// the new revision.
    pub fn delete_step(&mut self, index: usize) -> Result<u64> {
        if index == 0 || index > self.steps.len() {
            return Err(CommandError::NotFound {
                message: format!("no step {index} on the timeline (steps are 1..={})", self.steps.len()),
                candidates: Vec::new(),
            });
        }
        let pos = index - 1;
        self.steps.remove(pos);
        if pos < self.marker {
            self.marker -= 1;
        }
        self.replay_from(pos);
        Ok(self.restore())
    }

    /// The document after the first `n` steps.
    fn snapshot(&self, n: usize) -> &Document {
        self.steps[..n].iter().rev().find_map(|s| s.after.as_ref()).unwrap_or(&self.base)
    }

    /// Re-run steps `pos..` (0-based) from their invocations, each on the snapshot before it.
    fn replay_from(&mut self, pos: usize) {
        for i in pos..self.steps.len() {
            let mut work = self.snapshot(i).clone();
            let invs = self.steps[i].invocations.clone();
            let start_id = self.steps[i].next_id_before;
            let result = apply_all(&self.registry, &mut work, invs, start_id);
            let step = &mut self.steps[i];
            match result {
                Ok(applied) => {
                    self.high_water = self.high_water.max(work.next_id);
                    work.revision = step.revision;
                    step.command = applied.command();
                    step.summary = applied.summary();
                    step.after = Some(work);
                    step.error = None;
                }
                Err(e) => {
                    step.after = None;
                    step.error = Some(e.to_string());
                }
            }
        }
    }

    /// Make the document the snapshot at the marker with a new revision; returns it.
    fn restore(&mut self) -> u64 {
        let revision = self.doc.revision + 1;
        self.doc = self.snapshot(self.marker).clone();
        self.doc.revision = revision;
        self.refresh();
        revision
    }

    /// Keep at most [`MAX_STEPS`] steps.
    fn trim(&mut self) {
        while self.steps.len() > MAX_STEPS {
            if self.marker > 0 {
                let first = self.steps.remove(0);
                if let Some(after) = first.after {
                    self.base = after;
                }
                self.marker -= 1;
            } else {
                self.steps.pop();
            }
        }
    }

    /// Recompute every step's index and state from its position and the marker.
    fn refresh(&mut self) {
        for (i, s) in self.steps.iter_mut().enumerate() {
            s.index = i + 1;
            s.state = if s.error.is_some() {
                StepState::Failed
            } else if i < self.marker {
                StepState::Active
            } else {
                StepState::RolledBack
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inv(id: &str, params: Value) -> Invocation {
        Invocation { id: id.into(), params }
    }

    fn exec(s: &mut Session, id: &str, params: Value) -> Value {
        match s.execute(inv(id, params), Origin::Ui) {
            Ok(o) => o.result,
            Err(e) => panic!("{id} failed: {e}"),
        }
    }

    fn states(s: &Session) -> Vec<StepState> {
        s.history().iter().map(|h| h.state).collect()
    }

    fn x0(s: &Session, id: &str) -> f64 {
        (s.document().object(id).unwrap().bounds().x0 * 1e6).round() / 1e6
    }

    use StepState::{Active, Failed, RolledBack};

    /// rect obj-2 (10x10 at the origin), move +5, move +5.
    fn three_steps() -> Session {
        let mut s = Session::new(Document::default());
        exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}));
        exec(&mut s, "object.transform", json!({"ids": ["obj-2"], "translate": [5, 0]}));
        exec(&mut s, "object.transform", json!({"ids": ["obj-2"], "translate": [5, 0]}));
        s
    }

    #[test]
    fn goto_rolls_back_and_forward_without_losing_steps() {
        let mut s = three_steps();
        assert_eq!(s.marker(), 3);
        assert_eq!(x0(&s, "obj-2"), 5.0);
        let rev = s.goto(1).unwrap();
        assert_eq!(rev, 4);
        assert_eq!(x0(&s, "obj-2"), -5.0);
        assert_eq!(states(&s), [Active, RolledBack, RolledBack]);
        assert_eq!(s.goto(1).unwrap(), 4, "same marker: no new revision");
        s.goto(0).unwrap();
        assert!(s.document().objects.is_empty());
        s.goto(3).unwrap();
        assert_eq!(x0(&s, "obj-2"), 5.0);
        assert_eq!(s.document().revision, 6);
        assert!(s.goto(4).is_err());
        let v = s.history_view();
        assert_eq!(v.marker, 3);
        assert_eq!(v.steps.iter().map(|h| h.index).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn undo_and_redo_move_the_marker_by_one_step() {
        let mut s = three_steps();
        s.undo().unwrap();
        assert_eq!(s.marker(), 2);
        assert_eq!(states(&s), [Active, Active, RolledBack]);
        s.redo().unwrap();
        assert_eq!(s.marker(), 3);
        assert!(matches!(s.redo(), Err(CommandError::NothingTo("redo"))));
        s.goto(0).unwrap();
        assert!(matches!(s.undo(), Err(CommandError::NothingTo("undo"))));
    }

    #[test]
    fn edit_while_rolled_back_is_inserted_and_later_steps_replay() {
        let mut s = three_steps();
        s.goto(1).unwrap();
        // Inserted after step 1: a layer change, then the two moves replay on top of it.
        exec(&mut s, "layer.add", json!({"name": "Cut"}));
        assert_eq!(s.marker(), 2);
        assert_eq!(s.history()[1].command, "layer.add");
        assert_eq!(states(&s), [Active, Active, RolledBack, RolledBack]);
        assert_eq!(x0(&s, "obj-2"), -5.0, "later steps stay rolled back");
        s.goto(4).unwrap();
        assert_eq!(x0(&s, "obj-2"), 5.0);
        assert_eq!(s.document().layers.len(), 2);
        assert_eq!(states(&s), [Active; 4]);
    }

    #[test]
    fn inserted_steps_never_take_ids_later_steps_use() {
        let mut s = Session::new(Document::default());
        exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})); // obj-2
        exec(&mut s, "shape.ellipse", json!({"rx_mm": 2, "ry_mm": 2})); // obj-3
        exec(&mut s, "object.transform", json!({"ids": ["obj-3"], "translate": [20, 0]}));
        s.goto(1).unwrap();
        let r = exec(&mut s, "shape.rect", json!({"width_mm": 1, "height_mm": 1, "center": [-30, 0]}));
        let new_id = r["created"][0].as_str().unwrap().to_string();
        assert_ne!(new_id, "obj-3");
        s.goto(4).unwrap();
        assert_eq!(states(&s), [Active; 4]);
        // The move still applies to the ellipse, not to the inserted rect.
        assert_eq!(x0(&s, "obj-3"), 18.0);
        assert_eq!(x0(&s, &new_id), -30.5);
    }

    #[test]
    fn steps_that_no_longer_apply_are_kept_as_failed_and_skipped() {
        let mut s = three_steps();
        s.goto(1).unwrap();
        exec(&mut s, "object.delete", json!({"ids": ["obj-2"]}));
        assert_eq!(states(&s), [Active, Active, Failed, Failed]);
        let failed = &s.history()[2];
        assert!(failed.error.as_deref().unwrap().contains("obj-2"), "{:?}", failed.error);
        let v = serde_json::to_value(s.history_view()).unwrap();
        assert_eq!(v["steps"][2]["state"], "failed");
        assert_eq!(v["steps"][1]["state"], "active");
        assert!(v["steps"][0].get("error").is_none());
        // Moving to the end applies nothing more; redo has nothing left that changes anything.
        s.goto(4).unwrap();
        assert!(s.document().objects.is_empty());
        s.goto(2).unwrap();
        assert!(matches!(s.redo(), Err(CommandError::NothingTo("redo"))));
        // Undo skips failed steps too.
        s.goto(4).unwrap();
        s.undo().unwrap();
        assert_eq!(s.marker(), 1);
        // Deleting the delete makes the moves apply again.
        s.delete_step(2).unwrap();
        assert_eq!(states(&s), [Active, RolledBack, RolledBack]);
        s.goto(3).unwrap();
        assert_eq!(x0(&s, "obj-2"), 5.0);
    }

    #[test]
    fn delete_step_replays_and_moves_the_marker() {
        let mut s = three_steps();
        let rev = s.document().revision;
        assert_eq!(s.delete_step(2).unwrap(), rev + 1);
        assert_eq!(s.marker(), 2);
        assert_eq!(x0(&s, "obj-2"), 0.0, "one move left");
        assert_eq!(states(&s), [Active, Active]);
        s.delete_step(1).unwrap();
        assert_eq!(states(&s), [Failed]);
        assert!(s.document().objects.is_empty());
        assert!(matches!(s.delete_step(5), Err(CommandError::NotFound { .. })));
        assert!(s.delete_step(0).is_err());
    }

    #[test]
    fn history_view_shortens_file_contents() {
        let mut s = Session::new(Document::default());
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><!-- {} --><circle cx="10" cy="10" r="5"/></svg>"#,
            "x".repeat(2000)
        );
        exec(&mut s, "art.import_svg", json!({"svg": svg, "name": "Logo"}));
        let v = s.history_view();
        let params = &v.steps[0].invocations[0].params;
        assert_eq!(params["name"], "Logo");
        assert!(params["svg"].as_str().unwrap().contains("bytes omitted"));
        // The session itself keeps the whole invocation for replays.
        assert_eq!(s.history()[0].invocations[0].params["svg"].as_str().unwrap().len(), svg.len());
    }

    #[test]
    fn timeline_is_capped() {
        let mut s = Session::new(Document::default());
        exec(&mut s, "shape.rect", json!({"width_mm": 1, "height_mm": 1}));
        for _ in 0..MAX_STEPS + 5 {
            exec(&mut s, "object.transform", json!({"ids": ["obj-2"], "translate": [0.01, 0]}));
        }
        assert_eq!(s.history().len(), MAX_STEPS);
        assert_eq!(s.marker(), MAX_STEPS);
        s.goto(0).unwrap();
        assert_eq!(s.document().objects.len(), 1, "the oldest steps were folded into the start");
    }
}
