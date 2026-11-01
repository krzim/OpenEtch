//! Pending runs. `POST /api/runs` and the MCP tool `job_request_run` plan and encode the current document into a
//! [`Run`] that waits here; only the UI approval endpoint (module `approve`) can start one.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use commands::Origin;
use doc::Document;
use lmc::Packet;
use machine::{MachineProfile, RunReport};
use plan::{Stats, Target};
use serde::Serialize;

use crate::EngineError;

/// Most runs that may wait for approval at once.
pub const MAX_PENDING: usize = 8;
/// Finished runs kept for the status page.
pub const KEEP_FINISHED: usize = 20;

/// Where a run is in its life: `pending` -> `approved` -> (`awaiting_confirm` ->) `running` -> `finished` |
/// `failed` | `aborted` (stopped while streaming), or `cancelled` (stopped or refused before streaming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a person to approve it in the UI with the operator PIN.
    Pending,
    /// Approved; about to start (or about to wait for the physical input).
    Approved,
    /// Approved; waiting for the physical confirmation input (see `pin::PhysicalConfirm`).
    AwaitingConfirm,
    /// Being streamed to the board.
    Running,
    /// Ran to the end.
    Finished,
    /// The board or link failed; the abort sequence was sent.
    Failed,
    /// Stopped by a stop request while `running` (streaming); the abort sequence was sent.
    Aborted,
    /// Never streamed: stop pressed while pending, approved or awaiting confirmation, the document changed, or the
    /// physical input was not pressed in time.
    Cancelled,
}

impl RunState {
    /// True once the run can no longer change.
    pub fn is_final(self) -> bool {
        matches!(self, RunState::Finished | RunState::Failed | RunState::Aborted | RunState::Cancelled)
    }
}

/// A planned, encoded job.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub state: RunState,
    pub origin: Origin,
    /// Document revision the packets were made from. Approval is refused once the document has moved on.
    pub revision: u64,
    pub summary: String,
    pub stats: Stats,
    pub packets: Arc<Vec<Packet>>,
    pub created_unix_s: u64,
    pub error: Option<String>,
    pub report: Option<RunReport>,
}

/// The JSON view of a [`Run`] (no packets).
#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: String,
    pub state: RunState,
    pub origin: Origin,
    pub revision: u64,
    /// True when the document changed after the run was requested; such a run cannot be approved.
    pub stale: bool,
    pub summary: String,
    pub stats: Stats,
    pub packet_count: usize,
    pub created_unix_s: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<RunReport>,
}

impl Run {
    pub fn view(&self, current_revision: u64) -> RunView {
        RunView {
            id: self.id.clone(),
            state: self.state,
            origin: self.origin.clone(),
            revision: self.revision,
            stale: self.state == RunState::Pending && self.revision != current_revision,
            summary: self.summary.clone(),
            stats: self.stats,
            packet_count: self.packets.len(),
            created_unix_s: self.created_unix_s,
            error: self.error.clone(),
            report: self.report.clone(),
        }
    }
}

/// Every run the engine knows about.
#[derive(Debug, Default)]
pub struct RunStore {
    runs: Vec<Run>,
    next: u64,
}

impl RunStore {
    /// Store a new pending run. Refused when [`MAX_PENDING`] runs already wait.
    pub fn add(&mut self, origin: Origin, revision: u64, prepared: PreparedJob) -> Result<&Run, EngineError> {
        if self.runs.iter().filter(|r| r.state == RunState::Pending).count() >= MAX_PENDING {
            return Err(EngineError::Conflict(format!(
                "{MAX_PENDING} runs are already waiting for approval; approve or stop them first"
            )));
        }
        self.next += 1;
        let created_unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.runs.push(Run {
            id: format!("run-{}", self.next),
            state: RunState::Pending,
            origin,
            revision,
            summary: prepared.summary,
            stats: prepared.stats,
            packets: Arc::new(prepared.packets),
            created_unix_s,
            error: None,
            report: None,
        });
        self.prune();
        Ok(self.runs.last().expect("just pushed"))
    }

