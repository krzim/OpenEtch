//! Graceful shutdown on the simulated board, driven through the signal channel (no real signals): a signal during a
//! run lets it finish, then the engine exits; new work is refused meanwhile; a second signal, the grace limit or a
//! grace of 0 aborts the run; frames and the foot switch cycle stop.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use commands::{Invocation, Origin, Session};
use machine::{Link, MachineProfile, PollConfig, SimBoard, SimLink};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{AbortReason, Options, Outcome, run};
use crate::approve::{self, Approver};
use crate::approve::tests::{browser, pedal_profile, req, send};
use crate::frame::FrameRequest;
use crate::hw::{LinkKind, Machine};
use crate::pin::{self, ApprovalConfig, ApprovalMode, PinStore};
use crate::runs::{RunOptions, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, EngineError, api};

/// A connected engine in the `click` mode (no PIN) on a simulated board that reports BUSY for `busy_polls` status
/// polls (2 ms apart) after a list starts, so a run spends real time on the board; one rectangle in the document.
async fn engine(profile: MachineProfile, busy_polls: usize) -> Arc<AppState> {
    let board = SimBoard { busy_polls, ..SimBoard::default() };
    let link: Box<dyn Link> = Box::new(SimLink::with_board(board));
    let poll = PollConfig { interval: Duration::from_millis(2), finish_timeout: None, ..PollConfig::immediate() };
    let machine = Arc::new(Machine::with_link(link, LinkKind::Simulated, None, profile, None, poll));
    engine_on(machine, None).await
}

/// [`engine`] on `machine`, with the physical confirmation input `confirm` (`--confirm-input`).
async fn engine_on(machine: Arc<Machine>, confirm: Option<u8>) -> Arc<AppState> {
    machine.connect().await.expect("sim connects");
    let approval = ApprovalConfig {
        pin: PinStore::new(&pin::temp_dir("shutdown")),
        confirm: confirm.map(crate::pin::PhysicalConfirm::new),
    };
    let st = AppState::new(
        Session::new(doc::Document::default()),
        machine,
        SecurityConfig::default(),
        approval,
        crate::tests::test_projects(),
    );
    st.set_approval_mode(ApprovalMode::Click).expect("set once");
    let rect = Invocation { id: "shape.rect".into(), params: json!({"width_mm": 10, "height_mm": 10}) };
    st.execute(rect, Origin::Ui).expect("rectangle");
    st
}

fn opts(grace: Duration) -> Options {
    Options {
        grace,
        poll: Duration::from_millis(5),
        report_every: Duration::from_millis(100),
        abort_wait: Duration::from_secs(5),
    }
}

/// The shutdown coordinator, waiting for signals on the returned sender.
fn coordinator(st: &Arc<AppState>, o: Options) -> (mpsc::UnboundedSender<&'static str>, JoinHandle<Outcome>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, tokio::spawn(run(st.clone(), rx, o)))
}

/// Request and approve a run (Start in the app); returns its id.
async fn start(st: &Arc<AppState>, options: RunOptions) -> String {
    let view = st.request_run_with(Origin::Ui, options).await.expect("run requested");
    approve::approve(st, &view.id, Approver::Click, None).await.expect("approved");
    view.id
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

fn state_of(st: &AppState, id: &str) -> RunState {
    st.runs().get(id).expect("run exists").state
}

fn shutting_down_refusal(r: Result<impl std::fmt::Debug, EngineError>) {
    match r {
        Err(e @ EngineError::ShuttingDown(_)) => {
            assert_eq!(e.status(), StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(e.body()["code"], "shutting_down");
            assert!(e.to_string().contains("the engine is shutting down"), "{e}");
        }
        other => panic!("expected a shutting-down refusal, got {other:?}"),
    }
}

/// A signal while a run streams: the run finishes (no abort, no stop), the engine refuses new work meanwhile and
/// reports the shutdown, then the coordinator returns.
#[tokio::test]
async fn a_signal_during_a_run_lets_it_finish_then_exits() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 150).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions::default()).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    let epoch = st.machine.stop_epoch();
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    // Reported while it waits.
    let m = st.machine_status();
    assert_eq!(m["shutting_down"], true, "{m}");
    assert_eq!(m["shutdown"]["signal"], "SIGTERM");
    assert_eq!(m["shutdown"]["grace_s"], 60);
    assert_eq!(m["shutdown"]["waiting_for_run"], json!(id));
    assert_eq!(m["shutdown"]["aborting"], false);
    // New work is refused while the run goes on.
    shutting_down_refusal(st.request_run(Origin::Ui).await);
    shutting_down_refusal(st.frame_start(FrameRequest::default()).await);
    assert!(!done.is_finished(), "the engine waits for the run");
    assert_eq!(state_of(&st, &id), RunState::Running);
    let outcome = tokio::time::timeout(Duration::from_secs(20), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!((run.state, run.completed), (RunState::Finished, 1), "{:?}", run.error);
    assert_eq!(st.machine.stop_epoch(), epoch, "no stop and no abort");
    assert!(!st.machine.is_running() && !st.machine.is_framing());
    assert!(*st.shutdown.closing().borrow(), "connections are told to close");
}

