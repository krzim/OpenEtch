//! The foot switch cycle on the simulated board, the pedal pressed with `POST /api/sim/inputs` and read by the real
//! input watcher (`libs::spawn_io_watch`): press 1 frames, press 2 burns what was framed, framing resumes, press 3
//! burns again; an edit, an open door, Stop, a press during a burn, pending runs and the checklist.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use machine::MachineProfile;
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;

use crate::approve::tests::{
    DOOR_OPEN, door_profile, foot_events, machine_event, pedal_profile, pending_run, press, rect, req, run_count, send,
    set_inputs, state, state_of, state_with,
};
use crate::pin::ApprovalMode;
use crate::runs::RunState;
use crate::{AppState, Event, api};

/// The next `foot_cycle` phase (`framing`, `burning`, `off`).
async fn phase(rx: &mut Receiver<Event>) -> String {
    machine_event(rx, "foot_cycle").await.unwrap_or_default()
}

async fn cycle_status(app: &Router) -> Value {
    let (s, m) = send(app, req("GET", "/api/machine", None)).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    m["foot_cycle"].clone()
}

fn job_count(st: &AppState) -> u64 {
    st.job.count(&st.active_device().id)
}

/// A click-mode engine with the foot switch on input 14, the watcher running, and one rectangle to burn.
async fn setup(profile: MachineProfile, confirm: Option<u8>) -> (Arc<AppState>, Router, tokio::task::JoinHandle<()>) {
    let st = state_with(profile, ApprovalMode::Click, confirm).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    rect(&app).await;
    (st, app, watch)
}

#[tokio::test]
async fn press_frames_then_each_press_burns_the_framed_job_and_framing_resumes() {
    let (st, app, watch) = setup(pedal_profile(), None).await;
    // Require Framing is satisfied by the cycle's own frame.
    let device = st.active_device().id;
    crate::libs::lock(&st.libs.prefs).device_mut(&device).require_framing = true;
    let counters = st.machine.sim_counters().unwrap().clone();
    let c = cycle_status(&app).await;
    assert_eq!((&c["available"], &c["phase"], &c["idle_timeout_s"]), (&json!(true), &json!("off"), &json!(600)), "{c}");
    let mut ev = st.subscribe();

    // Press 1: framing, light only.
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    assert!(st.machine.is_framing());
    let c = cycle_status(&app).await;
    assert_eq!((&c["phase"], &c["revision"]), (&json!("framing"), &json!(st.revision())), "{c}");
    assert_eq!(counters.emitting_packets(), 0, "framing never fires");
    assert_eq!(run_count(&st), 0);

    // Press 2: burns the framed revision, then frames again.
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    assert_eq!(phase(&mut ev).await, "framing", "back to framing after the burn");
    assert_eq!(job_count(&st), 1);
    let fired = counters.emitting_packets();
    assert!(fired > 0);
    assert!(st.machine.is_framing());
    assert_eq!(cycle_status(&app).await["phase"], "framing");

    // Press 3 (and 4): burn again each time.
    for n in 2..=3 {
        press(&app, 0).await;
        assert_eq!(phase(&mut ev).await, "burning");
        assert_eq!(phase(&mut ev).await, "framing");
        assert_eq!(job_count(&st), n);
    }
    assert!(counters.emitting_packets() > fired);
    let runs = st.runs().views(st.revision());
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|r| r.state == RunState::Finished), "{runs:?}");
    // Stop ends it all.
    send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(phase(&mut ev).await, "off");
    watch.abort();
}

