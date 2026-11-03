//! Production features around the foot switch cycle (OpenEtch's production short list; ARCHITECTURE.md,
//! "Production"):
//!
//! - the production cockpit's state (`GET /api/production`, [`status_json`]) and its design lock ([`lock`]);
//! - notifications to ntfy, a webhook or Home Assistant, and the event feed MCP agents read ([`notify`]);
//! - the serial ledger: which value was marked on which slot in which run, re-marking a slot ([`ledger`],
//!   [`remark`]; the slots themselves are `commands::slots`);
//! - pause at a layer until the operator continues ([`layer_pause`]);
//! - the hour meter and maintenance reminders ([`meter`]);
//! - the extractor's run-on and start gating ([`extractor`]).
//!
//! **Safety.** Nothing here arms a run: the only path to the laser stays `approve` (which calls the hooks below for
//! the gates and the bookkeeping). Notifications are outbound only; no route or tool of this module starts, approves
//! or continues a burn for an agent: Continue at a layer pause is UI-only (or the foot switch), like Resume. Stop is
//! never gated: a layer pause or the cockpit's lock never holds it up.
//!
//! The hooks the rest of the engine calls: [`slot_plan_for_run`] / [`output_document`] / [`record_request`]
//! (`AppState::request_run_at`), [`before_run`] (approval, after the other gates), [`iteration_done`] and
//! [`after_run`] (the run task), [`lock::check_edit`] / [`lock::check_timeline`] (document edits).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use commands::SlotPlan;
use doc::Document;
use serde::Serialize;
use serde_json::{Value, json};

use crate::libs::lock as lock_mutex;
use crate::runs::RunState;
use crate::{AppState, EngineError, Event};

pub mod api;
pub mod extractor;
pub mod layer_pause;
pub mod ledger;
pub mod lock;
pub mod mcp;
pub mod meter;
pub mod notify;
pub mod remark;

#[cfg(test)]
mod tests;

/// Seconds since the Unix epoch.
pub(crate) fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a requested run will mark, kept until it ends (the ledger entry is written then).
#[derive(Debug, Clone)]
pub(crate) struct RunRecord {
    pub plan: SlotPlan,
    pub project: Option<String>,
    pub document: String,
    /// A re-mark of `(run_id, slot number)`: the counter does not move after it.
    pub remark_of: Option<(String, usize)>,
}

/// Most run records kept (pending runs are at most `runs::MAX_PENDING`; finished ones are taken out).
const MAX_RECORDS: usize = 64;
/// Cycle intervals kept for the average.
const KEPT_CYCLES: usize = 20;
/// A finished burn further than this from the previous one starts a new series (no cycle time).
const CYCLE_GAP_S: f64 = 1800.0;

/// Burn and cycle times of the foot switch cycle (or any series of runs).
#[derive(Debug, Default, Clone, Serialize)]
pub struct CycleTimes {
    /// Approval to the end of the last finished run, s.
    pub last_burn_s: Option<f64>,
    /// End of the previous finished run to the end of the last one, s.
    pub last_cycle_s: Option<f64>,
    /// Mean of the last cycles, s.
    pub average_cycle_s: Option<f64>,
    #[serde(skip)]
    recent: VecDeque<f64>,
    #[serde(skip)]
    last_end: Option<Instant>,
}

