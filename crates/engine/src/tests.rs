//! Route tests through the whole router (`tower::ServiceExt::oneshot`), on the simulated board.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use lmc::{Packet, Single};
use machine::{Link, LinkError, MachineProfile, PollConfig, Reply, SimLink};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::{LinkKind, Machine};
use crate::pin::{self, ApprovalConfig, PhysicalConfirm, PinStore};
use crate::projects::ProjectDir;
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, api};

/// The operator PIN the test states are set up with.
pub(crate) const TEST_PIN: &str = "246800";

/// Approval config in a fresh temp config dir with [`TEST_PIN`] set.
fn approval(confirm: Option<PhysicalConfirm>) -> ApprovalConfig {
    let dir = pin::temp_dir("engine");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    ApprovalConfig { pin: PinStore::new(&dir), confirm }
}

/// A fresh, not yet created projects directory under the temp dir.
pub(crate) fn test_projects() -> ProjectDir {
    ProjectDir::new(pin::temp_dir("projects").join("projects"))
}

/// A connected engine state on the simulated board with the generic profile.
pub(crate) async fn test_state() -> Arc<AppState> {
    test_state_with(SecurityConfig::default()).await
}

async fn test_state_with(security: SecurityConfig) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    state_on(machine, security, approval(None)).await
}

async fn state_on(machine: Arc<Machine>, security: SecurityConfig, approval: ApprovalConfig) -> Arc<AppState> {
    machine.connect().await.expect("sim connects");
    AppState::new(Session::new(doc::Document::default()), machine, security, approval, test_projects())
}

/// The approve body with the right PIN.
fn pin_body() -> Option<Value> {
    Some(json!({ "pin": TEST_PIN }))
}

