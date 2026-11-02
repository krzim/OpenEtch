//! A Ruida gantry through the whole engine on the Ruida simulator: device creation and activation, framing, runs
//! that only start when approved, the controller's file list (Send, list, start through approval, delete), Require
//! Framing and MCP refusals for stored files, Save RD file and its import round trip, and switching back to the
//! galvo. Unverified on hardware (no Ruida controller).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::ruida::SimRuidaHandle;
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::approve::tests::{browser, req, send};
use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, ApprovalMode, PinStore};
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, api};

const PIN: &str = "864201";

fn mcp(mut r: Request<Body>) -> Request<Body> {
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    r
}

async fn exec(app: &Router, id: &str, params: Value) {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params, "origin": "ui"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{id}: {v}");
}

async fn wait_final(st: &AppState, id: &str) -> Run {
    for _ in 0..2000 {
        let run = st.runs().get(id).cloned().expect("run exists");
        if run.state.is_final() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run {id} did not finish");
}

/// An engine on the simulators with a Ruida device (600 x 400 bed, rear-right origin, 60 % / 400 mm/s limits) made
/// and activated through the API; returns the app, the state and the built-in galvo's id.
async fn ruida_engine(mode: ApprovalMode) -> (Router, Arc<AppState>, String) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.unwrap();
    let dir = pin::temp_dir("ruida-engine");
    if mode == ApprovalMode::Pin {
        pin::set_pin(&dir, PIN).unwrap();
    }
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, crate::tests::test_projects());
    if mode != ApprovalMode::Pin {
        st.set_approval_mode(mode).expect("set once");
    }
    let app = api::router(st.clone(), None);
    let galvo = st.active_device().id;
    let profile = json!({"address": "192.168.1.100", "work_area_mm": [600.0, 400.0], "origin": "top_right", "max_power_pct": 60.0, "max_speed_mm_s": 400.0});
    let body = json!({"display_name": "CO2", "profile": profile});
    assert_eq!(send(&app, mcp(req("POST", "/api/devices/ruida", Some(body.clone())))).await.0, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("POST", "/api/devices/ruida", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((&v["device"]["kind"], &v["device"]["controllable"]), (&json!("ruida"), &json!(true)), "{v}");
    let id = v["device"]["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", &format!("/api/devices/{id}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    (app, st, galvo)
}

fn sim(st: &AppState) -> SimRuidaHandle {
    st.machine.sim_counters().unwrap().ruida_handle().expect("a simulated Ruida controller")
}

async fn approve(app: &Router, id: &str) -> (StatusCode, Value) {
    send(app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN})))).await
}

/// A 40 x 20 mm rectangle at 30 % and 50 mm/s.
async fn rect(app: &Router) {
    exec(app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    exec(app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 50}})).await;
}

#[tokio::test]
async fn a_ruida_device_frames_dark_and_runs_only_when_approved() {
    let (app, st, _) = ruida_engine(ApprovalMode::Pin).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["driver"], &m["link"], &m["connected"]), (&json!("ruida"), &json!("sim"), &json!(true)), "{m}");
    assert_eq!((&m["work_area_mm"], &m["max_power_pct"]), (&json!([600.0, 400.0]), &json!(60.0)), "{m}");
    let ctl = sim(&st);
    rect(&app).await;

    // Framing: rapid moves, never a cut; MCP cannot start it (the head moves).
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/frame", Some(json!({"continuous": false}))))).await.0, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for _ in 0..400 {
        if !st.machine.is_framing() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let r = ctl.report();
    assert!(r.segments >= 4 && r.burns == 0, "{r:?}");

    // A pending run sends nothing.
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["driver"], "ruida", "{v}");
    assert!(v["run"]["summary"].as_str().unwrap().contains("Ruida job"), "{v}");
    let before = ctl.report().datagrams;
    assert_eq!(send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": "000000"})))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, mcp(req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN}))))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(ctl.report().datagrams, before, "nothing was sent for the pending run");

    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = ctl.report();
    assert_eq!((r.burns, r.jobs_finished), (4, 1), "the rectangle's four sides: {r:?}");
    let burns: Vec<_> = ctl.with(|s| s.burns().cloned().collect());
    // 30 % at 50 mm/s; the rectangle is centred on the 600 x 400 bed, rear right origin: x 280..320, y 190..210 mm.
    assert!(burns.iter().all(|b| (b.power.percent() - 30.0).abs() < 0.01 && b.speed_um_s == 50_000), "{burns:?}");
    let xs: Vec<i64> = burns.iter().flat_map(|b| [b.from[0], b.to[0]]).collect();
    assert_eq!((xs.iter().min(), xs.iter().max()), (Some(&280_000), Some(&320_000)));
    assert_eq!(st.job.count(&st.active_device().id), 1);
}