#[tokio::test]
async fn an_edit_between_presses_frames_the_new_version_instead_of_burning() {
    let (st, app, watch) = setup(pedal_profile(), None).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    let framed = st.revision();
    // An edit (in the app or by an agent) after the frame started.
    rect(&app).await;
    assert_ne!(st.revision(), framed);
    press(&app, 0).await;
    let why = machine_event(&mut ev, "foot_cycle_notice").await.unwrap_or_default();
    assert!(why.contains("changed") && why.contains("not burned"), "{why}");
    assert_eq!(counters.emitting_packets(), 0, "the unframed revision was not burned");
    assert_eq!(run_count(&st), 0);
    let c = cycle_status(&app).await;
    assert_eq!((&c["phase"], &c["revision"]), (&json!("framing"), &json!(st.revision())), "framing the new version: {c}");
    assert!(st.machine.is_framing());
    // The next press burns the version now framed.
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    assert_eq!(phase(&mut ev).await, "framing");
    let runs = st.runs().views(st.revision());
    assert_eq!((runs.len(), runs[0].revision, runs[0].state), (1, st.revision(), RunState::Finished));
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

#[tokio::test]
async fn an_open_door_blocks_the_burn_and_keeps_framing() {
    let (st, app, watch) = setup(door_profile(), None).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    press(&app, DOOR_OPEN).await;
    let why = machine_event(&mut ev, "foot_cycle_notice").await.unwrap_or_default();
    assert!(why.contains("door") && why.contains("Still framing"), "{why}");
    assert_eq!(counters.emitting_packets(), 0);
    assert_eq!(run_count(&st), 0, "nothing left waiting");
    assert!(st.machine.is_framing());
    assert_eq!(cycle_status(&app).await["phase"], "framing");
    // Door closed: the next press burns.
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    assert_eq!(phase(&mut ev).await, "framing");
    assert!(counters.emitting_packets() > 0);
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

#[tokio::test]
async fn stop_ends_the_cycle_and_the_red_pointer() {
    let (st, app, watch) = setup(pedal_profile(), None).await;
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    // MCP's machine_stop is this call.
    st.stop().await.expect("stop");
    assert_eq!(phase(&mut ev).await, "off");
    assert!(!st.machine.is_framing());
    assert_eq!(cycle_status(&app).await["phase"], "off");
    // The next press starts a new cycle: it frames, it does not burn.
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    assert_eq!(run_count(&st), 0);
    // Stop right after a burn started: whatever the burn did, the cycle ends and the pointer stays off.
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(cycle_status(&app).await["phase"], "off");
    assert!(!st.machine.is_framing(), "Stop turned the pointer off");
    // Stop frame and another frame end the cycle too.
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!(phase(&mut ev).await, "off");
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(phase(&mut ev).await, "off");
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

/// A burn that holds the board (here: waiting for the physical confirmation input 3) is not disturbed by presses,
/// and they never queue a second burn.
#[tokio::test]
async fn a_press_during_a_burn_does_nothing() {
    let (st, app, watch) = setup(pedal_profile(), Some(3)).await;
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    let id = st.runs().views(st.revision())[0].id.clone();
    assert_eq!(state_of(&st, &id), RunState::AwaitingConfirm);
    let mut quiet = st.subscribe();
    press(&app, 0).await;
    press(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    // The moment between a burn's end and the frame restarting: a press read then is ignored too.
    crate::cycle::pressed(&st).await;
    assert!(foot_events(&mut quiet).is_empty(), "presses during the burn did nothing");
    assert_eq!((run_count(&st), cycle_status(&app).await["phase"].clone()), (1, json!("burning")));
    // The confirmation lets the burn go on; framing resumes, and one job was burned.
    set_inputs(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    set_inputs(&app, 1 << 3).await;
    assert_eq!(phase(&mut ev).await, "framing");
    assert_eq!((state_of(&st, &id), job_count(&st), run_count(&st)), (RunState::Finished, 1, 1));
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

/// Runs waiting in `pending` (an agent's over MCP, or requested in the app) are never burned by the foot switch: it
/// frames and burns the current document only.
#[tokio::test]
async fn the_foot_switch_never_approves_pending_runs() {
    let (st, app, watch) = setup(pedal_profile(), None).await;
    let agents = pending_run(&app, Some(json!({"origin": "agent:test", "repeat": 3}))).await;
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing", "the first press frames even with a run waiting");
    assert_eq!(state_of(&st, &agents), RunState::Pending);
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "burning");
    assert_eq!(phase(&mut ev).await, "framing");
    assert_eq!(state_of(&st, &agents), RunState::Pending, "the agent's run still waits for the app");
    assert_eq!(job_count(&st), 1, "one pass of the framed job, not the agent's repeat 3");
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

/// A foot cannot tick the Job Checklist: with it on, the cycle frames but every burn is refused.
#[tokio::test]
async fn the_checklist_refuses_every_foot_switch_burn() {
    let (st, app, watch) = setup(pedal_profile(), None).await;
    let device = st.active_device().id;
    crate::libs::lock(&st.libs.prefs).device_mut(&device).checklist =
        crate::prefs::Checklist { enabled: true, items: vec!["Eyewear on".into()] };
    let mut ev = st.subscribe();
    press(&app, 0).await;
    assert_eq!(phase(&mut ev).await, "framing");
    press(&app, 0).await;
    let why = machine_event(&mut ev, "foot_cycle_notice").await.unwrap_or_default();
    assert!(why.contains("Checklist"), "{why}");
    assert_eq!(run_count(&st), 0);
    assert_eq!(st.machine.sim_counters().unwrap().emitting_packets(), 0);
    assert!(st.machine.is_framing());
    send(&app, req("POST", "/api/machine/stop", None)).await;
    watch.abort();
}

/// Nothing to frame: the press says so and no cycle starts.
#[tokio::test]
async fn a_press_with_an_empty_job_says_why() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    let mut ev = st.subscribe();
    press(&app, 0).await;
    let why = machine_event(&mut ev, "foot_cycle_notice").await.unwrap_or_default();
    assert!(why.contains("nothing to frame"), "{why}");
    assert_eq!(cycle_status(&app).await["phase"], "off");
    watch.abort();
}
