//! The foot switch cycle (`--approval click` only): LightBurn's production loop on the profile's Start Marking input
//! (the foot switch, `approve::foot_switch_port`), which `libs::spawn_io_watch` reads and hands to [`pressed`].
//!
//! - With no cycle (and no run holding the board), a press (a release, then `pin::CONFIRM_READS` pressed reads)
//!   starts continuous red-pointer framing of the whole job (every object on an output layer, in the style of the
//!   last frame request: bounds, hull or contour; never only the selection or the tool layers, never the focus light
//!   or live framing) and the cycle is `framing`. A press while the Frame button's frame runs replaces that frame
//!   with the job's.
//! - While the cycle frames, a press burns the job that is being framed: the current document is planned into a run
//!   with the default options (the whole job once, no start input) and approved with `Approver::FootSwitch`; the
//!   cycle is `burning`. When the run finishes, framing starts again on the document as it is then (a serial number
//!   advanced by the burn included), and every further press burns again.
//!
//! Safety rules:
//! - Only the framed revision burns. When the document (its revision) or the device changed since the frame started,
//!   the press frames the new revision instead and says so (`foot_cycle_notice`); a run planned from another
//!   revision (an edit during planning) is discarded and the job re-framed.
//! - Every burn goes through `approve::approve`: the door interlock, Require Framing, Stop At and the stop epoch apply
//!   to each one, and the door is read before planning too. A refused burn (door open, ...) keeps the cycle framing
//!   and says why. A foot cannot tick the Job Checklist, so with the checklist on every burn is refused (Start in the
//!   app then).
//! - Stop (the app, Esc, MCP's `machine_stop`) ends the cycle and its frame ([`end`] from `AppState::stop`; a step
//!   that started before the stop sees the stop epoch change and turns the pointer off again). Stop frame or any
//!   other frame ends it too ([`sync`]), and so does a burn that does not finish (stopped, door opened, failed).
//! - A press while a run holds the board is not read at all (the watcher does not read then), so it never queues a
//!   burn; a press in the moment between a burn's end and the frame restarting is ignored.
//! - Runs waiting as `pending` (requested in the app, or prepared by an agent over MCP) are never approved by the
//!   foot switch: the cycle burns only the current document it framed. Pending runs are started in the app.
//! - The cycle's framing stops after [`IDLE_TIMEOUT_SECS`] without a press (each burn restarts the clock), and the
//!   cycle ends with it.
//!
//! State: `GET /api/machine` `foot_cycle` ([`status_json`]); machine events `foot_cycle` (detail: the new phase,
//! `framing`, `burning` or `off`) and `foot_cycle_notice` (detail: what happened, for a toast).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::approve::{self, Approver};
use crate::frame::{FrameRequest, Pointer};
use crate::libs::lock;
use crate::runs::{RunOptions, RunState};
use crate::{AppState, EngineError, Event};

/// The cycle's framing (and the cycle) ends after this long without a press.
pub const IDLE_TIMEOUT_SECS: u64 = 600;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Off,
    /// Framing document revision `revision` on machine generation `generation` with frame loop `frame_gen`.
    Framing { revision: u64, generation: u64, frame_gen: u64, started: Instant, since_unix_s: u64 },
    /// Burning `revision` as run `run_id`.
    Burning { run_id: String, revision: u64 },
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::Off => "off",
            Phase::Framing { .. } => "framing",
            Phase::Burning { .. } => "burning",
        }
    }
}

