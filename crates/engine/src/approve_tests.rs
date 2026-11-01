//! Approval modes (`--approval pin|click`) and the foot switch, through the whole router on the simulated board
//! (inputs set with `POST /api/sim/inputs`).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{InputPort, MachineProfile, PollConfig};
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;
use tower::ServiceExt;

use super::foot_switch_pressed;
use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, ApprovalMode, PinStore};
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, Event, api};

const PIN: &str = "975310";
/// The foot switch: the profile's Start Marking input, bit 14.
const PEDAL: u16 = 1 << 14;
/// The door interlock of [`door_profile`], bit 5 (open while set).
const DOOR_OPEN: u16 = 1 << 5;

fn pedal_profile() -> MachineProfile {
    MachineProfile { start_mark_input_bit: Some(14), ..MachineProfile::generic_lmc_fiber() }
}

fn door_profile() -> MachineProfile {
    let mut p = pedal_profile();
    p.io.door_protect = Some(InputPort { bit: 5, active_low: false });
    p
}

/// A connected simulator engine. In the `pin` mode the PIN is set and the mode is left at its default; in the
/// `click` mode no PIN file exists.
async fn state(profile: MachineProfile, mode: ApprovalMode) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("approve-modes");
    if mode == ApprovalMode::Pin {
        pin::set_pin(&dir, PIN).expect("set PIN");
    }
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let projects = crate::tests::test_projects();
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, projects);
    if mode != ApprovalMode::Pin {
        st.set_approval_mode(mode).expect("set once");
        assert_eq!(st.set_approval_mode(ApprovalMode::Pin), Err(mode), "the mode never changes once set");
    }
    st
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// `r` with the headers a browser adds to the app's own `fetch` (page scripts cannot set them).
fn browser(mut r: Request<Body>) -> Request<Body> {
    r.headers_mut().insert("sec-fetch-site", "same-origin".parse().unwrap());
    r.headers_mut().insert("origin", "http://localhost:7878".parse().unwrap());
    r
}

async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v)
}

async fn rect(app: &Router) {
    let body = json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 10}, "origin": "ui"});
    let (s, v) = send(app, req("POST", "/api/execute", Some(body))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
}

