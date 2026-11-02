//! Route tests of the User Tools menu (`crate::user_tools`): saved through `PUT /api/prefs` from the UI only, never
//! holding a machine action, and checked again when preferences are imported.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

async fn app(tag: &str) -> (Router, Arc<AppState>, std::path::PathBuf) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir(tag);
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    assert!(st.libs.open(&dir).is_empty());
    (api::router(st.clone(), None), st, dir)
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
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn user_tools_are_saved_from_the_ui_and_never_hold_machine_actions() {
    let (app, st, dir) = app("user-tools").await;
    let tools = json!([
        {"name": "Box maker", "category": "Generators", "kind": "url", "url": "https://example.com/box"},
        {"name": "Name tag", "kind": "commands", "steps": [{"id": "shape.rect", "params": {"width_mm": 60, "height_mm": 20}}, {"id": "path.dogbone", "params": {"ids": ["obj-1"], "diameter_mm": 1}}]},
    ]);
    let (s, v) = send(&app, req("PUT", "/api/prefs", Some(json!({"user_tools": tools})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["user_tools"], tools);
    // Kept in preferences.json, and shown by GET.
    let saved: Value = serde_json::from_slice(&std::fs::read(dir.join(crate::prefs::PREFS_FILE)).unwrap()).unwrap();
    assert_eq!(saved["user_tools"], tools);
    assert_eq!(send(&app, req("GET", "/api/prefs", None)).await.1["user_tools"], tools);
    // An MCP client cannot change them.
    let mut agent = req("PUT", "/api/prefs", Some(json!({"user_tools": []})));
    agent.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, agent).await.0, StatusCode::FORBIDDEN);
    // Machine actions are not document commands, so no tool can hold one; neither can it open a script URL.
    for (bad, why) in [
        (json!([{"name": "Burn", "kind": "commands", "steps": [{"id": "job_request_run", "params": {}}]}]), "not a document command"),
        (json!([{"name": "Frame", "kind": "commands", "steps": [{"id": "frame_start", "params": {}}]}]), "not a document command"),
        (json!([{"name": "X", "kind": "url", "url": "javascript:alert(1)"}]), "http"),
    ] {
        let (s, v) = send(&app, req("PUT", "/api/prefs", Some(json!({"user_tools": bad})))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert!(v["error"].as_str().unwrap_or_default().contains(why), "{v}");
    }
    assert_eq!(st.specs().iter().filter(|s| !matches!(s.risk, commands::Risk::Query | commands::Risk::Edit)).count(), 0, "the registry holds document commands only");
    // An imported preferences file is checked the same way.
    let mut file = send(&app, req("GET", "/api/prefs/export", None)).await.1;
    file["preferences"]["user_tools"] = json!([{"name": "Burn", "kind": "commands", "steps": [{"id": "job_request_run"}]}]);
    let (s, v) = send(&app, req("POST", "/api/prefs/import", Some(file))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(send(&app, req("GET", "/api/prefs", None)).await.1["user_tools"], tools, "nothing changed");
}