/// Once shutting down, every way to new work is refused (503 `shutting_down`), also over HTTP, and pending runs are
/// cancelled. Stop still works.
#[tokio::test]
async fn new_work_is_refused_while_shutting_down() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    let app = api::router(st.clone(), None);
    let pending = st.request_run(Origin::Ui).await.unwrap().id;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    tx.send("SIGINT").unwrap();
    assert_eq!(done.await.unwrap(), Outcome::Idle, "nothing ran: the engine exits at once");
    assert_eq!(state_of(&st, &pending), RunState::Cancelled, "pending runs are cancelled");
    shutting_down_refusal(approve::approve(&st, &pending, Approver::Click, None).await);
    shutting_down_refusal(approve::start_now(&st, RunOptions::default(), None, None).await);
    shutting_down_refusal(st.request_run(Origin::Agent("claude".into())).await);
    shutting_down_refusal(st.frame_start(FrameRequest::default()).await);
    shutting_down_refusal(st.pointer_to(0.0, 0.0, None, crate::frame::Pointer::Red).await);
    let device = st.active_device().id;
    shutting_down_refusal(st.activate_device(&device).await);
    assert!(!st.machine.try_begin_run(), "no claim is granted any more");
    // Over HTTP: the app's Start, a run request and a frame answer 503 with the code; the status says so.
    let rev = st.revision();
    let (s, v) = send(&app, browser(req("POST", "/api/job/start", Some(json!({"origin": "ui", "revision": rev}))))).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("shutting_down")), "{v}");
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("shutting_down")), "{v}");
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!((s, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("shutting_down")), "{v}");
    let (s, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((s, &m["shutting_down"], &m["shutdown"]["signal"]), (StatusCode::OK, &json!(true), &json!("SIGINT")));
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "Stop is never gated: {v}");
    // Before any signal nothing is refused (and the status says so).
    let fresh = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    assert_eq!(fresh.machine_status()["shutting_down"], false);
    assert!(fresh.machine_status()["shutdown"].is_null());
    fresh.request_run(Origin::Ui).await.expect("not shutting down");
}

/// A run approved but not streaming yet (waiting for its physical confirmation) is waited for too: it starts when
/// the input is pressed, finishes, and the engine then exits.
#[tokio::test]
async fn an_approved_run_waiting_to_start_is_waited_for() {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    let st = engine_on(machine, Some(3)).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions::default()).await;
    assert_eq!(state_of(&st, &id), RunState::AwaitingConfirm);
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!done.is_finished(), "the approved run is waited for");
    assert_eq!(state_of(&st, &id), RunState::AwaitingConfirm, "and not cancelled");
    assert_eq!(counters.emitting_packets(), 0, "nothing fired yet");
    // The person confirms: released (it is), then pressed.
    counters.set_inputs(1 << 3);
    let outcome = tokio::time::timeout(Duration::from_secs(20), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    assert_eq!(state_of(&st, &id), RunState::Finished);
    assert!(counters.emitting_packets() > 0, "the job ran");
}

/// A frame is stopped (pointer off) at the first signal, and the engine exits.
#[tokio::test]
async fn frames_are_stopped() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    st.frame_start(FrameRequest { continuous: Some(true), ..FrameRequest::default() }).await.unwrap();
    until("framing", || st.machine.is_framing()).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    tx.send("SIGTERM").unwrap();
    assert_eq!(done.await.unwrap(), Outcome::Idle);
    assert!(!st.machine.is_framing(), "the frame stopped");
}

