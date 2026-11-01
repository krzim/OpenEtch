//! Approval modes (`--approval pin|click`) and one-click Start, through the whole router on the simulated board
//! (inputs set with `POST /api/sim/inputs`). The helpers serve the foot switch cycle's tests too (`cycle_tests.rs`).

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

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, ApprovalMode, PhysicalConfirm, PinStore};
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, Event, api};

pub(crate) const PIN: &str = "975310";
/// The foot switch: the profile's Start Marking input, bit 14.
pub(crate) const PEDAL: u16 = 1 << 14;
/// The door interlock of [`door_profile`], bit 5 (open while set).
pub(crate) const DOOR_OPEN: u16 = 1 << 5;

pub(crate) fn pedal_profile() -> MachineProfile {
    MachineProfile { start_mark_input_bit: Some(14), ..MachineProfile::generic_lmc_fiber() }
}

pub(crate) fn door_profile() -> MachineProfile {
    let mut p = pedal_profile();
    p.io.door_protect = Some(InputPort { bit: 5, active_low: false });
    p
}

/// A connected simulator engine. In the `pin` mode the PIN is set and the mode is left at its default; in the
/// `click` mode no PIN file exists.
pub(crate) async fn state(profile: MachineProfile, mode: ApprovalMode) -> Arc<AppState> {
    state_with(profile, mode, None).await
}

/// [`state`] with the physical confirmation input (`--confirm-input`): an approved run then holds the board until
/// that input is pressed.
pub(crate) async fn state_with(profile: MachineProfile, mode: ApprovalMode, confirm: Option<u8>) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("approve-modes");
    if mode == ApprovalMode::Pin {
        pin::set_pin(&dir, PIN).expect("set PIN");
    }
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: confirm.map(PhysicalConfirm::new) };
    let projects = crate::tests::test_projects();
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, projects);
    if mode != ApprovalMode::Pin {
        st.set_approval_mode(mode).expect("set once");
        assert_eq!(st.set_approval_mode(ApprovalMode::Pin), Err(mode), "the mode never changes once set");
    }
    st
}

pub(crate) fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

/// `r` with the headers a browser adds to the app's own `fetch` (page scripts cannot set them).
pub(crate) fn browser(mut r: Request<Body>) -> Request<Body> {
    r.headers_mut().insert("sec-fetch-site", "same-origin".parse().unwrap());
    r.headers_mut().insert("origin", "http://localhost:7878".parse().unwrap());
    r
}

pub(crate) async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v)
}

pub(crate) async fn rect(app: &Router) {
    let body = json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 10}, "origin": "ui"});
    let (s, v) = send(app, req("POST", "/api/execute", Some(body))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
}