/// Wait until run `id` reaches a final state.
async fn wait_final(st: &AppState, id: &str) -> Run {
    for _ in 0..500 {
        let run = st.runs().get(id).cloned().expect("run exists");
        if run.state.is_final() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {id} did not finish: {:?}", st.runs().get(id).map(|r| r.state));
}

/// Draw a rectangle and request a run; returns the run id.
async fn pending_run(app: &Router) -> String {
    rect(app, 10.0, 10.0).await;
    let (s, v) = send(app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

fn app(st: &Arc<AppState>) -> Router {
    api::router(st.clone(), None)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v)
}

async fn rect(app: &Router, w: f64, h: f64) -> String {
    let (s, v) = send(
        app,
        req(
            "POST",
            "/api/execute",
            Some(json!({"id": "shape.rect", "params": {"width_mm": w, "height_mm": h}, "origin": "ui"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    v["result"]["created"][0].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn commands_are_listed() {
    let st = test_state().await;
    let (s, v) = send(&app(&st), req("GET", "/api/commands", None)).await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = v.as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"shape.rect") && ids.contains(&"doc.inspect"));
    assert!(v[0]["input_schema"].is_object());
}

#[tokio::test]
async fn execute_edits_and_reports_failures() {
    let st = test_state().await;
    let app = app(&st);
    let mut events = st.subscribe();
    let id = rect(&app, 10.0, 5.0).await;
    assert_eq!(
        events.recv().await.unwrap(),
        crate::Event::DocumentChanged { revision: 1, origin: commands::Origin::Ui }
    );

    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(doc["revision"], 1);
    assert_eq!(doc["objects"][0]["id"], id.as_str());

    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "no.such", "params": {}})))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("no.such"));

    let (s, v) =
        send(&app, req("POST", "/api/execute", Some(json!({"id": "object.delete", "params": {"ids": ["obj-99"]}}))))
            .await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");

    let (s, _) = send(&app, req("POST", "/api/execute", Some(json!({"params": {}})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", "/api/execute", Some(json!({"id": "doc.inspect", "origin": "root"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, hist) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(hist.as_array().unwrap().len(), 1);
    assert_eq!(hist[0]["command"], "shape.rect");
    assert_eq!(hist[0]["origin"]["kind"], "ui");
}

#[tokio::test]
async fn batch_is_atomic() {
    let st = test_state().await;
    let app = app(&st);
    let good = json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 10}});
    let bad = json!({"id": "object.delete", "params": {"ids": ["obj-404"]}});
    let (s, v) =
        send(&app, req("POST", "/api/batch", Some(json!({"commands": [good, good, bad], "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["ok"], false, "{v}");
    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(doc["revision"], 0);
    assert!(doc["objects"].as_array().unwrap().is_empty());

    let (_, v) =
        send(&app, req("POST", "/api/batch", Some(json!({"commands": [good, good], "origin": "agent:test"})))).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["revision"], 1);
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
    let (_, hist) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(hist.as_array().unwrap().len(), 1);
    assert_eq!(hist[0]["origin"], json!({"kind": "agent", "name": "test"}));
    // One undo removes both.
    let (_, v) = send(&app, req("POST", "/api/undo", Some(json!({"origin": "ui"})))).await;
    assert_eq!(v["ok"], true);
    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert!(doc["objects"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn undo_and_redo() {
    let st = test_state().await;
    let app = app(&st);
    let (_, v) = send(&app, req("POST", "/api/undo", None)).await;
    assert_eq!(v["ok"], false);
    rect(&app, 10.0, 10.0).await;
    let (_, v) = send(&app, req("POST", "/api/undo", Some(json!({"origin": "ui"})))).await;
    assert_eq!(v, json!({"ok": true, "revision": 2}));
    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert!(doc["objects"].as_array().unwrap().is_empty());
    let (_, v) = send(&app, req("POST", "/api/redo", None)).await;
    assert_eq!(v, json!({"ok": true, "revision": 3}));
    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(doc["objects"].as_array().unwrap().len(), 1);
    let (_, v) = send(&app, req("POST", "/api/redo", None)).await;
    assert_eq!(v["ok"], false);
}

#[tokio::test]
async fn preview_and_svg() {
    let st = test_state().await;
    let app = app(&st);
    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["segments"].as_array().unwrap().is_empty());

    rect(&app, 20.0, 10.0).await;
    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::OK);
    let segs = v["segments"].as_array().unwrap();
    assert!(segs.iter().any(|g| g["kind"] == "mark" && g["layer"] == "layer-1"));
    assert!((v["stats"]["mark_length_mm"].as_f64().unwrap() - 60.0).abs() < 0.1);

    let res = app.clone().oneshot(req("GET", "/api/preview.svg", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "image/svg+xml");
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let svg = String::from_utf8(body.to_vec()).unwrap();
    assert!(svg.starts_with("<svg") && svg.contains("stroke-linecap"));

    // Out of field: preview reports the planning error, the SVG still renders.
    send(
        &app,
        req(
            "POST",
            "/api/execute",
            Some(json!({"id": "object.transform", "params": {"ids": ["obj-2"], "translate": [200, 0]}})),
        ),
    )
    .await;
    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("obj-2"));
    let res = app.clone().oneshot(req("GET", "/api/preview.svg", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn runs_never_start_without_approval() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 10.0, 10.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let before = (counters.singles(), counters.packets());

    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["state"], "pending");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!((counters.singles(), counters.packets()), before);
    assert_eq!(counters.laser_singles(), 0);

    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["connected"], true);
    assert_eq!(m["simulated"], true);
    assert_eq!(m["pending_runs"][0]["id"], id.as_str());
    assert!(m["field_mm"].as_f64().unwrap() > 100.0);

    // An MCP client (or a non-UI origin) cannot approve.
    let mut r = req("POST", &format!("/api/runs/{id}/approve"), Some(json!({})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    let (s, _) = send(&app, r).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"origin": "agent:x"})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
    assert_eq!(counters.packets(), before.1);
    assert_eq!(counters.laser_singles(), 0);

    // A person approves with the PIN: now (and only now) the job reaches the board.
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["state"], "approved");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    loop {
        if let crate::Event::RunFinished { id: rid, ok, state, error } = events.recv().await.unwrap() {
            assert_eq!((rid.as_str(), ok, state, error), (id.as_str(), true, RunState::Finished, None));
            break;
        }
    }
    assert!(counters.packets() > before.1);
    assert!(counters.laser_singles() > 0);
    assert!(counters.emitting_packets() > 0);

    // Approving twice is refused.
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn approve_refuses_unknown_and_stale_runs() {
    let st = test_state().await;
    let app = app(&st);
    let (s, v) = send(&app, req("POST", "/api/runs/run-404/approve", pin_body())).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["ok"], false);

    // Empty document: nothing to run.
    let (s, _) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    rect(&app, 10.0, 10.0).await;
    let (_, v) = send(&app, req("POST", "/api/runs", None)).await;
    let id = v["id"].as_str().unwrap().to_owned();
    rect(&app, 5.0, 5.0).await;
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(st.machine.sim_counters().unwrap().laser_singles(), 0);
}

#[tokio::test]
async fn power_above_the_profile_limit_is_refused() {
    let mut profile = MachineProfile::generic_lmc_fiber();
    profile.max_power_pct = 50.0;
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    let st = state_on(machine, SecurityConfig::default(), approval(None)).await;
    let app = app(&st);
    rect(&app, 10.0, 10.0).await;
    let (_, v) = send(
        &app,
        req(
            "POST",
            "/api/execute",
            Some(json!({"id": "layer.update", "params": {"id": "layer-1", "settings": {"power_pct": 60}}})),
        ),
    )
    .await;
    assert_eq!(v["ok"], true, "{v}");
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("50"));
    assert!(st.machine_status()["pending_runs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn frame_is_light_only_and_stops() {
    let st = test_state().await;
    let app = app(&st);
    let (s, _) = send(&app, req("POST", "/api/machine/frame", Some(json!({})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "nothing to frame");
    let id = rect(&app, 10.0, 10.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"ids": [id]})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(counters.packets() > 0);
    assert!(st.machine.is_framing());
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["framing"], true);
    let (s, _) = send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!st.machine.is_framing());
    assert_eq!(counters.laser_singles(), 0);
    assert_eq!(counters.emitting_packets(), 0);
}

#[tokio::test]
async fn stop_is_always_allowed_and_cancels_pending_runs() {
    let st = test_state_with(SecurityConfig::new(Some("t0k3n-abcdefghijklmnop".into()), Vec::new())).await;
    let app = app(&st);
    // No token: refused for the API, but stop still works.
    let (s, _) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let mut r =
        req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 4, "height_mm": 4}})));
    r.headers_mut().insert("authorization", "Bearer t0k3n-abcdefghijklmnop".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::OK);
    let mut r = req("POST", "/api/runs", None);
    r.headers_mut().insert("authorization", "Bearer t0k3n-abcdefghijklmnop".parse().unwrap());
    let (s, v) = send(&app, r).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    let (s, v) = send(&app, req("POST", "/api/machine/stop", Some(json!({})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["cancelled_runs"], 1);
    assert_eq!(v["connected"], true);
    // A query token only counts for the WebSocket.
    let (s, _) = send(&app, req("GET", "/api/machine?token=t0k3n-abcdefghijklmnop", None)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(&app, req("POST", "/mcp?token=t0k3n-abcdefghijklmnop", Some(json!({})))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(&app, req("GET", "/api/events?token=t0k3n-abcdefghijklmnop", None)).await;
    assert_ne!(s, StatusCode::UNAUTHORIZED, "the WebSocket accepts ?token= (this request is no upgrade, so not 101)");
    let mut r = req("GET", "/api/machine", None);
    r.headers_mut().insert("authorization", "Bearer t0k3n-abcdefghijklmnop".parse().unwrap());
    let (s, m) = send(&app, r).await;
    assert_eq!(s, StatusCode::OK);
    assert!(m["pending_runs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn foreign_hosts_and_origins_are_refused() {
    let st = test_state().await;
    let app = app(&st);
    let mut r = req("GET", "/api/document", None);
    r.headers_mut().insert("host", "evil.example:7878".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    let mut r = req("POST", "/api/execute", Some(json!({"id": "doc.inspect"})));
    r.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    let mut r = req("GET", "/api/document", None);
    r.headers_mut().insert("origin", "http://localhost:5173".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::OK);
    let (s, _) = send(&app, req("GET", "/api/nope", None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn mcp_endpoint_is_mounted() {
    let st = test_state().await;
    let app = app(&st);
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}});
    let r = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost:7878")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(Body::from(init.to_string()))
        .unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().contains_key("mcp-session-id"));
}

#[tokio::test]
async fn approval_pin_errors_have_codes() {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    let dir = pin::temp_dir("codes");
    let st = state_on(
        machine,
        SecurityConfig::default(),
        ApprovalConfig { pin: PinStore::new(&dir), confirm: Some(PhysicalConfirm::new(14)) },
    )
    .await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let approve = |body: Option<Value>| req("POST", &format!("/api/runs/{id}/approve"), body);
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(
        m["approval"],
        json!({"pin_set": false, "pin_changed": false, "locked_until_unix_s": null,
               "physical_confirm": {"input_bit": 14, "timeout_s": 30}})
    );
    let (s, v) = send(&app, approve(pin_body())).await;
    assert_eq!((s, &v["code"], &v["ok"]), (StatusCode::FORBIDDEN, &json!("pin_not_set"), &json!(false)), "{v}");

    // A PIN set while the engine runs is never used: approvals are refused until a restart.
    pin::set_pin(&dir, TEST_PIN).unwrap();
    let (s, v) = send(&app, approve(pin_body())).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_changed")), "{v}");
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["approval"]["pin_set"], &m["approval"]["pin_changed"]), (&json!(false), &json!(true)));
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
    let _ = std::fs::remove_dir_all(&dir);

    // After a "restart" (a store loaded with the PIN in place):
    let st = test_state().await;
    let app = self::app(&st);
    let id = pending_run(&app).await;
    let approve = |body: Option<Value>| req("POST", &format!("/api/runs/{id}/approve"), body);
    for body in [None, Some(json!({})), Some(json!({"pin": ""}))] {
        let (s, v) = send(&app, approve(body)).await;
        assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_required")), "{v}");
    }
    let mut events = st.subscribe();
    for _ in 1..pin::MAX_FAILURES {
        let (s, v) = send(&app, approve(Some(json!({"pin": "135790"})))).await;
        assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_invalid")), "{v}");
    }
    let (s, v) = send(&app, approve(Some(json!({"pin": "135790"})))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_locked")), "{v}");
    assert!(v["error"].as_str().unwrap().contains("another"), "{v}");
    let crate::Event::ApprovalLocked { until_unix_s } = events.recv().await.unwrap() else {
        panic!("expected approval_locked")
    };
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["approval"]["locked_until_unix_s"], json!(until_unix_s));
    let (s, v) = send(&app, approve(pin_body())).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_locked")), "the right PIN is refused while locked");
    assert!(events.try_recv().is_err(), "a refusal while locked starts no new lockout");
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
    let c = st.machine.sim_counters().unwrap();
    assert_eq!((c.laser_singles(), c.emitting_packets()), (0, 0));
}

/// Finding 1: STOP's abort sequence wins the controller lock before the approved run does (and clears the abort
/// request). The run must still never start.
#[tokio::test]
async fn stop_between_approval_and_lock_never_runs() {
    let st = test_state().await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let counters = st.machine.sim_counters().unwrap().clone();

    // Hold the controller, as a frame chunk or status poll would.
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let m = st.machine.clone();
    let holder = tokio::spawn(async move {
        m.blocking(move |m, ctl| {
            let _ = locked_tx.send(());
            let _ = release_rx.recv();
            // Exactly what `Machine::abort` does once it holds the lock: the abort request is cleared here.
            m.request_stop();
            ctl.abort()
        })
        .await
    });
    locked_rx.await.unwrap();
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    release_tx.send(()).unwrap();
    holder.await.unwrap().unwrap().unwrap();

    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Cancelled, "stopped before streaming: {:?}", run.error);
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0));
    assert!(!st.machine.is_running());
}

/// The same race through the real stop endpoint: whichever side gets the controller first, nothing fires.
#[tokio::test]
async fn stop_endpoint_during_approval_never_fires() {
    for _ in 0..5 {
        let st = test_state().await;
        let app = app(&st);
        let id = pending_run(&app).await;
        let counters = st.machine.sim_counters().unwrap().clone();
        let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let m = st.machine.clone();
        let holder = tokio::spawn(async move {
            m.blocking(move |_, _| {
                let _ = locked_tx.send(());
                let _ = release_rx.recv();
            })
            .await
        });
        locked_rx.await.unwrap();
        let epoch = st.machine.stop_epoch();
        let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
        assert_eq!(s, StatusCode::OK);
        let stopper = tokio::spawn({
            let app = app.clone();
            async move { send(&app, req("POST", "/api/machine/stop", None)).await }
        });
        while st.machine.stop_epoch() == epoch {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        release_tx.send(()).unwrap();
        holder.await.unwrap().unwrap();
        let (s, v) = stopper.await.unwrap();
        assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
        // The run was approved but had not reached the controller: cancelled, and counted.
        assert_eq!(v["cancelled_runs"], 1, "{v}");
        let run = wait_final(&st, &id).await;
        assert_eq!(run.state, RunState::Cancelled, "{:?}", run.error);
        assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0));
    }
}

