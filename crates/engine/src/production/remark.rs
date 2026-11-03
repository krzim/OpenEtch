//! Re-mark one slot with the value it had (the serial ledger's "this slot failed, mark the same value again"):
//! `POST /api/production/remark {run_id, slot}` (the app) and MCP `serial_remark_request` plan a PENDING run of that
//! slot's pocket only, with the slot's texts and barcodes showing exactly what the ledger recorded for it, and send
//! nothing. It is approved like every run: the operator PIN, or Start on the pending run in the app; never by the foot
//! switch (which burns only the document it framed) and never by an agent. The counter does not move after it.
//!
//! The run is planned from the engine's current document (its design as it is now) with the counter set to the
//! recorded value for that slot; the slot must still exist (the same variable-text offset). It is refused while the
//! document has changed from what the app shows (`revision`), like any run request.

use std::sync::Arc;

use commands::Origin;
use serde_json::{Value, json};

use super::RunRecord;
use crate::runs::{self, RunOptions};
use crate::{AppState, EngineError, Event};

/// Plan a pending re-mark of slot `slot` (1-based) of ledger run `run_id` (the one that ended at `time_unix_s`, when
/// given: run ids start again after a restart). `revision`: the document revision the caller shows, if it knows it.
pub async fn request(
    st: &Arc<AppState>,
    run_id: &str,
    time_unix_s: Option<u64>,
    slot: usize,
    origin: Origin,
    revision: Option<u64>,
) -> Result<Value, EngineError> {
    st.refuse_if_shutting_down("requesting a run")?;
    st.require_controllable()?;
    if st.machine.is_gantry() {
        return Err(EngineError::BadRequest("re-marking a slot is available for galvo (LMC) devices".into()));
    }
    let entry = st
        .production
        .ledger
        .find(run_id, time_unix_s)?
        .ok_or_else(|| EngineError::NotFound(format!("run '{run_id}' is not in the serial ledger")))?;
    let recorded = entry
        .slots
        .iter()
        .find(|s| s.number == slot)
        .cloned()
        .ok_or_else(|| EngineError::NotFound(format!("run '{run_id}' has no slot {slot}")))?;
    let (doc, document_id) = {
        let doc = st.document();
        (doc, st.active_document_id())
    };
    if let Some(shown) = revision
        && shown != doc.revision
    {
        return Err(EngineError::Conflict(format!(
            "the document changed (revision {shown} -> {}); nothing was requested: check it and try again",
            doc.revision
        )));
    }
    let now = commands::slot_plan(&doc);
    let current = now
        .slots
        .iter()
        .find(|s| s.offset == recorded.offset)
        .cloned()
        .ok_or_else(|| EngineError::Conflict(format!("the document has no slot with offset {} any more", recorded.offset)))?;
    // The slot alone, at the recorded value: the counter is set so that its own offset gives that value; empty slots
    // and Stop at End do not apply to a re-mark.
    let mut one = doc.clone();
    one.variables.current = recorded.value.saturating_sub(recorded.offset);
    one.variables.empty_slots.clear();
    one.variables.stop_at_end = false;
    let plan = commands::slot_plan(&one);
    let shown: Vec<String> = plan
        .slots
        .iter()
        .find(|s| s.offset == recorded.offset)
        .map(|s| s.values.iter().map(|v| v.shown.clone()).collect())
        .unwrap_or_default();
    let options = RunOptions { selection: Some(current.objects.clone()), ..RunOptions::default() };
    let config = st.machine.config();
    let generation = config.generation;
    let run_index = st.job.count(&st.active_device().id);
    let correction = st.job.times.correction(&st.active_device().id).factor;
    let opts = options.clone();
    let mut prepared = tokio::task::spawn_blocking(move || {
        let out = commands::prepare_output(&one).map_err(|e| EngineError::BadRequest(format!("text: {e}")))?;
        runs::prepare_job_with(&out, &config.target, &config.profile, &opts, run_index)
    })
    .await??
    .with_correction(correction);
    prepared.generation = generation;
    let label = if shown.is_empty() { recorded.value.to_string() } else { shown.join(", ") };
    prepared.summary = format!("Re-mark slot {slot} ({}) with {label}: {}", recorded.label, prepared.summary);
    let view = {
        let mut store = st.runs();
        st.refuse_if_shutting_down("requesting a run")?;
        if st.machine.generation() != generation {
            return Err(EngineError::Conflict("the device changed while the run was planned; request it again".into()));
        }
        let run = store.add(origin.clone(), doc.revision, prepared)?;
        run.document_id = document_id;
        run.view(doc.revision)
    };
    let mut record_plan = plan.clone();
    record_plan.slots.retain(|s| s.offset == recorded.offset);
    st.production.keep_record(
        &view.id,
        RunRecord {
            plan: record_plan,
            project: st.project_name(),
            document: doc.name.clone(),
            remark_of: Some((run_id.to_owned(), slot)),
        },
    );
    tracing::info!(run_id = %view.id, of = run_id, slot, "re-mark requested; waiting for approval in the UI");
    st.emit(Event::RunPending { run_id: view.id.clone(), origin, summary: view.summary.clone() });
    Ok(json!({ "ok": true, "id": view.id, "run": view, "shown": shown, "message": crate::state::APPROVAL_MESSAGE }))
}
