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
use crate::pin::{self, ApprovalConfig, ApprovalMode, PinStore};
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
    grbl_engine_in(homing, ApprovalMode::Pin).await
}

/// [`grbl_engine`] in the approval mode `mode` (`click`: no PIN file, Start in the app in a browser).
async fn grbl_engine_in(homing: bool, mode: ApprovalMode) -> (Router, Arc<AppState>, String, String) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.unwrap();
    let dir = pin::temp_dir("grbl-engine");
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
    assert_eq!(m["pulse_width_enabled"], json!(true), "the B6 is a MOPA with Q-pulse on: {m}");
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

/// The galvo's input port features (main's foot switch sampling, `GET /api/machine/inputs`) are LMC-only: refused on a
/// gantry, and back on the galvo the input reads a streaming frame takes reach the engine's handle (the controller
/// rebuilt after the GRBL driver shares it).
#[tokio::test]
async fn input_port_features_are_galvo_only_and_survive_a_driver_switch() {
    let (app, st, _grbl, galvo) = grbl_engine(false).await;
    let (s, v) = send(&app, req("GET", "/api/machine/inputs", None)).await;
    assert!(s.is_client_error(), "a gantry has no input port: {s} {v}");
    assert_eq!(sim(&st).report().lines_received, 3, "refused before the board is touched: init's queries only");
    assert!(crate::approve::foot_switch_port(&st).is_none());
    let (s, v) = send(&app, req("POST", &format!("/api/devices/{galvo}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, req("GET", "/api/machine/inputs", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    exec(&app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    st.machine.sim_counters().unwrap().set_inputs(1 << 14);
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let samples = st.machine.take_input_samples();
    assert!(!samples.is_empty() && samples.iter().all(|&(_, b)| b == 1 << 14), "{samples:?}");
    st.machine.request_stop();
    assert!(!st.machine.has_input_samples(), "a stop discards them");
}

/// One-click Start (main's `--approval click`) on a GRBL gantry: the same gates as on the galvo. MCP clients and
/// requests that do not come from the app in a browser are refused, an open safety door refuses it with nothing sent,
/// and a browser Start runs the G-code. The foot switch cycle stays off (no input port).
#[tokio::test]
async fn one_click_start_on_a_gantry_keeps_the_gates() {
    let (app, st, _, _) = grbl_engine_in(false, ApprovalMode::Click).await;
    assert!(crate::approve::foot_switch_port(&st).is_none());
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 25}})).await;
    let start = || crate::approve::tests::browser(req("POST", "/api/job/start", None));
    let mut r = start();
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN, "MCP never starts a run");
    let (s, v) = send(&app, req("POST", "/api/job/start", None)).await;
    assert_eq!((s, &v["code"]), (StatusCode::FORBIDDEN, &json!("browser_required")), "{v}");
    board.set_door(true);
    let (s, v) = send(&app, start()).await;
    assert_eq!(s, StatusCode::CONFLICT, "an open door refuses Start: {v}");
    assert!(v["error"].as_str().unwrap_or_default().contains("door"), "{v}");
    board.set_door(false);
    assert!(board.report().fires.is_empty());
    assert!(st.runs().views(st.revision()).iter().all(|r| r.state.is_final()), "nothing is left waiting");

    let (s, v) = send(&app, start()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["run"]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["run"]["driver"], "grbl", "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert_eq!(board.report().cuts(), 4, "{:?}", board.report().fires);
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

/// Save G-code: the document as a `.gcode` file for the active GRBL gantry, exactly the lines a run would stream,
/// sending nothing; a galvo refuses it. The file reads back through the G-code import (`format=gcode`).
#[tokio::test]
async fn gcode_exports_without_sending_and_imports_back() {
    let (app, st, _, galvo) = grbl_engine(false).await;
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 25}})).await;
    let before = board.report().lines_received;
    let res = app.clone().oneshot(req("GET", "/api/export/gcode", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let disposition = res.headers()["content-disposition"].to_str().unwrap().to_owned();
    assert!(disposition.contains(".gcode"), "{disposition}");
    let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.starts_with("; OpenEtch G-code for Diode (400 x 300 mm bed)\n"), "{text}");
    assert!(text.contains("\nG90 G21\nM4 S0\nG0 X180 Y140\n"), "{text}");
    assert!(text.contains(" S300 F1500\n") && text.ends_with("M5\n"), "{text}");
    assert_eq!(board.report().lines_received, before, "export sends nothing to the machine");
    assert!(board.report().fires.is_empty());
    // Unknown query parameters and unknown ids are refused.
    assert_eq!(send(&app, req("GET", "/api/export/gcode?pin=1", None)).await.0, StatusCode::BAD_REQUEST);
    assert!(!send(&app, req("GET", "/api/export/gcode?ids=nope", None)).await.0.is_success());

    // Import it back as a new project: one path set at S300, the rectangle's size.
    let r = Request::builder()
        .method("POST")
        .uri("/api/document/import?format=gcode&mode=replace&name=rect.gcode&keep_position=true")
        .header("host", "localhost:7878")
        .body(Body::from(text))
        .unwrap();
    let (s, v) = send(&app, r).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["result"]["size_mm"], json!([40.0, 20.0]), "{v}");

    // A galvo has no G-code.
    let (s, _) = send(&app, req("POST", &format!("/api/devices/{galvo}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(send(&app, req("GET", "/api/export/gcode", None)).await.0, StatusCode::CONFLICT);
}

/// Framing a gantry moves its head, so agents cannot start it (MCP tool or MCP headers); stopping still works.
#[tokio::test]
async fn agents_cannot_frame_a_gantry() {
    let (app, st, _, _) = grbl_engine(false).await;
    exec(&app, "shape.rect", json!({"width_mm": 40, "height_mm": 20})).await;
    let before = sim(&st).report().lines_received;
    let r = crate::mcp::GalvoMcp::new(st.clone()).call("frame_start", serde_json::Map::new()).await;
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["isError"], true, "{v}");
    assert!(v.to_string().contains("only a person"), "{v}");
    let mut r = req("POST", "/api/machine/frame", Some(json!({"continuous": false})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    // The red-dot jog is a one-point frame: on a gantry it moves the head, so the same rule.
    let mut r = req("POST", "/api/machine/pointer", Some(json!({"x": 10, "y": 10})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert_eq!(sim(&st).report().lines_received, before, "nothing was sent");
    assert!(!st.machine.is_framing());
    let r = crate::mcp::GalvoMcp::new(st.clone()).call("machine_stop", serde_json::Map::new()).await;
    assert_ne!(serde_json::to_value(&r).unwrap()["isError"], true);
}

/// The GRBL layer options (constant power, air assist, overscan) through the whole engine: set like any layer setting
/// (an agent may edit them: an edit fires nothing), they shape the exported G-code and the run's, and the laser and
/// the air come on only inside the approved run; Stop or the run's end leaves both off.
#[tokio::test]
async fn grbl_layer_options_shape_the_gcode_and_fire_only_in_the_armed_run() {
    let (app, st, _, _) = grbl_engine(false).await;
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 4})).await;
    // As MCP would send it: an edit, never a burn.
    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "layer.update", "params": {"id": "layer-1", "settings": {
        "mode": "fill", "power_pct": 40, "speed_mm_s": 20, "hatch": {"interval_mm": 1.0, "angle_deg": 0.0},
        "options": {"constant_power": true, "air_assist": true, "overscan_mm": 3.0}}}})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "layer.update", "params": {"id": "layer-1", "settings": {
        "options": {"overscan_mm": 80.0}}}})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "overscan above 50 mm is refused: {v}");

    let res = app.clone().oneshot(req("GET", "/api/export/gcode", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.contains("layer options: M3 constant power on some layers, air assist M8/M9, overscan"), "{text}");
    let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with(';')).collect();
    assert_eq!(lines[..2], ["G90 G21", "M3 S0"], "{text}");
    assert!(lines[2].starts_with("G0 ") && lines[3] == "M8", "air on after the first travel, before the first cut: {text}");
    assert_eq!(lines[lines.len() - 2..], ["M5", "M9"], "{text}");
    // Every scan line: a run-in at S0, the cut at S400, a run-out at S0, 3 mm each side.
    assert!(lines.iter().any(|l| l.starts_with("G0 X187 ")), "run-in 3 mm before the hatch line at x = 190: {text}");
    assert!(lines.iter().any(|l| l.starts_with("G1 X210 ") && l.contains("S400")), "{text}");
    assert!(lines.iter().any(|l| l.starts_with("G1 X213 ") && l.contains("S0")), "{text}");
    assert!(board.report().fires.is_empty() && !board.report().air_now, "the export sent nothing");

    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert!(board.report().fires.is_empty() && !board.report().air_now, "a pending run sends nothing");
    // MCP cannot approve it.
    let mut r = req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert!(board.report().fires.is_empty());
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = board.report();
    assert!(r.cuts() >= 4, "{:?}", r.fires);
    assert!(r.fires.iter().all(|f| f.kind == FireKind::Cut && f.s == 400.0 && f.air), "every cut at S400 with air: {:?}", r.fires);
    // The overscan moves never fired: every cut lies within the rectangle (x 190..210 on the bed).
    assert!(r.fires.iter().all(|f| f.from[0].min(f.to[0]) >= 190.0 - 1e-6 && f.from[0].max(f.to[0]) <= 210.0 + 1e-6), "{:?}", r.fires);
    assert!(r.lines.iter().any(|l| l == "M8") && r.lines.iter().any(|l| l == "M3 S0"), "{:?}", r.lines);
    assert!(!r.air_now && !r.lit_now, "air and laser off after the run");
}