/// A link that panics when a list packet is streamed, recording every single command.
struct PanicLink {
    inner: SimLink,
    singles: Arc<Mutex<Vec<Single>>>,
}

impl Link for PanicLink {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        self.singles.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(cmd);
        let r = self.inner.send_single(cmd);
        self.inner.clear_traffic();
        r
    }

    fn send_packet(&mut self, _packet: &Packet) -> Result<(), LinkError> {
        panic!("simulated driver bug while streaming");
    }
}

/// Finding 4: a panic in the run task still sends the abort sequence.
#[tokio::test]
async fn a_panicking_run_is_aborted() {
    let singles = Arc::new(Mutex::new(Vec::new()));
    let link = PanicLink { inner: SimLink::new(), singles: singles.clone() };
    let machine = Arc::new(Machine::with_link(
        Box::new(link),
        LinkKind::Simulated,
        None,
        MachineProfile::generic_lmc_fiber(),
        None,
        PollConfig::immediate(),
    ));
    let st = state_on(machine, SecurityConfig::default(), approval(None)).await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let epoch = st.machine.stop_epoch();
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Failed);
    assert!(run.error.as_deref().unwrap_or_default().contains("abort sequence sent"), "{:?}", run.error);
    assert!(st.machine.stop_epoch() > epoch, "the engine's abort ran");
    let singles = singles.lock().unwrap().clone();
    let opened = singles.iter().rposition(|&c| c == Single::fiber_set_mo(true)).expect("the run opened the MO");
    let after = &singles[opened..];
    for closing in [Single::stop_execute(), Single::fiber_set_mo(false), Single::write_port(0)] {
        assert!(after.contains(&closing), "{closing} not sent after the panic: {after:?}");
    }
    assert!(!st.machine.is_running());
}