/// A second signal aborts the run with the normal abort sequence; the engine then exits.
#[tokio::test]
async fn a_second_signal_aborts() {
    let st = engine(MachineProfile::generic_lmc_fiber(), usize::MAX).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions::default()).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(state_of(&st, &id), RunState::Running, "one signal never aborts");
    assert!(!done.is_finished());
    tx.send("SIGINT").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::Aborted { reason: AbortReason::Signal("SIGINT"), released: true });
    assert_eq!(state_of(&st, &id), RunState::Aborted);
    assert!(!st.machine.is_running());
}

/// `--shutdown-grace-secs 0` aborts a run at the first signal.
#[tokio::test]
async fn grace_zero_aborts_at_once() {
    let st = engine(MachineProfile::generic_lmc_fiber(), usize::MAX).await;
    let (tx, done) = coordinator(&st, opts(Duration::ZERO));
    let id = start(&st, RunOptions::default()).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::Aborted { reason: AbortReason::GraceExpired, released: true });
    assert_eq!(state_of(&st, &id), RunState::Aborted);
}

/// The grace limit running out aborts the run.
#[tokio::test]
async fn the_grace_limit_aborts() {
    let st = engine(MachineProfile::generic_lmc_fiber(), usize::MAX).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_millis(300)));
    let id = start(&st, RunOptions::default()).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(10), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::Aborted { reason: AbortReason::GraceExpired, released: true });
    assert_eq!(state_of(&st, &id), RunState::Aborted);
}

/// Stop in the app keeps working while the engine waits: the run is aborted and the engine exits.
#[tokio::test]
async fn stop_works_while_waiting() {
    let st = engine(MachineProfile::generic_lmc_fiber(), usize::MAX).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions::default()).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    st.stop().await.expect("stop");
    let outcome = tokio::time::timeout(Duration::from_secs(10), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    assert_eq!(state_of(&st, &id), RunState::Aborted);
}

/// A continuous run would never end by itself: it ends after its current iteration, finished, not aborted.
#[tokio::test]
async fn a_continuous_run_ends_after_its_current_iteration() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 40).await;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions { continuous: true, ..RunOptions::default() }).await;
    until("the run to stream", || state_of(&st, &id) == RunState::Running).await;
    let epoch = st.machine.stop_epoch();
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(20), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(run.completed >= 1);
    assert_eq!(st.machine.stop_epoch(), epoch, "never aborted");
}

/// The foot switch cycle ends at the first signal (its frame off), and a press does nothing afterwards.
#[tokio::test]
async fn the_foot_switch_cycle_ends() {
    let st = engine(pedal_profile(), 3).await;
    crate::cycle::pressed(&st).await;
    assert_eq!(st.machine_status()["foot_cycle"]["phase"], "framing");
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    tx.send("SIGTERM").unwrap();
    assert_eq!(done.await.unwrap(), Outcome::Idle);
    let m = st.machine_status();
    assert_eq!(m["foot_cycle"]["phase"], "off", "{m}");
    assert!(m["foot_cycle"]["notice"].as_str().unwrap().contains("shutting down"), "{m}");
    assert!(!st.machine.is_framing());
    crate::cycle::pressed(&st).await;
    assert!(!st.machine.is_framing(), "a press frames nothing");
    assert_eq!(st.machine_status()["foot_cycle"]["phase"], "off");
}

/// The run claim and the shutdown share one word: a run that holds the machine is reported, and no claim is
/// granted afterwards.
#[tokio::test]
async fn the_claim_and_the_shutdown_never_miss_each_other() {
    let m = Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate());
    assert!(m.try_begin_run());
    assert!(m.begin_shutdown(), "a run held the machine");
    assert!(m.is_running() && m.is_shutting_down());
    m.end_run();
    assert!(!m.is_running() && m.is_shutting_down());
    assert!(!m.try_begin_run(), "refused once shutting down");
    let idle = Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate());
    assert!(!idle.begin_shutdown(), "nothing held it");
    assert!(!idle.try_begin_run());
}