/// The Move window's position readout: `?refresh=true` asks an idle board for a status report and the answer carries
/// the head's position in machine and canvas (field) coordinates. A refresh never moves or fires.
#[tokio::test]
async fn the_move_window_reads_the_heads_position() {
    let (app, st, _, _) = grbl_engine(false).await;
    let board = sim(&st);
    let (s, v) = send(&app, req("POST", "/api/machine/jog", Some(json!({"x": -150.0, "y": 100.0, "feed": 3000})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    board.run_blocks(5);
    let (s, g) = send(&app, req("GET", "/api/machine/grbl?refresh=true", None)).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["position"]["machine"], json!([50.0, 250.0]), "{g}");
    assert_eq!(g["position"]["field"], json!([-150.0, 100.0]), "{g}");
    assert_eq!(send(&app, req("GET", "/api/machine/grbl?nope=1", None)).await.0, StatusCode::BAD_REQUEST);
    let r = board.report();
    assert!(r.fires.is_empty(), "{:?}", r.fires);
}

/// Console macros are saved with the device in the engine's preferences (UI only), and a macro that could turn the
/// laser on is refused when it is saved, before it could ever reach the console.
#[tokio::test]
async fn console_macros_are_saved_per_device_and_checked_like_console_lines() {
    let (app, st, id, _) = grbl_engine(false).await;
    let put = |v: Value| req("PUT", "/api/prefs", Some(v));
    let (s, v) = send(&app, put(json!({"console_macros": [{"name": "Fire", "lines": ["M8", "M3 S1000"]}]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("M3"), "{v}");
    let mut r = put(json!({"console_macros": []}));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN, "agents cannot write macros");
    let macros = json!([{"name": "Air on", "lines": ["M8"]}, {"name": "Park", "lines": ["$J=G90 G21 X0 Y0 F3000", "M9"]}]);
    let (s, v) = send(&app, put(json!({"console_macros": macros}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = send(&app, req("GET", "/api/prefs", None)).await;
    assert_eq!((&v["device"], &v["device_prefs"]["console_macros"]), (&json!(id), &macros), "{v}");
    // Running one is sending its lines through the console, one by one.
    for line in ["$J=G90 G21 X0 Y0 F3000", "M9"] {
        let (s, v) = send(&app, req("POST", "/api/machine/console", Some(json!({"line": line})))).await;
        assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{line}: {v}");
    }
    assert!(sim(&st).report().fires.is_empty());
}

/// Air assist left on from the console (`M8` is a console line) does not leak into a job's passes without air assist:
/// a job that uses air sets it for its first pass either way, and the run ends with the air off. The laser fires only
/// inside the approved run throughout.
#[tokio::test]
async fn console_air_does_not_leak_into_passes_without_air_assist() {
    let (app, st, _, _) = grbl_engine(false).await;
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 4})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 30, "speed_mm_s": 20, "options": {"air_assist": false}}})).await;
    // A second pass (sub-layer) of the same layer with air assist.
    exec(&app, "layer.sub_add", json!({"layer": "layer-1", "settings": {"power_pct": 20, "options": {"air_assist": true}}})).await;
    let (s, v) = send(&app, req("POST", "/api/machine/console", Some(json!({"line": "M8"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{v}");
    assert!(board.report().air_now && board.report().fires.is_empty(), "the console switched the air, nothing fired");

    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = board.report();
    assert!(r.fires.iter().all(|f| f.kind == FireKind::Cut), "{:?}", r.fires);
    let main: Vec<_> = r.fires.iter().filter(|f| f.s == 300.0).collect();
    let sub: Vec<_> = r.fires.iter().filter(|f| f.s == 200.0).collect();
    assert!(!main.is_empty() && !sub.is_empty() && main.len() + sub.len() == r.fires.len(), "{:?}", r.fires);
    assert!(main.iter().all(|f| !f.air), "the pass without air assist ran without air: {:?}", r.fires);
    assert!(sub.iter().all(|f| f.air), "the pass with air assist ran with air: {:?}", r.fires);
    assert!(!r.air_now && !r.lit_now, "air and laser off after the run");
}

/// Ramp, lead-in/out and path pauses through the whole engine on the simulator: set as layer options (an edit fires
/// nothing), they shape the run's G-code, every lit move is a cut inside the approved run (the pauses dwell with the
/// laser off), and the ramp steps the power down towards the edges. Unverified on hardware (no gantry).
#[tokio::test]
async fn ramps_leads_and_path_pauses_fire_only_as_cuts_in_the_armed_run() {
    let (app, st, _, _) = grbl_engine(false).await;
    let board = sim(&st);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 10})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 40, "speed_mm_s": 20, "options": {
        "lead_in": {"style": "arc", "length_mm": 3.0, "angle_deg": 90.0},
        "lead_out": {"length_mm": 2.0, "angle_deg": 45.0},
        "start_pause_ms": 20.0, "end_pause_ms": 10.0}}})).await;
    let (s, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "layer.update", "params": {"id": "layer-1", "settings": {
        "options": {"lead_in": {"length_mm": 2.0, "angle_deg": 120.0}}}}})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(false)), "a lead angle beyond 90 degrees is refused: {v}");
    let res = app.clone().oneshot(req("GET", "/api/export/gcode", None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let text = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(text.contains("lead-in, lead-out, start/end pauses"), "{text}");
    assert!(text.contains("G4 P0.02") && text.contains("G4 P0.01"), "{text}");
    assert!(board.report().fires.is_empty());

    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = board.report();
    assert!(r.fires.iter().all(|f| f.kind == FireKind::Cut && f.s == 400.0), "no dwell ever fired: {:?}", r.fires);
    // The leads cut outside the rectangle (x 190..210, y 145..155 on the bed); the sides themselves are cut too.
    let outside = |p: [f64; 2]| p[0] < 190.0 - 1e-6 || p[0] > 210.0 + 1e-6 || p[1] < 145.0 - 1e-6 || p[1] > 155.0 + 1e-6;
    assert!(r.fires.iter().any(|f| outside(f.from)), "the lead-in starts outside: {:?}", r.fires);
    assert!(r.fires.iter().any(|f| outside(f.to)), "the lead-out ends outside: {:?}", r.fires);
    assert!(!r.lit_now);

    // A fill with a 2 mm ramp on its outer edge: the power steps from 10 % at the edge to 40 %.
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"mode": "fill", "hatch": {"interval_mm": 1.0, "angle_deg": 0.0},
        "options": {"ramp_length_mm": 2.0, "ramp_outer_edge": true, "ramp_min_power_pct": 10.0, "overscan_mm": 2.0}}})).await;
    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let before = board.report().fires.len();
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    let r = board.report();
    let fires = &r.fires[before..];
    assert!(fires.iter().all(|f| f.kind == FireKind::Cut && (100.0..=400.0).contains(&f.s)), "{fires:?}");
    assert!(fires.iter().any(|f| f.s == 400.0) && fires.iter().any(|f| f.s < 200.0), "stepped power: {fires:?}");
    // Overscan stays dark: every lit move lies within the rectangle.
    assert!(fires.iter().all(|f| !outside(f.from) && !outside(f.to)), "{fires:?}");
    assert!(!r.lit_now);
}