pub(crate) async fn pending_run(app: &Router, options: Option<Value>) -> String {
    let (s, v) = send(app, req("POST", "/api/runs", options)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

pub(crate) fn approve(id: &str, body: Option<Value>) -> Request<Body> {
    req("POST", &format!("/api/runs/{id}/approve"), body)
}

pub(crate) async fn set_inputs(app: &Router, bits: u16) {
    let (s, v) = send(app, req("POST", "/api/sim/inputs", Some(json!({"bits": bits})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// Release the pedal (long enough for the watcher to see it), then press it.
pub(crate) async fn press(app: &Router, others: u16) {
    set_inputs(app, others).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    set_inputs(app, others | PEDAL).await;
}

pub(crate) fn state_of(st: &AppState, id: &str) -> RunState {
    st.runs().get(id).expect("run exists").state
}

pub(crate) async fn wait_final(st: &AppState, id: &str) -> Run {
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
pub(crate) async fn machine_event(rx: &mut Receiver<Event>, name: &str) -> Option<String> {
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

/// The foot switch machine events (`foot_cycle`, `foot_cycle_notice`) received so far.
pub(crate) fn foot_events(rx: &mut Receiver<Event>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let Event::Machine { event, .. } = e
            && event.starts_with("foot_")
        {
            out.push(event);
        }
    }
    out
}

/// `POST /api/job/start` with these run options (one-click Start).
pub(crate) fn start(body: Option<Value>) -> Request<Body> {
    req("POST", "/api/job/start", body)
}

/// The pending runs `GET /api/machine` lists.
pub(crate) async fn pending(app: &Router) -> Vec<Value> {
    let (_, m) = send(app, req("GET", "/api/machine", None)).await;
    m["pending_runs"].as_array().cloned().unwrap_or_default()
}

/// How many runs the engine knows (one-click Start and the foot switch cycle create theirs).
pub(crate) fn run_count(st: &AppState) -> usize {
    st.runs().views(st.revision()).len()
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
    assert_eq!((&m["foot_cycle"]["available"], &m["foot_cycle"]["phase"]), (&json!(false), &json!("off")), "{m}");
    rect(&app).await;
    let id = pending_run(&app, None).await;
    let mut events = st.subscribe();
    press(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(state_of(&st, &id), RunState::Pending, "the foot switch never approves in the PIN mode");
    assert!(!st.machine.is_framing(), "nor frames");
    assert!(foot_events(&mut events).is_empty());
    set_inputs(&app, 0).await;

    // One-click Start does not exist in the PIN mode, browser or not.
    let (s, v) = send(&app, browser(start(None))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("approval_mode")), "{v}");
    assert_eq!(run_count(&st), 1, "nothing planned");

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

/// LightBurn's Start: one request plans and approves the current document, from the app in a browser only.
#[tokio::test]
async fn one_click_start_burns_from_the_app_and_refuses_everything_else() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let counters = st.machine.sim_counters().unwrap().clone();
    // Nothing to burn: refused, nothing left waiting.
    let (s, v) = send(&app, browser(start(None))).await;
    assert!(s.is_client_error() && v["error"].as_str().unwrap().contains("nothing"), "{s} {v}");
    assert_eq!(run_count(&st), 0);
    rect(&app).await;

    // No Origin and no fetch metadata (curl, an agent's HTTP client), MCP client headers, another site, a non-UI
    // origin: refused before anything is planned.
    let (s, v) = send(&app, start(Some(json!({})))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("browser_required")), "{v}");
    for marker in ["mcp-session-id", "mcp-protocol-version", "x-galvo-agent"] {
        let mut r = browser(start(None));
        r.headers_mut().insert(marker, "x".parse().unwrap());
        let (s, v) = send(&app, r).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{marker}: {v}");
    }
    for site in ["cross-site", "same-site", "none"] {
        let mut r = browser(start(None));
        r.headers_mut().insert("sec-fetch-site", site.parse().unwrap());
        assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN, "{site}");
    }
    let mut r = browser(start(None));
    r.headers_mut().insert("origin", "http://evil.example".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, browser(start(Some(json!({"origin": "agent:x"}))))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, browser(start(Some(json!({"bogus": 1}))))).await.0, StatusCode::BAD_REQUEST, "unknown options");
    assert_eq!(run_count(&st), 0, "nothing planned");
    assert_eq!(counters.emitting_packets(), 0, "nothing fired");

    // The app's Start button: planned, approved and burned in one click, nothing left pending.
    let (s, v) = send(&app, browser(start(Some(json!({"origin": "ui"}))))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["id"], json!(id), "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);
    assert!(pending(&app).await.is_empty());
    // Run options as for POST /api/runs.
    let (s, v) = send(&app, browser(start(Some(json!({"repeat": 2}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, v["id"].as_str().unwrap()).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 2), "{:?}", run.error);
}

/// The door, the Job Checklist, Require Framing, a busy board and Stop At refuse one-click Start, and a refused
/// Start leaves nothing waiting.
#[tokio::test]
async fn one_click_start_keeps_every_gate() {
    let st = state(door_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let counters = st.machine.sim_counters().unwrap().clone();
    rect(&app).await;
    set_inputs(&app, DOOR_OPEN).await;
    let (s, v) = send(&app, browser(start(None))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("door"), "{v}");
    set_inputs(&app, 0).await;

    let device = st.active_device().id;
    crate::libs::lock(&st.libs.prefs).device_mut(&device).checklist =
        crate::prefs::Checklist { enabled: true, items: vec!["Eyewear on".into()] };
    let (s, v) = send(&app, browser(start(None))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Checklist"), "{v}");
    let (s, v) = send(&app, browser(start(Some(json!({"checklist": [false]}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    crate::libs::lock(&st.libs.prefs).device_mut(&device).require_framing = true;
    let (s, v) = send(&app, browser(start(Some(json!({"checklist": [true]}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Require Framing"), "{v}");
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Another run holds the machine.
    assert!(st.machine.try_begin_run());
    let (s, v) = send(&app, browser(start(Some(json!({"checklist": [true]}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    st.machine.end_run();
    assert_eq!(run_count(&st), 0, "refused Starts leave nothing");
    assert_eq!(counters.emitting_packets(), 0);

    // Framed, ticked, door closed: started.
    let (s, v) = send(&app, browser(start(Some(json!({"checklist": [true]}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, v["id"].as_str().unwrap()).await.state, RunState::Finished);

    // Stop At reached.
    let (s, v) = send(&app, req("PUT", "/api/job/settings", Some(json!({"stop_at": 1})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, browser(start(Some(json!({"checklist": [true]}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Stop At"), "{v}");
    assert_eq!(run_count(&st), 1);
}

/// Start burns only the document revision the app showed: an edit that came in just before the click (an agent's,
/// over MCP or HTTP) refuses the Start, and nothing is planned, left waiting or fired.
#[tokio::test]
async fn one_click_start_burns_only_the_revision_the_app_showed() {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let counters = st.machine.sim_counters().unwrap().clone();
    rect(&app).await;
    let shown = st.revision();
    // An agent's edit lands between the person looking at the job and clicking Start.
    let body = json!({"id": "shape.rect", "params": {"width_mm": 30, "height_mm": 30}, "origin": "agent:test"});
    let (s, v) = send(&app, req("POST", "/api/execute", Some(body))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_ne!(st.revision(), shown);
    let (s, v) = send(&app, browser(start(Some(json!({"revision": shown}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("changed since Start was clicked"), "{v}");
    assert_eq!(run_count(&st), 0, "nothing planned or left waiting");
    assert_eq!(counters.emitting_packets(), 0, "nothing fired");
    assert_eq!(send(&app, browser(start(Some(json!({"revision": "x"}))))).await.0, StatusCode::BAD_REQUEST);
    // The revision the app shows now: started.
    let (s, v) = send(&app, browser(start(Some(json!({"revision": st.revision()}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, v["id"].as_str().unwrap()).await;
    assert_eq!((run.state, run.revision), (RunState::Finished, st.revision()), "{:?}", run.error);
}

/// A Stop that comes in while Start still checks its gates (here: held at the device preferences, before the door is
/// read) cancels that Start: nothing fires after a Stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_start_checks_the_door_cancels_the_start() {
    let st = state(door_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    let counters = st.machine.sim_counters().unwrap().clone();
    rect(&app).await;
    let (locked_tx, locked_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let held = st.clone();
    let holder = std::thread::spawn(move || {
        let _prefs = crate::libs::lock(&held.libs.prefs);
        locked_tx.send(()).unwrap();
        release_rx.recv().unwrap();
    });
    locked_rx.recv().unwrap();
    let app2 = app.clone();
    let click = tokio::spawn(async move { send(&app2, browser(start(None))).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    st.stop().await.expect("stop");
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    let (s, v) = click.await.unwrap();
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("stopped"), "{v}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(counters.emitting_packets(), 0, "nothing fired after the Stop");
    assert!(!st.machine.is_running());
    assert_eq!(run_count(&st), 0, "nothing left waiting");
}

#[test]
fn mcp_has_no_way_to_approve() {
    for name in crate::mcp::TOOL_NAMES {
        for word in ["approve", "pin", "arm", "input", "fire"] {
            assert!(!name.contains(word), "MCP tool {name}");
        }
    }
}
