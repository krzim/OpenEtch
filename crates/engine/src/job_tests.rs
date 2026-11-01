//! Route tests of job control, framing modes, calibration and device settings, on the simulated board.

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
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

async fn state_with(profile: MachineProfile) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("jobtests");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    st.job.open(dir.join(crate::job::COUNTERS_FILE));
    st
}

fn pedal_profile() -> MachineProfile {
    MachineProfile { start_mark_input_bit: Some(14), ..MachineProfile::generic_lmc_fiber() }
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

fn mcp(mut r: Request<Body>) -> Request<Body> {
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    r
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

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params})))).await;
    assert!(s == StatusCode::OK && v["ok"] == true, "{id}: {v}");
    v["result"].clone()
}

async fn rect(app: &Router, x: f64) -> String {
    let r = exec(app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    let id = r["created"][0].as_str().unwrap().to_owned();
    exec(app, "object.transform", json!({"ids": [id], "translate": [x, 0]})).await;
    id
}

async fn request(app: &Router, body: Value) -> String {
    let (s, v) = send(app, req("POST", "/api/runs", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn approve(app: &Router, id: &str) -> (StatusCode, Value) {
    send(app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": TEST_PIN})))).await
}

async fn wait_final(st: &AppState, id: &str) -> Run {
    for _ in 0..1000 {
        let run = st.runs().get(id).cloned().expect("run exists");
        if run.state.is_final() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run {id} did not finish: {:?}", st.runs().get(id).map(|r| r.state));
}

#[tokio::test]
async fn repeat_counts_and_stop_at() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let (_, job) = send(&app, req("GET", "/api/job", None)).await;
    assert_eq!((job["state"].as_str(), job["count"].as_u64()), (Some("idle"), Some(0)));
    let id = request(&app, json!({"repeat": 3})).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert!(m["pending_runs"][0]["summary"].as_str().unwrap().contains("3 times"), "{m}");
    let packets = st.machine.sim_counters().unwrap().packets();
    let per_run = st.runs().get(&id).unwrap().packet_count() as u64;
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 3), "{:?}", run.error);
    assert_eq!(st.machine.sim_counters().unwrap().packets() - packets, 3 * per_run);
    let (_, job) = send(&app, req("GET", "/api/job", None)).await;
    assert_eq!(job["count"], 3);
    // Stop At: approvals are refused once the count reaches it; a continuous run stops there.
    send(&app, req("PUT", "/api/job/settings", Some(json!({"stop_at": 3})))).await;
    let id = request(&app, json!({})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::CONFLICT);
    send(&app, req("PUT", "/api/job/settings", Some(json!({"stop_at": 5})))).await;
    let id = request(&app, json!({"continuous": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 2), "{:?}", run.error);
    let (_, job) = send(&app, req("POST", "/api/job/count/reset", None)).await;
    assert_eq!((job["count"].as_u64(), job["stop_at"].as_u64()), (Some(0), Some(5)));
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"repeat": 0})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"repeats": 2})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unknown options are refused");
}