    pub fn get(&self, id: &str) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Run> {
        self.runs.iter_mut().find(|r| r.id == id)
    }

    /// All runs, oldest first.
    pub fn views(&self, current_revision: u64) -> Vec<RunView> {
        self.runs.iter().map(|r| r.view(current_revision)).collect()
    }

    /// Runs waiting for approval, oldest first.
    pub fn pending(&self, current_revision: u64) -> Vec<RunView> {
        self.runs.iter().filter(|r| r.state == RunState::Pending).map(|r| r.view(current_revision)).collect()
    }

    /// Cancel every run that is not streaming yet (`pending`, `approved`, `awaiting_confirm`); returns how many. An
    /// approved run's task sees the stop epoch change and ends without sending anything.
    pub fn cancel_not_streaming(&mut self, reason: &str) -> usize {
        let mut n = 0;
        let waiting = |r: &&mut Run| matches!(r.state, RunState::Pending | RunState::Approved | RunState::AwaitingConfirm);
        for r in self.runs.iter_mut().filter(waiting) {
            r.state = RunState::Cancelled;
            r.error = Some(reason.to_owned());
            r.packets = Arc::new(Vec::new());
            n += 1;
        }
        n
    }

    /// Drop the oldest finished runs beyond [`KEEP_FINISHED`] (and their packets).
    fn prune(&mut self) {
        let finished = |r: &Run| r.state.is_final();
        let mut excess = self.runs.iter().filter(|r| finished(r)).count().saturating_sub(KEEP_FINISHED);
        self.runs.retain(|r| {
            if excess > 0 && finished(r) {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Move an approved run to a non-final state (`awaiting_confirm`, `running`).
    pub fn set_state(&mut self, id: &str, state: RunState) {
        if let Some(r) = self.get_mut(id).filter(|r| !r.state.is_final()) {
            r.state = state;
        }
    }

    /// Record how an approved run ended (`state` is final).
    pub fn finish(&mut self, id: &str, state: RunState, report: Option<RunReport>, error: Option<String>) {
        debug_assert!(state.is_final());
        if let Some(r) = self.get_mut(id) {
            r.state = state;
            r.report = report;
            r.error = error;
            r.packets = Arc::new(Vec::new());
        }
        self.prune();
    }
}

/// A job ready to be stored as a run.
#[derive(Debug, Clone)]
pub struct PreparedJob {
    pub packets: Vec<Packet>,
    pub stats: Stats,
    pub summary: String,
}

/// The layers' names as people see them (`"Fill", "Outline"`), each once, in first-use order; a layer the document
/// no longer has shows its id.
fn layer_names<'a>(doc: &Document, ids: impl Iterator<Item = &'a str>) -> String {
    let mut seen = Vec::new();
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    let names: Vec<String> = seen
        .iter()
        .map(|id| match doc.layers.iter().find(|l| l.id == *id) {
            Some(l) => format!("\"{}\"", l.name),
            None => (*id).to_owned(),
        })
        .collect();
    names.join(", ")
}

/// Plan, encode and check `doc` for this machine. Nothing is sent anywhere: the result is only data.
///
/// Besides `plan`'s own refusals, the packets go through [`machine::check_job`] (the profile's hard limits) and a
/// read-back with [`plan::simulate`], so a run that would be refused at approval time is refused now instead.
pub fn prepare_job(doc: &Document, target: &Target, profile: &MachineProfile) -> Result<PreparedJob, EngineError> {
    let job = plan::plan_job(doc, target)?;
    let packets = plan::encode(&job, target)?;
    machine::check_job(profile, &packets)?;
    let sim = plan::simulate(&packets, target);
    let marks = sim.segments.iter().filter(|s| s.kind == plan::SegmentKind::Mark).count();
    if marks == 0 {
        return Err(EngineError::Plan(plan::PlanError::Empty));
    }
    let stats = job.stats;
    let summary = format!(
        "{} polylines on {}, {:.1} mm marked, about {:.1} s",
        stats.polyline_count,
        layer_names(doc, job.passes.iter().map(|p| p.layer_id.as_str())),
        stats.mark_length_mm,
        stats.estimated_time_s
    );
    Ok(PreparedJob { packets, stats, summary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn prepared() -> PreparedJob {
        PreparedJob { packets: vec![Packet::empty()], stats: Stats::default(), summary: "s".into() }
    }

    #[test]
    fn summaries_name_layers_by_their_names() {
        // Regression: the pending run read "on layer-1, layer-21" instead of the layer names.
        let mut d = Document::default();
        d.layers[0].name = "Fill".into();
        let mut outline = d.layers[0].clone();
        outline.id = "layer-21".into();
        outline.name = "Outline".into();
        d.layers.push(outline);
        let ids = ["layer-1", "layer-21", "layer-1", "layer-9"];
        assert_eq!(layer_names(&d, ids.into_iter()), "\"Fill\", \"Outline\", layer-9");
    }

    #[test]
    fn pending_runs_are_capped_and_cancelled() {
        let mut store = RunStore::default();
        for _ in 0..MAX_PENDING {
            store.add(Origin::Ui, 1, prepared()).unwrap();
        }
        assert!(matches!(store.add(Origin::Ui, 1, prepared()), Err(EngineError::Conflict(_))));
        assert_eq!(store.cancel_not_streaming("stop"), MAX_PENDING);
        assert!(store.pending(1).is_empty());
        assert!(store.add(Origin::Ui, 1, prepared()).is_ok());
    }

    #[test]
    fn stop_cancels_every_run_not_yet_streaming() {
        let mut store = RunStore::default();
        let ids: Vec<String> = (0..5).map(|_| store.add(Origin::Ui, 1, prepared()).unwrap().id.clone()).collect();
        store.set_state(&ids[1], RunState::Approved);
        store.set_state(&ids[2], RunState::AwaitingConfirm);
        store.set_state(&ids[3], RunState::Running);
        store.finish(&ids[4], RunState::Finished, None, None);
        assert_eq!(store.cancel_not_streaming("stop"), 3);
        let states: Vec<RunState> = ids.iter().map(|id| store.get(id).unwrap().state).collect();
        assert_eq!(
            states,
            [RunState::Cancelled, RunState::Cancelled, RunState::Cancelled, RunState::Running, RunState::Finished]
        );
    }

    #[test]
    fn views_mark_stale_runs() {
        let mut store = RunStore::default();
        let id = store.add(Origin::Ui, 3, prepared()).unwrap().id.clone();
        assert!(!store.get(&id).unwrap().view(3).stale);
        assert!(store.get(&id).unwrap().view(4).stale);
    }

    #[test]
    fn finished_runs_are_pruned() {
        let mut store = RunStore::default();
        for _ in 0..(KEEP_FINISHED + 5) {
            let id = store.add(Origin::Ui, 1, prepared()).unwrap().id.clone();
            store.finish(&id, RunState::Failed, None, Some("x".into()));
        }
        assert_eq!(store.views(1).len(), KEEP_FINISHED);
    }

    #[test]
    fn states_serialise_as_the_contract_names() {
        let names: Vec<Value> = [
            RunState::Pending,
            RunState::Approved,
            RunState::AwaitingConfirm,
            RunState::Running,
            RunState::Finished,
            RunState::Failed,
            RunState::Aborted,
            RunState::Cancelled,
        ]
        .iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
        assert_eq!(
            Value::Array(names),
            serde_json::json!([
                "pending",
                "approved",
                "awaiting_confirm",
                "running",
                "finished",
                "failed",
                "aborted",
                "cancelled"
            ])
        );
    }

    #[test]
    fn final_states_do_not_move() {
        let mut store = RunStore::default();
        let id = store.add(Origin::Ui, 1, prepared()).unwrap().id.clone();
        store.set_state(&id, RunState::Running);
        store.finish(&id, RunState::Aborted, None, Some("stop".into()));
        store.set_state(&id, RunState::Running);
        let v = store.get(&id).unwrap().view(1);
        assert_eq!((v.state, v.packet_count, v.error.as_deref()), (RunState::Aborted, 0, Some("stop")));
    }
}
