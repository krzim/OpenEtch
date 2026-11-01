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
        crate::Event::DocumentChanged { revision: 1, origin: commands::Origin::Ui, document_id: "default".into() }
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
    assert_eq!(hist["marker"], 1);
    assert_eq!(hist["steps"].as_array().unwrap().len(), 1);
    assert_eq!(hist["steps"][0]["command"], "shape.rect");
    assert_eq!(hist["steps"][0]["origin"]["kind"], "ui");
    assert_eq!(hist["steps"][0]["index"], 1);
    assert_eq!(hist["steps"][0]["state"], "active");
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
    assert_eq!(hist["steps"].as_array().unwrap().len(), 1);
    assert_eq!(hist["steps"][0]["origin"], json!({"kind": "agent", "name": "test"}));
    assert_eq!(hist["steps"][0]["invocations"].as_array().unwrap().len(), 2, "{hist}");
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

    // Sub-layers: every job pass is listed (segments index it) and every layer lists its main settings and sub-layers.
    let (_, r) = send(
        &app,
        exec("layer.sub_add", json!({"layer": "layer-1", "name": "Clean", "settings": {"power_pct": 5}, "cleanup": {"every_n_passes": 1}})),
    )
    .await;
    assert_eq!(r["result"]["index"], 0, "{r}");
    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::OK);
    let passes = v["passes"].as_array().unwrap();
    assert_eq!(passes.iter().map(|p| p["sub_layer"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 1]);
    assert!(v["segments"].as_array().unwrap().iter().all(|g| g["pass"].as_u64().is_some_and(|p| p < 2)));
    let subs = v["layers"][0]["sub_layers"].as_array().unwrap();
    assert_eq!((subs[0]["label"].as_str(), subs[1]["label"].as_str()), (Some("Main"), Some("Clean")));
    assert_eq!((subs[1]["power_pct"].as_f64(), subs[1]["planned_passes"].as_u64()), (Some(5.0), Some(1)));
    assert_eq!(subs[1]["cleanup"]["every_n_passes"], 1);
    send(&app, req("POST", "/api/undo", None)).await;

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
async fn run_summaries_name_the_layers() {
    let st = test_state().await;
    let app = app(&st);
    send(&app, exec("layer.update", json!({"id": "layer-1", "name": "Engrave logo"}))).await;
    rect(&app, 10.0, 10.0).await;
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let summary = v["run"]["summary"].as_str().unwrap();
    assert!(summary.contains("on \"Engrave logo\","), "{summary}");
    assert!(!summary.contains("layer-1"), "{summary}");
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
        json!({"pin_set": false, "pin_changed": false, "locked_until_unix_s": null, "sim_pin": false,
               "physical_confirm": {"input_bit": 14, "timeout_s": 30}, "mode": "pin", "foot_switch": null})
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
        crate::Event::DocumentChanged { revision: 2, origin: commands::Origin::Agent("cli".into()), document_id: "default".into() }
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
    assert_eq!((s, v), (StatusCode::OK, json!({"name": null, "display_name": null, "saved_revision": null, "revision": 0, "dirty": false})));
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
    assert_eq!(events.recv().await.unwrap(), crate::Event::ProjectSaved { name: "Coaster (v2)".into(), revision: 1, document_id: "default".into() });
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v, json!({"name": "Coaster (v2)", "display_name": null, "saved_revision": 1, "revision": 1, "dirty": false}));
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
    assert_eq!(v, json!({"name": null, "display_name": null, "saved_revision": null, "revision": 2, "dirty": false}));

    let (s, v) = send(&app, req("POST", "/api/projects/open", Some(json!({"name": "Coaster (v2)"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["created"], json!(["obj-2"]));
    assert_eq!(v["revision"], 3);
    let doc = st.document();
    assert_eq!(doc::Document { revision: 0, ..doc }, doc::Document { revision: 0, ..saved_doc });
    let (_, v) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(v, json!({"name": "Coaster (v2)", "display_name": null, "saved_revision": 3, "revision": 3, "dirty": false}));
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
    for name in ["../x", "a//b", "templates/x", "1/2/3/4/5/x", "..", ".hidden", "C:\\x", "..\\x", "/etc/passwd", "x/../../y", "", "con"] {
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
        crate::Event::DocumentChanged { revision: 2, origin: commands::Origin::Ui, document_id: "default".into() }
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

    // An .openetch upload replaces the document and is not a saved project: it is unsaved (the engine cannot
    // save back to the file it came from), so New and Open ask first.
    let saved = doc::file::to_openetch_json(&doc::Document { name: "Upload".into(), ..doc::Document::default() });
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=openetch", saved)).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().name, "Upload");
    assert_eq!(st.document().field_mm, Some(150.0), "the file had none: the document's is kept");
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((&p["name"], &p["dirty"]), (&json!(null), &json!(true)), "{p}");

    // Refusals: all JSON, nothing changed.
    let rev = st.revision();
    for (uri, body, status) in [
        ("/api/document/upload", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=pdf", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=svg&mode=replace", SVG.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=svg&field_mm=150", SVG.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=svg&mime=image/png", SVG.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=svg&width_mm=inf", SVG.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&layer=layer-1", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=openetch&width_mm=10", b"{}".to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=image", SVG.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=dxf", vec![0xff, 0xfe, 0x3c], StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&mode=append", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=wide", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        // Rust parses these as floats; they must not fall back to the document's field size.
        ("/api/document/upload?format=lbrn2&field_mm=NaN", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=nan", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=inf", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=-infinity", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
        ("/api/document/upload?format=lbrn2&field_mm=1e999", LBRN2.as_bytes().to_vec(), StatusCode::BAD_REQUEST),
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
/// `OPENETCH_LBRN_SAMPLES` (OS path-list separator) is uploaded at a field size other than the profile's, saved,
/// reopened, downloaded as LightBurn (byte for byte the uploaded file: the download uses the import's field size)
/// and uploaded again with the same object count.
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
        let source = String::from_utf8_lossy(&bytes).trim_start_matches('\u{feff}').to_owned();
        let (s, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=120", bytes)).await;
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
        // Regression: imported at another field size than the document's, the export moved every shape.
        assert!(xml == source, "{} does not come back byte for byte", path.display());
        let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2", xml)).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(st.document().objects.len(), objects, "{}", path.display());
        println!("{}: {objects} object(s) through upload, save, open, download, upload", path.display());
        let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
    }
}

const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#;

/// A 1x1 PNG.
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/// A DXF with one 10 mm line ($INSUNITS 4 = mm).
const DXF: &str = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n30\n0.0\n11\n10.0\n21\n0.0\n31\n0.0\n0\nENDSEC\n0\nEOF\n";

#[tokio::test]
async fn svg_uploads_add_art_on_the_given_layer() {
    let st = test_state().await;
    let app = app(&st);
    let (_, v) = send(&app, exec("layer.add", json!({"name": "Engrave"}))).await;
    let layer = v["result"]["id"].as_str().unwrap().to_owned();
    let uri = format!("/api/document/upload?format=svg&layer={layer}&name=Logo&width_mm=20");
    let (s, v) = send(&app, raw("POST", &uri, format!("\u{feff}{SVG}"))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let id = v["result"]["created"][0].as_str().unwrap().to_owned();
    let o = st.document().object(&id).cloned().unwrap();
    assert_eq!((o.layer.as_str(), o.name.as_str()), (layer.as_str(), "Logo"));
    assert_eq!(v["result"]["bounds"]["size"], json!([20.0, 20.0]));
    // mode=merge is accepted (it is what SVG uploads always do); a broken SVG is refused like a command.
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=svg&mode=merge", SVG)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let rev = st.revision();
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=svg", LBRN2)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");
    assert_eq!(st.revision(), rev);
    assert_eq!(st.history().last().unwrap().command, "art.import_svg");
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn image_and_dxf_uploads_go_to_their_import_commands() {
    use base64::Engine as _;
    let st = test_state().await;
    let app = app(&st);
    let png = base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap();
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=image&width_mm=12&name=Photo", png.clone())).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let id = v["result"]["created"][0].as_str().unwrap().to_owned();
    let o = st.document().object(&id).cloned().unwrap();
    assert_eq!(o.name, "Photo");
    match &o.geometry {
        doc::Geometry::Image { width_mm, mime, data_base64, .. } => {
            assert_eq!((*width_mm, mime.as_str()), (12.0, "image/png"));
            assert_eq!(&**data_base64, PNG_1X1);
        }
        other => panic!("expected an image, got {other:?}"),
    }
    // GET /api/history does not repeat the file.
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert!(h.to_string().len() < 4000, "{h}");

    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=dxf&name=Jig", DXF)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    assert_eq!(v["result"]["bounds"]["size"][0], json!(10.0), "{v}");
    assert_eq!(st.history().last().unwrap().command, "art.import_dxf");
    // Several entities arrive as one object each, grouped (LightBurn-style), by upload and by execute alike.
    let two = DXF.replace("0\nENDSEC\n0\nEOF", "0\nCIRCLE\n8\n0\n10\n20\n20\n0\n30\n0\n40\n5\n0\nENDSEC\n0\nEOF");
    let (s, v) = send(&app, raw("POST", "/api/document/upload?format=dxf&name=Jig", two.clone())).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let group = v["result"]["group"].as_str().unwrap().to_owned();
    assert_eq!(st.document().group_members(&group).len(), 2, "{v}");
    assert_eq!(v["result"]["created"].as_array().unwrap().len(), 2, "{v}");
    let (_, v) = send(&app, exec("art.import_dxf", json!({"content": two}))).await;
    assert!(v["result"]["group"].is_string(), "{v}");
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn history_routes_roll_back_insert_and_delete() {
    let st = test_state().await;
    let app = app(&st);
    let id = rect(&app, 10.0, 10.0).await;
    let mv = || exec("object.transform", json!({"ids": [id.clone()], "translate": [5, 0]}));
    send(&app, mv()).await;
    send(&app, mv()).await;
    let x0 = |st: &AppState| st.document().object(&id).map(|o| (o.bounds().x0 * 1e6).round() / 1e6);
    assert_eq!(x0(&st), Some(5.0));

    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/history/goto", Some(json!({"marker": 1})))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"ok": true, "marker": 1, "revision": 4}));
    assert_eq!(events.recv().await.unwrap(), crate::Event::DocumentChanged { revision: 4, origin: commands::Origin::Ui, document_id: "default".into() });
    assert_eq!(events.recv().await.unwrap(), crate::Event::HistoryChanged { marker: 1, steps: 3, revision: 4, document_id: "default".into() });
    assert_eq!(x0(&st), Some(-5.0));
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    let states: Vec<&str> = h["steps"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["active", "rolled_back", "rolled_back"]);

    // An edit while rolled back is inserted after the marker.
    let (_, v) = send(&app, exec("object.rename", json!({"id": id, "name": "Plate"}))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(h["marker"], 2);
    assert_eq!(h["steps"][1]["command"], "object.rename");
    assert_eq!(h["steps"][3]["state"], "rolled_back");

    // Delete the first move (step 3), then roll to the end: one move left.
    let (_, v) = send(&app, req("POST", "/api/history/delete", Some(json!({"index": 3})))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, v) = send(&app, req("POST", "/api/history/goto", Some(json!({"marker": 3})))).await;
    assert_eq!(v["marker"], 3);
    assert_eq!(x0(&st), Some(0.0));
    assert_eq!(st.document().object(&id).unwrap().name, "Plate");

    // Deleting the rectangle leaves the later steps failed (red), kept on the timeline.
    send(&app, req("POST", "/api/history/delete", Some(json!({"index": 1})))).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    let states: Vec<&str> = h["steps"].as_array().unwrap().iter().map(|s| s["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["failed", "failed"]);
    assert!(h["steps"][0]["error"].as_str().unwrap().contains(&id), "{h}");

    // Refusals.
    let (s, v) = send(&app, req("POST", "/api/history/goto", Some(json!({"marker": 9})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");
    let (s, v) = send(&app, req("POST", "/api/history/delete", Some(json!({"index": 0})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "{v}");
    let (s, _) = send(&app, req("POST", "/api/history/goto", Some(json!({"marker": -1})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", "/api/history/goto", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

/// Regression: an edit after Ctrl+Z was inserted before the undone steps, so redo or "go to end" brought them back;
/// File > New kept the old document's steps.
#[tokio::test]
async fn edit_after_undo_discards_and_new_document_starts_a_fresh_timeline() {
    let st = test_state().await;
    let app = app(&st);
    let id = rect(&app, 10.0, 10.0).await;
    let mv = || exec("object.transform", json!({"ids": [id.clone()], "translate": [5, 0]}));
    send(&app, mv()).await;
    send(&app, mv()).await;
    send(&app, req("POST", "/api/undo", None)).await;
    send(&app, req("POST", "/api/undo", None)).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!((&h["marker"], &h["marker_moved_by"], &h["undone_steps"]), (&json!(1), &json!("undo"), &json!(2)));
    send(&app, exec("object.rename", json!({"id": id, "name": "Plate"}))).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    let commands: Vec<&str> = h["steps"].as_array().unwrap().iter().map(|s| s["command"].as_str().unwrap()).collect();
    assert_eq!(commands, ["shape.rect", "object.rename"]);
    assert_eq!((&h["marker"], &h["marker_moved_by"]), (&json!(2), &json!("edit")));
    let (_, v) = send(&app, req("POST", "/api/redo", None)).await;
    assert_eq!(v["ok"], false, "nothing to redo: {v}");
    assert_eq!(st.document().object(&id).unwrap().bounds().x0, -5.0);

    // A timeline move keeps insert-and-replay.
    send(&app, mv()).await;
    send(&app, req("POST", "/api/history/goto", Some(json!({"marker": 1})))).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(h["marker_moved_by"], "timeline");
    send(&app, exec("layer.add", json!({"name": "Cut"}))).await;
    assert_eq!(st.history().len(), 4);

    // File > New: a fresh timeline whose start is the replaced document.
    send(&app, exec("doc.new", json!({}))).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!((h["steps"].as_array().unwrap().len(), &h["marker"]), (1, &json!(1)), "{h}");
    assert_eq!(h["steps"][0]["command"], "doc.new");
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

#[tokio::test]
async fn preview_has_layers_and_filters_and_fill_regions_are_served() {
    let st = test_state().await;
    let app = app(&st);
    let (_, v) = send(&app, exec("layer.add", json!({"name": "Engrave", "settings": {"mode": "fill"}}))).await;
    let fill = v["result"]["id"].as_str().unwrap().to_owned();
    rect(&app, 10.0, 10.0).await;
    send(&app, exec("shape.ellipse", json!({"rx_mm": 5, "ry_mm": 5, "center": [20, 0], "layer": fill}))).await;

    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let layers = v["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[1]["layer_id"], json!(fill));
    assert_eq!(layers[1]["mode"], "fill");
    assert!(layers[1]["segments"].as_u64().unwrap() > 4);
    assert!(layers[0]["estimated_time_s"].as_f64().unwrap() > 0.0);
    assert!(v["segments"].as_array().unwrap().iter().all(|s| s["pass"].is_u64()));

    let (s, f) = send(&app, req("GET", &format!("/api/preview?layers={fill}"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(f["segments"].as_array().unwrap().iter().all(|s| s["layer"] == json!(fill)));
    assert_eq!(f["layers"].as_array().unwrap().len(), 1);
    assert_eq!(f["stats"], v["stats"]);
    let (s, _) = send(&app, req("GET", "/api/preview?colour=red", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, r) = send(&app, req("GET", "/api/fill_regions", None)).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let layers = r["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 1, "only fill layers: {r}");
    assert_eq!(layers[0]["layer_id"], json!(fill));
    assert_eq!(layers[0]["object_ids"], json!(["obj-4"]));
    assert!(layers[0]["paths"][0].as_str().unwrap().starts_with('M'));
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

/// The synthetic LightBurn prefs (devices "JCZFiber" 153.8 mm, the default, and "Bench110" 110 x 100 mm) plus
/// unrelated secrets that must never come out.
const LB_PREFS: &str = include_str!("../../machine/tests/fixtures/lightburn-prefs.json");

/// `LBRN2` as LightBurn 2.x writes it, naming its device.
fn lbrn2_for(device: &str) -> String {
    LBRN2.replacen("<LightBurnProject ", &format!("<LightBurnProject DeviceName=\"{device}\" "), 1)
}

/// A state whose LightBurn prefs.ini is the synthetic fixture (in a temp dir, with a temp config dir).
async fn lightburn_state() -> (Arc<AppState>, std::path::PathBuf) {
    let st = test_state().await;
    let dir = pin::temp_dir("lightburn-routes");
    std::fs::create_dir_all(&dir).unwrap();
    let prefs = dir.join("prefs.ini");
    std::fs::write(&prefs, LB_PREFS).unwrap();
    st.lightburn.configure(crate::lightburn::LightBurnConfig {
        prefs_paths: vec![prefs],
        device: None,
        store_dir: Some(dir.join("config")),
    });
    (st, dir)
}

#[tokio::test]
async fn lightburn_routes_list_upload_and_delete_devices() {
    // Nothing known: available false.
    let st = test_state().await;
    let (s, v) = send(&app(&st), req("GET", "/api/lightburn", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"available": false, "source": null, "default_device": null, "devices": []}));
    let (_, m) = send(&app(&st), req("GET", "/api/machine", None)).await;
    assert_eq!(m["lightburn"], json!(null));

    let (st, dir) = lightburn_state().await;
    let app = app(&st);
    let (s, v) = send(&app, req("GET", "/api/lightburn", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["available"], &v["source"], &v["default_device"]), (&json!(true), &json!("file"), &json!("JCZFiber")));
    assert_eq!(
        v["devices"],
        json!([
            {"name": "JCZFiber", "display_name": "JCZFiber (Unknown)", "field_mm": 153.8, "width_mm": 153.8, "height_mm": 153.8},
            {"name": "Bench110", "display_name": "Bench 110 lens", "field_mm": 110.0, "width_mm": 110.0, "height_mm": 100.0},
        ])
    );
    let text = v.to_string();
    assert!(!text.contains("secret") && !text.contains("cor"), "{text}");

    // GET /api/machine compares the active (generic) profile with the matched device.
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["lightburn"]["device"], &m["lightburn"]["field_mm"]), (&json!("JCZFiber"), &json!(153.8)), "{m}");
    let diffs = m["lightburn"]["diffs"].as_array().unwrap();
    let field = diffs.iter().find(|d| d["field"] == "field_mm").unwrap();
    assert_eq!((&field["base_value"], &field["lightburn_value"]), (&json!(150.0), &json!(153.8)));
    assert!(diffs.iter().all(|d| d.get("note").is_some()));

    // Upload (a Pi without LightBurn): only the parsed devices are kept, and they win over the file.
    let mut events = st.subscribe();
    let pi = r#"{"RestPendantSecret": "pi-secret", "DefaultDevice": 0,
        "DeviceList": [{"Name": "PiFiber", "DisplayName": "PiFiber (USB)", "Width": 110, "Height": 110}]}"#;
    let (s, v) = send(&app, raw("POST", "/api/lightburn/prefs", pi)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["source"], &v["default_device"]), (&json!("uploaded"), &json!("PiFiber")));
    assert_eq!(events.recv().await.unwrap(), crate::Event::machine("lightburn", None));
    let stored = std::fs::read_to_string(dir.join("config").join(crate::lightburn::STORE_FILE)).unwrap();
    assert!(stored.contains("PiFiber") && !stored.contains("pi-secret"), "{stored}");
    // Refusals change nothing.
    let too_big = vec![b' '; crate::lightburn::MAX_PREFS_UPLOAD_BYTES + 1];
    for (body, status) in [
        (too_big, StatusCode::PAYLOAD_TOO_LARGE),
        (Vec::new(), StatusCode::BAD_REQUEST),
        (b"{\"DeviceList\": []}".to_vec(), StatusCode::BAD_REQUEST),
        (b"<xml/>".to_vec(), StatusCode::BAD_REQUEST),
    ] {
        let (s, v) = send(&app, raw("POST", "/api/lightburn/prefs", body)).await;
        assert_eq!((s, &v["ok"]), (status, &json!(false)), "{v}");
    }
    assert_eq!(send(&app, req("GET", "/api/lightburn", None)).await.1["default_device"], json!("PiFiber"));
    // Delete: back to the local file.
    let (s, v) = send(&app, req("DELETE", "/api/lightburn/prefs", None)).await;
    assert_eq!((s, &v["source"]), (StatusCode::OK, &json!("file")), "{v}");
    assert_eq!(std::fs::read_to_string(dir.join("prefs.ini")).unwrap(), LB_PREFS, "prefs.ini is never written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn lightburn_imports_take_the_device_field() {
    let (st, dir) = lightburn_state().await;
    let app = app(&st);
    // The file names its device: that device's field.
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2", lbrn2_for("Bench 110 lens"))).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!((&v["result"]["field_mm"], &v["result"]["field_source"]), (&json!(110.0), &json!("file device Bench110")));
    assert_eq!(st.document().field_mm, Some(110.0));
    // The recorded step carries the field, so replays do not depend on prefs.ini.
    assert_eq!(st.history().last().unwrap().invocations[0].params["field_mm"], json!(110.0));
    // A device nobody knows, or none (LightBurn 1.x): the active device.
    for content in [lbrn2_for("Someone else (Unknown)"), LBRN2.to_owned()] {
        let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": content}))).await;
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(
            (&v["result"]["field_mm"], &v["result"]["field_source"], &v["result"]["device"]),
            (&json!(150.0), &json!("active device Generic LMC fiber (EzCad)"), &json!("generic-lmc-fiber-ezcad"))
        );
    }
    // The caller's field wins.
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=120", lbrn2_for("JCZFiber"))).await;
    assert_eq!((&v["result"]["field_mm"], &v["result"]["field_source"]), (&json!(120.0), &json!("caller")));
    // A merge too (the document keeps its own field; the export default is the remembered one).
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&mode=merge", lbrn2_for("JCZFiber (Unknown)"))).await;
    assert_eq!((&v["result"]["field_mm"], &v["result"]["field_source"]), (&json!(153.8), &json!("file device JCZFiber")));
    assert_eq!(st.document().field_mm, Some(120.0));
    let (_, v) = send(&app, exec("doc.export_lbrn2", json!({}))).await;
    assert_eq!(v["result"]["field_mm"], json!(153.8));

    // Without LightBurn devices: the active device's field.
    st.lightburn.configure(crate::lightburn::LightBurnConfig::default());
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2", lbrn2_for("JCZFiber"))).await;
    assert_eq!((&v["result"]["field_mm"], &v["result"]["field_source"]), (&json!(150.0), &json!("active device Generic LMC fiber (EzCad)")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn new_starts_at_the_machine_field() {
    let st = test_state().await;
    let app = app(&st);
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&field_mm=110", LBRN2)).await;
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().field_mm, Some(110.0));
    let (_, v) = send(&app, exec("doc.new", json!({}))).await;
    assert_eq!((&v["ok"], &v["result"]["field_mm"]), (&json!(true), &json!(150.0)), "{v}");
    assert_eq!(st.document().field_mm, Some(st.machine.profile().field_mm));
    // An explicit field size still wins.
    send(&app, exec("doc.new", json!({"field_mm": 120}))).await;
    assert_eq!(st.document().field_mm, Some(120.0));
}

#[tokio::test]
async fn downloads_after_open_use_the_opened_file_name() {
    let st = test_state().await;
    let app = app(&st);
    let disposition = |stem: &str, ext: &str| format!("attachment; filename=\"{stem}.{ext}\"; filename*=UTF-8''{stem}.{ext}");
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&name=Coaster.lbrn2", LBRN2)).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((&p["name"], &p["display_name"]), (&json!(null), &json!("Coaster")), "{p}");
    for (format, ext) in [("lbrn2", "lbrn2"), ("openetch", "openetch")] {
        let (_, h, _) = fetch(&app, &format!("/api/document/download?format={format}")).await;
        assert_eq!(h["content-disposition"], disposition("Coaster", ext));
    }
    // Edits keep it; a merge keeps it (and names its group after the file).
    rect(&app, 2.0, 2.0).await;
    let (_, v) = send(&app, raw("POST", "/api/document/upload?format=lbrn2&mode=merge&name=Badge", LBRN2)).await;
    assert_eq!(v["ok"], true, "{v}");
    let group = v["result"]["created_groups"][0].as_str().unwrap().to_owned();
    assert_eq!(st.document().groups.iter().find(|g| g.id == group).unwrap().name, "Badge");
    assert_eq!(fetch(&app, "/api/document/download?format=lbrn2").await.1["content-disposition"], disposition("Coaster", "lbrn2"));
    // An .openetch upload with a name; through execute, a replacing import's name.
    let saved = doc::file::to_openetch_json(&doc::Document { name: "Inside".into(), ..doc::Document::default() });
    send(&app, raw("POST", "/api/document/upload?format=openetch&name=Tag.openetch", saved)).await;
    assert_eq!(fetch(&app, "/api/document/download").await.1["content-disposition"], disposition("Tag", "openetch"));
    send(&app, exec("doc.import_lbrn2", json!({"content": LBRN2, "name": "Plate"}))).await;
    assert_eq!(fetch(&app, "/api/document/download").await.1["content-disposition"], disposition("Plate", "openetch"));
    // A saved project's name wins; New forgets the file name.
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Saved"})))).await;
    assert_eq!(fetch(&app, "/api/document/download").await.1["content-disposition"], disposition("Saved", "openetch"));
    send(&app, exec("doc.new", json!({}))).await;
    assert_eq!(fetch(&app, "/api/document/download").await.1["content-disposition"], disposition("Untitled", "openetch"));
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!(p["display_name"], json!(null));
    // A replacing upload without a name falls back to the document name.
    send(&app, raw("POST", "/api/document/upload?format=lbrn2&name=Coaster", LBRN2)).await;
    send(&app, raw("POST", "/api/document/upload?format=lbrn2", LBRN2)).await;
    assert_eq!(fetch(&app, "/api/document/download").await.1["content-disposition"], disposition("Untitled", "openetch"));
    let _ = std::fs::remove_dir_all(st.projects.root().parent().unwrap());
}

/// The synthetic gantry `.lbdev` ("Test Bed 400": Custom GCode over TCP, 400 x 300 mm, rear-left origin).
const LB_GANTRY: &str = include_str!("../../machine/tests/fixtures/gantry.lbdev");

/// A state whose device registry lives in a temp config dir (with the B6 fixture as the built-in device, so a
/// B6-lens LightBurn device finds jcz150.cor next to it) and whose LightBurn prefs.ini is the synthetic fixture.
async fn devices_state() -> (Arc<AppState>, std::path::PathBuf) {
    let (st, dir) = lightburn_state().await;
    let profile = crate::cli::resolve_default(crate::cli::DEFAULT_PROFILE);
    let (b6, cor) = crate::devices::load_profile(&profile, false).unwrap();
    let (registry, notes) =
        crate::devices::DeviceRegistry::open(dir.join("config").join("devices"), b6, cor, Some(&profile), || None).unwrap();
    assert!(notes[0].contains("no LightBurn devices"), "{notes:?}");
    st.install_devices(registry);
    (st, dir)
}

#[tokio::test]
async fn devices_routes_import_list_show_and_remove() {
    let (st, dir) = devices_state().await;
    let app = app(&st);
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("GET", "/api/devices", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((&v["active"], v["devices"].as_array().unwrap().len()), (&json!("commarker-b6-mopa-60w-ezcad"), 1));
    // A .lbdev upload; then LightBurn on this computer (the fixture prefs.ini).
    let (s, v) = send(&app, raw("POST", "/api/devices/import", LB_GANTRY)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v, json!({"ok": true, "added": ["test-bed-400"], "updated": [], "skipped": []}));
    assert_eq!(events.recv().await.unwrap(), crate::Event::DevicesChanged);
    let (s, v) = send(&app, raw("POST", "/api/devices/import?from=lightburn&add=all", "")).await;
    assert_eq!((s, &v["added"]), (StatusCode::OK, &json!(["jczfiber-unknown", "bench-110-lens"])), "{v}");
    let (_, v) = send(&app, raw("POST", "/api/devices/import?from=lightburn&add=JCZFiber", "")).await;
    assert_eq!(v["skipped"][0]["reason"], json!("already imported, unchanged"), "{v}");
    let (_, v) = send(&app, req("GET", "/api/devices", None)).await;
    let ids: Vec<&str> = v["devices"].as_array().unwrap().iter().map(|d| d["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["commarker-b6-mopa-60w-ezcad", "test-bed-400", "jczfiber-unknown", "bench-110-lens"]);
    let bed = &v["devices"][1];
    assert_eq!(
        (&bed["kind"], &bed["origin"], &bed["workspace_mm"], &bed["controllable"], &bed["source"], &bed["active"]),
        (&json!("gantry"), &json!("top_left"), &json!([400.0, 300.0]), &json!(false), &json!("lbdev"), &json!(false))
    );
    let text = v.to_string();
    for leaked in ["secret", "someone", "10.0.0.5", "M1403", "guid"] {
        assert!(!text.contains(leaked), "{leaked} in {text}");
    }
    // Details: settings read-only, the profile and its diffs against the base.
    let (s, d) = send(&app, req("GET", "/api/devices/jczfiber-unknown", None)).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!((&d["base"], &d["profile"]["field_mm"]), (&json!("ComMarker B6 MOPA 60W"), &json!(153.75)));
    assert!(d["diffs"].as_array().unwrap().iter().any(|x| x["field"] == "mark_speed.max_mm_s"), "{d}");
    let (_, d) = send(&app, req("GET", "/api/devices/test-bed-400", None)).await;
    assert!(d["reason"].as_str().unwrap().contains("design-only"), "{d}");
    assert_eq!(send(&app, req("GET", "/api/devices/nope", None)).await.0, StatusCode::NOT_FOUND);
    // Refusals.
    for (uri, body, status) in [
        ("/api/devices/import", "", StatusCode::BAD_REQUEST),
        ("/api/devices/import", "{\"DeviceList\": []}", StatusCode::BAD_REQUEST),
        ("/api/devices/import", "not json secret-1", StatusCode::BAD_REQUEST),
        ("/api/devices/import?add=Nobody", LB_GANTRY, StatusCode::NOT_FOUND),
        ("/api/devices/import?from=elsewhere", "", StatusCode::BAD_REQUEST),
        ("/api/devices/import?bogus=1", LB_GANTRY, StatusCode::BAD_REQUEST),
    ] {
        let (s, v) = send(&app, raw("POST", uri, body)).await;
        assert_eq!(s, status, "{uri}: {v}");
        assert!(!v.to_string().contains("secret-1"), "{v}");
    }
    // Remove: never the built-in or the active device.
    assert_eq!(send(&app, req("DELETE", "/api/devices/commarker-b6-mopa-60w-ezcad", None)).await.0, StatusCode::CONFLICT);
    let (s, v) = send(&app, req("DELETE", "/api/devices/bench-110-lens", None)).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    assert_eq!(send(&app, req("GET", "/api/devices", None)).await.1["devices"].as_array().unwrap().len(), 3);
    assert_eq!(std::fs::read_to_string(dir.join("prefs.ini")).unwrap(), LB_PREFS, "prefs.ini is never written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn activating_a_device_swaps_the_machine_and_is_refused_while_busy() {
    let (st, dir) = devices_state().await;
    let app = app(&st);
    send(&app, raw("POST", "/api/devices/import", LB_GANTRY)).await;
    send(&app, raw("POST", "/api/devices/import?from=lightburn", "")).await;
    let pending = pending_run(&app).await;
    let mut events = st.subscribe();

    // The B6 lens from LightBurn: the profile is swapped and the board initialised again; pending runs are cancelled.
    let (s, v) = send(&app, req("POST", "/api/devices/jczfiber-unknown/activate", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["active"]["id"], &v["warnings"]), (&json!("jczfiber-unknown"), &json!([])));
    assert_eq!(events.recv().await.unwrap(), crate::Event::DeviceChanged { id: "jczfiber-unknown".into() });
    let profile = st.machine.profile();
    assert_eq!((profile.name.as_str(), profile.field_mm, profile.mark_speed.max_mm_s), ("JCZFiber (Unknown)", 153.75, 7000.0));
    assert_eq!(st.machine.generation(), 1);
    assert!(st.machine.config().cor.is_some(), "jcz150.cor found next to the built-in profile");
    assert_eq!(st.runs().get(&pending).unwrap().state, RunState::Cancelled);
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["device"]["id"], &m["connected"], &m["field_mm"]), (&json!("jczfiber-unknown"), &json!(true), &json!(153.75)));
    assert_eq!(m["sim_counters"]["laser_singles"], json!(0), "re-initialising fires nothing");
    // New documents take its field; a run can be requested and approved.
    send(&app, exec("doc.new", json!({}))).await;
    assert_eq!(st.document().field_mm, Some(153.75));
    let id = pending_run(&app).await;
    // A run planned for the previous configuration is never approved.
    let generic = MachineProfile::generic_jcz_galvo();
    st.machine.blocking(move |m, ctl| m.swap_config(ctl, generic, None)).await.unwrap().unwrap();
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), pin_body())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("another device"), "{v}");

    // Busy: an approved run, or framing, refuses the change (409) and changes nothing.
    let id = pending_run(&app).await;
    st.runs().set_state(&id, RunState::Approved);
    let (s, v) = send(&app, req("POST", "/api/devices/test-bed-400/activate", None)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    st.runs().finish(&id, RunState::Cancelled, None, None);
    st.machine.set_framing_gen(st.machine.frame_gen());
    let (s, v) = send(&app, req("POST", "/api/devices/test-bed-400/activate", None)).await;
    assert_eq!((s, v["error"].as_str().unwrap().contains("framing")), (StatusCode::CONFLICT, true), "{v}");
    st.machine.set_framing_gen(0);
    assert_eq!(st.active_device().id, "jczfiber-unknown");
    // Only the UI may change the device.
    let mut r = req("POST", "/api/devices/test-bed-400/activate", None);
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, req("POST", "/api/devices/test-bed-400/activate", Some(json!({"origin": "agent:x"})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/devices/nope/activate", None)).await.0, StatusCode::NOT_FOUND);

    // A design-only gantry: its bed drives new documents and LightBurn files; planning, runs and framing are refused.
    let generation = st.machine.generation();
    let (s, v) = send(&app, req("POST", "/api/devices/test-bed-400/activate", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(st.machine.generation(), generation, "the machine keeps its profile");
    assert!(st.machine.profile().name.contains("Generic"));
    send(&app, exec("doc.new", json!({}))).await;
    let d = st.document();
    assert_eq!((d.workspace_mm, d.field_mm), (Some([400.0, 300.0]), Some(300.0)));
    rect(&app, 10.0, 10.0).await;
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("can't drive Custom GCode"), "{v}");
    let (s, _) = send(&app, req("POST", "/api/machine/frame", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = send(&app, req("GET", "/api/preview", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(fetch(&app, "/api/preview.svg").await.0, StatusCode::OK, "the SVG still shows the document");
    let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": LBRN2}))).await;
    assert_eq!(
        (&v["result"]["field_source"], &v["result"]["workspace_mm"], &v["result"]["origin"]),
        (&json!("active device Test Bed 400"), &json!([400.0, 300.0]), &json!("top_left")),
        "{v}"
    );
    let (_, v) = send(&app, exec("doc.export_lbrn2", json!({}))).await;
    assert!(v["result"]["content"].as_str().unwrap().contains("MirrorY=\"True\""), "{v}");
    // A file naming a registry device is laid out for it (by display name), whatever is active.
    let (_, v) = send(&app, exec("doc.import_lbrn2", json!({"content": lbrn2_for("JCZFiber (Unknown)")}))).await;
    assert_eq!(
        (&v["result"]["field_mm"], &v["result"]["field_source"], &v["result"]["device"]),
        (&json!(153.8), &json!("file device JCZFiber (Unknown)"), &json!("jczfiber-unknown")),
        "LightBurn's width, as the file was laid out"
    );
    // The choice survives a restart of the registry.
    let (again, _) = crate::devices::DeviceRegistry::open(
        dir.join("config").join("devices"),
        MachineProfile::generic_lmc_fiber(),
        None,
        None,
        || None,
    )
    .unwrap();
    assert_eq!(again.active().id, "test-bed-400");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs are planned with variable text as of the request (`commands::prepare_output`), a text that cannot be
/// rendered refuses the run, and `advance_variables_after_run` advances the counter only with auto-advance on.
#[tokio::test]
async fn runs_render_variable_text_and_auto_advance() {
    let st = test_state().await;
    let app = app(&st);
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    let text = json!({ "text": "0ddd", "font": font, "height_mm": 5, "variable": { "mode": "serial" } });
    let (s, v) = send(&app, exec("shape.text", text)).await;
    assert_eq!((s, v["ok"].clone()), (StatusCode::OK, json!(true)), "{v}");
    let id = v["result"]["created"][0].as_str().unwrap().to_string();
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // Auto-advance off: nothing changes after a run.
    let rev = st.revision();
    st.advance_variables_after_run();
    assert_eq!(st.revision(), rev);
    let (_, v) = send(&app, exec("var.set_serial", json!({ "auto_advance": true, "current": 41 }))).await;
    assert_eq!(v["ok"], true, "{v}");
    st.advance_variables_after_run();
    assert_eq!(st.document().variables.current, 42);
    let doc = st.document();
    let doc::Geometry::Text(t) = &doc.object(&id).unwrap().geometry else { panic!() };
    assert_eq!(t.shown.as_deref(), Some("042"));

    // A variable text whose font is gone refuses the run instead of burning a stale number.
    let mut broken = st.document();
    if let doc::Geometry::Text(t) = &mut broken.object_mut(&id).unwrap().geometry {
        t.font.family = "Missing Font".into();
    }
    let content = doc::file::try_to_openetch_json(&broken).unwrap();
    let (_, v) = send(&app, exec("doc.load", json!({ "content": content }))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("not installed"), "{v}");
}

/// The simulator test PIN (`--sim-pin`).
const SIM_PIN: &str = "135797";

fn sim_approval() -> (std::path::PathBuf, ApprovalConfig) {
    let dir = pin::temp_dir("simpin-engine");
    let store = PinStore::simulator(&dir, SIM_PIN).expect("sim PIN");
    (dir, ApprovalConfig { pin: store, confirm: None })
}

#[tokio::test]
async fn the_sim_pin_approves_runs_on_the_simulator_and_is_reported() {
    let (dir, approval) = sim_approval();
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    let st = state_on(machine, SecurityConfig::default(), approval).await;
    let app = app(&st);
    let (s, v) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!((v["approval"]["sim_pin"].clone(), v["approval"]["pin_set"].clone()), (json!(true), json!(true)));
    assert_eq!(v["approval"]["pin_changed"], false);
    let id = pending_run(&app).await;
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({ "pin": "000000" })))).await;
    assert_eq!((s, v["code"].clone()), (StatusCode::FORBIDDEN, json!("pin_invalid")));
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({ "pin": SIM_PIN })))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(st.machine.sim_counters().unwrap().laser_singles() > 0, "the simulator ran it");
    // Never persisted: no PIN file, not even the config directory.
    assert!(!dir.join(pin::PIN_FILE).exists() && !dir.exists());
}

#[tokio::test]
async fn the_sim_pin_is_refused_when_a_usb_device_is_active() {
    // A machine on a USB link (a SimLink stands in for the board, so nothing real is opened).
    let machine = Arc::new(Machine::with_link(
        Box::new(SimLink::new()),
        LinkKind::Usb,
        None,
        MachineProfile::generic_lmc_fiber(),
        None,
        PollConfig::immediate(),
    ));
    // Startup refuses it outright...
    assert!(pin::check_sim_pin_allowed(machine.kind() == LinkKind::Simulated, true, Some("1")).is_err());
    // ...and an engine set up with one anyway never approves with it.
    let (dir, approval) = sim_approval();
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    let app = app(&st);
    let id = pending_run(&app).await;
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({ "pin": SIM_PIN })))).await;
    assert_eq!((s, v["code"].clone()), (StatusCode::FORBIDDEN, json!("sim_pin_real_link")), "{v}");
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending, "nothing was armed");
    assert!(!st.machine.is_running());
    assert!(!dir.exists(), "nothing written");
}

#[tokio::test]
async fn machine_status_reports_the_focus_light_and_a_file_pin_is_not_a_sim_pin() {
    let st = test_state().await;
    let (_, v) = send(&app(&st), req("GET", "/api/machine", None)).await;
    assert_eq!((v["focus_light"].clone(), v["approval"]["sim_pin"].clone()), (json!(false), json!(false)));
    let mut profile = MachineProfile::generic_lmc_fiber();
    profile.focus_light_port_bit = Some(9);
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    let st = state_on(machine, SecurityConfig::default(), approval(None)).await;
    let (_, v) = send(&app(&st), req("GET", "/api/machine", None)).await;
    assert_eq!(v["focus_light"], true);
}