#[tokio::test]
async fn start_input_starts_each_iteration_and_stop_wins() {
    let st = state_with(pedal_profile()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let id = request(&app, json!({"repeat": 2, "start_input": true})).await;
    let (s, v) = approve(&app, &id).await;
    assert_eq!((s, v["state"].as_str()), (StatusCode::OK, Some("awaiting_confirm")), "{v}");
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (_, job) = send(&app, req("GET", "/api/job", None)).await;
    assert_eq!(job["state"], "waiting_for_start_input", "{job}");
    assert_eq!(counters.emitting_packets(), 0, "nothing fires before the pedal");
    // Two presses: release-then-press each time.
    for n in 1..=2 {
        counters.set_inputs(1 << 14);
        for _ in 0..400 {
            if st.runs().get(&id).is_none_or(|r| r.completed >= n || r.state.is_final()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        counters.set_inputs(0);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 2), "{:?}", run.error);
    // A run waiting for the pedal is cancelled by stop.
    let id = request(&app, json!({"start_input": true})).await;
    approve(&app, &id).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    send(&app, req("POST", "/api/machine/stop", None)).await;
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Cancelled);
    // Without an input bit in the profile, asking for one is refused.
    let st2 = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app2 = api::router(st2.clone(), None);
    rect(&app2, 0.0).await;
    let (s, _) = send(&app2, req("POST", "/api/runs", Some(json!({"start_input": true})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// A pause pressed while no run streams (here: while the job waits for the pedal) is not lost when the pedal
/// starts the iteration: nothing is sent until resume, and a stop while held ends the job without firing.
#[tokio::test]
async fn pause_between_iterations_holds_the_next_one() {
    let st = state_with(pedal_profile()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let press = |down: bool| counters.set_inputs(if down { 1 << 14 } else { 0 });
    for stop_while_held in [false, true] {
        let id = request(&app, json!({"start_input": true})).await;
        assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let (_, v) = send(&app, req("POST", "/api/machine/pause", None)).await;
        assert_eq!(v["running"], true, "{v}");
        let before = counters.emitting_packets();
        press(true);
        tokio::time::sleep(Duration::from_millis(400)).await;
        press(false);
        let (_, job) = send(&app, req("GET", "/api/job", None)).await;
        assert_eq!(job["state"], "paused", "{job}");
        assert_eq!(counters.emitting_packets(), before, "nothing fires while paused");
        assert!(!st.runs().get(&id).unwrap().state.is_final());
        if stop_while_held {
            send(&app, req("POST", "/api/machine/stop", None)).await;
            let run = wait_final(&st, &id).await;
            assert_eq!((run.state, run.completed), (RunState::Cancelled, 0), "{:?}", run.error);
            assert_eq!(counters.emitting_packets(), before);
        } else {
            assert_eq!(send(&app, req("POST", "/api/machine/resume", None)).await.0, StatusCode::OK);
            let run = wait_final(&st, &id).await;
            assert_eq!((run.state, run.completed), (RunState::Finished, 1), "{:?}", run.error);
            assert!(counters.emitting_packets() > before);
        }
    }
    // A pause left over from an ended job does not hold the next approval.
    st.machine.pause();
    let id = request(&app, json!({})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
}

/// The active device's correction file is not changed under a job: refused before anything is stored.
#[tokio::test]
async fn correction_file_of_the_active_device_is_refused_while_a_job_holds_the_machine() {
    let st = state_with(pedal_profile()).await;
    let app = api::router(st.clone(), None);
    let builtin = st.active_device().id;
    let (_, v) = send(&app, req("POST", &format!("/api/devices/{builtin}/duplicate"), None)).await;
    let copy = v["device"]["id"].as_str().unwrap().to_owned();
    send(&app, req("POST", &format!("/api/devices/{copy}/activate"), None)).await;
    assert_eq!(st.active_device().id, copy);
    rect(&app, 0.0).await;
    let id = request(&app, json!({"start_input": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    let before = st.active_device().profile.unwrap().cor_file;
    let cor = include_bytes!("../../../fixtures/jcz150.cor");
    let put = Request::builder().method("PUT").uri(format!("/api/devices/{copy}/cor?name=new.cor")).header("host", "localhost:7878").body(Body::from(cor.to_vec())).unwrap();
    assert_eq!(send(&app, put).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&app, req("DELETE", &format!("/api/devices/{copy}/cor"), None)).await.0, StatusCode::CONFLICT);
    assert_eq!(st.devices().get(&copy).unwrap().profile.as_ref().unwrap().cor_file, before, "nothing was stored");
    send(&app, req("POST", "/api/machine/stop", None)).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Cancelled);
}

/// The preview is planned with the next run's index, so an auto-rotating fill previews at the angle it will be
/// marked at (and Start Here segment numbers match the run).
#[tokio::test]
async fn preview_follows_the_job_counter_for_auto_rotate() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"mode": "fill", "hatch": {"interval_mm": 1.0, "angle_deg": 0.0,
        "angle_increment_deg": 90.0, "cross_hatch": false}, "options": {"auto_rotate": true}}})).await;
    let horizontal = || async {
        let (_, p) = send(&app, req("GET", "/api/preview", None)).await;
        let marks: Vec<Value> = p["segments"].as_array().unwrap().iter().filter(|s| s["kind"] == "mark").cloned().collect();
        let flat = marks.iter().filter(|s| (s["from"][1].as_f64().unwrap() - s["to"][1].as_f64().unwrap()).abs() < 1e-6).count();
        flat * 2 > marks.len()
    };
    assert!(horizontal().await, "run 0: hatch at 0°");
    let id = request(&app, json!({})).await;
    approve(&app, &id).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    exec(&app, "shape.rect", json!({"width_mm": 1, "height_mm": 1})).await; // a new revision, so a new plan
    assert!(!horizontal().await, "run 1: hatch at 90°");
}

/// Start and stop on the same input: every run would stop as it starts, so it is refused up front.
#[tokio::test]
async fn start_input_on_the_stop_input_is_refused() {
    let p = MachineProfile { stop_mark_input_bit: Some(14), ..pedal_profile() };
    let st = state_with(p).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"start_input": true})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"selection": ["x"], "from_segment": 1})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn selection_and_start_here_shape_the_run() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    let a = rect(&app, -20.0).await;
    rect(&app, 20.0).await;
    let whole = request(&app, json!({})).await;
    let only = request(&app, json!({"selection": [a]})).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    let runs = m["pending_runs"].as_array().unwrap();
    let mark = |id: &str| runs.iter().find(|r| r["id"] == id).unwrap()["stats"]["mark_length_mm"].as_f64().unwrap();
    assert!((mark(&whole) - 80.0).abs() < 1e-6 && (mark(&only) - 40.0).abs() < 1e-6);
    let (_, preview) = send(&app, req("GET", "/api/preview", None)).await;
    let segs = preview["segments"].as_array().unwrap();
    assert!(segs.iter().all(|s| s["t_s"].is_number()));
    let from = request(&app, json!({"from_segment": 5})).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    let r = m["pending_runs"].as_array().unwrap().iter().find(|r| r["id"] == from.as_str()).unwrap().clone();
    assert!((r["stats"]["mark_length_mm"].as_f64().unwrap() - 40.0).abs() < 1e-6, "{r}");
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"selection": ["obj-404"]})))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn pause_resume_rules() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    let (s, v) = send(&app, req("POST", "/api/machine/pause", None)).await;
    assert_eq!((s, &v["running"]), (StatusCode::OK, &json!(false)));
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/resume", None))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/machine/resume", Some(json!({"origin": "agent:x"})))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/machine/resume", None)).await.0, StatusCode::CONFLICT);
    let (_, v) = send(&app, req("POST", "/api/job/end_after_current", None)).await;
    assert_eq!(v["active"], false);
}