impl CycleTimes {
    fn finished(&mut self, burn_s: f64, now: Instant) {
        self.last_burn_s = Some(burn_s);
        if let Some(prev) = self.last_end {
            let gap = now.saturating_duration_since(prev).as_secs_f64();
            if gap <= CYCLE_GAP_S {
                self.last_cycle_s = Some(gap);
                self.recent.push_back(gap);
                while self.recent.len() > KEPT_CYCLES {
                    self.recent.pop_front();
                }
            } else {
                self.recent.clear();
                self.last_cycle_s = None;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let avg = (!self.recent.is_empty()).then(|| self.recent.iter().sum::<f64>() / self.recent.len() as f64);
        self.average_cycle_s = avg;
        self.last_end = Some(now);
    }
}

/// The production state (one field of `AppState`). In memory until [`Production::open`] gives it the config
/// directory (tests run without one).
#[derive(Debug, Default)]
pub struct Production {
    dir: Mutex<Option<PathBuf>>,
    pub notify: notify::Notifier,
    pub ledger: ledger::Ledger,
    pub meter: meter::Meter,
    pub extractor: extractor::Extractor,
    pub lock: lock::EditLock,
    pub pause: layer_pause::LayerPause,
    records: Mutex<HashMap<String, RunRecord>>,
    cycles: Mutex<CycleTimes>,
}

impl Production {
    /// Use `<config-dir>` for the ledger, the hour meter, the extractor settings and the notification targets.
    /// Returns notes for the console (unreadable files that were skipped).
    pub fn open(&self, dir: &Path) -> Vec<String> {
        *lock_mutex(&self.dir) = Some(dir.to_path_buf());
        let mut notes = Vec::new();
        self.ledger.open(dir);
        notes.extend(self.meter.open(dir));
        notes.extend(self.extractor.open(dir));
        notes.extend(self.notify.open(dir));
        notes
    }

    /// Forget a removed device's hour meter, reminders and extractor settings.
    pub(crate) fn forget_device(&self, id: &str) {
        self.meter.forget(|d| d == id);
        self.extractor.forget(|d| d == id);
    }

    /// The burn and cycle times.
    pub fn cycle_times(&self) -> CycleTimes {
        lock_mutex(&self.cycles).clone()
    }

    pub(crate) fn keep_record(&self, run_id: &str, record: RunRecord) {
        let mut r = lock_mutex(&self.records);
        if r.len() >= MAX_RECORDS
            && let Some(oldest) = r.keys().next().cloned()
        {
            r.remove(&oldest);
        }
        r.insert(run_id.to_owned(), record);
    }

    fn take_record(&self, run_id: &str) -> Option<RunRecord> {
        lock_mutex(&self.records).remove(run_id)
    }
}

/// Start the background tasks: the notification listener and sender (module `notify`).
pub fn spawn(st: Arc<AppState>) {
    notify::spawn(st);
}

/// `machine` event `production` (detail: what changed): clients refetch `GET /api/production`.
pub(crate) fn changed(st: &AppState, what: &str) {
    st.emit(Event::machine("production", Some(what.to_owned())));
}

// ---- hooks of a run's life -----------------------------------------------------------------------------------------

/// The slots the next run of `doc` marks, or `None` when the document has no serial-number or CSV slots. A plan that
/// cannot run (Stop at End with every value used) is refused here, before anything is planned.
pub(crate) fn slot_plan_for_run(doc: &Document) -> Result<Option<SlotPlan>, EngineError> {
    let plan = commands::slot_plan(doc);
    if plan.slots.is_empty() {
        return Ok(None);
    }
    if let Some(why) = plan.refusal() {
        return Err(EngineError::Conflict(why));
    }
    Ok(Some(plan))
}

/// The document a run plans: empty and past-End slots left out, the marked slots at their values. Unchanged without
/// slots.
pub(crate) fn output_document(doc: &Document, plan: Option<&SlotPlan>) -> Document {
    match plan {
        Some(p) => commands::apply_slot_plan(doc, p),
        None => doc.clone(),
    }
}

/// A run was requested: keep what it marks for its ledger entry.
pub(crate) fn record_request(st: &AppState, run_id: &str, plan: Option<SlotPlan>, document: &str) {
    let Some(plan) = plan else { return };
    st.production.keep_record(
        run_id,
        RunRecord { plan, project: st.project_name(), document: document.to_owned(), remark_of: None },
    );
}

/// Before a run is armed (from `approve`, after the door, checklist and framing gates): the extractor's gate, and the
/// extractor switched on for the job when set so.
pub(crate) async fn before_run(st: &Arc<AppState>) -> Result<(), EngineError> {
    extractor::before_run(st).await
}

/// A run was approved and holds the machine: the extractor switched on for the job (module `extractor`).
pub(crate) fn run_started(st: &Arc<AppState>) {
    extractor::run_started(st);
}

/// One iteration finished on the board: the hour meter (the board's mark time when it gave one, else the estimate)
/// and Stop At reached.
pub(crate) fn iteration_done(st: &Arc<AppState>, device: &str, measured_s: Option<f64>, estimated_s: f64) {
    meter::record(st, device, measured_s, estimated_s);
    if let Some(stop_at) = st.job.stop_at()
        && st.job.count(device) == stop_at
    {
        let count = st.job.count(device);
        st.production.notify.emit(
            st,
            notify::Kind::StopAtReached,
            notify::Operation::Mark,
            &format!("Stop At reached: {count} of {stop_at} done"),
            None,
        );
    }
}

/// A run ended (from the run task, after the job log line): the ledger entry, the burn and cycle times, the serial
/// counter (moved on by what was marked, not for a re-mark) and the extractor's run-on.
pub(crate) fn after_run(st: &Arc<AppState>, run_id: &str, device: &str, state: RunState, started: Instant, completed: u32) {
    let record = st.production.take_record(run_id);
    let finished = state == RunState::Finished;
    if finished {
        lock_mutex(&st.production.cycles).finished(started.elapsed().as_secs_f64(), Instant::now());
    }
    if let Some(r) = &record
        && matches!(state, RunState::Finished | RunState::Aborted | RunState::Failed)
    {
        st.production.ledger.append(&ledger::entry(run_id, device, state, completed, r));
    }
    if finished {
        advance_counter(st, run_id, record.as_ref());
    }
    extractor::after_run(st);
    changed(st, "run_ended");
}

/// Move the serial number / CSV row on after a finished run: as before when every slot was marked (`var.advance`),
/// else by what was marked (`var.set_serial current`). Not after a re-mark, and not when the counter was changed
/// meanwhile (someone set it during the burn).
fn advance_counter(st: &Arc<AppState>, run_id: &str, record: Option<&RunRecord>) {
    if record.is_some_and(|r| r.remark_of.is_some()) {
        return;
    }
    let Some(plan) = record.map(|r| &r.plan).filter(|p| p.skips()) else {
        st.advance_variables_after_run();
        return;
    };
    if !plan.auto_advance {
        return;
    }
    let now = st.document().variables.current;
    if now != plan.current {
        tracing::info!(run_id, before = plan.current, now, "serial counter changed during the run; not moved on");
        return;
    }
    let inv = commands::Invocation { id: "var.set_serial".into(), params: json!({ "current": plan.next_current() }) };
    if let Err(e) = st.execute(inv, commands::Origin::Script) {
        tracing::warn!(error = %e, "could not move the serial counter on after the run");
    }
}

// ---- the cockpit's status ------------------------------------------------------------------------------------------

/// `GET /api/production` (and MCP `production_status`): everything the production cockpit shows.
pub fn status_json(st: &AppState) -> Value {
    let doc = st.document();
    let plan = commands::slot_plan(&doc);
    let refusal = plan.refusal();
    let job = st.job_status();
    let device = st.active_device().id;
    let cycles = st.production.cycle_times();
    let count = st.job.count(&device);
    let stop_at = st.job.stop_at();
    let left = stop_at.map(|s| s.saturating_sub(count));
    #[allow(clippy::cast_precision_loss)]
    let eta_s = match (left, cycles.average_cycle_s.or(cycles.last_burn_s)) {
        (Some(n), Some(c)) => Some(n as f64 * c),
        _ => None,
    };
    json!({
        "revision": doc.revision,
        "document": doc.name,
        "project": st.project_name(),
        "device": device,
        "approval_mode": st.approval_mode().as_str(),
        "lock": st.production.lock.status_json(),
        "job": job,
        "foot_cycle": crate::cycle::status_json(st),
        "slots": plan,
        "slots_refusal": refusal,
        "layer_pause": st.production.pause.status_json(),
        "cycle": {
            "last_burn_s": cycles.last_burn_s,
            "last_cycle_s": cycles.last_cycle_s,
            "average_cycle_s": cycles.average_cycle_s,
            "count": count,
            "stop_at": stop_at,
            "left": left,
            "eta_s": eta_s,
        },
        "maintenance": meter::status_json(st, &device),
        "extractor": extractor::status_json(st, &device),
        "ledger": st.production.ledger.read(10).unwrap_or_default(),
        "notifications": st.production.notify.summary_json(),
    })
}
