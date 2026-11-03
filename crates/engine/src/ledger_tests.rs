//! The job ledger on the simulated board: what a burn keeps, search and verdicts, Re-run (never started by itself),
//! the storage caps, and the MCP tools.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use rmcp::model::{ContentBlock, JsonObject};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::runs::RunState;
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

async fn state(tag: &str) -> (Arc<AppState>, Router, std::path::PathBuf) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir(tag);
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    st.job.open(dir.join(crate::job::COUNTERS_FILE));
    assert!(st.libs.open(&dir).is_empty());
    let app = api::router(st.clone(), None);
    (st, app, dir)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn raw(app: &Router, r: Request<Body>) -> (StatusCode, Vec<u8>) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    (status, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let (s, bytes) = raw(app, r).await;
    (s, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

async fn ok(app: &Router, r: Request<Body>) -> Value {
    let (s, v) = send(app, r).await;
    assert!(s.is_success(), "{s}: {v}");
    v
}

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let v = ok(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params})))).await;
    assert_eq!(v["ok"], true, "{id}: {v}");
    v["result"].clone()
}

/// Request a run of the active document, approve it with the PIN and wait for it to end.
async fn burn(st: &AppState, app: &Router) -> String {
    let v = ok(app, req("POST", "/api/runs", Some(json!({})))).await;
    let id = v["id"].as_str().unwrap().to_owned();
    ok(app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": TEST_PIN})))).await;
    for _ in 0..2000 {
        if st.runs().get(&id).is_some_and(|r| r.state.is_final()) {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run {id} did not end");
}

/// The newest ledger entry, once its files are written (they are written in the background).
async fn newest_with_files(app: &Router) -> Value {
    for _ in 0..1000 {
        let v = ok(app, req("GET", "/api/job-log?limit=1", None)).await;
        let e = v["entries"][0].clone();
        if e["thumbnail"] == true && e["rerun"] == true {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the ledger files were not written");
}

/// A coin: a filled rectangle on a "Blue" layer at 40 % / 200 ns, and a serial number text at 41.
async fn coin(app: &Router) {
    exec(app, "layer.update", json!({"id": "layer-1", "name": "Blue", "settings": {"power_pct": 40, "pulse_width_ns": 200, "mode": "fill"}})).await;
    exec(app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    exec(app, "shape.text", json!({ "text": "0ddd", "font": font, "height_mm": 4, "variable": { "mode": "serial" }, "name": "Serial" })).await;
    exec(app, "var.set_serial", json!({ "auto_advance": true, "current": 41 })).await;
}

#[tokio::test]
async fn a_burn_keeps_settings_values_thumbnail_and_document_and_rerun_only_requests() {
    let (st, app, dir) = state("ledger-a").await;
    coin(&app).await;
    let run = burn(&st, &app).await;
    assert_eq!(st.runs().get(&run).unwrap().state, RunState::Finished);
    assert_eq!(st.document().variables.current, 42, "the counter moved on after the burn");

    let e = newest_with_files(&app).await;
    let id = e["id"].as_str().unwrap().to_owned();
    assert!(id.ends_with(&format!("-{run}")), "{id}");
    assert_eq!(e["layers"][0]["name"], "Blue");
    assert_eq!((e["layers"][0]["settings"]["power_pct"].clone(), e["layers"][0]["settings"]["pulse_width_ns"].clone()), (json!(40.0), json!(200.0)));
    assert!(e["layers"][0]["estimated_time_s"].as_f64().unwrap() > 0.0, "{e}");
    assert_eq!(e["layers"][0]["objects"], 2);
    assert_eq!(e["variables"]["current"], 41);
    assert_eq!(e["variables"]["texts"][0]["value"], "041", "{e}");
    assert_eq!(e["variables"]["texts"][0]["name"], "Serial");
    assert!(e["estimated_time_s"].as_f64().unwrap() > 0.0);
    assert_eq!(e["verdict"], Value::Null);

    let (s, png) = raw(&app, req("GET", &format!("/api/job-log/{id}/thumbnail.png"), None)).await;
    assert_eq!((s, &png[1..4]), (StatusCode::OK, &b"PNG"[..]));
    let (s, _) = raw(&app, req("GET", "/api/job-log/1-run-99/thumbnail.png", None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = raw(&app, req("GET", "/api/job-log/..%2Fx/thumbnail.png", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "ids are never paths");

    // Search: every word must match somewhere (layer name, value, settings, note).
    let hits = |q: &str| {
        let app = app.clone();
        let q = q.to_owned();
        async move { ok(&app, req("GET", &format!("/api/job-log?q={q}"), None)).await["entries"].as_array().unwrap().len() }
    };
    assert_eq!(hits("blue").await, 1);
    assert_eq!(hits("blue%20041").await, 1);
    assert_eq!(hits("200%20ns").await, 1);
    assert_eq!(hits("blue%20green").await, 0);

    // Verdicts and notes.
    let v = ok(&app, req("POST", &format!("/api/job-log/{id}/mark"), Some(json!({"verdict": "scrap", "note": "Too dark at the rim"})))).await;
    assert_eq!((v["verdict"].clone(), v["note"].clone(), v["noted_by"].clone()), (json!("scrap"), json!("Too dark at the rim"), json!("ui")));
    assert_eq!(hits("rim").await, 1);
    let n = |q: &str| {
        let app = app.clone();
        let q = q.to_owned();
        async move { ok(&app, req("GET", &format!("/api/job-log?verdict={q}"), None)).await["entries"].as_array().unwrap().len() }
    };
    assert_eq!((n("scrap").await, n("ok").await, n("unmarked").await), (1, 0, 0));
    let (s, _) = send(&app, req("GET", "/api/job-log?verdict=maybe", None)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", &format!("/api/job-log/{id}/mark"), Some(json!({"verdict": "meh"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", &format!("/api/job-log/{id}/mark"), Some(json!({"note": "x".repeat(2001)})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, csv) = raw(&app, req("GET", "/api/job-log?format=csv", None)).await;
    let csv = String::from_utf8(csv).unwrap();
    assert_eq!(s, StatusCode::OK);
    assert!(csv.contains(",scrap,Too dark at the rim,Blue: 40% "), "{csv}");

    // Re-run exactly as burned: a new active document with the counter as burned, and a PENDING run only.
    let before = st.document();
    let counters = st.machine.sim_counters().unwrap().clone();
    let sent = (counters.singles(), counters.packets());
    let v = ok(&app, req("POST", &format!("/api/job-log/{id}/rerun"), Some(json!({})))).await;
    let doc_id = v["document_id"].as_str().unwrap().to_owned();
    assert_eq!(st.active_document_id(), doc_id);
    assert_ne!(doc_id, "default");
    assert_eq!(st.document().variables.current, 41, "the serial number as burned");
    assert_eq!(st.document().objects.len(), before.objects.len());
    let pending = v["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["state"], "pending", "{v}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(st.runs().get(&pending).unwrap().state, RunState::Pending, "never approved by the ledger");
    assert!(!st.machine.is_running());
    assert_eq!((counters.singles(), counters.packets()), sent, "nothing was sent to the board");
    let docs = ok(&app, req("GET", "/api/documents", None)).await;
    let opened = docs["documents"].as_array().unwrap().iter().find(|d| d["id"] == doc_id.as_str()).unwrap().clone();
    assert!(opened["display_name"].as_str().unwrap().contains("as burned"), "{opened}");
    assert_eq!(opened["dirty"], false);
    // The person approves it as any run; the burn is a new ledger entry.
    ok(&app, req("POST", &format!("/api/runs/{pending}/approve"), Some(json!({"pin": TEST_PIN})))).await;
    for _ in 0..2000 {
        if st.runs().get(&pending).is_some_and(|r| r.state.is_final()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let again = newest_with_files(&app).await;
    assert_eq!(again["variables"]["texts"][0]["value"], "041", "the same serial number again");

    let (s, v) = send(&app, req("POST", "/api/job-log/1-run-99/rerun", Some(json!({})))).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn storage_caps_prune_and_settings_are_ui_only() {
    let (st, app, dir) = state("ledger-b").await;
    let v = ok(&app, req("GET", "/api/job-ledger/settings", None)).await;
    assert_eq!(v["settings"], json!({"keep_documents": 100, "max_document_mb": 25, "keep_thumbnails": 2000, "thumbnail_px": 160}));
    let mut agent = req("PUT", "/api/job-ledger/settings", Some(json!({"keep_documents": 0})));
    agent.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, agent).await.0, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, req("PUT", "/api/job-ledger/settings", Some(json!({"keep_documents": 0, "origin": "agent:x"})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    for bad in [json!({"thumbnail_px": 8}), json!({"keep_documents": 99999}), json!({"nope": 1})] {
        let (s, v) = send(&app, req("PUT", "/api/job-ledger/settings", Some(bad.clone()))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {v}");
    }
    let v = ok(&app, req("PUT", "/api/job-ledger/settings", Some(json!({"keep_documents": 1, "keep_thumbnails": 2, "thumbnail_px": 96})))).await;
    assert_eq!(v["settings"]["keep_documents"], 1);
    exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    for _ in 0..3 {
        burn(&st, &app).await;
        newest_with_files(&app).await;
        // Distinct end times keep the ids apart.
        tokio::time::sleep(Duration::from_millis(1050)).await;
    }
    let all = ok(&app, req("GET", "/api/job-log", None)).await;
    let e = all["entries"].as_array().unwrap();
    assert_eq!(e.len(), 3, "the log keeps every run");
    assert_eq!(e.iter().map(|x| x["rerun"] == true).collect::<Vec<_>>(), [true, false, false], "one document kept");
    assert_eq!(e.iter().map(|x| x["thumbnail"] == true).collect::<Vec<_>>(), [true, true, false], "two thumbnails kept");
    assert!(e[1]["rerun_reason"].as_str().unwrap().contains("newest 1 jobs"), "{}", e[1]);
    let png = st.libs.job_ledger(|l| l.thumbnail(e[0]["id"].as_str().unwrap())).unwrap();
    let img = image::load_from_memory(&png).unwrap();
    assert!(img.width().max(img.height()) <= 96);
    // Turning documents off deletes the kept ones at once.
    let v = ok(&app, req("PUT", "/api/job-ledger/settings", Some(json!({"keep_documents": 0})))).await;
    assert_eq!((v["usage"]["documents"].clone(), v["usage"]["thumbnails"].clone()), (json!(0), json!(2)));
    let e = ok(&app, req("GET", "/api/job-log?limit=1", None)).await["entries"][0].clone();
    assert!(e["rerun_reason"].as_str().unwrap().contains("keeps no documents"), "{e}");
    // Settings survive a restart of the store.
    assert_eq!(crate::ledger::Ledger::open(&dir).settings().keep_thumbnails, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

fn text_of(r: &rmcp::model::CallToolResult) -> Value {
    let t = r
        .content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    serde_json::from_str(&t).unwrap_or(Value::String(t))
}

fn obj(v: Value) -> JsonObject {
    v.as_object().cloned().unwrap_or_default()
}

#[tokio::test]
async fn agents_search_read_mark_and_open_but_never_run() {
    let (st, app, dir) = state("ledger-c").await;
    coin(&app).await;
    burn(&st, &app).await;
    newest_with_files(&app).await;
    let mcp = crate::mcp::GalvoMcp::new(st.clone());
    // "What settings made last week's blue coins?"
    let r = mcp.call("job_ledger_search", obj(json!({"q": "blue", "since_unix_s": 1}))).await;
    assert_eq!(r.is_error, Some(false), "{:?}", text_of(&r));
    let v = text_of(&r);
    assert_eq!(v["count"], 1);
    assert!(v["now_unix_s"].as_u64().unwrap() > 1_700_000_000);
    assert_eq!(v["entries"][0]["layers"][0]["settings"]["power_pct"], 40.0);
    let id = v["entries"][0]["id"].as_str().unwrap().to_owned();
    let r = mcp.call("job_ledger_get", obj(json!({"id": id, "thumbnail": true}))).await;
    assert!(r.content.iter().any(|c| matches!(c, ContentBlock::Image(i) if i.mime_type == "image/png")));
    let r = mcp.call("job_ledger_mark", obj(json!({"id": id, "verdict": "ok", "note": "good blue"}))).await;
    let v = text_of(&r);
    assert_eq!((v["verdict"].clone(), v["noted_by"].clone()), (json!("ok"), json!("agent:mcp")), "{v}");
    let r = mcp.call("job_ledger_mark", obj(json!({"id": id, "verdict": "approve"}))).await;
    assert_eq!(r.is_error, Some(true));
    // Open as burned: a new active document, no run.
    let runs_before = st.runs().views(st.revision()).len();
    let r = mcp.call("job_ledger_open", obj(json!({"id": id}))).await;
    assert_eq!(r.is_error, Some(false), "{:?}", text_of(&r));
    assert_eq!(st.active_document_id(), text_of(&r)["document_id"].as_str().unwrap());
    assert_eq!(st.runs().views(st.revision()).len(), runs_before, "opening never requests a run");
    // Marks over HTTP from an MCP client are recorded as the agent's.
    let mut r = req("POST", &format!("/api/job-log/{id}/mark"), Some(json!({"verdict": null, "note": ""})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.1["noted_by"], "agent:mcp");
    let _ = std::fs::remove_dir_all(&dir);
}
