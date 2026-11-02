//! Run approval: the only code in the engine that creates a `machine::Armed`, and so the only path to
//! `Controller::run_parts` (which needs one). It is reachable only from `POST /api/runs/{id}/approve` and
//! `POST /api/job/start`, which refuse requests carrying an MCP client header, and from the foot switch cycle
//! (module `cycle`). Nothing in the MCP server calls it.
//!
//! **Who may approve** ([`Approver`], by the engine's [`ApprovalMode`], `--approval`): in the `pin` mode (the
//! default) only a request with the operator PIN (module `pin`). In the `click` mode there is no PIN: the Start
//! button of the web app (the endpoints first check the browser's `Sec-Fetch-Site: same-origin` and `Origin`,
//! `security::same_origin_browser_request`), which with [`start_now`] plans and approves the current document in one
//! action (LightBurn's Start), or the foot switch cycle (module `cycle`: the first press frames the job, the next
//! burns the revision that was framed). A non-browser client could imitate the browser headers (on loopback no token
//! is needed); the foot switch is the path backed by hardware. Every other gate (Stop At, the Job Checklist, Require
//! Framing, the door interlock, the stop epoch) is the same in both modes.
//!
//! Stop is never lost: [`approve`] captures the machine's stop epoch while it claims the machine (inside the runs
//! lock, which `AppState::stop` also takes to bump the epoch), and no iteration starts streaming once the epoch has
//! changed, even if the stop's abort sequence won the controller lock first and cleared the abort request. A run
//! stopped before it holds the controller ends `cancelled` (nothing was sent for it); one stopped after it has
//! become `running` ends `aborted`.
//!
//! **Iterations** (LightBurn's Repeat, Run Continuously and Start Marking input, module `job`): one approval covers
//! the iterations the run's options ask for: `repeat` times, or until stopped for `continuous`; each iteration gets
//! its own arming token here, after (when the run asks for it) the start-marking input has been released and
//! pressed. Before every iteration after the first, the run ends when Stop At is reached or the operator asked to
//! end after the current iteration. The job counter counts every iteration that ran to the end. A run approved with
//! the foot switch counts that press as its first iteration's start-input press (it is the same input, released and
//! pressed; the foot switch cycle's runs never ask for the start input); later iterations wait for their own press.
//! The physical confirmation (`--confirm-input`) still waits for its own press after any approval.
//!
//! **Shutdown** (module `shutdown`): once the engine is shutting down every approval and Start is refused (503
//! `shutting_down`); the machine claim refuses it atomically, so a run approved in that moment is either refused or
//! waited for. A run already approved keeps every gate and is left to finish.

use std::sync::Arc;
use std::time::{Duration, Instant};

use machine::grbl::GrblError;
use machine::{Armed, ControllerError, DriverJob, MachineDriver, RunReport};
use serde_json::{Value, json};

use crate::hw::{LinkKind, Machine};
use crate::pin::{ApprovalMode, CONFIRM_POLL, CONFIRM_READS, PhysicalConfirm};
use crate::runs::{RunOptions, RunState};
use crate::{AppState, EngineError, Event};

/// Longest wait for the start-marking input before one iteration (LightBurn waits without limit; an armed run
/// waiting for ever is not wanted, so it ends after an hour).
pub const START_INPUT_TIMEOUT: Duration = Duration::from_secs(3600);

/// Who approves a run (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Approver<'a> {
    /// The `pin` mode: the operator PIN the request carried (`None`: it carried none).
    Pin(Option<&'a str>),
    /// The `click` mode: the web app's Start button (the endpoint checked the browser headers).
    Click,
    /// The `click` mode: a press of the foot switch (module `cycle`).
    FootSwitch,
}

impl Approver<'_> {
    fn label(self) -> &'static str {
        match self {
            Approver::Pin(_) => "operator PIN",
            Approver::Click => "Start in the app",
            Approver::FootSwitch => "foot switch",
        }
    }
}

