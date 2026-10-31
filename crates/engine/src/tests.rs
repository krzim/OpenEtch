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
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, api};

/// The operator PIN the test states are set up with.
pub(crate) const TEST_PIN: &str = "2468";

/// Approval config in a fresh temp config dir with [`TEST_PIN`] set.
fn approval(confirm: Option<PhysicalConfirm>) -> ApprovalConfig {
    let dir = pin::temp_dir("engine");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    ApprovalConfig { pin: PinStore::new(&dir), confirm }
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
    AppState::new(Session::new(doc::Document::default()), machine, security, approval)
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
    assert_eq!(m["approval"], json!({"pin_set": false, "physical_confirm": {"input_bit": 14, "timeout_s": 30}}));

    let (s, v) = send(&app, approve(pin_body())).await;
    assert_eq!((s, &v["code"], &v["ok"]), (StatusCode::FORBIDDEN, &json!("pin_not_set"), &json!(false)), "{v}");
    pin::set_pin(&dir, TEST_PIN).unwrap();
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["approval"]["pin_set"], true);

    for body in [None, Some(json!({})), Some(json!({"pin": ""}))] {
        let (s, v) = send(&app, approve(body)).await;
        assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_required")), "{v}");
    }
    for _ in 1..pin::MAX_FAILURES {
        let (s, v) = send(&app, approve(Some(json!({"pin": "1357"})))).await;
        assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_invalid")), "{v}");
    }
    let (s, v) = send(&app, approve(Some(json!({"pin": "1357"})))).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_locked")), "{v}");
    let (s, v) = send(&app, approve(pin_body())).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("pin_locked")), "the right PIN is refused while locked");
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending);
    let c = st.machine.sim_counters().unwrap();
    assert_eq!((c.laser_singles(), c.emitting_packets()), (0, 0));
    let _ = std::fs::remove_dir_all(&dir);
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
    assert_eq!(run.state, RunState::Aborted, "{:?}", run.error);
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
        let run = wait_final(&st, &id).await;
        assert_eq!(run.state, RunState::Aborted, "{:?}", run.error);
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
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval(None));
    let (s, v) = send(&app(&st), req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["ok"], &v["connected"]), (&json!(true), &json!(false)));
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
