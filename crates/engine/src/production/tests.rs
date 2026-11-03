//! Production end to end on the simulated board, through the whole router: pause at a layer (Continue in the app, the
//! foot switch, Stop during the wait), serial slots and the ledger with a re-mark, Stop at End, the cockpit's design
//! lock, the extractor's gate and run-on, the hour meter, the notification feed and a webhook, and the MCP tools.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use rmcp::model::{ContentBlock, JsonObject};
use serde_json::{Value, json};

use crate::approve::tests::{PEDAL, browser, pedal_profile, pending_run, req, send, set_inputs, state, wait_final};
use crate::mcp::GalvoMcp;
use crate::pin::ApprovalMode;
use crate::runs::RunState;
use crate::{AppState, api};

fn mcp_req(mut r: axum::http::Request<axum::body::Body>) -> axum::http::Request<axum::body::Body> {
    r.headers_mut().insert("mcp-session-id", "s1".parse().unwrap());
    r
}

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params, "origin": "ui"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{id}: {v}");
    v["result"].clone()
}

/// Two layers with a square each; the second pauses before it.
async fn two_layers(app: &Router) -> String {
    let doc: Value = send(app, req("GET", "/api/document", None)).await.1;
    let first = doc["layers"][0]["id"].as_str().unwrap().to_owned();
    exec(app, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "layer": first})).await;
    let second = exec(app, "layer.add", json!({"name": "Frost"})).await["id"].as_str().unwrap().to_owned();
    exec(app, "shape.rect", json!({"width_mm": 6, "height_mm": 6, "center": [20, 0], "layer": second})).await;
    exec(app, "layer.update", json!({"id": second, "settings": {"options": {"operator_pause": true}}})).await;
    second
}