async fn pending_run(app: &Router, options: Option<Value>) -> String {
    let (s, v) = send(app, req("POST", "/api/runs", options)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

fn approve(id: &str, body: Option<Value>) -> Request<Body> {
    req("POST", &format!("/api/runs/{id}/approve"), body)
}

async fn set_inputs(app: &Router, bits: u16) {
    let (s, v) = send(app, req("POST", "/api/sim/inputs", Some(json!({"bits": bits})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// Release the pedal (long enough for the watcher to see it), then press it.
async fn press(app: &Router, others: u16) {
    set_inputs(app, others).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    set_inputs(app, others | PEDAL).await;
}

fn state_of(st: &AppState, id: &str) -> RunState {
    st.runs().get(id).expect("run exists").state
}

async fn wait_final(st: &AppState, id: &str) -> Run {
    for _ in 0..1000 {
        let run = st.runs().get(id).cloned().expect("run exists");
        if run.state.is_final() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run {id} did not finish: {:?}", st.runs().get(id).map(|r| r.state));
}

/// The next machine event named `name` (its detail), waiting up to 3 s.
async fn machine_event(rx: &mut Receiver<Event>, name: &str) -> Option<String> {
    let wait = async {
        loop {
            match rx.recv().await {
                Ok(Event::Machine { event, detail }) if event == name => return detail,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event channel: {e}"),
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(3), wait).await.unwrap_or_else(|_| panic!("no {name} event"))
}

/// Foot-switch machine events received so far.
fn foot_events(rx: &mut Receiver<Event>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let Event::Machine { event, .. } = e
            && event.starts_with("foot_switch")
        {
            out.push(event);
        }
    }
    out
}

#[tokio::test]
async fn click_mode_approves_only_the_apps_start_button_in_a_browser() {
    let st = state(MachineProfile::generic_lmc_fiber(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["approval"]["mode"], &m["approval"]["foot_switch"]), (&json!("click"), &json!(null)), "{m}");
    assert_eq!(m["approval"]["pin_set"], false, "no operator PIN is needed");
    rect(&app).await;
    let id = pending_run(&app, None).await;

    // No Origin and no fetch metadata (curl, an agent's HTTP client): refused, with what to do instead.
    let (s, v) = send(&app, approve(&id, Some(json!({})))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("browser_required")), "{v}");
    let msg = v["error"].as_str().unwrap();
    assert!(msg.contains("no Origin") && msg.contains("foot switch"), "{msg}");
    // A PIN does not stand in for the browser.
    let (s, v) = send(&app, approve(&id, Some(json!({"pin": PIN})))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("browser_required")), "{v}");
    // Sec-Fetch-Site without Origin, from another site, or same-site: refused.
    let mut r = approve(&id, None);
    r.headers_mut().insert("sec-fetch-site", "same-origin".parse().unwrap());
    assert_eq!(send(&app, r).await.1["code"], "browser_required");
    for site in ["cross-site", "same-site", "none"] {
        let mut r = browser(approve(&id, None));
        r.headers_mut().insert("sec-fetch-site", site.parse().unwrap());
        assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN, "{site}");
    }
    // An MCP client header, even with the browser headers: refused.
    for marker in ["mcp-session-id", "mcp-protocol-version", "x-galvo-agent"] {
        let mut r = browser(approve(&id, None));
        r.headers_mut().insert(marker, "x".parse().unwrap());
        let (s, v) = send(&app, r).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{marker}: {v}");
        assert!(v["error"].as_str().unwrap().contains("person"), "{v}");
    }
    // A non-UI origin in the body: refused.
    assert_eq!(send(&app, browser(approve(&id, Some(json!({"origin": "agent:x"}))))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(state_of(&st, &id), RunState::Pending);
    let counters = st.machine.sim_counters().unwrap().clone();
    assert_eq!(counters.emitting_packets(), 0, "nothing fired");

    // The app's Start button: approved without a PIN.
    let (s, v) = send(&app, browser(approve(&id, Some(json!({}))))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);
}

#[tokio::test]
async fn pin_mode_is_unchanged_and_ignores_the_foot_switch() {
    let st = state(pedal_profile(), ApprovalMode::Pin).await;
    assert_eq!(st.approval_mode(), ApprovalMode::Pin, "the default");
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["approval"]["mode"], &m["approval"]["foot_switch"]), (&json!("pin"), &json!(null)), "{m}");
    rect(&app).await;
    let id = pending_run(&app, None).await;
    let mut events = st.subscribe();
    press(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(state_of(&st, &id), RunState::Pending, "the foot switch never approves in the PIN mode");
    assert!(foot_events(&mut events).is_empty());
    set_inputs(&app, 0).await;

    // The browser headers do not replace the PIN.
    let (s, v) = send(&app, browser(approve(&id, None))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_required")), "{v}");
    let (s, v) = send(&app, browser(approve(&id, Some(json!({"pin": "000000"}))))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_invalid")), "{v}");
    // Without the browser headers the PIN approves, as before.
    let (s, v) = send(&app, approve(&id, Some(json!({"pin": PIN})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    watch.abort();
}

#[tokio::test]
async fn the_foot_switch_approves_exactly_one_pending_run() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["approval"]["foot_switch"], json!({"input_bit": 14, "active_low": false}), "{m}");
    let counters = st.machine.sim_counters().unwrap().clone();
    let mut events = st.subscribe();

    // Nothing waiting.
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_ignored").await.as_deref(), Some("no_pending"));

    // Two waiting (one requested in the app, one as an agent would): nothing is approved.
    rect(&app).await;
    let a = pending_run(&app, None).await;
    let b = pending_run(&app, Some(json!({"origin": "agent:test"}))).await;
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_ignored").await.as_deref(), Some("several_pending"));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!((state_of(&st, &a), state_of(&st, &b)), (RunState::Pending, RunState::Pending));
    assert_eq!(counters.emitting_packets(), 0);

    // An edit makes both stale; the one run requested after it is the only one the switch can approve.
    rect(&app).await;
    let c = pending_run(&app, Some(json!({"origin": "agent:test"}))).await;
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_approved").await.as_deref(), Some(c.as_str()));
    let run = wait_final(&st, &c).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);
    assert_eq!((state_of(&st, &a), state_of(&st, &b)), (RunState::Pending, RunState::Pending), "stale runs untouched");
    // Still held after the run: no second approval without a release.
    let d = {
        rect(&app).await;
        pending_run(&app, None).await
    };
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(state_of(&st, &d), RunState::Pending, "a held pedal approves nothing");
    watch.abort();
}

#[tokio::test]
async fn the_foot_switch_does_nothing_while_a_run_holds_the_board() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = pending_run(&app, None).await;
    let mut events = st.subscribe();
    // Another run holds the machine.
    assert!(st.machine.try_begin_run());
    press(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(state_of(&st, &id), RunState::Pending);
    // It ends while the pedal is still held: that press is not counted afterwards.
    st.machine.end_run();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(state_of(&st, &id), RunState::Pending);
    assert!(foot_events(&mut events).is_empty(), "no foot switch event while the board was held");
    // Release and press: approved.
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_approved").await.as_deref(), Some(id.as_str()));
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    watch.abort();
}

/// The press that approves a run with `start_input` starts its first iteration; each later iteration waits for its
/// own press (the run owns the input while it holds the board, so those presses approve nothing else).
#[tokio::test]
async fn a_foot_switch_approval_starts_the_first_iteration_and_later_ones_wait() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = pending_run(&app, Some(json!({"repeat": 2, "start_input": true}))).await;
    let mut events = st.subscribe();
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_approved").await.as_deref(), Some(id.as_str()));
    assert_eq!(machine_event(&mut events, "waiting_for_start_input").await.as_deref(), Some(id.as_str()));
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!((run.completed, run.state), (1, RunState::AwaitingConfirm), "first iteration ran without a second press");
    press(&app, 0).await;
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 2), "{:?}", run.error);
    assert!(foot_events(&mut events).is_empty(), "the second press belonged to the run");
    watch.abort();
}

#[tokio::test]
async fn an_open_door_still_refuses_start_and_the_foot_switch() {
    let st = state(door_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = pending_run(&app, None).await;
    set_inputs(&app, DOOR_OPEN).await;
    let (s, v) = send(&app, browser(approve(&id, None))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("door"), "{v}");
    let mut events = st.subscribe();
    press(&app, DOOR_OPEN).await;
    let why = machine_event(&mut events, "foot_switch_refused").await.unwrap_or_default();
    assert!(why.contains("door"), "{why}");
    assert_eq!(state_of(&st, &id), RunState::Pending);
    assert_eq!(st.machine.sim_counters().unwrap().emitting_packets(), 0);
    // Door closed: the next press approves.
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "foot_switch_approved").await.as_deref(), Some(id.as_str()));
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    watch.abort();
}

#[tokio::test]
async fn stop_wins_over_a_foot_switch_approval() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = pending_run(&app, Some(json!({"repeat": 3, "start_input": true}))).await;
    let mut events = st.subscribe();
    press(&app, 0).await;
    assert_eq!(machine_event(&mut events, "waiting_for_start_input").await.as_deref(), Some(id.as_str()));
    let counters = st.machine.sim_counters().unwrap().clone();
    let fired = counters.emitting_packets();
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    // Stop cancels a run that is not streaming (here: waiting for its second press).
    assert_eq!((run.state, run.completed), (RunState::Cancelled, 1), "{:?}", run.error);
    press(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(counters.emitting_packets(), fired, "nothing fires after Stop");
    watch.abort();
}

/// A press approves only the run that was already the single one waiting when the press began: a run requested
/// (e.g. by an agent) while the pedal goes down, or one that replaced the run the operator saw, is not approved. The
/// watcher's side (what was waiting when the press began, read by read) is `libs::tests`; it is not timed through
/// the watcher here, where the 50 ms polls would make the test depend on scheduling.
#[tokio::test]
async fn a_press_never_approves_a_run_requested_during_it() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let a = pending_run(&app, Some(json!({"origin": "agent:test"}))).await;
    let mut events = st.subscribe();
    // The press began with nothing waiting.
    foot_switch_pressed(&st, &[]).await;
    assert_eq!(machine_event(&mut events, "foot_switch_ignored").await.as_deref(), Some("changed_during_press"));
    // The press began while another run waited (since replaced).
    foot_switch_pressed(&st, &["run-gone".to_owned()]).await;
    assert_eq!(machine_event(&mut events, "foot_switch_ignored").await.as_deref(), Some("changed_during_press"));
    // The press began while two waited and one was cancelled during it.
    foot_switch_pressed(&st, &[a.clone(), "run-gone".to_owned()]).await;
    assert_eq!(machine_event(&mut events, "foot_switch_ignored").await.as_deref(), Some("changed_during_press"));
    assert_eq!(state_of(&st, &a), RunState::Pending);
    assert_eq!(st.machine.sim_counters().unwrap().emitting_packets(), 0);
    // The press began while it was the one waiting: approved.
    foot_switch_pressed(&st, std::slice::from_ref(&a)).await;
    assert_eq!(machine_event(&mut events, "foot_switch_approved").await.as_deref(), Some(a.as_str()));
    assert_eq!(wait_final(&st, &a).await.state, RunState::Finished);
}

/// A Job Checklist cannot be ticked with a foot: with it on, a press is refused (Start with the ticks approves).
#[tokio::test]
async fn the_checklist_refuses_a_foot_switch_press() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let device = st.active_device().id;
    crate::libs::lock(&st.libs.prefs).device_mut(&device).checklist =
        crate::prefs::Checklist { enabled: true, items: vec!["Eyewear on".into()] };
    rect(&app).await;
    let id = pending_run(&app, None).await;
    let mut events = st.subscribe();
    foot_switch_pressed(&st, std::slice::from_ref(&id)).await;
    let why = machine_event(&mut events, "foot_switch_refused").await.unwrap_or_default();
    assert!(why.contains("Checklist"), "{why}");
    assert_eq!(state_of(&st, &id), RunState::Pending);
    let (s, v) = send(&app, browser(approve(&id, Some(json!({"checklist": [true]}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

#[test]
fn mcp_has_no_way_to_approve() {
    for name in crate::mcp::TOOL_NAMES {
        for word in ["approve", "pin", "arm", "input", "fire"] {
            assert!(!name.contains(word), "MCP tool {name}");
        }
    }
}
