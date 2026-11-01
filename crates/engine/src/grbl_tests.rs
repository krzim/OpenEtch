//! A GRBL gantry through the whole engine on the GRBL simulator: device creation and activation, framing, pending
//! runs and approval, the console, jogging and homing, Stop with a full buffer, and switching back to the galvo.
//! Unverified on hardware (no gantry).

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::grbl::{FireKind, SimGrblHandle};
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::{AppState, api};

const PIN: &str = "975310";

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

/// An engine on the simulators with a GRBL device (400 x 300 bed, front-left origin) made and activated through the
/// API; returns the app, the state, the GRBL device id and the built-in galvo's id.
async fn grbl_engine(homing: bool) -> (Router, Arc<AppState>, String, String) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.unwrap();
    let dir = pin::temp_dir("grbl-engine");
    pin::set_pin(&dir, PIN).unwrap();
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, crate::tests::test_projects());
    let app = api::router(st.clone(), None);
    let galvo = st.active_device().id;
    let profile = json!({"port": "COM9", "work_area_mm": [400.0, 300.0], "origin": "bottom_left", "homing": homing, "max_feed_mm_min": 6000.0});
    // Agents cannot add devices.
    let mut r = req("POST", "/api/devices/grbl", Some(json!({"display_name": "Diode", "profile": profile})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    let (s, v) = send(&app, req("POST", "/api/devices/grbl", Some(json!({"display_name": "Diode", "profile": profile})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((&v["device"]["kind"], &v["device"]["controllable"]), (&json!("grbl"), &json!(true)), "{v}");
    let id = v["device"]["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", &format!("/api/devices/{id}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    (app, st, id, galvo)
}

fn sim(st: &AppState) -> SimGrblHandle {
    st.machine.sim_counters().unwrap().grbl_handle().expect("a simulated GRBL board")
}

async fn approve(app: &Router, id: &str) -> (StatusCode, Value) {
    send(app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN})))).await
}

#[tokio::test]
async fn a_grbl_device_frames_dark_and_runs_only_when_approved() {
    let (app, st, _, _) = grbl_engine(false).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["driver"], &m["link"], &m["connected"]), (&json!("grbl"), &json!("sim"), &json!(true)), "{m}");
    assert_eq!(m["work_area_mm"], json!([400.0, 300.0]));
    assert_eq!(m["grbl"]["board"]["laser_mode"], true, "{m}");
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 25}})).await;

    // Framing: the head moves with the laser off.
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false, "pointer": "focus"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for _ in 0..400 {
        if !st.machine.is_framing() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let r = board.report();
    assert!(r.fires.is_empty(), "framing fired: {:?}", r.fires);
    assert!(r.lines.iter().any(|l| l.starts_with("$J=G90G21X180Y140")), "the rectangle's corner on the bed: {:?}", r.lines);

    // A pending run sends nothing; the program is checked G-code.
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["driver"], "grbl");
    assert!(v["run"]["packet_count"].as_u64().unwrap() >= 6, "{v}");
    assert!(v["run"]["summary"].as_str().unwrap().contains("G-code lines"), "{v}");
    let lines_before = board.report().lines_received;
    // Approval needs the PIN (and never comes from MCP).
    assert_eq!(send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": "000000"})))).await.0, StatusCode::FORBIDDEN);
    let mut r = req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert_eq!(board.report().lines_received, lines_before, "nothing was sent for the pending run");
    assert!(board.report().fires.is_empty());

    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = board.report();
    assert_eq!(r.cuts(), 4, "the rectangle's four sides: {:?}", r.fires);
    assert!(r.fires.iter().all(|f| f.kind == FireKind::Cut && f.s == 300.0), "30 % of $30=1000: {:?}", r.fires);
    assert!(!r.overflowed && r.max_rx_bytes <= machine::grbl::RX_BUFFER);
    assert_eq!(st.job.count(&st.active_device().id), 1);
}

