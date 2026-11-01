//! Run approval: the only code in the engine that creates a `machine::Armed`, and so the only path to
//! `Controller::run` (which needs one). It is reachable only from `POST /api/runs/{id}/approve`, which refuses requests
//! carrying an MCP client header; [`approve`] itself requires the operator PIN (module `pin`). Nothing in the MCP
//! server calls it.
//!
//! Stop is never lost: [`approve`] captures the machine's stop epoch while it claims the machine (inside the runs
//! lock, which `AppState::stop` also takes to bump the epoch), and the run never starts streaming once the epoch has
//! changed, even if the stop's abort sequence won the controller lock first and cleared the abort request. A run
//! stopped before it holds the controller ends `cancelled` (nothing was sent for it); one stopped after it has
//! become `running` ends `aborted`.

use std::sync::Arc;
use std::time::Instant;

use lmc::Packet;
use machine::{Armed, ControllerError, RunReport};
use serde_json::{Value, json};

use crate::hw::Machine;
use crate::pin::{CONFIRM_READS, PhysicalConfirm};
use crate::runs::RunState;
use crate::{AppState, EngineError, Event};

/// Check the operator PIN, then arm and start pending run `id` in a background task. Refused unless the PIN is
/// right, the run is pending, made from the current document revision, and no other run holds the machine.
pub(crate) async fn approve(state: &Arc<AppState>, id: &str, pin: Option<&str>) -> Result<Value, EngineError> {
    if let Err((e, locked_until)) = state.approval.pin.verify_reporting(pin) {
        if let Some(until_unix_s) = locked_until {
            state.emit(Event::ApprovalLocked { until_unix_s });
        }
        return Err(state.approval.pin.error(e));
    }
    let confirm = state.approval.confirm;
    let current = state.revision();
    let (packets, epoch, run_state) = {
        let mut store = state.runs();
        let run = store.get_mut(id).ok_or_else(|| EngineError::NotFound(format!("no run with id '{id}'")))?;
        if run.state != RunState::Pending {
            let st = serde_json::to_value(run.state).ok().and_then(|v| v.as_str().map(str::to_owned));
            return Err(EngineError::Conflict(format!("run '{id}' is {}, not pending", st.unwrap_or_default())));
        }
        if run.generation != state.machine.generation() {
            run.state = RunState::Cancelled;
            run.error = Some("the device changed after the run was requested".into());
            return Err(EngineError::Conflict(format!(
                "run '{id}' was planned for another device; request a new run"
            )));
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
        // Captured after claiming the machine, still under the runs lock (see the module docs).
        let epoch = state.machine.stop_epoch();
        run.state = if confirm.is_some() { RunState::AwaitingConfirm } else { RunState::Approved };
        (run.packets.clone(), epoch, run.state)
    };
    // End any frame loop; `Controller::run` stops a frame still on the board itself.
    state.machine.bump_frame_gen();
    tracing::warn!(run_id = id, physical_confirm = confirm.is_some(), "run approved with the operator PIN");
    state.emit(Event::machine("run_approved", Some(id.to_owned())));

    let st = state.clone();
    let run_id = id.to_owned();
    tokio::spawn(async move {
        let (final_state, report, error) = execute(&st, &run_id, packets, epoch, confirm).await;
        st.machine.end_run();
        if let Some(e) = &error {
            tracing::warn!(run_id = %run_id, state = ?final_state, error = %e, "run did not finish");
            if final_state == RunState::Failed {
                st.machine.set_error(Some(e.clone()));
            }
        }
        st.runs().finish(&run_id, final_state, report, error.clone());
        let ok = final_state == RunState::Finished;
        st.emit(Event::RunFinished { id: run_id.clone(), ok, state: final_state, error });
        st.emit(Event::machine("run_finished", Some(run_id)));
    });
    let state_name = serde_json::to_value(run_state).unwrap_or(Value::Null);
    Ok(json!({ "ok": true, "id": id, "run_id": id, "state": state_name }))
}

type Outcome = (RunState, Option<RunReport>, Option<String>);

/// Wait for the physical input (if configured), then stream the run unless a stop came in since approval.
async fn execute(
    st: &Arc<AppState>,
    id: &str,
    packets: Arc<Vec<Packet>>,
    epoch: u64,
    confirm: Option<PhysicalConfirm>,
) -> Outcome {
    if let Some(c) = confirm {
        st.emit(Event::machine("awaiting_confirm", Some(id.to_owned())));
        match wait_for_confirm(&st.machine, epoch, c).await {
            ConfirmEnd::Confirmed => {}
            ConfirmEnd::Stopped => {
                return (RunState::Cancelled, None, Some("stopped while waiting for the physical confirmation".into()));
            }
            ConfirmEnd::TimedOut => {
                let msg = format!(
                    "input {} was not pressed within {} s of approval; request and approve the run again",
                    c.input_bit,
                    c.timeout.as_secs()
                );
                return (RunState::Cancelled, None, Some(msg));
            }
            ConfirmEnd::Failed(e) => return (RunState::Failed, None, Some(format!("physical confirmation: {e}"))),
            ConfirmEnd::Panicked(e) => return panicked(st, e).await,
        }
    }
    let armed = Armed::approved_in_ui(id.to_owned());
    let (task_state, run_id) = (st.clone(), id.to_owned());
    let result = st
        .machine
        .blocking(move |m, ctl| {
            // With the controller held, and under the runs lock that `AppState::stop` bumps the epoch under: either
            // the stop came first (it has marked this run cancelled; nothing is sent) or the run is now `running`
            // and a later stop aborts it.
            {
                let mut runs = task_state.runs();
                if m.stop_epoch() != epoch {
                    return None;
                }
                runs.set_state(&run_id, RunState::Running);
            }
            task_state.emit(Event::machine("run_started", Some(run_id)));
            Some(m.ensure_ready(ctl).and_then(|()| ctl.run(&packets, armed)))
        })
        .await;
    match result {
        Ok(None) => (RunState::Cancelled, None, Some("cancelled by stop before streaming".into())),
        Ok(Some(Ok(report))) => (RunState::Finished, Some(report), None),
        Ok(Some(Err(ControllerError::Aborted))) => (RunState::Aborted, None, Some("aborted by stop".into())),
        Ok(Some(Err(e))) => (RunState::Failed, None, Some(e.to_string())),
        // The blocking task panicked: the controller may have left the MO open or the laser-enable bit set.
        Err(e) => panicked(st, e.to_string()).await,
    }
}

/// A run task panicked: send the abort sequence and report the run failed.
async fn panicked(st: &Arc<AppState>, e: String) -> Outcome {
    tracing::error!(error = %e, "run task panicked; sending the abort sequence");
    let abort = match st.machine.abort().await {
        Ok(_) => "abort sequence sent".to_owned(),
        Err(a) => format!("abort failed too ({a}); use the hardware E-stop"),
    };
    (RunState::Failed, None, Some(format!("internal error ({e}); {abort}")))
}

enum ConfirmEnd {
    Confirmed,
    Stopped,
    TimedOut,
    Failed(String),
    Panicked(String),
}

/// Poll `ReadPort` until input `c.input_bit` has been seen released and then pressed for [`CONFIRM_READS`] reads in a
/// row. Ends early on a stop (epoch change), a read error or the timeout.
async fn wait_for_confirm(machine: &Arc<Machine>, epoch: u64, c: PhysicalConfirm) -> ConfirmEnd {
    let deadline = Instant::now() + c.timeout;
    let mask = 1u16 << (c.input_bit & 15);
    let (mut released, mut pressed) = (false, 0);
    loop {
        if machine.stop_epoch() != epoch {
            return ConfirmEnd::Stopped;
        }
        if Instant::now() >= deadline {
            return ConfirmEnd::TimedOut;
        }
        let read = machine
            .blocking(move |m, ctl| {
                if m.stop_epoch() != epoch {
                    return Ok(None);
                }
                m.ensure_ready(ctl)?;
                m.read_inputs(ctl).map(Some)
            })
            .await;
        match read {
            Ok(Ok(Some(bits))) if bits & mask == 0 => (released, pressed) = (true, 0),
            Ok(Ok(Some(_))) if released => {
                pressed += 1;
                if pressed >= CONFIRM_READS {
                    return ConfirmEnd::Confirmed;
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => return ConfirmEnd::Stopped,
            Ok(Err(e)) => return ConfirmEnd::Failed(e.to_string()),
            Err(e) => return ConfirmEnd::Panicked(e.to_string()),
        }
        tokio::time::sleep(c.poll).await;
    }
}