fn confirm(timeout_ms: u64) -> PhysicalConfirm {
    PhysicalConfirm { input_bit: 14, timeout: Duration::from_millis(timeout_ms), poll: Duration::from_millis(5) }
}

async fn confirm_state(timeout_ms: u64) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    state_on(machine, SecurityConfig::default(), approval(Some(confirm(timeout_ms)))).await
}

#[tokio::test]
async fn physical_confirm_starts_the_run_on_release_then_press() {
    let st = confirm_state(5000).await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!((s, &v["state"]), (StatusCode::OK, &json!("awaiting_confirm")), "{v}");
    tokio::time::sleep(Duration::from_millis(60)).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["runs"][0]["state"], "awaiting_confirm");
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0), "nothing fires before the input");
    counters.set_inputs(1 << 14);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);
}

#[tokio::test]
async fn physical_confirm_times_out_and_ignores_a_held_input() {
    let st = confirm_state(300).await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    // Held (or stuck, or wired inverted) from before the approval: never counts as a press.
    counters.set_inputs(1 << 14);
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Cancelled);
    assert!(run.error.as_deref().unwrap_or_default().contains("not pressed"), "{:?}", run.error);
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0));
    assert!(!st.machine.is_running());
}

#[tokio::test]
async fn stop_cancels_a_run_awaiting_confirmation() {
    let st = confirm_state(5000).await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["cancelled_runs"], 1, "an awaiting_confirm run counts: {v}");
    counters.set_inputs(1 << 14);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Cancelled, "{:?}", run.error);
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0));
}