#[tokio::test]
async fn limits_and_galvo_only_options_are_refused_when_requested() {
    let (app, st, _) = ruida_engine(ApprovalMode::Pin).await;
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    for (settings, why) in [
        (json!({"power_pct": 61, "speed_mm_s": 50}), "power"),
        (json!({"power_pct": 30, "speed_mm_s": 401}), "speed"),
    ] {
        exec(&app, "layer.update", json!({"id": "layer-1", "settings": settings})).await;
        let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
        assert!(s.is_client_error(), "{why}: {s} {v}");
        assert!(v["error"].as_str().unwrap_or_default().contains(why), "{why}: {v}");
    }
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 50}})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"start_input": true})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    // The GRBL layer options are not built for Ruida: a lead-in, a path pause and a fill ramp (planned into the
    // pass's powers, not its gantry options) are each refused, for a run and for Save RD file alike.
    for settings in [
        json!({"power_pct": 30, "speed_mm_s": 50, "options": {"lead_in": {"style": "line", "length_mm": 2.0, "angle_deg": 90.0}}}),
        json!({"power_pct": 30, "speed_mm_s": 50, "options": {"start_pause_ms": 5.0}}),
        json!({"power_pct": 30, "speed_mm_s": 50, "mode": "fill", "options": {"ramp_length_mm": 2.0, "ramp_min_power_pct": 5.0}}),
    ] {
        exec(&app, "layer.update", json!({"id": "layer-1", "settings": settings})).await;
        for r in [req("POST", "/api/runs", None), req("GET", "/api/export/rd", None)] {
            let (s, v) = send(&app, r).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{settings}: {v}");
            assert!(v["error"].as_str().unwrap_or_default().contains("GRBL layer options"), "{settings}: {v}");
        }
    }
    assert_eq!(sim(&st).report().burns, 0);
}

#[tokio::test]
async fn send_list_start_through_approval_and_delete_files() {
    let (app, st, _) = ruida_engine(ApprovalMode::Pin).await;
    let ctl = sim(&st);
    rect(&app).await;
    // Send is UI only, names are checked, and it stores without firing.
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/files/send", Some(json!({"name": "RECT"}))))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "bad\u{e9}"})))).await.0, StatusCode::BAD_REQUEST);
    let (s, v) = send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "RECT"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["file"], json!({"index": 1, "name": "RECT"}));
    assert_eq!(ctl.report().burns, 0, "stored, not run");
    let (s, v) = send(&app, req("GET", "/api/machine/files", None)).await;
    assert_eq!((s, &v["files"]), (StatusCode::OK, &json!([{"index": 1, "name": "RECT"}])), "{v}");

    // Starting it is a run: MCP cannot ask for one, a stale name is refused at approval, and only the PIN starts it.
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/files/1/run", Some(json!({"name": "RECT"}))))).await.0, StatusCode::FORBIDDEN);
    let mut agent = req("POST", "/api/machine/files/1/run", Some(json!({"name": "RECT", "origin": "agent"})));
    agent.headers_mut().remove("mcp-session-id");
    assert_eq!(send(&app, agent).await.0, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "OTHER"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let stale = v["run"]["id"].as_str().unwrap().to_owned();
    let (s, v) = approve(&app, &stale).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &stale).await;
    assert_eq!(run.state, RunState::Failed, "the name no longer matches");
    assert!(run.error.unwrap_or_default().contains("read the file list again"));
    assert_eq!(ctl.report().burns, 0);

    let (s, v) = send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "RECT"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((&v["run"]["driver"], &v["run"]["state"]), (&json!("ruida"), &json!("pending")), "{v}");
    let id = v["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(ctl.report().burns, 0, "a pending run sends nothing");
    assert_eq!(send(&app, mcp(req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN}))))).await.0, StatusCode::FORBIDDEN);
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert_eq!((ctl.report().burns, ctl.report().jobs_finished), (4, 1));

    // Delete needs the name too, and is UI only.
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/files/1/delete", Some(json!({"name": "RECT"}))))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/machine/files/1/delete", Some(json!({"name": "X"})))).await.0, StatusCode::CONFLICT);
    let (s, v) = send(&app, req("POST", "/api/machine/files/1/delete", Some(json!({"name": "RECT"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(ctl.report().files.is_empty());
}

#[tokio::test]
async fn a_long_job_is_sent_whole_and_a_send_cancels_pending_stored_runs() {
    let (app, st, _) = ruida_engine(ApprovalMode::Pin).await;
    let ctl = sim(&st);
    for i in 0..100 {
        let (x, y) = (f64::from(i % 10) * 25.0 - 120.0, f64::from(i / 10) * 25.0 - 120.0);
        exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [x, y]})).await;
    }
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 50}})).await;
    let datagrams = ctl.report().datagrams;
    let (s, v) = send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "GRID"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(ctl.report().datagrams - datagrams > 4, "the job took several datagrams");
    assert_eq!(ctl.report().burns, 0);
    // A pending start of file 1, then another Send: the list changed, so the pending run is cancelled.
    let (_, v) = send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "GRID"})))).await;
    let stale = v["run"]["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "GRID"})))).await;
    assert_eq!((s, &v["file"]["index"]), (StatusCode::OK, &json!(2)), "{v}");
    assert_eq!(st.runs().get(&stale).unwrap().state, RunState::Cancelled);
    assert_eq!(approve(&app, &stale).await.0, StatusCode::CONFLICT);
    assert_eq!(ctl.report().burns, 0);
    // Starting the second copy burns all 100 squares.
    let (_, v) = send(&app, req("POST", "/api/machine/files/2/run", Some(json!({"name": "GRID"})))).await;
    let id = v["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    assert_eq!((ctl.report().burns, ctl.report().jobs_finished), (400, 1));
}

#[tokio::test]
async fn stored_files_obey_require_framing_the_checklist_and_stop() {
    let (app, st, _) = ruida_engine(ApprovalMode::Pin).await;
    let ctl = sim(&st);
    rect(&app).await;
    send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "A"})))).await;
    // A pending stored-file run, then Require Framing and the checklist turned on.
    let (_, v) = send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "A"})))).await;
    let id = v["run"]["id"].as_str().unwrap().to_owned();
    let (s, _) = send(&app, req("PUT", "/api/prefs", Some(json!({"require_framing": true})))).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::CONFLICT, "Require Framing: {v}");
    assert!(v["error"].as_str().unwrap().contains("cannot be framed"), "{v}");
    // Framing the document does not count for a stored file.
    send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "A"})))).await.0, StatusCode::CONFLICT);
    let prefs = json!({"require_framing": false, "checklist": {"enabled": true, "items": ["Lid closed"]}});
    assert_eq!(send(&app, req("PUT", "/api/prefs", Some(prefs))).await.0, StatusCode::OK);
    assert_eq!(approve(&app, &id).await.0, StatusCode::CONFLICT, "the checklist is not ticked");
    // Stop cancels the pending run; nothing fired.
    assert_eq!(send(&app, req("POST", "/api/machine/stop", None)).await.0, StatusCode::OK);
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Cancelled);
    assert_eq!(ctl.report().burns, 0);
    assert!(ctl.with(|s| s.log.contains(&ruida::command::Command::StopProcess)), "Stop reached the controller");
}

