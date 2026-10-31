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

/// Where a run is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a person to approve it in the UI.
    Pending,
    Running,
    Completed,
    Failed,
    /// Stopped before it ran (stop pressed, or the document changed).
    Cancelled,
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

    /// Cancel every pending run; returns how many.
    pub fn cancel_pending(&mut self, reason: &str) -> usize {
        let mut n = 0;
        for r in self.runs.iter_mut().filter(|r| r.state == RunState::Pending) {
            r.state = RunState::Cancelled;
            r.error = Some(reason.to_owned());
            r.packets = Arc::new(Vec::new());
            n += 1;
        }
        n
    }

    /// Drop the oldest finished runs beyond [`KEEP_FINISHED`] (and their packets).
    fn prune(&mut self) {
        let finished = |r: &Run| !matches!(r.state, RunState::Pending | RunState::Running);
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

    /// Record how an approved run ended.
    pub fn finish(&mut self, id: &str, result: Result<RunReport, String>) {
        if let Some(r) = self.get_mut(id) {
            match result {
                Ok(report) => {
                    r.state = RunState::Completed;
                    r.report = Some(report);
                }
                Err(e) => {
                    r.state = RunState::Failed;
                    r.error = Some(e);
                }
            }
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
    let layers: Vec<&str> = {
        let mut v: Vec<&str> = job.passes.iter().map(|p| p.layer_id.as_str()).collect();
        v.dedup();
        v
    };
    let stats = job.stats;
    let summary = format!(
        "{} polylines on {}, {:.1} mm marked, about {:.1} s",
        stats.polyline_count,
        layers.join(", "),
        stats.mark_length_mm,
        stats.estimated_time_s
    );
    Ok(PreparedJob { packets, stats, summary })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared() -> PreparedJob {
        PreparedJob { packets: vec![Packet::empty()], stats: Stats::default(), summary: "s".into() }
    }

    #[test]
    fn pending_runs_are_capped_and_cancelled() {
        let mut store = RunStore::default();
        for _ in 0..MAX_PENDING {
            store.add(Origin::Ui, 1, prepared()).unwrap();
        }
        assert!(matches!(store.add(Origin::Ui, 1, prepared()), Err(EngineError::Conflict(_))));
        assert_eq!(store.cancel_pending("stop"), MAX_PENDING);
        assert!(store.pending(1).is_empty());
        assert!(store.add(Origin::Ui, 1, prepared()).is_ok());
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
            store.finish(&id, Err("x".into()));
        }
        assert_eq!(store.views(1).len(), KEEP_FINISHED);
    }
}
