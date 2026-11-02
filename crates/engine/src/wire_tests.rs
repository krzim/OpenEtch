//! The B6 galvo path on the wire: every byte the engine sends the simulated LMC board for connect, a frame and an
//! approved run of a fixed document, with the B6 profile and its lens correction, compared with digests recorded
//! on main (see [`GOLDEN`]); with the axis swap undone the run's bytes equal those recorded before the machine-driver
//! interface was introduced. Any change to what the galvo receives fails this test.

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
use crate::runs::RunState;
use crate::security::SecurityConfig;
use crate::{AppState, api};

const PIN: &str = "975310";
const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
const COR: &[u8] = include_bytes!("../../../fixtures/jcz150.cor");

/// FNV-1a, 64 bit: enough to tell two byte streams apart in a test.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
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

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params, "origin": "ui"})))).await;
    assert_eq!((s, &v["ok"]), (StatusCode::OK, &json!(true)), "{id}: {v}");
    v
}

/// (connect, frame, run) wire bytes of the fixed job on the profile `toml`.
async fn b6_wire(toml: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let profile = MachineProfile::from_toml(toml).expect("B6 fixture");
    let cor = corfile::CorTable::parse(COR).expect("cor fixture");
    let machine = Arc::new(Machine::simulated(profile, Some(cor), PollConfig::immediate()));
    let counters = machine.sim_counters().unwrap().clone();
    counters.record();
    machine.connect().await.expect("sim connects");
    let connect = counters.take_wire();
    let dir = pin::temp_dir("wire-golden");
    pin::set_pin(&dir, PIN).expect("set PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, crate::tests::test_projects());
    let app = api::router(st.clone(), None);
    exec(&app, "shape.rect", json!({"width_mm": 20, "height_mm": 12})).await;
    exec(&app, "shape.ellipse", json!({"rx_mm": 6, "ry_mm": 4, "center": [15, -10]})).await;
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"mode": "fill_then_line", "power_pct": 35, "speed_mm_s": 800,
        "frequency_khz": 60, "hatch": {"interval_mm": 0.5, "angle_deg": 30.0}}})).await;

    counters.record();
    let (s, v) = send(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false, "mode": "hull"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for _ in 0..400 {
        if !st.machine.is_framing() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!st.machine.is_framing(), "the frame ends");
    let frame = counters.take_wire();

    let (s, v) = send(&app, req("POST", "/api/runs", None)).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    counters.record();
    let (s, v) = send(&app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": PIN})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for _ in 0..1000 {
        if st.runs().get(&id).is_some_and(|r| r.state.is_final()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let run = st.runs().get(&id).cloned().unwrap();
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    (connect, frame, counters.take_wire())
}

fn digests(wire: &(Vec<u8>, Vec<u8>, Vec<u8>)) -> [(usize, u64); 3] {
    let (connect, frame, run) = wire;
    [(connect.len(), fnv(connect)), (frame.len(), fnv(frame)), (run.len(), fnv(run))]
}

#[tokio::test]
async fn the_b6_galvo_wire_is_unchanged() {
    assert_eq!(digests(&b6_wire(B6).await), GOLDEN, "the bytes sent to the galvo changed");
}

/// The B6 profile as it was before main's hardware-verified axis swap (`swap_xy = false`): the run's bytes are those
/// recorded before the machine-driver interface, so the interface changed nothing a run sends.
#[tokio::test]
async fn the_b6_run_wire_is_unchanged_by_the_driver_interface() {
    let unswapped = B6.replacen("swap_xy = true ", "swap_xy = false", 1);
    assert_ne!(unswapped, B6, "the fixture swaps the axes");
    let got = digests(&b6_wire(&unswapped).await);
    assert_eq!(got[0], GOLDEN[0], "connect");
    assert_eq!(got[2], PRE_INTERFACE_RUN, "the run's bytes changed");
}

/// Recorded with this test on main at 8f91368 (the galvo fixes merged into gantry-grbl), and equal on the merge: the
/// connect is the one first recorded at 48f3c9d, before the machine-driver interface. The frame and run differ from
/// that first recording because of main's hardware-verified changes: the B6 fixture's `swap_xy = true` (transposes
/// every frame and run coordinate, same lengths), the short jump delay along frame outlines (same packet count), and
/// one `ReadPort` input sample while the frame streams (one 12-byte single more).
const GOLDEN: [(usize, u64); 3] = [(50952, 18112136541962948892), (3228, 2872483029218445548), (6444, 12758818638159827868)];

/// The run digest recorded at 48f3c9d, before the machine-driver interface, with `swap_xy = false`.
const PRE_INTERFACE_RUN: (usize, u64) = (6444, 3063143965411556796);