/// Check who approves (`approver`, against the engine's approval mode), then arm and start pending run `id` in a
/// background task. Refused unless the approver fits the mode (and in the `pin` mode the PIN is right), the run is
/// pending, made from the current document revision, no other run holds the machine, and the job counter has not
/// reached Stop At.
///
/// `checklist`: the Job Checklist items the operator ticked (module `libs`: Require Framing Before Start, the Job
/// Checklist and the door interlock are checked before the machine is claimed).
pub(crate) async fn approve(
    state: &Arc<AppState>,
    id: &str,
    approver: Approver<'_>,
    checklist: Option<&[bool]>,
) -> Result<Value, EngineError> {
    // Before the PIN is looked at, so a refusal costs no attempt (checked again when the machine is claimed).
    state.refuse_if_shutting_down("approving a run")?;
    match (state.approval_mode(), approver) {
        (ApprovalMode::Pin, Approver::Pin(pin)) => {
            // The simulator test PIN (`--sim-pin`) never approves anything on a real laser, whatever started the
            // engine.
            if state.approval.pin.is_sim() && state.machine.kind() != crate::hw::LinkKind::Simulated {
                return Err(crate::pin::sim_pin_on_real_link());
            }
            if let Err((e, locked_until)) = state.approval.pin.verify_reporting(pin) {
                if let Some(until_unix_s) = locked_until {
                    state.emit(Event::ApprovalLocked { until_unix_s });
                }
                return Err(state.approval.pin.error(e));
            }
        }
        (ApprovalMode::Click, Approver::Click | Approver::FootSwitch) => {}
        (mode, _) => {
            let how = match mode {
                ApprovalMode::Pin => "with the operator PIN",
                ApprovalMode::Click => "with Start in the app or the foot switch",
            };
            return Err(EngineError::Approval {
                code: "approval_mode",
                message: format!("this engine approves runs {how} (--approval {})", mode.as_str()),
            });
        }
    }
    let confirm = state.approval.confirm;
    let pending_revision = state.runs().get(id).filter(|r| r.state == RunState::Pending).map(|r| r.revision);
    if let Some(rev) = pending_revision {
        crate::libs::check_before_approve(state, rev, checklist).await?;
    }
    let (current, active_document) = state.active_revision_and_document();
    let device = state.active_device().id;
    if state.job.at_stop(&device) {
        return Err(EngineError::Conflict(format!(
            "the job counter reached Stop At ({}); reset the count or change Stop At",
            state.job.stop_at().unwrap_or_default()
        )));
    }
    let (parts, epoch, run_state, options, estimate, planned_s) = {
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
        // Revisions are unique across open documents (module `documents`), so the revision check below covers this;
        // a run is still never approved while another document is active, whatever the revisions say.
        if run.document_id != active_document {
            run.state = RunState::Cancelled;
            run.error = Some(format!("run of document '{}', but '{active_document}' is active", run.document_id));
            return Err(EngineError::Conflict(format!(
                "run '{id}' was made from document '{}' and '{active_document}' is active; request a new run",
                run.document_id
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
            // The claim and a shutdown share one atomic word: a run claimed now is waited for, else it is refused.
            state.refuse_if_shutting_down("approving a run")?;
            return Err(EngineError::Conflict("another run is in progress".into()));
        }
        // Captured after claiming the machine, still under the runs lock (see the module docs).
        let epoch = state.machine.stop_epoch();
        run.state = if confirm.is_some() || run.options.start_input { RunState::AwaitingConfirm } else { RunState::Approved };
        let planned_s = run.stats.estimated_time_s / run.estimate_correction;
        (run.driver_job(), epoch, run.state, run.options.clone(), run.stats.estimated_time_s, planned_s)
    };
    // End any frame loop; `Controller::run_parts` stops a frame still on the board itself.
    state.machine.bump_frame_gen();
    // A pause asked for before this approval (e.g. as the previous job ended) does not carry over.
    state.machine.clear_pause();
    tracing::warn!(run_id = id, approved_by = approver.label(), physical_confirm = confirm.is_some(), iterations = ?options.iterations(), "run approved");
    state.job.begin(id, &device, options.iterations(), estimate, estimate / planned_s);
    state.emit(Event::machine("run_approved", Some(id.to_owned())));

    let foot_switch = approver == Approver::FootSwitch;
    let st = state.clone();
    let run_id = id.to_owned();
    let started = Instant::now();
    tokio::spawn(progress_ticker(state.clone(), id.to_owned()));
    tokio::spawn(async move {
        let table = options.table.clone();
        let job = Job { id: &run_id, device: &device, parts, epoch, confirm, options, foot_switch, planned_s };
        let (final_state, report, error) = execute(&st, job).await;
        // Repeat Marking: back to the table's start once the job finished (not after a stop).
        if final_state == RunState::Finished
            && table.is_some_and(|t| t.return_to_start)
            && let Err(e) = move_table(&st, epoch, 0.0).await
        {
            tracing::warn!(run_id = %run_id, error = %e, "table: could not return to the start position");
        }
        // The job's progress is cleared before the machine is released, so a job approved right after this one
        // never has its progress wiped by this task.
        st.job.end(&run_id);
        st.machine.end_run();
        if let Some(e) = &error {
            tracing::warn!(run_id = %run_id, state = ?final_state, error = %e, "run did not finish");
            if final_state == RunState::Failed {
                st.machine.set_error(Some(e.clone()));
            }
        }
        st.runs().finish(&run_id, final_state, report, error.clone());
        crate::libs::after_run(&st, &run_id, &device, final_state, started, error.clone());
        let ok = final_state == RunState::Finished;
        if ok {
            // Serial numbers / CSV rows move on once per finished job (the job's packets were rendered at request
            // time, so repeats of one job share its values).
            st.advance_variables_after_run();
        }
        st.emit(Event::RunFinished { id: run_id.clone(), ok, state: final_state, error });
        st.emit(Event::machine("run_finished", Some(run_id)));
    });
    let state_name = serde_json::to_value(run_state).unwrap_or(Value::Null);
    Ok(json!({ "ok": true, "id": id, "run_id": id, "state": state_name }))
}

/// One-click Start (`POST /api/job/start`, the `click` mode only; the endpoint checked the browser headers): plan the
/// current document into a run with `options` and approve it at once, as LightBurn's Start does. The gates are those
/// of [`approve`] (Require Framing, the Job Checklist `checklist`, the door, Stop At, another run, the stop epoch) and
/// the cheap ones are checked before planning, so an open door or a busy board answers at once. A run that is not
/// approved is discarded (nothing is left waiting), and a Stop pressed at any moment after the request arrived (while
/// the door is read or the job is planned) wins.
///
/// `revision`: the document revision the app showed when Start was clicked (the app always sends it). When the
/// document is at another revision (an agent's or a script's edit came in just before the click), nothing is started:
/// Start burns only the version the person was looking at.
pub(crate) async fn start_now(
    state: &Arc<AppState>,
    options: RunOptions,
    checklist: Option<&[bool]>,
    revision: Option<u64>,
) -> Result<Value, EngineError> {
    if state.approval_mode() != ApprovalMode::Click {
        return Err(EngineError::Approval {
            code: "approval_mode",
            message: "this engine approves runs with the operator PIN: request the run, then approve it with the PIN".into(),
        });
    }
    state.refuse_if_shutting_down("Start")?;
    // Captured first, so a Stop during the door read below cancels this Start too.
    let epoch = state.machine.stop_epoch();
    let changed = |shown: u64, now: u64| {
        EngineError::Conflict(format!(
            "the document changed since Start was clicked (revision {shown} -> {now}); nothing was started: check the \
             job and press Start again"
        ))
    };
    if let Some(shown) = revision
        && state.revision() != shown
    {
        return Err(changed(shown, state.revision()));
    }
    if state.machine.is_running() {
        return Err(EngineError::Conflict("another run is in progress".into()));
    }
    crate::libs::check_before_approve(state, revision.unwrap_or_else(|| state.revision()), checklist).await?;
    let view = state.request_run_with(commands::Origin::Ui, options).await?;
    let discard = || {
        state.runs().discard(&view.id);
        state.emit(Event::machine("run_discarded", Some(view.id.clone())));
    };
    if state.machine.stop_epoch() != epoch {
        discard();
        return Err(EngineError::Conflict("stopped while the job was being planned; nothing was started".into()));
    }
    if let Some(shown) = revision
        && view.revision != shown
    {
        discard();
        return Err(changed(shown, view.revision));
    }
    match approve(state, &view.id, Approver::Click, checklist).await {
        Ok(mut v) => {
            v["run"] = serde_json::to_value(&view).unwrap_or(Value::Null);
            Ok(v)
        }
        Err(e) => {
            discard();
            Err(e)
        }
    }
}

/// The foot switch of the cycle (module `cycle`): the profile's Start Marking input (`start_mark_input_bit`, at the
/// level of `start_mark_active_low`) while the engine runs in the `click` approval mode; `None` otherwise.
pub(crate) fn foot_switch_port(st: &AppState) -> Option<machine::InputPort> {
    if st.approval_mode() != ApprovalMode::Click || st.machine.is_grbl() {
        return None;
    }
    let profile = st.machine.profile();
    profile.start_mark_input_bit.map(|bit| machine::InputPort { bit, active_low: profile.start_mark_active_low })
}

/// Send `job_progress` events while the job of `run_id` holds the machine.
async fn progress_ticker(st: Arc<AppState>, run_id: String) {
    loop {
        tokio::time::sleep(crate::job::PROGRESS_INTERVAL).await;
        if st.job.active_run().as_deref() != Some(run_id.as_str()) {
            break;
        }
        st.job.tick(st.machine.is_paused());
        st.emit(Event::JobProgress { status: st.job_status() });
    }
}

type Outcome = (RunState, Option<RunReport>, Option<String>);

/// What one approval runs.
struct Job<'a> {
    id: &'a str,
    device: &'a str,
    parts: DriverJob,
    epoch: u64,
    confirm: Option<PhysicalConfirm>,
    options: RunOptions,
    /// Approved with the foot switch: that press was the first iteration's start-input press.
    foot_switch: bool,
    /// The planner's estimate of one iteration, before the learned correction (module `marktime`), s.
    planned_s: f64,
}

/// Wait for the physical confirmation (if configured), then run the iterations the options ask for, each after the
/// start input when asked, unless a stop came in since approval.
async fn execute(st: &Arc<AppState>, job: Job<'_>) -> Outcome {
    let id = job.id;
    if let Some(c) = job.confirm {
        st.emit(Event::machine("awaiting_confirm", Some(id.to_owned())));
        match wait_for_input(&st.machine, job.epoch, c.input_bit, false, true, c.timeout, c.poll, &|| false).await {
            InputEnd::Confirmed => {}
            InputEnd::Stopped | InputEnd::Ended => {
                return (RunState::Cancelled, None, Some("stopped while waiting for the physical confirmation".into()));
            }
            InputEnd::TimedOut => {
                let msg = format!(
                    "input {} was not pressed within {} s of approval; request and approve the run again",
                    c.input_bit,
                    c.timeout.as_secs()
                );
                return (RunState::Cancelled, None, Some(msg));
            }
            InputEnd::Failed(e) => return (RunState::Failed, None, Some(format!("physical confirmation: {e}"))),
            InputEnd::Panicked(e) => return panicked(st, e).await,
        }
    }
    let iterations = job.options.iterations();
    let profile = st.machine.profile();
    let start_bit = if job.options.start_input { profile.start_mark_input_bit } else { None };
    // LightBurn's StartMarkBitHigh = false: the pedal pulls the input low.
    let start_low = profile.start_mark_active_low;
    let mut completed = 0u32;
    let mut last: Option<RunReport> = None;
    loop {
        let iteration = completed + 1;
        if completed > 0 {
            if st.machine.stop_epoch() != job.epoch {
                return (RunState::Aborted, last, Some(format!("stopped after {completed} run(s)")));
            }
            if st.job.ending() {
                return (RunState::Finished, last, None);
            }
            if st.job.at_stop(job.device) {
                return (RunState::Finished, last, None);
            }
        }
        if let Some(bit) = start_bit.filter(|_| !(completed == 0 && job.foot_switch)) {
            st.job.waiting(iteration);
            st.runs().set_state(id, RunState::AwaitingConfirm);
            // The job state is `waiting_for_start_input` (GET /api/job); the event has the same name. The older
            // `awaiting_start_input` is still sent for clients that listen for it (the web UI until its next batch).
            st.emit(Event::machine("waiting_for_start_input", Some(id.to_owned())));
            st.emit(Event::machine("awaiting_start_input", Some(id.to_owned())));
            let full = job.options.full_pulse.unwrap_or(true);
            // While the engine shuts down (module `shutdown`), a run asked to end after its current iteration (a
            // continuous run) does not wait for another press: its last iteration has finished, so it ends here
            // instead of holding the restart until the grace limit.
            let between = completed > 0;
            let end_early = || between && st.shutting_down() && st.job.ending();
            match wait_for_input(&st.machine, job.epoch, bit, start_low, full, START_INPUT_TIMEOUT, CONFIRM_POLL, &end_early)
                .await
            {
                InputEnd::Confirmed => {}
                InputEnd::Ended => return (RunState::Finished, last, None),
                InputEnd::Stopped if completed == 0 => {
                    return (RunState::Cancelled, None, Some("stopped while waiting for the start input".into()));
                }
                InputEnd::Stopped => return (RunState::Aborted, last, Some(format!("stopped after {completed} run(s)"))),
                InputEnd::TimedOut if completed == 0 => {
                    return (RunState::Cancelled, None, Some(format!("start input {bit} was not pressed within an hour")));
                }
                InputEnd::TimedOut => return (RunState::Finished, last, None),
                InputEnd::Failed(e) => return (RunState::Failed, last, Some(format!("start input: {e}"))),
                InputEnd::Panicked(e) => return panicked(st, e).await,
            }
        }
        // Pause pressed while no run streams (between iterations, while waiting for the start input or the
        // physical confirmation): hold here, before the MO is opened, until resumed or stopped.
        hold_while_paused(st, job.epoch).await;
        match run_once(st, id, &job.parts, job.epoch, iteration).await {
            Once::Done(report) => {
                learn_run_time(st, &job, &report);
                completed += 1;
                last = Some(report);
                let count = st.job.increment(job.device);
                st.job.completed(completed);
                if let Some(r) = st.runs().get_mut(id) {
                    r.completed = completed;
                }
                st.emit(Event::machine("job_count", Some(count.to_string())));
                if iterations.is_some_and(|n| completed >= n) {
                    return (RunState::Finished, last, None);
                }
                // Repeat Marking: the table moves on by the increment, then settles.
                if let Some(t) = &job.options.table {
                    match move_table(st, job.epoch, t.increment * f64::from(completed)).await {
                        Ok(true) => tokio::time::sleep(Duration::from_millis(u64::from(t.settle_ms))).await,
                        Ok(false) => return (RunState::Aborted, last, Some(format!("stopped after {completed} run(s)"))),
                        Err(e) => return (RunState::Failed, last, Some(format!("table: {e}"))),
                    }
                }
            }
            Once::NotStarted if completed == 0 => {
                return (RunState::Cancelled, None, Some("cancelled by stop before streaming".into()));
            }
            Once::NotStarted => return (RunState::Aborted, last, Some(format!("stopped after {completed} run(s)"))),
            Once::Ended(outcome) => return outcome,
        }
    }
}

/// Record what a finished iteration took on a real board next to the planner's estimate (module `marktime`), so the
/// device's next estimates are corrected. Galvo (LMC) runs only: GRBL reports no mark time. Not for the simulated
/// board, a job of several lists (the board's count may
/// cover only the last one), an iteration that was paused (the controller's count of `StopList`s, so a pause too
/// short for the progress ticker to see counts too), or a reply without a mark time.
fn learn_run_time(st: &Arc<AppState>, job: &Job<'_>, report: &RunReport) {
    let one_list = matches!(&job.parts, DriverJob::Lmc(parts) if parts.len() == 1);
    if st.machine.kind() != LinkKind::Usb || !one_list || st.machine.progress().pauses > 0 || st.job.iteration_paused() {
        return;
    }
    let Some(measured_s) = report.mark_time_s() else { return };
    let c = st.job.times.record(job.device, job.planned_s, measured_s);
    tracing::info!(
        run_id = job.id,
        device = job.device,
        planned_s = job.planned_s,
        measured_s,
        correction = c.factor,
        runs = c.runs,
        "run time recorded"
    );
}

/// Wait while a pause is asked for, unless a stop came in (the caller's next step sees the stop).
async fn hold_while_paused(st: &Arc<AppState>, epoch: u64) {
    while st.machine.pause_requested() && st.machine.stop_epoch() == epoch {
        tokio::time::sleep(CONFIRM_POLL).await;
    }
}

enum Once {
    Done(RunReport),
    /// A stop came in before this iteration took the controller; nothing was sent for it.
    NotStarted,
    Ended(Outcome),
}

/// One iteration: arm, claim the controller, run every part.
async fn run_once(st: &Arc<AppState>, id: &str, parts: &DriverJob, epoch: u64, iteration: u32) -> Once {
    let armed = Armed::approved_in_ui(id.to_owned());
    let (task_state, run_id, parts) = (st.clone(), id.to_owned(), parts.clone());
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
            task_state.job.started(iteration);
            task_state.emit(Event::machine("run_started", Some(run_id)));
            Some(m.ensure_ready(ctl).and_then(|()| ctl.run_job(&parts, armed)))
        })
        .await;
    match result {
        Ok(None) => Once::NotStarted,
        Ok(Some(Ok(report))) => Once::Done(report),
        Ok(Some(Err(ControllerError::Aborted))) => Once::Ended((RunState::Aborted, None, Some("aborted by stop".into()))),
        Ok(Some(Err(
            e @ (ControllerError::StopInput(_)
            | ControllerError::DoorOpen(_)
            | ControllerError::Grbl(GrblError::DoorOpen | GrblError::Alarm(_))),
        ))) => Once::Ended((RunState::Aborted, None, Some(e.to_string()))),
        Ok(Some(Err(e))) => Once::Ended((RunState::Failed, None, Some(e.to_string()))),
        // The blocking task panicked: the controller may have left the MO open or the laser-enable bit set.
        Err(e) => Once::Ended(panicked(st, e.to_string()).await),
    }
}