/// Saved Move positions are stored with the device in the engine's preferences (UI only: an agent cannot save them),
/// must lie on the bed, need a GRBL device, and going to one is an ordinary laser-off jog.
#[tokio::test]
async fn saved_move_positions_are_ui_only_and_on_the_bed() {
    let (app, st, id, galvo) = grbl_engine(false).await;
    let board = sim(&st);
    let put = |v: Value| req("PUT", "/api/prefs", Some(v));
    let spots = json!([{"name": "Jig corner", "x_mm": 12.5, "y_mm": 30.0}, {"name": "Park", "x_mm": 400.0, "y_mm": 300.0}]);
    let mut r = put(json!({"move_positions": spots}));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN, "agents cannot save positions");
    let (s, v) = send(&app, put(json!({"move_positions": [{"name": "Off", "x_mm": 401.0, "y_mm": 0.0}]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains("off the 400 x 300 mm bed"), "{v}");
    let (s, v) = send(&app, put(json!({"move_positions": [{"name": " ", "x_mm": 1.0, "y_mm": 1.0}]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = send(&app, put(json!({"move_positions": spots}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = send(&app, req("GET", "/api/prefs", None)).await;
    assert_eq!((&v["device"], &v["device_prefs"]["move_positions"]), (&json!(id), &spots), "{v}");
    // Going to one: the Move window jogs to its field position (machine 12.5, 30 on a front-left origin).
    let (s, v) = send(&app, req("POST", "/api/machine/jog", Some(json!({"x": -187.5, "y": -120.0, "feed": 3000})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    board.run_blocks(5);
    let (_, g) = send(&app, req("GET", "/api/machine/grbl?refresh=true", None)).await;
    assert_eq!(g["position"]["machine"], json!([12.5, 30.0]), "{g}");
    assert!(board.report().fires.is_empty());
    // A galvo has no Move positions.
    let (s, _) = send(&app, req("POST", &format!("/api/devices/{galvo}/activate"), None)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(&app, put(json!({"move_positions": []}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
}
