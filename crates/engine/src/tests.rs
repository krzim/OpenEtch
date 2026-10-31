//! Route tests through the whole router (`tower::ServiceExt::oneshot`), on the simulated board.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::runs::RunState;
use crate::security::SecurityConfig;
use crate::{AppState, api};

/// A connected engine state on the simulated board with the generic profile.
pub(crate) async fn test_state() -> Arc<AppState> {
    test_state_with(SecurityConfig::default()).await
}

async fn test_state_with(security: SecurityConfig) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    AppState::new(Session::new(doc::Document::default()), machine, security)
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
        crate::Event::DocumentChanged { revision: 1, origin: Some(commands::Origin::Ui) }
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

    // The UI approves: now (and only now) the job reaches the board.
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for _ in 0..200 {
        if st.runs().get(&id).unwrap().state != RunState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let run = st.runs().get(&id).unwrap().clone();
    assert_eq!(run.state, RunState::Completed, "{:?}", run.error);
    assert!(counters.packets() > before.1);
    assert!(counters.laser_singles() > 0);
    assert!(counters.emitting_packets() > 0);

    // Approving twice is refused.
    let (s, _) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), None)).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn approve_refuses_unknown_and_stale_runs() {
    let st = test_state().await;
    let app = app(&st);
    let (s, v) = send(&app, req("POST", "/api/runs/run-404/approve", Some(json!({})))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["ok"], false);

    // Empty document: nothing to run.
    let (s, _) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    rect(&app, 10.0, 10.0).await;
    let (_, v) = send(&app, req("POST", "/api/runs", None)).await;
    let id = v["id"].as_str().unwrap().to_owned();
    rect(&app, 5.0, 5.0).await;
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), None)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(st.machine.sim_counters().unwrap().laser_singles(), 0);
}

#[tokio::test]
async fn power_above_the_profile_limit_is_refused() {
    let mut profile = MachineProfile::generic_lmc_fiber();
    profile.max_power_pct = 50.0;
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.unwrap();
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default());
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
    let (s, m) = send(&app, req("GET", "/api/machine?token=t0k3n-abcdefghijklmnop", None)).await;
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