#[tokio::test]
async fn framing_modes_pointer_and_live_framing_stay_light_only() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    let id = rect(&app, 0.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"mode": "hull", "continuous": false})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(send(&app, req("POST", "/api/machine/frame", Some(json!({"pointer": "focus"})))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, req("POST", "/api/machine/frame", Some(json!({"speed_mm_s": 1e9})))).await.0, StatusCode::BAD_REQUEST);
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"mode": "contour", "live": true})))).await;
    assert_eq!((s, &v["live"]), (StatusCode::OK, &json!(true)), "{v}");
    exec(&app, "object.transform", json!({"ids": [id], "translate": [5, 0]})).await;
    let updated = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(crate::Event::Machine { event, .. }) = events.recv().await
                && event == "frame_updated"
            {
                return true;
            }
        }
    })
    .await;
    assert_eq!(updated, Ok(true), "the live frame followed the edit");
    send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    let (s, v) = send(&app, req("POST", "/api/machine/pointer", Some(json!({"x": 3.0, "y": -4.0})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0), "framing never fires");
}

/// Every framing style, pointer choice and blanking setting, once and looping, plus the red-dot jog and a stop in
/// the middle of a frame: every list packet sent decodes as light only and no single command opens the MO or sets
/// the laser-enable bit.
#[tokio::test]
async fn every_framing_mode_is_light_only() {
    let profile = MachineProfile { focus_light_port_bit: Some(9), ..MachineProfile::generic_lmc_fiber() };
    let st = state_with(profile).await;
    let app = api::router(st.clone(), None);
    let a = rect(&app, -20.0).await;
    rect(&app, 20.0).await;
    exec(&app, "shape.ellipse", json!({"rx_mm": 1, "ry_mm": 1, "center": [-20, 0]})).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    let packets = counters.packets();
    let mut frames = 0;
    for mode in ["bounds", "hull", "outline"] {
        for individually in [false, true] {
            for pointer in ["red", "focus", "both"] {
                for blanking in [false, true] {
                    let body = json!({"mode": mode, "individually": individually, "outside_only": individually, "pointer": pointer,
                        "blanking": blanking, "continuous": false, "speed_mm_s": 2000.0});
                    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(body))).await;
                    assert_eq!(s, StatusCode::OK, "{mode} {pointer}: {v}");
                    frames += 1;
                }
            }
        }
    }
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"ids": [a], "mode": "outline", "pointer": "both"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    tokio::time::sleep(Duration::from_millis(50)).await;
    send(&app, req("POST", "/api/machine/stop", None)).await;
    for pointer in ["red", "focus", "both"] {
        let (s, v) = send(&app, req("POST", "/api/machine/pointer", Some(json!({"x": 1.0, "y": 2.0, "pointer": pointer})))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    assert!(counters.packets() >= packets + frames, "the frames were sent");
    assert_eq!((counters.laser_singles(), counters.emitting_packets()), (0, 0), "framing never fires");
}

/// Raw steps cannot drive a Z lift past its configured travel.
#[tokio::test]
async fn z_axis_moves_stay_within_travel() {
    let axis = machine::ExtensionAxis { role: machine::AxisRole::Z, steps_per_mm: 100.0, min_mm: 0.0, max_mm: 10.0, invert: false };
    let st = state_with(MachineProfile { axis: Some(axis), ..MachineProfile::generic_lmc_fiber() }).await;
    let app = api::router(st.clone(), None);
    let (s, v) = send(&app, req("POST", "/api/machine/axis", Some(json!({"z_mm": 5.0})))).await;
    assert_eq!((s, &v["position"]), (StatusCode::OK, &json!(500)), "{v}");
    assert_eq!(send(&app, req("POST", "/api/machine/axis", Some(json!({"steps": 1000})))).await.0, StatusCode::OK);
    assert_eq!(send(&app, req("POST", "/api/machine/axis", Some(json!({"steps": 1001})))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, req("POST", "/api/machine/axis", Some(json!({"z_mm": 11.0})))).await.0, StatusCode::BAD_REQUEST);
    assert!(!st.machine.is_running(), "the claim is released");
}

#[tokio::test]
async fn controller_info_preview_png_and_axis() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    let (_, info) = send(&app, req("GET", "/api/machine/info", None)).await;
    assert_eq!(info["connected"], true);
    assert!(info["serial_hex"].as_str().unwrap().starts_with("0x"));
    rect(&app, 0.0).await;
    let (s, png) = raw(&app, req("GET", "/api/preview.png?width=200", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(&png[1..4], b"PNG");
    let (s, _) = send(&app, req("POST", "/api/machine/axis", Some(json!({"z_mm": 1.0})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no axis in the profile");
    assert_eq!(send(&app, mcp(req("POST", "/api/machine/axis", Some(json!({"steps": 1}))))).await.0, StatusCode::FORBIDDEN);
    let (_, job) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(job["job"]["state"], "idle");
}

#[tokio::test]
async fn calibration_wizard_and_device_settings() {
    let st = state_with(MachineProfile::generic_lmc_fiber()).await;
    let app = api::router(st.clone(), None);
    let (s, v) = send(&app, req("POST", "/api/calibration/pattern", Some(json!({"size_mm": 100})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["created"].as_array().unwrap().len(), 5);
    let measurements = json!({"top_mm": 101.0, "bottom_mm": 101.0, "middle_x_mm": 101.0, "left_mm": 99.0, "right_mm": 99.0,
        "middle_y_mm": 99.0, "diagonal_up_mm": 142.1, "diagonal_down_mm": 142.1});
    let (s, v) = send(&app, req("POST", "/api/calibration/solve", Some(json!({"size_mm": 100, "measurements": measurements})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["calibration"]["scale_x"].as_f64().unwrap() < 1.0 && v["calibration"]["scale_y"].as_f64().unwrap() > 1.0);
    // The built-in device is read-only: duplicate it, make the copy active, then apply.
    let builtin = st.active_device().id;
    let body = json!({"size_mm": 100, "measurements": measurements, "apply": true});
    assert_eq!(send(&app, mcp(req("POST", "/api/calibration/solve", Some(body.clone())))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/calibration/solve", Some(body.clone()))).await.0, StatusCode::CONFLICT);
    let (s, v) = send(&app, req("POST", &format!("/api/devices/{builtin}/duplicate"), None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let copy = v["device"]["id"].as_str().unwrap().to_owned();
    send(&app, req("POST", &format!("/api/devices/{copy}/activate"), None)).await;
    let (s, v) = send(&app, req("POST", "/api/calibration/solve", Some(body))).await;
    assert_eq!((s, &v["applied"]), (StatusCode::OK, &json!(true)), "{v}");
    assert!(st.machine.profile().calibration.scale_x < 1.0, "the machine runs with the new correction");
    assert!(st.machine.target().calibration.scale_x < 1.0, "and plans with it");
    // Device settings: edit, refuse raised limits and MCP clients, export.
    let (s, v) = send(&app, req("PUT", &format!("/api/devices/{copy}"), Some(json!({"profile": {"red_dot": {"offset_x_mm": 0.5}}, "display_name": "Mine"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(st.machine.profile().red_dot.offset_x_mm, 0.5);
    let (s, _) = send(&app, req("PUT", &format!("/api/devices/{copy}"), Some(json!({"profile": {"max_power_pct": 150}})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, mcp(req("PUT", &format!("/api/devices/{copy}"), Some(json!({"display_name": "x"}))))).await.0, StatusCode::FORBIDDEN);
    let (s, text) = raw(&app, req("GET", &format!("/api/devices/{copy}/lbdev"), None)).await;
    assert_eq!(s, StatusCode::OK);
    let lbdev: Value = serde_json::from_slice(&text).unwrap();
    assert_eq!(lbdev["DeviceList"][0]["DisplayName"], "Mine");
    // EzCad markcfg import.
    let markcfg = include_bytes!("../../../fixtures/markcfg7.ini");
    let r = Request::builder().method("POST").uri("/api/devices/import/markcfg?name=Shop").header("host", "localhost:7878").body(Body::from(markcfg.to_vec())).unwrap();
    let (s, v) = send(&app, r).await;
    assert_eq!((s, &v["device"]["source"]), (StatusCode::CREATED, &json!("markcfg")), "{v}");
}