/// The cycle's state (one field of `libs::Libs`). See the module docs.
#[derive(Debug, Default)]
pub struct FootCycle {
    phase: Phase,
    /// Bumped when a cycle starts or ends, so a step of a cycle that has ended changes nothing.
    id: u64,
    /// The machine's stop epoch when the cycle started: a stop since then ends it.
    epoch: u64,
    /// The last notice (`GET /api/machine` shows it).
    notice: Option<String>,
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `GET /api/machine` `foot_cycle`: `{available, phase, revision, run_id, since_unix_s, idle_timeout_s, notice}`.
/// `available`: the engine runs in the `click` mode and the profile has a foot switch (Start Marking input).
pub(crate) fn status_json(st: &AppState) -> Value {
    let available = approve::foot_switch_port(st).is_some();
    let c = lock(&st.libs.foot_cycle);
    let mut v = json!({
        "available": available, "phase": c.phase.name(), "revision": null, "run_id": null, "since_unix_s": null,
        "idle_timeout_s": IDLE_TIMEOUT_SECS, "notice": c.notice,
    });
    match &c.phase {
        Phase::Off => {}
        Phase::Framing { revision, since_unix_s, .. } => {
            v["revision"] = json!(revision);
            v["since_unix_s"] = json!(since_unix_s);
        }
        Phase::Burning { run_id, revision } => {
            v["revision"] = json!(revision);
            v["run_id"] = json!(run_id);
        }
    }
    v
}

/// End the cycle (`cycle`: only that one; `None`: whichever runs). `why`: the notice to send, if any. The frame is not
/// touched here: the callers have ended it (Stop) or it has ended ([`sync`]).
pub(crate) fn end(st: &AppState, cycle: Option<u64>, why: Option<&str>) {
    {
        let mut c = lock(&st.libs.foot_cycle);
        if c.phase == Phase::Off || cycle.is_some_and(|id| id != c.id) {
            return;
        }
        c.phase = Phase::Off;
        c.id += 1;
    }
    tracing::info!(reason = why.unwrap_or("stop"), "foot switch cycle ended");
    st.emit(Event::machine("foot_cycle", Some("off".into())));
    if let Some(w) = why {
        notice(st, w);
    }
}

fn notice(st: &AppState, text: &str) {
    lock(&st.libs.foot_cycle).notice = Some(text.to_owned());
    st.emit(Event::machine("foot_cycle_notice", Some(text.to_owned())));
}

/// Called by the watcher on every read cycle: end a framing cycle whose frame is gone (Stop frame, another frame, the
/// idle timeout) or whose foot switch is no longer set up (`watched` false).
pub(crate) fn sync(st: &AppState, watched: bool) {
    let (id, frame_gen, started) = {
        let c = lock(&st.libs.foot_cycle);
        match c.phase {
            Phase::Framing { frame_gen, started, .. } => (c.id, frame_gen, started),
            _ => return,
        }
    };
    if !watched {
        end(st, Some(id), Some("The foot switch is no longer set up; the foot switch cycle ended."));
    } else if !owns_frame(st, frame_gen) {
        let why = if started.elapsed() + Duration::from_secs(2) >= Duration::from_secs(IDLE_TIMEOUT_SECS) {
            "No foot switch press for 10 minutes: framing stopped and the foot switch cycle ended."
        } else {
            "Framing was stopped, so the foot switch cycle ended. Press the foot switch to frame again."
        };
        end(st, Some(id), Some(why));
    }
}

fn owns_frame(st: &AppState, frame_gen: u64) -> bool {
    st.machine.is_framing() && st.machine.frame_gen() == frame_gen
}

/// A press of the foot switch (released, then pressed; the watcher only reads it while no run holds the board).
pub(crate) async fn pressed(st: &Arc<AppState>) {
    let (phase, id) = {
        let c = lock(&st.libs.foot_cycle);
        (c.phase.clone(), c.id)
    };
    match phase {
        Phase::Burning { .. } => tracing::info!("foot switch pressed while the cycle burns; ignored"),
        Phase::Framing { revision, generation, frame_gen, .. } if owns_frame(st, frame_gen) => {
            burn(st, id, revision, generation).await;
        }
        Phase::Off | Phase::Framing { .. } => match frame(st, None).await {
            Ok(_) => tracing::info!("foot switch: framing the job; the next press burns it"),
            Err(e) => {
                tracing::warn!(error = %e, "foot switch: could not frame the job");
                notice(st, &format!("Foot switch: could not frame the job: {e}"));
            }
        },
    }
}

/// The frame of the whole job, in the style of the last frame request (see the module docs).
fn job_frame(last: Option<FrameRequest>) -> FrameRequest {
    FrameRequest {
        ids: None,
        tool_layers_only: false,
        pointer: Pointer::Red,
        live: false,
        continuous: Some(true),
        max_seconds: Some(IDLE_TIMEOUT_SECS),
        ..last.unwrap_or_default()
    }
}

/// Start the cycle's framing of the current job: a new cycle (`cycle: None`) or the next step of cycle `Some(id)`
/// (after a burn, or to frame an edited job). Returns the cycle id. When the cycle ended or a stop came in while the
/// frame started, the frame is stopped again and this fails.
async fn frame(st: &Arc<AppState>, cycle: Option<u64>) -> Result<u64, EngineError> {
    let epoch = match cycle {
        None => st.machine.stop_epoch(),
        Some(id) => {
            let c = lock(&st.libs.foot_cycle);
            if c.id != id {
                return Err(EngineError::Conflict("the foot switch cycle ended".into()));
            }
            c.epoch
        }
    };
    // The Frame button's last request stays what the Start Framing input repeats.
    let previous = lock(&st.libs.last_frame).clone();
    let started = st.frame_start(job_frame(previous.clone())).await;
    *lock(&st.libs.last_frame) = previous;
    started?;
    let frame_gen = st.machine.frame_gen();
    let (generation, revision) = st.libs.framed_at().unwrap_or((st.machine.generation(), st.revision()));
    let id = {
        let mut c = lock(&st.libs.foot_cycle);
        let current = cycle.is_none_or(|id| id == c.id);
        if !current || st.machine.stop_epoch() != epoch {
            None
        } else {
            if cycle.is_none() {
                c.id += 1;
                c.epoch = epoch;
            }
            c.phase = Phase::Framing { revision, generation, frame_gen, started: Instant::now(), since_unix_s: now_unix() };
            Some(c.id)
        }
    };
    let Some(id) = id else {
        // Stopped while the frame started: the pointer goes off again.
        if let Err(e) = st.frame_stop().await {
            tracing::warn!(error = %e, "foot switch cycle: could not stop the frame after a stop");
        }
        return Err(EngineError::Conflict("stopped".into()));
    };
    st.emit(Event::machine("foot_cycle", Some("framing".into())));
    Ok(id)
}

/// True while cycle `id` still runs and no stop came in since it started.
fn active(st: &AppState, id: u64) -> bool {
    let c = lock(&st.libs.foot_cycle);
    c.id == id && c.phase != Phase::Off && st.machine.stop_epoch() == c.epoch
}

/// The second press: burn revision `framed` (framed on machine generation `generation`), unless the job changed.
async fn burn(st: &Arc<AppState>, id: u64, framed: u64, generation: u64) {
    if st.revision() != framed || st.machine.generation() != generation {
        return reframe(st, id).await;
    }
    // The door, Require Framing and the checklist before planning, so a refusal answers at once.
    if let Err(e) = crate::libs::check_before_approve(st, framed, None).await {
        return refused(st, id, &e);
    }
    // Subscribed before approving, so the run's end is never missed.
    let events = st.subscribe();
    let view = match st.request_run_with(commands::Origin::Ui, RunOptions::default()).await {
        Ok(v) => v,
        Err(e) => return refused(st, id, &e),
    };
    let discard = |st: &AppState| {
        st.runs().discard(&view.id);
        st.emit(Event::machine("run_discarded", Some(view.id.clone())));
    };
    if !active(st, id) {
        // Stopped while the job was planned.
        return discard(st);
    }
    if view.revision != framed {
        discard(st);
        return reframe(st, id).await;
    }
    match approve::approve(st, &view.id, Approver::FootSwitch, None).await {
        Ok(_) => {
            {
                let mut c = lock(&st.libs.foot_cycle);
                if c.id == id {
                    c.phase = Phase::Burning { run_id: view.id.clone(), revision: framed };
                }
            }
            tracing::warn!(run_id = %view.id, revision = framed, "foot switch: burning the framed job");
            st.emit(Event::machine("foot_cycle", Some("burning".into())));
            tokio::spawn(after_burn(st.clone(), id, view.id.clone(), events));
        }
        Err(e) => {
            discard(st);
            if st.revision() != framed {
                reframe(st, id).await;
            } else {
                refused(st, id, &e);
            }
        }
    }
}

/// The job changed since it was framed: frame the new version instead of burning.
async fn reframe(st: &Arc<AppState>, id: u64) {
    match frame(st, Some(id)).await {
        Ok(_) => notice(st, "Foot switch: the job changed since it was framed, so it was not burned. Framing the new version: press again to burn it."),
        Err(e) => end(st, Some(id), Some(&format!("Foot switch: could not frame the changed job ({e}); the foot switch cycle ended."))),
    }
}

/// A burn was refused (door open, checklist, Stop At, ...): the cycle keeps framing.
fn refused(st: &AppState, id: u64, e: &EngineError) {
    tracing::warn!(error = %e, "foot switch: burn refused");
    if active(st, id) {
        notice(st, &format!("Foot switch: not burned: {e}. Still framing."));
    }
}

/// Wait for the burn to end; framing starts again after a finished one, anything else ends the cycle.
async fn after_burn(st: Arc<AppState>, id: u64, run_id: String, mut events: broadcast::Receiver<Event>) {
    let state = wait_finished(&st, &run_id, &mut events).await;
    drop(events);
    let burning = {
        let c = lock(&st.libs.foot_cycle);
        c.id == id && matches!(&c.phase, Phase::Burning { run_id: r, .. } if *r == run_id)
    };
    if !burning {
        return;
    }
    if state != RunState::Finished {
        let name = serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        let why = format!("Foot switch: run {run_id} did not finish ({name}); the foot switch cycle ended.");
        return end(&st, Some(id), Some(&why));
    }
    if let Err(e) = frame(&st, Some(id)).await {
        end(&st, Some(id), Some(&format!("Foot switch: could not frame the job again ({e}); the foot switch cycle ended.")));
    }
}

/// How run `run_id` ended: its `run_finished` event (sent after the job counter and serial numbers moved on), or,
/// when events were missed, its recorded final state.
async fn wait_finished(st: &AppState, run_id: &str, events: &mut broadcast::Receiver<Event>) -> RunState {
    loop {
        match tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
            Ok(Ok(Event::RunFinished { id, state, .. })) if id == run_id => return state,
            Ok(Ok(_)) => {}
            Ok(Err(broadcast::error::RecvError::Closed)) => return RunState::Failed,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) | Err(_) => {
                let recorded = st.runs().get(run_id).map(|r| r.state);
                match recorded {
                    None => return RunState::Cancelled,
                    Some(s) if s.is_final() => {
                        // The run's end is recorded just before its event; let the rest of the ending happen.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        return s;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "cycle_tests.rs"]
mod tests;
