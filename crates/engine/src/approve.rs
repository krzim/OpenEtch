//! UI approval of a pending run: the only code in the engine that creates a `machine::Armed`, and so the only path
//! to `Controller::run` (which needs one). It is reachable only from `POST /api/runs/{id}/approve`; the HTTP handler
//! refuses requests carrying an MCP client header before calling [`approve`]. Nothing in the MCP server calls it.

use std::sync::Arc;

use machine::Armed;
use serde_json::{Value, json};

use crate::runs::RunState;
use crate::{AppState, EngineError, Event};

/// Arm and start pending run `id` on the machine in a background task. Refused unless the run is pending, made from
/// the current document revision, and no other run holds the machine.
pub(crate) async fn approve(state: &Arc<AppState>, id: &str) -> Result<Value, EngineError> {
    let current = state.revision();
    let packets = {
        let mut store = state.runs();
        let run = store.get_mut(id).ok_or_else(|| EngineError::NotFound(format!("no run with id '{id}'")))?;
        if run.state != RunState::Pending {
            let st = format!("{:?}", run.state).to_lowercase();
            return Err(EngineError::Conflict(format!("run '{id}' is {st}, not pending")));
        }
        if run.revision != current {
            run.state = RunState::Cancelled;
            run.error = Some("the document changed after the run was requested".into());
            return Err(EngineError::Conflict(format!(
                "the document changed since run '{id}' was requested (revision {} -> {current}); request a new run",
                run.revision
            )));
        }
        if !state.machine.try_begin_run() {
            return Err(EngineError::Conflict("another run is in progress".into()));
        }
        run.state = RunState::Running;
        run.packets.clone()
    };
    // End any frame loop; `Controller::run` stops a frame still on the board itself.
    state.machine.bump_frame_gen();
    let armed = Armed::approved_in_ui(id.to_owned());
    tracing::warn!(run_id = id, "run approved in the UI; starting");
    state.emit(Event::machine("run_started", Some(id.to_owned())));

    let st = state.clone();
    let run_id = id.to_owned();
    tokio::spawn(async move {
        let result = st
            .machine
            .blocking(move |m, ctl| {
                m.ensure_ready(ctl)?;
                ctl.run(&packets, armed)
            })
            .await;
        let outcome = match result {
            Ok(Ok(report)) => Ok(report),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        st.machine.end_run();
        let error = outcome.as_ref().err().cloned();
        if let Some(e) = &error {
            tracing::warn!(run_id = %run_id, error = %e, "run failed");
            st.machine.set_error(Some(e.clone()));
        }
        st.runs().finish(&run_id, outcome);
        let final_state = if error.is_none() { RunState::Completed } else { RunState::Failed };
        st.emit(Event::RunFinished { run_id: run_id.clone(), ok: error.is_none(), state: final_state, error });
        st.emit(Event::machine("run_finished", Some(run_id)));
    });
    Ok(json!({ "ok": true, "run_id": id, "state": "running" }))
}