/// The foot switch burn (the case that ruined a coin): a restart while the cycle burns lets the burn finish, then
/// the engine exits; the cycle does not frame again afterwards (pointer off).
#[tokio::test]
async fn a_foot_switch_burn_finishes_and_nothing_frames_after_it() {
    let st = engine(pedal_profile(), 150).await;
    crate::cycle::pressed(&st).await;
    assert_eq!(st.machine_status()["foot_cycle"]["phase"], "framing");
    crate::cycle::pressed(&st).await;
    until("the foot switch burn to stream", || st.job_status()["state"] == "running").await;
    let id = st.job.active_run().expect("the burn holds the machine");
    let epoch = st.machine.stop_epoch();
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    assert_eq!(state_of(&st, &id), RunState::Running, "the burn goes on");
    let outcome = tokio::time::timeout(Duration::from_secs(20), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!((run.state, run.completed), (RunState::Finished, 1), "{:?}", run.error);
    assert_eq!(st.machine.stop_epoch(), epoch, "no stop and no abort");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!st.machine.is_framing(), "the cycle does not frame again");
    assert_eq!(st.machine_status()["foot_cycle"]["phase"], "off");
}

/// A continuous run between iterations, waiting for its start input (the pedal), ends there at a shutdown: its last
/// iteration has finished, nothing more fires, and the restart is not held until the grace limit.
#[tokio::test]
async fn a_continuous_run_waiting_for_its_start_input_ends_there() {
    let st = engine_on(Arc::new(Machine::simulated(pedal_profile(), None, PollConfig::immediate())), None).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions { continuous: true, start_input: true, ..RunOptions::default() }).await;
    until("the first wait for the pedal", || st.job_status()["state"] == "waiting_for_start_input").await;
    counters.set_inputs(1 << 14);
    until("the first iteration", || st.runs().get(&id).is_some_and(|r| r.completed >= 1)).await;
    counters.set_inputs(0);
    until("the wait for the next press", || {
        let j = st.job_status();
        j["state"] == "waiting_for_start_input" && j["completed"] == 1
    })
    .await;
    let (fired, epoch) = (counters.emitting_packets(), st.machine.stop_epoch());
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), done).await.expect("not held until the grace limit").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!((run.state, run.completed), (RunState::Finished, 1), "{:?}", run.error);
    assert_eq!(counters.emitting_packets(), fired, "nothing more fired");
    assert_eq!(st.machine.stop_epoch(), epoch, "ended, not aborted");
}

/// A paused run is waited for: it neither aborts nor ends; resumed, it finishes and the engine exits.
#[tokio::test]
async fn a_paused_run_is_waited_for_until_resumed() {
    let st = engine_on(Arc::new(Machine::simulated(pedal_profile(), None, PollConfig::immediate())), None).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    let id = start(&st, RunOptions { start_input: true, ..RunOptions::default() }).await;
    until("the wait for the pedal", || st.job_status()["state"] == "waiting_for_start_input").await;
    st.machine.pause();
    // The pedal is pressed while paused: the iteration is held before anything is sent (module `approve`).
    counters.set_inputs(1 << 14);
    tokio::time::sleep(Duration::from_millis(400)).await;
    counters.set_inputs(0);
    assert_eq!(st.job_status()["state"], "paused");
    let before = counters.emitting_packets();
    tx.send("SIGTERM").unwrap();
    until("the shutdown to begin", || st.shutting_down()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!done.is_finished(), "the paused run is waited for");
    assert!(!state_of(&st, &id).is_final());
    assert_eq!(counters.emitting_packets(), before, "nothing fires while paused");
    st.machine.resume();
    let outcome = tokio::time::timeout(Duration::from_secs(20), done).await.expect("exits").unwrap();
    assert_eq!(outcome, Outcome::RunEnded);
    assert_eq!(state_of(&st, &id), RunState::Finished);
}

/// A stuck board (a board call that never returns holds the controller) never keeps the engine from exiting: the
/// abort is bounded, and the outcome is a failure (the exit status says so).
#[tokio::test]
async fn a_stuck_board_never_keeps_the_engine_from_exiting() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    // A run that holds the machine, its board call hung inside the controller lock.
    assert!(st.machine.try_begin_run());
    let m = st.machine.clone();
    let stuck = tokio::spawn(async move { m.blocking(|_, _| std::thread::sleep(Duration::from_millis(2500))).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let o = Options { abort_wait: Duration::from_millis(300), ..opts(Duration::ZERO) };
    let (tx, done) = coordinator(&st, o);
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(2), done).await.expect("the abort is bounded").unwrap();
    assert_eq!(outcome, Outcome::Aborted { reason: AbortReason::GraceExpired, released: false });
    assert!(outcome.failed(), "a failure exit status");
    assert!(*st.shutdown.closing().borrow(), "the engine exits");
    assert!(!Outcome::RunEnded.failed() && !Outcome::Idle.failed());
    assert!(!Outcome::Aborted { reason: AbortReason::GraceExpired, released: true }.failed());
    stuck.await.unwrap().unwrap();
    st.machine.end_run();
}