#[tokio::test]
async fn the_console_refuses_what_could_fire_and_is_ui_only() {
    let (app, st, _, _) = grbl_engine(false).await;
    let board = sim(&st);
    let before = board.report().lines_received;
    for bad in ["M3 S1000", "M4", "S255", "G1 X10 F100 S1", "$N0=M3 S100", "$RST=*", "!", "~"] {
        let (s, v) = send(&app, req("POST", "/api/machine/console", Some(json!({"line": bad})))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {v}");
    }
    let mut r = req("POST", "/api/machine/console", Some(json!({"line": "$$"})));
    r.headers_mut().insert("x-galvo-agent", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert_eq!(board.report().lines_received, before, "nothing refused was sent");
    let (s, v) = send(&app, req("POST", "/api/machine/console", Some(json!({"line": "$$"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    assert!(v["lines"].as_array().unwrap().iter().any(|l| l == "$32=1"), "{v}");
    let (s, v) = send(&app, req("POST", "/api/machine/console", Some(json!({"line": "G0 X20 Y20"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    // Jogging moves the head, dark; homing is off for this device.
    let (s, v) = send(&app, req("POST", "/api/machine/jog", Some(json!({"dx": 5.0, "dy": -2.5, "feed": 1200})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _) = send(&app, req("POST", "/api/machine/jog", Some(json!({"x": 300.0, "y": 0.0})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "off the bed");
    assert_eq!(send(&app, req("POST", "/api/machine/home", None)).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(send(&app, req("POST", "/api/machine/jog/cancel", None)).await.0, StatusCode::OK);
    let (_, g) = send(&app, req("GET", "/api/machine/grbl", None)).await;
    assert_eq!(g["profile"]["work_area_mm"], json!([400.0, 300.0]), "{g}");
    board.run_blocks(10);
    let r = board.report();
    assert!(r.fires.is_empty(), "{:?}", r.fires);
    assert!(r.lines.iter().any(|l| l == "$J=G91G21X5Y-2.5F1200"), "{:?}", r.lines);
}

#[tokio::test]
async fn homing_unlocks_a_board_that_starts_locked() {
    let (app, st, _, _) = grbl_engine(true).await;
    exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "locked until homed: {v}");
    let (s, v) = send(&app, req("POST", "/api/machine/home", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(sim(&st).report().fires.is_empty());
}

#[tokio::test]
async fn stop_halts_a_grbl_run_with_a_full_buffer() {
    let (app, st, _, _) = grbl_engine(false).await;
    // Many short cuts: more lines than GRBL's buffers hold.
    exec(&app, "shape.rect", json!({"width_mm": 100, "height_mm": 100})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"mode": "fill", "power_pct": 50, "speed_mm_s": 50,
        "hatch": {"interval_mm": 0.5, "angle_deg": 0.0}}})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let board = sim(&st);
    board.stall(true);
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    for _ in 0..400 {
        let r = board.report();
        if r.planner_blocks == machine::grbl::PLANNER_BLOCKS && r.rx_bytes > 90 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let at_stop = board.report();
    assert!(at_stop.rx_bytes > 90, "the buffer is full: {}", at_stop.rx_bytes);
    let (s, v) = send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Aborted, "{:?}", run.error);
    let r = board.report();
    let stop: Vec<_> = r.realtime.iter().skip_while(|e| e.byte != b'!').collect();
    assert_eq!(stop[0].byte, b'!');
    assert_eq!(stop[1].byte, 0x18);
    assert!(stop[0].rx_bytes > 90, "the stop did not wait behind the buffered lines");
    assert_eq!(r.blocks_executed, at_stop.blocks_executed, "nothing moved (or fired) after the stop");
    assert!(r.fires.is_empty() && !r.lit_now);
    board.stall(false);
}

#[tokio::test]
async fn switching_back_to_the_galvo_restores_the_lmc_driver() {
    let (app, st, grbl, galvo) = grbl_engine(false).await;
    exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"speed_mm_s": 50}})).await;
    let (_, v) = send(&app, req("POST", "/api/runs", None)).await;
    let stale = v["id"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", &format!("/api/devices/{galvo}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(st.runs().get(&stale).unwrap().state, RunState::Cancelled, "a GRBL run never runs on the galvo");
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!((&m["driver"], &m["link"]), (&json!("lmc"), &json!("sim")), "{m}");
    assert!(m.get("grbl").is_none());
    let counters = st.machine.sim_counters().unwrap().clone();
    let (_, v) = send(&app, req("POST", "/api/runs", None)).await;
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["driver"], "lmc");
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    assert!(counters.emitting_packets() > 0);
    assert_eq!(send(&app, req("POST", "/api/machine/console", Some(json!({"line": "$$"})))).await.0, StatusCode::CONFLICT);
    // And to the gantry again: a fresh simulated board.
    let (s, _) = send(&app, req("POST", &format!("/api/devices/{grbl}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(sim(&st).report().lines_received, 3, "init's queries only");
}

#[tokio::test]
async fn grbl_profiles_are_edited_and_checked() {
    let (app, st, id, _) = grbl_engine(false).await;
    let (s, v) = send(&app, req("PUT", &format!("/api/devices/{id}"), Some(json!({"profile": {"work_area_mm": [200.0, 150.0], "origin": "top_left"}})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let d = st.devices().get(&id).cloned().unwrap();
    assert_eq!((d.workspace_mm, d.origin), ([200.0, 150.0], machine::Origin::TopLeft));
    assert_eq!(st.machine.grbl_profile().unwrap().work_area_mm, [200.0, 150.0], "the active machine was set up again");
    for bad in [json!({"s_max": 0}), json!({"origin": "center"}), json!({"max_power_pct": 150}), json!({"nope": 1})] {
        let (s, v) = send(&app, req("PUT", &format!("/api/devices/{id}"), Some(json!({"profile": bad})))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }
    let (_, v) = send(&app, req("GET", &format!("/api/devices/{id}"), None)).await;
    assert_eq!(v["grbl"]["origin"], "top_left", "{v}");
    // Options a GRBL run does not have yet are refused when the run is requested.
    exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"start_input": true})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    // A speed above the feed limit is refused, never clamped.
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"speed_mm_s": 500}})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v["error"].as_str().unwrap().contains("mm/min"), "{v}");
}