#[tokio::test]
async fn click_mode_starts_a_stored_file_from_the_app_only_and_never_by_foot() {
    let (app, st, _) = ruida_engine(ApprovalMode::Click).await;
    assert!(crate::approve::foot_switch_port(&st).is_none(), "no foot switch on a gantry");
    rect(&app).await;
    send(&app, req("POST", "/api/machine/files/send", Some(json!({"name": "A"})))).await;
    let (_, v) = send(&app, req("POST", "/api/machine/files/1/run", Some(json!({"name": "A"})))).await;
    let id = v["run"]["id"].as_str().unwrap().to_owned();
    // Without the browser's headers (a script) it is refused.
    assert_eq!(send(&app, req("POST", &format!("/api/runs/{id}/approve"), None)).await.0, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, browser(req("POST", &format!("/api/runs/{id}/approve"), None))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    assert_eq!(sim(&st).report().burns, 4);
}

async fn raw(app: &Router, uri: &str, body: Vec<u8>) -> (StatusCode, Value) {
    let r = Request::builder().method("POST").uri(uri).header("host", "localhost:7878").body(Body::from(body)).unwrap();
    send(app, r).await
}

#[tokio::test]
async fn save_rd_and_import_it_back() {
    let (app, st, galvo) = ruida_engine(ApprovalMode::Pin).await;
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20, "center": [30, -50]})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 50}})).await;
    let before = st.document().objects[0].bounds();
    let datagrams = sim(&st).report().datagrams;
    let res = app.clone().oneshot(req("GET", "/api/export/rd", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers()["content-disposition"].to_str().unwrap().ends_with(".rd\""));
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    let read = ruida::read_rd(&bytes, None).unwrap();
    assert_eq!(read.file_sum_ok, Some(true));
    assert_eq!(read.job.layers.len(), 1);
    assert_eq!(sim(&st).report().datagrams, datagrams, "the export sent nothing");

    // Import with the active device's origin and bed, keeping the position: the same rectangle on a new layer.
    let (s, v) = raw(&app, "/api/document/import?format=rd&keep_position=true&name=back.rd", bytes.clone()).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let doc = st.document();
    let layer = doc.layers.last().unwrap();
    assert_eq!(layer.settings.speed_mm_s, 50.0);
    assert!((layer.settings.power_pct - 30.0).abs() < 0.01);
    let imported = doc.objects.iter().filter(|o| o.layer == layer.id).map(doc::Object::bounds).reduce(|a, b| a.union(b)).unwrap();
    for (a, b) in [(imported.x0, before.x0), (imported.y0, before.y0), (imported.x1, before.x1), (imported.y1, before.y1)] {
        assert!((a - b).abs() < 0.002, "{imported:?} vs {before:?}");
    }

    // Without a Ruida device: no RD export, and the import counts from the default corner.
    let (s, _) = send(&app, req("POST", &format!("/api/devices/{galvo}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(send(&app, req("GET", "/api/export/rd", None)).await.0, StatusCode::CONFLICT);
    let (s, v) = raw(&app, "/api/document/import?format=rd", bytes).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["driver"], "lmc", "{m}");
    assert!(st.machine.sim_counters().unwrap().ruida_report().is_none());
}
