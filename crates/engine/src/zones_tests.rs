//! Route tests of the jigs and placement features on the simulated board: Re-mark region (clipped runs), keep-out
//! zones of jigs and of the device (refused runs, whoever requests them), pocket framing, and MCP's
//! `job_request_run` with a region.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::frame::{FrameRequest, outline_for};
use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

async fn state() -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("zonetests");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    st.job.open(dir.join(crate::job::COUNTERS_FILE));
    st
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
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params})))).await;
    assert!(s == StatusCode::OK && v["ok"] == true, "{id}: {v}");
    v["result"].clone()
}

fn pending(st: &AppState) -> usize {
    st.machine_status()["pending_runs"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn region_runs_mark_only_inside_the_region() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 20})).await;
    let (s, whole) = send(&app, req("POST", "/api/runs", Some(json!({})))).await;
    assert_eq!(s, StatusCode::CREATED, "{whole}");
    // The square's left edge only: a thin region around x = -10.
    let region = json!([[-11, -30], [-9, -30], [-9, 30], [-11, 30]]);
    let (s, part) = send(&app, req("POST", "/api/runs", Some(json!({"region": region})))).await;
    assert_eq!(s, StatusCode::CREATED, "{part}");
    let mark = |v: &Value| v["run"]["stats"]["mark_length_mm"].as_f64().unwrap();
    assert!((mark(&whole) - 80.0).abs() < 1e-6, "{whole}");
    // The left edge (20 mm) plus 1 mm of the top and bottom edges inside the region.
    assert!((mark(&part) - 22.0).abs() < 1e-6, "{part}");
    assert!(part["run"]["summary"].as_str().unwrap().contains("re-marking a region"));
    // Nothing inside, a broken polygon, or with Start Here: refused, nothing left pending.
    let n = pending(&st);
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"region": [[50, 50], [51, 50], [51, 51]]})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"region": [[0, 0], [1, 1]]})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"region": region, "from_segment": 1})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(pending(&st), n);
}

#[tokio::test]
async fn keep_out_zones_refuse_runs_from_jigs_and_the_device() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 20})).await;
    let jig = exec(&app, "jig.create", json!({"rows": 1, "cols": 1, "pocket": {"kind": "circle", "diameter_mm": 30}, "center": [0, 0]})).await;
    let jig = jig["jig"]["id"].as_str().unwrap().to_string();
    // A wall crossing the square's right edge.
    exec(&app, "jig.add_keep_out", json!({"jig": jig, "rect": [8, -2, 12, 2], "name": "Clamp"})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("keep-out zone"), "{v}");
    // MCP's request is refused the same way.
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"origin": "agent:test"})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Re-marking a region away from the wall is fine; switching the wall off clears it.
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"region": [[-12, -12], [0, -12], [0, 12], [-12, 12]]})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    exec(&app, "jig.remove_keep_out", json!({"jig": jig, "keep_out": "Clamp", "enabled": false})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    // A device zone, set in the app only.
    let zone = json!([{"name": "Fixture", "points": [[-12, -2], [-8, -2], [-8, 2], [-12, 2]]}]);
    let (s, _) = send(&app, req("PUT", "/api/prefs", Some(json!({"keep_out": zone, "origin": "agent:test"})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("PUT", "/api/prefs", Some(json!({"keep_out": zone})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["device_prefs"]["keep_out"][0]["name"], json!("Fixture"));
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Fixture (device)"), "{v}");
    let (s, _) = send(&app, req("PUT", "/api/prefs", Some(json!({"keep_out": [{"name": "Bad", "points": [[0, 0], [1, 1]]}]})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pocket_framing_traces_the_pockets() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    exec(&app, "jig.create", json!({"rows": 1, "cols": 2, "pitch_x_mm": 30, "pocket": {"kind": "rect", "width_mm": 20, "height_mm": 10}, "center": [0, 0]})).await;
    let d = st.document();
    let j = d.jigs[0].id.clone();
    let one: FrameRequest = serde_json::from_value(json!({"jig_pockets": {"jig": j, "pockets": [2]}, "mode": "bounds"})).unwrap();
    let polys = outline_for(&d, &one).unwrap();
    let xs: Vec<f64> = polys[0].iter().map(|p| p.0).collect();
    assert!((xs.iter().cloned().fold(f64::INFINITY, f64::min) - 5.0).abs() < 1e-6 && (xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) - 25.0).abs() < 1e-6, "{polys:?}");
    let all: FrameRequest = serde_json::from_value(json!({"jig_pockets": {"jig": j}, "mode": "outline"})).unwrap();
    assert_eq!(outline_for(&d, &all).unwrap().len(), 2);
    let bad: FrameRequest = serde_json::from_value(json!({"jig_pockets": {"jig": j, "pockets": [9]}})).unwrap();
    assert!(outline_for(&d, &bad).is_err());
    let both: FrameRequest = serde_json::from_value(json!({"ids": ["obj-1"], "jig_pockets": {"jig": j}})).unwrap();
    assert!(outline_for(&d, &both).is_err());
    // A re-mark region frames as its closed outline.
    let region: FrameRequest = serde_json::from_value(json!({"region": [[0, 0], [4, 0], [4, 3]]})).unwrap();
    assert_eq!(outline_for(&d, &region).unwrap(), vec![vec![(0.0, 0.0), (4.0, 0.0), (4.0, 3.0), (0.0, 0.0)]]);
    let line: FrameRequest = serde_json::from_value(json!({"region": [[0, 0], [4, 0], [8, 0]]})).unwrap();
    assert!(outline_for(&d, &line).is_err());
}

#[tokio::test]
async fn mcp_requests_a_region_run_and_never_approves() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 20})).await;
    let mcp = crate::mcp::GalvoMcp::new(st.clone());
    let mut a = serde_json::Map::new();
    a.insert("region".into(), json!([[-11, -30], [-9, -30], [-9, 30], [-11, 30]]));
    let r = mcp.call("job_request_run", a).await;
    let text = serde_json::to_string(&r).unwrap();
    assert!(text.contains("re-marking a region"), "{text}");
    let runs = st.machine_status()["pending_runs"].clone();
    assert_eq!(runs.as_array().unwrap().len(), 1);
    assert_eq!(runs[0]["state"], json!("pending"));
}