/// Move the Repeat Marking table to `units` (mm or degrees from its start). `Ok(false)`: a stop came in first.
async fn move_table(st: &Arc<AppState>, epoch: u64, units: f64) -> Result<bool, String> {
    let r = st
        .machine
        .blocking(move |m, ctl| -> Result<bool, String> {
            if m.stop_epoch() != epoch {
                return Ok(false);
            }
            let axis = m.profile().axis.ok_or("the profile has no table axis")?;
            let position = axis.table_steps(units)?;
            m.ensure_ready(ctl).map_err(|e| e.to_string())?;
            ctl.move_axis(position).map_err(|e| e.to_string())?;
            Ok(true)
        })
        .await;
    match r {
        Ok(v) => v,
        Err(e) => Err(e.to_string()),
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

enum InputEnd {
    Confirmed,
    Stopped,
    /// `end_early` asked to end the wait (nothing was pressed; nothing fires).
    Ended,
    TimedOut,
    Failed(String),
    Panicked(String),
}

/// Poll `ReadPort` until input `bit` is pressed (high, or low when `active_low`) for [`CONFIRM_READS`] reads in a
/// row; with `full_pulse` it must
/// first be seen released (so a stuck, held or inverted input never starts anything). Ends early on a stop (epoch
/// change), `end_early`, a read error or the timeout.
#[allow(clippy::too_many_arguments)]
async fn wait_for_input(
    machine: &Arc<Machine>,
    epoch: u64,
    bit: u8,
    active_low: bool,
    full_pulse: bool,
    timeout: Duration,
    poll: Duration,
    end_early: &(dyn Fn() -> bool + Sync),
) -> InputEnd {
    let deadline = Instant::now() + timeout;
    let port = machine::InputPort { bit, active_low };
    let (mut released, mut pressed) = (!full_pulse, 0);
    loop {
        if machine.stop_epoch() != epoch {
            return InputEnd::Stopped;
        }
        if end_early() {
            return InputEnd::Ended;
        }
        if Instant::now() >= deadline {
            return InputEnd::TimedOut;
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
            Ok(Ok(Some(bits))) if !port.active(bits) => (released, pressed) = (true, 0),
            Ok(Ok(Some(_))) if released => {
                pressed += 1;
                if pressed >= CONFIRM_READS {
                    return InputEnd::Confirmed;
                }
            }
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => return InputEnd::Stopped,
            Ok(Err(e)) => return InputEnd::Failed(e.to_string()),
            Err(e) => return InputEnd::Panicked(e.to_string()),
        }
        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
#[path = "approve_tests.rs"]
pub(crate) mod tests;