/// A stuck board while framing (no run): the frame stop is bounded too, so the engine still exits; the frame loop
/// has been told to end, and the pointer goes off once the board answers.
#[tokio::test]
async fn a_stuck_board_while_framing_never_keeps_the_engine_from_exiting() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    st.frame_start(FrameRequest { continuous: Some(true), ..FrameRequest::default() }).await.unwrap();
    until("framing", || st.machine.is_framing()).await;
    let m = st.machine.clone();
    let stuck = tokio::spawn(async move { m.blocking(|_, _| std::thread::sleep(Duration::from_millis(2500))).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let o = Options { abort_wait: Duration::from_millis(300), ..opts(Duration::from_secs(60)) };
    let (tx, done) = coordinator(&st, o);
    tx.send("SIGTERM").unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(2), done).await.expect("the frame stop is bounded").unwrap();
    assert_eq!(outcome, Outcome::Idle);
    assert!(!outcome.failed());
    stuck.await.unwrap().unwrap();
    until("the frame loop to end", || !st.machine.is_framing()).await;
}

/// Every request that would touch the board (or the USB bus) is refused once the engine shuts down, reads included:
/// the board info and inputs, a GRBL or Ruida status refresh, a board scan, the Find My Laser wizard's activation and
/// a change to the active device (refused before anything is written). Reads that need no board still answer.
#[tokio::test]
async fn board_reads_are_refused_while_shutting_down() {
    let st = engine(MachineProfile::generic_lmc_fiber(), 3).await;
    let app = api::router(st.clone(), None);
    // Before the signal they answer.
    assert_eq!(send(&app, req("GET", "/api/machine/info", None)).await.0, StatusCode::OK);
    assert_eq!(send(&app, req("GET", "/api/machine/inputs", None)).await.0, StatusCode::OK);
    // A GRBL gantry made now and then a Ruida one (not active), to ask their status routes.
    let profile = json!({"port": "COM9", "work_area_mm": [400.0, 300.0], "origin": "bottom_left"});
    let (s, v) = send(&app, req("POST", "/api/devices/grbl", Some(json!({"display_name": "Diode", "profile": profile})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let grbl = v["device"]["id"].as_str().unwrap().to_owned();
    let galvo = st.active_device().id;
    let (tx, done) = coordinator(&st, opts(Duration::from_secs(60)));
    tx.send("SIGTERM").unwrap();
    assert_eq!(done.await.unwrap(), Outcome::Idle);
    let refused = |what: &str, s: StatusCode, v: &serde_json::Value| {
        assert_eq!((s, v["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("shutting_down")), "{what}: {v}");
    };
    for uri in ["/api/machine/info", "/api/machine/inputs", "/api/devices/scan"] {
        let (s, v) = send(&app, req("GET", uri, None)).await;
        refused(uri, s, &v);
    }
    let edit = json!({"display_name": "Renamed"});
    let (s, v) = send(&app, req("PUT", &format!("/api/devices/{galvo}"), Some(edit.clone()))).await;
    refused("edit the active device", s, &v);
    // Another device's stored settings are only a file: that still works.
    let (s, v) = send(&app, req("PUT", &format!("/api/devices/{grbl}"), Some(edit))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, req("POST", "/api/devices/wizard", Some(json!({"markcfg_base64": "", "activate": true})))).await;
    refused("wizard with activation", s, &v);
    // The GRBL status route is asked on a galvo: it says so (409) before anything else, refresh or not; the
    // refresh refusal itself is covered with a GRBL gantry active (`grbl_tests`).
    assert_eq!(send(&app, req("GET", "/api/machine/grbl?refresh=true", None)).await.0, StatusCode::CONFLICT);
    let (s, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((s, &m["shutting_down"]), (StatusCode::OK, &json!(true)), "the status needs no board");
}