async fn start(app: &Router) -> String {
    let rev = send(app, req("GET", "/api/document", None)).await.1["revision"].as_u64().unwrap();
    let (s, v) = send(app, browser(req("POST", "/api/job/start", Some(json!({"origin": "ui", "revision": rev}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..600 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn click_engine() -> (Arc<AppState>, Router) {
    let st = state(pedal_profile(), ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    (st, app)
}

#[tokio::test]
async fn a_layer_pause_waits_with_nothing_emitting_until_continue() {
    let (st, app) = click_engine().await;
    two_layers(&app).await;
    let id = start(&app).await;
    let view = st.runs().get(&id).unwrap().view(0);
    assert_eq!(view.layer_pauses, ["Frost"], "{}", view.summary);
    assert!(view.summary.contains("waiting for the operator before \"Frost\""), "{}", view.summary);
    wait_for("the layer pause", || st.production.pause.waiting().is_some()).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let fired = (counters.emitting_packets(), counters.laser_singles());
    let job = send(&app, req("GET", "/api/job", None)).await.1;
    assert_eq!((job["state"].as_str(), job["waiting_layer"].as_str()), (Some("waiting_at_layer"), Some("Frost")), "{job}");
    let p = send(&app, req("GET", "/api/production", None)).await.1;
    assert_eq!(p["layer_pause"]["waiting"], true);
    assert_eq!(p["layer_pause"]["foot_switch"], true);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!((counters.emitting_packets(), counters.laser_singles()), fired, "nothing emits while waiting");
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Running);
    // An agent cannot continue (it would fire the laser); neither can a script origin.
    let (s, _) = send(&app, mcp_req(req("POST", "/api/job/continue", Some(json!({}))))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, req("POST", "/api/job/continue", Some(json!({"origin": "script"})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("POST", "/api/job/continue", Some(json!({"origin": "ui", "run_id": id})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > fired.0, "the second stage marked");
    assert_eq!(st.job.count(&st.active_device().id), 1, "one iteration of two stages");
    let (s, _) = send(&app, req("POST", "/api/job/continue", Some(json!({"origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "nothing waits any more");
    // The feed has the layer pause and the finished job.
    let feed = send(&app, req("GET", "/api/notifications/events?after=0", None)).await.1;
    let kinds: Vec<&str> = feed["events"].as_array().unwrap().iter().filter_map(|e| e["event"].as_str()).collect();
    assert!(kinds.contains(&"layer_pause"), "{feed}");
}

#[tokio::test]
async fn the_foot_switch_continues_and_stop_wins_during_the_wait() {
    let (st, app) = click_engine().await;
    two_layers(&app).await;
    let id = start(&app).await;
    wait_for("the layer pause", || st.production.pause.waiting().is_some()).await;
    set_inputs(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    set_inputs(&app, PEDAL).await;
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    set_inputs(&app, 0).await;
    // Stop during the wait: the run ends there, the second layer never streams.
    let id = start(&app).await;
    wait_for("the layer pause", || st.production.pause.waiting().is_some()).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let fired = counters.emitting_packets();
    let (s, _) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Aborted);
    assert!(run.error.as_deref().unwrap_or_default().contains("Frost"), "{:?}", run.error);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(counters.emitting_packets(), fired, "nothing streamed after the Stop");
    assert!(st.production.pause.waiting().is_none());
}

/// Three pockets with a serial text each (offsets 0..2), counting from 7 with auto-advance.
async fn pockets(app: &Router) {
    for k in 0..3 {
        exec(
            app,
            "shape.text",
            json!({"text": "0dd", "font": {"family": "Hershey Sans", "kind": "stroke"}, "height_mm": 4,
                   "center": [-30 + 30 * k, 0], "variable": {"mode": "serial", "offset": k}}),
        )
        .await;
    }
    exec(app, "var.set_serial", json!({"start": 1, "end": 10, "current": 7, "advance_by": 3, "auto_advance": true})).await;
}

#[tokio::test]
async fn the_ledger_records_slots_and_a_re_mark_keeps_the_counter() {
    let (st, app) = click_engine().await;
    pockets(&app).await;
    exec(&app, "var.set_serial", json!({"empty_slots": [1]})).await;
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    assert_eq!(st.document().variables.current, 9, "two slots marked: 7 and 8");
    let l = send(&app, req("GET", "/api/production/ledger", None)).await.1;
    let e = &l["entries"][0];
    assert_eq!((e["run_id"].as_str(), e["counter"].as_i64()), (Some(id.as_str()), Some(7)));
    let shown: Vec<(&str, bool)> =
        e["slots"].as_array().unwrap().iter().map(|s| (s["values"][0]["shown"].as_str().unwrap(), s["marked"].as_bool().unwrap())).collect();
    assert_eq!(shown, [("07", true), ("08", false), ("08", true)], "slot 3 took slot 2's value");
    assert_eq!(e["slots"][1]["skipped"], "empty");
    let (s, csv) = send(&app, req("GET", "/api/production/ledger?format=csv", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(csv.as_str().unwrap().lines().count(), 4);
    // Re-mark slot 3 with 08: a pending run of that pocket only, approved like any run; the counter stays.
    let (s, v) = send(&app, req("POST", "/api/production/remark", Some(json!({"run_id": id, "slot": 3, "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["shown"], json!(["08"]));
    let remark = v["id"].as_str().unwrap().to_owned();
    assert!(v["run"]["summary"].as_str().unwrap().starts_with("Re-mark slot 3"), "{v}");
    assert_eq!(st.runs().get(&remark).unwrap().state, RunState::Pending, "nothing fires before approval");
    let (s, v) = send(&app, browser(req("POST", &format!("/api/runs/{remark}/approve"), Some(json!({}))))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &remark).await.state, RunState::Finished);
    assert_eq!(st.document().variables.current, 9, "a re-mark does not move the counter");
    let l = send(&app, req("GET", &format!("/api/production/ledger?run={id}"), None)).await.1;
    let last = l["entries"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["remark_of"], json!({"run_id": id, "slot": 3}));
    assert_eq!(last["slots"].as_array().unwrap().len(), 1);
    assert_eq!(last["slots"][0]["values"][0]["shown"], "08");
}

#[tokio::test]
async fn stop_at_end_refuses_once_every_value_is_used() {
    let (st, app) = click_engine().await;
    pockets(&app).await;
    exec(&app, "var.set_serial", json!({"end": 8, "stop_at_end": true})).await;
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    assert_eq!(st.document().variables.current, 9, "7 and 8 marked, slot 3 past End; held one past End");
    let rev = st.revision();
    let (s, v) = send(&app, browser(req("POST", "/api/job/start", Some(json!({"origin": "ui", "revision": rev}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("up to End (8)"), "{v}");
    assert!(st.runs().views(rev).iter().all(|r| r.state != RunState::Pending), "nothing left pending");
}

#[tokio::test]
async fn the_cockpit_lock_refuses_edits_but_never_stop_or_the_counter() {
    let (st, app) = click_engine().await;
    exec(&app, "shape.rect", json!({"width_mm": 5, "height_mm": 5})).await;
    let lock = |on: bool| req("POST", "/api/production/lock", Some(json!({"on": on, "holder": "ipad", "label": "iPad", "origin": "ui"})));
    let (s, _) = send(&app, mcp_req(lock(true))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "agents cannot lock or unlock");
    let (s, v) = send(&app, lock(true)).await;
    assert_eq!((s, v["locked"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 2, "height_mm": 2}, "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("locked by the production cockpit on iPad"), "{v}");
    let (s, _) = send(&app, req("POST", "/api/undo", Some(json!({})))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let mcp = GalvoMcp::new(st.clone());
    let r = mcp.call("execute", obj(json!({"id": "shape.rect", "params": {"width_mm": 2, "height_mm": 2}}))).await;
    assert_eq!(r.is_error, Some(true));
    // Every other path that changes the active document: a batch, an upload, a CSV file, undo's siblings, opening a
    // new active document (refused before it opens one), switching documents.
    let locked = |s: StatusCode, v: &Value| {
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert!(v["error"].as_str().unwrap_or_default().contains("locked by the production cockpit"), "{v}");
    };
    let rect = json!({"id": "shape.rect", "params": {"width_mm": 2, "height_mm": 2}});
    let (s, v) = send(&app, req("POST", "/api/batch", Some(json!({"commands": [rect], "origin": "ui"})))).await;
    locked(s, &v);
    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "var.load_csv", "params": {"content": "a\nb"}, "origin": "ui"})))).await;
    locked(s, &v);
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm"><rect width="5" height="5"/></svg>"#;
    let upload = axum::http::Request::builder()
        .method("POST")
        .uri("/api/document/upload?format=svg&name=a.svg")
        .header("host", "localhost:7878")
        .body(axum::body::Body::from(svg))
        .unwrap();
    let (s, v) = send(&app, upload).await;
    locked(s, &v);
    for (uri, body) in [("/api/redo", json!({})), ("/api/history/goto", json!({"marker": 0}))] {
        let (s, v) = send(&app, req("POST", uri, Some(body))).await;
        locked(s, &v);
    }
    let docs = |app: &Router| {
        let app = app.clone();
        async move { send(&app, req("GET", "/api/documents", None)).await.1["documents"].as_array().map_or(0, Vec::len) }
    };
    let open = docs(&app).await;
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({"origin": "ui"})))).await;
    locked(s, &v);
    assert_eq!(docs(&app).await, open, "nothing was opened");
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({"activate": false, "origin": "ui"})))).await;
    assert_eq!(s, StatusCode::CREATED, "a document opened beside the active one changes nothing burned: {v}");
    let other = v["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", &format!("/api/documents/{other}/activate"), Some(json!({"origin": "ui"})))).await;
    locked(s, &v);
    // The counter, queries, framing and Stop still work.
    exec(&app, "var.set_serial", json!({"current": 3})).await;
    exec(&app, "doc.inspect", json!({})).await;
    let (s, _) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(&app, lock(false)).await;
    assert_eq!((s, v["locked"].as_bool()), (StatusCode::OK, Some(false)));
    exec(&app, "shape.rect", json!({"width_mm": 2, "height_mm": 2})).await;
}

fn obj(v: Value) -> JsonObject {
    v.as_object().cloned().unwrap_or_default()
}

fn text_of(r: &rmcp::model::CallToolResult) -> String {
    r.content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_extractor_gate_and_the_hour_meter() {
    let (st, app) = click_engine().await;
    exec(&app, "shape.rect", json!({"width_mm": 5, "height_mm": 5})).await;
    let config = json!({"config": {"output": {"kind": "none"}, "gate": {"kind": "input", "bit": 6}}, "origin": "ui"});
    let (s, _) = send(&app, mcp_req(req("PUT", "/api/production/extractor", Some(config.clone())))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("PUT", "/api/production/extractor", Some(config))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let rev = st.revision();
    let (s, v) = send(&app, browser(req("POST", "/api/job/start", Some(json!({"origin": "ui", "revision": rev}))))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("extractor is not running (input 6"), "{v}");
    set_inputs(&app, 1 << 6).await;
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    // The meter counted the run (estimated: the simulator gives no mark time).
    let m = send(&app, req("GET", "/api/production/meter", None)).await.1;
    assert_eq!(m["runs"], 1);
    assert!(m["estimated_hours"].as_f64().unwrap() > 0.0 && m["measured_hours"] == 0.0, "{m}");
    let body = json!({"reminders": [{"name": "Clean the lens", "every_runs": 1}], "origin": "ui"});
    let (s, m) = send(&app, req("PUT", "/api/production/maintenance", Some(body))).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["reminders"][0]["due"], false, "counts from now");
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    let m = send(&app, req("GET", "/api/production/meter", None)).await.1;
    assert_eq!(m["due"][0]["name"], "Clean the lens", "{m}");
    let rid = m["due"][0]["id"].as_str().unwrap().to_owned();
    let (s, _) = send(&app, mcp_req(req("POST", &format!("/api/production/maintenance/{rid}/done"), Some(json!({}))))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, m) = send(&app, req("POST", &format!("/api/production/maintenance/{rid}/done"), Some(json!({"origin": "ui"})))).await;
    assert_eq!((s, m["due"].as_array().map(Vec::len)), (StatusCode::OK, Some(0)), "{m}");
    let feed = st.production.notify.feed(0, 100).0;
    assert!(feed.iter().any(|e| e.event == super::notify::Kind::MaintenanceDue), "{feed:?}");
}

#[tokio::test]
async fn the_busy_output_runs_on_after_the_job() {
    let mut profile = pedal_profile();
    profile.io.busy_light = Some(3);
    let st = state(profile, ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 5, "height_mm": 5})).await;
    let config = json!({"config": {"output": {"kind": "lmc_busy"}, "run_on_s": 1}, "origin": "ui"});
    let (s, v) = send(&app, req("PUT", "/api/production/extractor", Some(config))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    wait_for("the run-on", || st.production.extractor.state().on).await;
    assert!(st.production.extractor.state().off_at_unix_s.is_some());
    wait_for("the run-on to end", || !st.production.extractor.state().on).await;
    assert!(st.production.extractor.state().last_error.is_none(), "{:?}", st.production.extractor.state());
}

#[tokio::test]
async fn a_refused_approval_leaves_the_extractor_off() {
    let mut profile = pedal_profile();
    profile.io.busy_light = Some(3);
    let st = state(profile, ApprovalMode::Click).await;
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 5, "height_mm": 5})).await;
    let config = json!({"config": {"output": {"kind": "lmc_busy"}, "auto": true, "gate": {"kind": "switched_on"}}, "origin": "ui"});
    let (s, v) = send(&app, req("PUT", "/api/production/extractor", Some(config))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let approve = |id: &str| browser(req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"origin": "ui"}))));
    // A stale run: refused by the run checks before the extractor is touched.
    let id = pending_run(&app, None).await;
    exec(&app, "shape.rect", json!({"width_mm": 2, "height_mm": 2})).await;
    let (s, v) = send(&app, approve(&id)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(!st.production.extractor.state().on, "{:?}", st.production.extractor.state());
    // Refused after the gate switched it on (the machine is taken): switched off again.
    let id = pending_run(&app, None).await;
    assert!(st.machine.try_begin_run());
    let (s, v) = send(&app, approve(&id)).await;
    st.machine.end_run();
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("another run"), "{v}");
    assert!(!st.production.extractor.state().on, "{:?}", st.production.extractor.state());
    // Approved: switched on for the job.
    let (s, v) = send(&app, approve(&id)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(st.production.extractor.state().on);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

#[tokio::test]
async fn notifications_reach_a_webhook_and_only_the_ui_configures_them() {
    if std::process::Command::new("curl").arg("--version").output().is_err() {
        eprintln!("curl is not installed; skipping the webhook delivery test");
        return;
    }
    let got: Arc<Mutex<Vec<Value>>> = Arc::default();
    let sink = got.clone();
    let hook = Router::new().route(
        "/hook",
        axum::routing::post(move |body: axum::body::Bytes| {
            let sink = sink.clone();
            async move {
                sink.lock().unwrap().push(serde_json::from_slice(&body).unwrap_or(Value::Null));
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, hook).await });
    let (st, app) = click_engine().await;
    super::spawn(st.clone());
    let targets = json!({"targets": [{"name": "Shop", "kind": "webhook", "url": format!("http://127.0.0.1:{port}/hook"),
                                      "events": ["job_done", "stop_at_reached"]}], "origin": "ui"});
    let (s, _) = send(&app, mcp_req(req("PUT", "/api/notifications", Some(targets.clone())))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "an agent cannot point the feed anywhere");
    let (s, v) = send(&app, req("PUT", "/api/notifications", Some(targets))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let target = v["targets"][0]["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", "/api/notifications/test", Some(json!({"id": target, "origin": "ui"})))).await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    exec(&app, "shape.rect", json!({"width_mm": 5, "height_mm": 5})).await;
    st.job.set_stop_at(Some(1));
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    for _ in 0..500 {
        if got.lock().unwrap().len() >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let bodies = got.lock().unwrap().clone();
    let kinds: Vec<&str> = bodies.iter().filter_map(|b| b["event"].as_str()).collect();
    assert_eq!(kinds, ["test", "stop_at_reached", "job_done"], "{bodies:?}");
    assert_eq!(bodies[2]["operation"], "mark");
    assert_eq!(bodies[2]["run_id"], json!(id));
    // MCP reads the same feed, after a cursor.
    let mcp = GalvoMcp::new(st.clone());
    let r = mcp.call("production_events", obj(json!({"after": 0}))).await;
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    let last = v["last_seq"].as_u64().unwrap();
    assert!(v["events"].as_array().unwrap().iter().any(|e| e["event"] == "job_done"), "{v}");
    let r = mcp.call("production_events", obj(json!({"after": last, "wait_s": 0}))).await;
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    assert_eq!(v["events"], json!([]));
}

#[tokio::test]
async fn mcp_reads_production_and_only_requests_a_pending_re_mark() {
    let (st, app) = click_engine().await;
    pockets(&app).await;
    let id = start(&app).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    let mcp = GalvoMcp::new(st.clone());
    for name in super::mcp::TOOL_NAMES {
        assert!(GalvoMcp::tools().iter().any(|t| t.name == name), "{name}");
    }
    let v: Value = serde_json::from_str(&text_of(&mcp.call("production_status", JsonObject::new()).await)).unwrap();
    assert_eq!(v["slots"]["current"], 10);
    assert_eq!(v["cycle"]["count"], 1);
    let v: Value = serde_json::from_str(&text_of(&mcp.call("serial_ledger", obj(json!({"run_id": id}))).await)).unwrap();
    assert_eq!(v["entries"][0]["slots"].as_array().unwrap().len(), 3);
    let v: Value = serde_json::from_str(&text_of(&mcp.call("hour_meter", JsonObject::new()).await)).unwrap();
    assert_eq!(v["runs"], 1);
    let r = mcp.call("serial_remark_request", obj(json!({"run_id": id, "slot": 2}))).await;
    assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
    let v: Value = serde_json::from_str(&text_of(&r)).unwrap();
    let remark = v["id"].as_str().unwrap();
    assert_eq!(st.runs().get(remark).unwrap().state, RunState::Pending, "an agent's re-mark waits for a person");
    // The design commands are open to agents: a layer pause and Stop at End.
    let layer = st.document().layers[0].id.clone();
    let r = mcp
        .call("execute", obj(json!({"id": "layer.update", "params": {"id": layer, "settings": {"options": {"operator_pause": true}}}})))
        .await;
    assert_eq!(r.is_error, Some(false), "{}", text_of(&r));
    let r = mcp.call("execute", obj(json!({"id": "var.slots", "params": {}}))).await;
    assert!(text_of(&r).contains("\"advance\""), "{}", text_of(&r));
}

#[test]
fn the_production_mcp_module_never_arms_or_continues() {
    let src = include_str!("mcp.rs");
    for forbidden in ["Armed", "approved_in_ui", "approve::", "request_continue", ".run(", "Controller", "pin::", "extractor::set"] {
        assert!(!src.contains(forbidden), "production/mcp.rs must not reference {forbidden}");
    }
}