/// Finding 15: with no board connected, the emergency stop still answers 200.
#[tokio::test]
async fn stop_without_a_board_is_ok() {
    // Machine::usb opens nothing until connect, which this test never calls.
    let machine = Arc::new(Machine::usb(MachineProfile::generic_lmc_fiber(), None));
    let st = AppState::new(
        Session::new(doc::Document::default()),
        machine,
        SecurityConfig::default(),
        approval(None),
        test_projects(),
    );
    let (s, v) = send(&app(&st), req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["ok"], &v["connected"]), (&json!(true), &json!(false)));
}

/// With no board connected, ending a frame is not an error either.
#[tokio::test]
async fn frame_stop_without_a_board_is_ok() {
    let machine = Arc::new(Machine::usb(MachineProfile::generic_lmc_fiber(), None));
    let st = AppState::new(
        Session::new(doc::Document::default()),
        machine,
        SecurityConfig::default(),
        approval(None),
        test_projects(),
    );
    let (s, v) = send(&app(&st), req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"ok": true, "connected": false})));
    // A connected board answers plain ok.
    let st = test_state().await;
    let (s, v) = send(&app(&st), req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"ok": true})));
}

/// A link that sleeps a little on every status poll, so a run spends real time on the board.
struct SlowLink {
    inner: SimLink,
}

impl Link for SlowLink {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        if cmd == Single::get_version() {
            std::thread::sleep(Duration::from_millis(1));
        }
        let r = self.inner.send_single(cmd);
        self.inner.clear_traffic();
        r
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        let r = self.inner.send_packet(packet);
        self.inner.clear_traffic();
        r
    }
}

/// A run stopped once it is streaming ends `aborted`, and the stop does not count it as cancelled.
#[tokio::test]
async fn a_streaming_run_is_aborted_not_cancelled() {
    // The board stays BUSY after ExecuteList, so the run is still on the board when it is stopped.
    let board = machine::SimBoard { busy_polls: usize::MAX, ..machine::SimBoard::default() };
    let link: Box<dyn Link> = Box::new(SlowLink { inner: SimLink::with_board(board) });
    let poll = PollConfig { finish_timeout: Some(Duration::from_secs(20)), ..PollConfig::immediate() };
    let profile = MachineProfile::generic_lmc_fiber();
    let machine = Arc::new(Machine::with_link(link, LinkKind::Simulated, None, profile, None, poll));
    let st = state_on(machine, SecurityConfig::default(), approval(None)).await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK);
    while st.runs().get(&id).unwrap().state != RunState::Running {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["cancelled_runs"], 0, "a running run is not cancelled: {v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Aborted, "{:?}", run.error);
}

/// Previews are coalesced: requests queued behind a running plan share one plan of the newest document instead of
/// each planning the revision it arrived at.
#[tokio::test]
async fn previews_are_coalesced() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 10.0, 10.0).await;
    // Hold the in-flight slot, as a long plan would, while requests and edits pile up.
    let held = st.hold_preview_slot().await;
    let mut tasks = Vec::new();
    for i in 0..6 {
        rect(&app, 5.0 + f64::from(i), 5.0).await;
        let app = app.clone();
        tasks.push(tokio::spawn(async move { send(&app, req("GET", "/api/preview", None)).await }));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(st.previews_computed(), 0, "nothing plans while the slot is held");
    drop(held);
    for t in tasks {
        let (s, v) = t.await.unwrap();
        assert_eq!(s, StatusCode::OK, "{v}");
        // Every answer is of the newest document (7 rectangles), not of the revision it arrived at.
        let marks = v["segments"].as_array().unwrap().iter().filter(|g| g["kind"] == "mark").count();
        assert_eq!(marks, 7 * 4);
    }
    assert_eq!(st.previews_computed(), 1, "six queued requests, one plan");
    // The cached result is dropped once nobody waits; a request after an edit plans again.
    rect(&app, 3.0, 3.0).await;
    assert_eq!(send(&app, req("GET", "/api/preview", None)).await.0, StatusCode::OK);
    assert_eq!(st.previews_computed(), 2);
}


/// Finding 8: ending a frame leaves runs alone.
#[tokio::test]
async fn frame_stop_does_not_cancel_runs() {
    let st = test_state().await;
    let app = app(&st);
    let id = pending_run(&app).await;
    let (s, v) = send(&app, req("POST", "/api/machine/frame", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let epoch = st.machine.stop_epoch();
    let (s, _) = send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!st.machine.is_framing());
    assert_eq!(st.machine.stop_epoch(), epoch);
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

#[tokio::test]
async fn undo_events_carry_the_origin() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 3.0, 3.0).await;
    let mut events = st.subscribe();
    let (_, v) = send(&app, req("POST", "/api/undo", Some(json!({"origin": "agent:cli"})))).await;
    assert_eq!(v["ok"], true);
    assert_eq!(
        events.recv().await.unwrap(),
        crate::Event::DocumentChanged { revision: 2, origin: commands::Origin::Agent("cli".into()) }
    );
}

/// A hand-written LightBurn project: one fill layer, a 20x10 rectangle centred at (80, 75) and a triangle, laid
/// out for a 150 mm field (the generic profile's).
pub(crate) const LBRN2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.00" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="Fill"/>
        <maxPower Value="30"/>
        <speed Value="2000"/>
        <frequency Value="100000"/>
        <QPulseWidth Value="200"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="1" W="20" H="10" Cr="0">
        <XForm>1 0 0 1 80 75</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0">
        <XForm>1 0 0 1 75 75</XForm>
        <VertList>V0 0c0x1c1x1V5 0c0x1c1x1V5 5c0x1c1x1</VertList>
        <PrimList>L0 1L1 2L2 0</PrimList>
    </Shape>
</LightBurnProject>"#;

fn exec(id: &str, params: Value) -> Request<Body> {
    req("POST", "/api/execute", Some(json!({"id": id, "params": params})))
}

/// A raw (non-JSON) request body.
fn raw(method: &str, uri: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder().method(method).uri(uri).header("host", "localhost:7878").body(body.into()).unwrap()
}

/// GET a file: status, headers and body text.
async fn fetch(app: &Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = app.clone().oneshot(req("GET", uri, None)).await.unwrap();
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn projects_save_open_and_list() {
    let st = test_state().await;
    let app = app(&st);
    let (_, doc) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(doc["field_mm"], json!(st.machine.profile().field_mm), "set from the profile at startup");
    let (s, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!({"name": null, "saved_revision": null, "revision": 0, "dirty": false})));
    let (s, v) = send(&app, req("GET", "/api/projects", None)).await;
    assert_eq!((s, v), (StatusCode::OK, json!([])), "a missing projects dir lists nothing");

    rect(&app, 10.0, 5.0).await;
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v["dirty"], true);

    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Coaster (v2)"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["ok"], &v["name"]), (&json!(true), &json!("Coaster (v2)")));
    let path = std::path::PathBuf::from(v["path"].as_str().unwrap());
    assert!(path.ends_with("Coaster (v2).openetch") && path.is_file(), "{path:?}");
    assert_eq!(events.recv().await.unwrap(), crate::Event::ProjectSaved { name: "Coaster (v2)".into(), revision: 1 });
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v, json!({"name": "Coaster (v2)", "saved_revision": 1, "revision": 1, "dirty": false}));
    let saved_doc = st.document();

    let (_, list) = send(&app, req("GET", "/api/projects", None)).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list[0]["name"], "Coaster (v2)");
    assert!(list[0]["size"].as_u64().unwrap() > 100);
    assert!(list[0]["modified_unix_s"].as_u64().unwrap() > 1_600_000_000);

    // doc.new clears the project; the empty document is clean.
    let (_, v) = send(&app, exec("doc.new", json!({}))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v, json!({"name": null, "saved_revision": null, "revision": 2, "dirty": false}));

    let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "Coaster (v2)"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["created"], json!(["obj-2"]));
    assert_eq!(v["revision"], 3);
    let doc = st.document();
    assert_eq!(doc::Document { revision: 0, ..doc }, doc::Document { revision: 0, ..saved_doc });
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v, json!({"name": "Coaster (v2)", "saved_revision": 3, "revision": 3, "dirty": false}));
    // Opening is one undo step like any edit.
    assert_eq!(st.history().last().unwrap().command, "doc.load");

    // Save as another name, then saving over it: the UI's save replaces by default, MCP's does not.
    let (s, _) = send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Other"})))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Coaster (v2)"})))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) =
        send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Other", "overwrite": false})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, _) =
        send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Coaster (v2)", "overwrite": false})))).await;
    assert_eq!(s, StatusCode::OK, "saving the current project needs no overwrite");

    let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "Missing"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::NOT_FOUND, &json!(false)), "{v}");
    let (s, _) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "Other", "overwrite": true})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", "/api/projects/save", Some(json!({})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A corrupt project file is refused and leaves the document alone.
    std::fs::write(st.projects.root().join("Broken.openetch"), "{nope").unwrap();
    let rev = st.revision();
    let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "Broken"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(st.revision(), rev);
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn project_names_cannot_leave_the_directory() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 4.0, 4.0).await;
    let root = st.projects.root().to_path_buf();
    let base = root.parent().unwrap().to_path_buf();
    for name in ["../x", "a/b", "..", ".hidden", "C:\\x", "..\\x", "/etc/passwd", "x/../../y", "", "con"] {
        for route in ["/api/projects/save", "/api/projects/open"] {
            let (s, v) = send(&app, req("POST", route, Some(json!({"name": name})))).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{route} {name:?}: {v}");
            assert!(v["error"].as_str().unwrap().contains("invalid project name"), "{v}");
        }
    }
    assert!(!base.join("x.openetch").exists());
    assert!(!root.exists(), "nothing was written");
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn download_names_the_file() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 10.0, 10.0).await;
    let (s, h, body) = fetch(&app, "/api/document/download?format=openetch").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(h["content-type"], "application/json; charset=utf-8");
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"Untitled.openetch\"; filename*=UTF-8''Untitled.openetch"
    );
    assert_eq!(h["cache-control"], "no-store");
    assert_eq!(doc::file::from_openetch_json(&body).unwrap(), st.document());
    // The default format is openetch.
    assert_eq!(fetch(&app, "/api/document/download").await.2, body);

    // A non-ASCII document name: an ASCII fallback plus the UTF-8 name.
    let (_, v) = send(&app, exec("doc.rename", json!({"name": "Schild \"Müller\""}))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, h, _) = fetch(&app, "/api/document/download?format=openetch").await;
    assert_eq!(
        h["content-disposition"],
        "attachment; filename=\"Schild _M_ller_.openetch\"; filename*=UTF-8''Schild%20%22M%C3%BCller%22.openetch"
    );
    // The project name wins over the document name.
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Mug wrap"})))).await;
    let (s, h, body) = fetch(&app, "/api/document/download?format=lbrn2").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(h["content-type"], "application/xml; charset=utf-8");
    assert_eq!(h["content-disposition"], "attachment; filename=\"Mug wrap.lbrn2\"; filename*=UTF-8''Mug%20wrap.lbrn2");
    assert_eq!(h["x-openetch-warnings"], "0");
    assert!(body.starts_with("<?xml") && body.contains("<LightBurnProject"), "{body}");
    // The rectangle at the field centre lands at the centre of LightBurn's 150 mm workspace.
    assert!(body.contains("1 0 0 1 75 75"), "{body}");

    let (s, v) = send(&app, req("GET", "/api/document/download?format=svg", None)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::BAD_REQUEST, &json!(false)), "{v}");
    let (s, _) = send(&app, req("GET", "/api/document/download?format=openetch&x=1", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(st.history().iter().all(|h| h.command != "doc.export_lbrn2"), "downloads are not edits");
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn upload_replaces_and_merges() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app, 4.0, 4.0).await;
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Before"})))).await;

    let mut events = st.subscribe();
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&mode=replace", LBRN2)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    assert_eq!(v["result"]["created"].as_array().unwrap().len(), 2, "{v}");
    assert_eq!(v["result"]["mode"], "replace");
    assert_eq!(v["result"]["field_mm"], json!(150.0), "the document's (profile's) field");
    assert_eq!(v["result"]["bounds"]["min"], json!([-5.0, -5.0]));
    assert_eq!(
        events.recv().await.unwrap(),
        crate::Event::DocumentChanged { revision: 2, origin: commands::Origin::Ui }
    );
    assert_eq!(st.history().last().unwrap().origin, commands::Origin::Ui);
    // A replacing import is a new document no project holds (as the web treats it): the project name is cleared,
    // so Save cannot overwrite "Before" with it, and it is unsaved.
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((&p["name"], &p["dirty"]), (&json!(null), &json!(true)), "{p}");
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "After"})))).await;

    // Merge (with a BOM, as some writers add) keeps what is there; field_mm can be given.
    let with_bom = format!("\u{feff}{LBRN2}");
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&mode=merge&field_mm=110", with_bom)).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["result"]["reused_layers"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["result"]["field_mm"], json!(110.0));
    assert_eq!(st.document().objects.len(), 4);
    assert_eq!(st.document().field_mm, Some(150.0), "the document keeps its own field size");
    // A merge edits the current project's document: it stays the current project, now unsaved.
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((&p["name"], &p["dirty"]), (&json!("After"), &json!(true)), "{p}");
    // The same rule through /api/execute (mode defaults to replace).
    let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": LBRN2, "mode": "merge"}))).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.project_name().as_deref(), Some("After"));
    let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": LBRN2}))).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.project_name(), None);

    // An .openetch upload replaces the document and is not a saved project.
    let saved = doc::file::to_openetch_json(&doc::Document { name: "Upload".into(), ..doc::Document::default() });
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=openetch", saved)).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().name, "Upload");
    assert_eq!(st.document().field_mm, Some(150.0), "the file had none: the document's is kept");
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((&p["name"], &p["dirty"]), (&json!(null), &json!(false)), "{p}");

    // Refusals: all JSON, nothing changed.
    let rev = st.revision();
    for (uri, body, status) in [
        ("/api/document/upload", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=svg", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&mode=append", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=wide", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        // Rust parses these as floats; they must not fall back to the document's field size.
        ("/api/document/upload?format=lbrn2&field_mm=NaN", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=nan", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=inf", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=-infinity", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=1e999", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&name=x", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=openetch&mode=merge", b"{}".to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2", vec![0xff, 0xfe, 0x3c], StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2", Vec::new(), StatusCode::BAD_REQUEST),
    ] {
        let (s, v) = send(&app, raw("POST", uri, body)).await;
        assert_eq!((s, &v["ok"]), (status, &json!(false)), "{uri}: {v}");
    }
    // A file the command refuses answers like /api/execute: 200 {ok: false}.
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=openetch", LBRN2)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");
    assert!(v["error"].as_str().unwrap().contains("import"), "{v}");
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=-3", LBRN2)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");
    assert_eq!(st.revision(), rev);
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn project_names_differing_in_case_are_the_same_project() {
    let st = test_state().await;
    let app = app(&st);
    let save = |name: &str, overwrite: Option<bool>| {
        let mut body = json!({"name": name});
        if let Some(o) = overwrite {
            body["overwrite"] = json!(o);
        }
        req("POST", "/api/projects/save", Some(body))
    };
    rect(&app, 4.0, 4.0).await;
    let (s, v) = send(&app, save("ok name (1)", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    send(&app, exec("doc.new", json!({}))).await;
    rect(&app, 8.0, 8.0).await;

    // A differently-cased name is the same file on Windows and macOS: refused without overwrite.
    let (s, v) = send(&app, save("OK NAME (1)", Some(false))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let error = v["error"].as_str().unwrap();
    assert!(error.contains("ok name (1)") && error.contains("case"), "{v}");
    let (_, list) = send(&app, req("GET", "/api/projects", None)).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");

    // Open finds it by another spelling; the current project keeps the name on disk.
    let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "OK Name (1)"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["name"], "ok name (1)");
    assert_eq!(st.project_name().as_deref(), Some("ok name (1)"));
    // Re-casing the current project needs no overwrite; it is renamed in place, not duplicated.
    let (s, v) = send(&app, save("OK NAME (1)", Some(false))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, list) = send(&app, req("GET", "/api/projects", None)).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["OK NAME (1)"]);

    // From another project, the UI's save (overwrite by default) replaces it after its confirmation.
    send(&app, save("Other", None)).await;
    let (s, _) = send(&app, save("ok NAME (1)", Some(false))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = send(&app, save("ok NAME (1)", None)).await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = send(&app, req("GET", "/api/projects", None)).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ok NAME (1)", "Other"]);
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

/// Uploads and imports run on a blocking thread: while a large one converts, the async runtime (here a single
/// thread) keeps serving other tasks.
#[tokio::test(flavor = "current_thread")]
async fn uploads_do_not_block_the_runtime() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let st = test_state().await;
    let app = app(&st);
    let shapes: String = (0..6000)
        .map(|i| {
            let (x, y) = (10 + i % 100, 10 + (i / 100) % 100);
            format!(r#"<Shape Type="Rect" CutIndex="1" W="1" H="1" Cr="0"><XForm>1 0 0 1 {x} {y}</XForm></Shape>"#)
        })
        .collect();
    let big = LBRN2.replace("</LightBurnProject>", &format!("{shapes}</LightBurnProject>"));
    let (ticks, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
    let ticker = {
        let (ticks, done) = (ticks.clone(), done.clone());
        tokio::spawn(async move {
            while !done.load(Ordering::SeqCst) {
                ticks.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        })
    };
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=150", big.clone())).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let during_upload = ticks.swap(0, Ordering::SeqCst);
    let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": big, "mode": "merge"}))).await;
    assert_eq!(v["ok"], true, "{v}");
    let during_execute = ticks.load(Ordering::SeqCst);
    done.store(true, Ordering::SeqCst);
    ticker.await.unwrap();
    assert_eq!(st.document().objects.len(), 2 * 6002);
    // Run on the runtime thread, the ticker would only get the few polls between the request's own await points.
    assert!(during_upload > 100, "the runtime was blocked during the upload ({during_upload} ticks)");
    assert!(during_execute > 100, "the runtime was blocked during the import ({during_execute} ticks)");
}

#[tokio::test]
async fn uploads_above_50_mb_are_refused() {

    let st = test_state().await;
    let app = app(&st);
    let big = vec![b' '; api::MAX_UPLOAD_BYTES + 1];
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2", big)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::PAYLOAD_TOO_LARGE, &json!(false)), "{v}");
    assert!(v["error"].as_str().unwrap().contains("50 MB"), "{v}");
    // The 50 MB limit is the upload route's own: other routes keep theirs.
    const { assert!(api::MAX_UPLOAD_BYTES > api::MAX_BODY_BYTES) };
    let (s, _) = send(&app, raw("POST", "/api/execute", vec![b' '; api::MAX_BODY_BYTES + 1])).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(st.revision(), 0);
}

/// Real LightBurn projects (never in the repo): every `.lbrn2` in the directories listed in
/// `OPENETCH_LBRN_SAMPLES` (OS path-list separator) is uploaded, saved, reopened, downloaded as LightBurn and
/// uploaded again with the same object count.
#[tokio::test]
async fn real_lightburn_files_go_through_the_routes() {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("note: OPENETCH_LBRN_SAMPLES is not set; skipping the real-file route test");
        return;
    };
    let files: Vec<std::path::PathBuf> = std::env::split_paths(&dirs)
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|rd| rd.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lbrn2"))
        .collect();
    for path in files {
        let st = test_state().await;
        let app = app(&st);
        let bytes = std::fs::read(&path).unwrap();
        let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=150", bytes)).await;
        assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{}: {v}", path.display());
        let objects = st.document().objects.len();
        let (s, v) = send(&app, req("POST", "/api/projects/save", Some(json!({"name": "sample"})))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        send(&app, exec("doc.new", json!({}))).await;
        let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "sample"})))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(st.document().objects.len(), objects);
        let (s, _, xml) = fetch(&app, "/api/document/download?format=lbrn2").await;
        assert_eq!(s, StatusCode::OK);
        let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2", xml)).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(st.document().objects.len(), objects, "{}", path.display());
        println!("{}: {objects} object(s) through upload, save, open, download, upload", path.display());
        let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
    }
}
