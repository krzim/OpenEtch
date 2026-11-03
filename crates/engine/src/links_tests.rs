//! The links between the feature packages on the simulated board: a jig's pockets in the serial ledger and the job
//! ledger, and the finish library's result colours as the example projects' colour choices.

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

async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
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

#[tokio::test]
async fn jig_pockets_reach_the_serial_ledger_and_the_job_ledger() {
    let (st, app, dir) = state("links-pockets").await;
    let j = exec(&app, "jig.create", json!({"rows": 1, "cols": 3, "pitch_x_mm": 30, "name": "Tags",
        "pocket": {"kind": "circle", "diameter_mm": 25}, "center": [0, 0]}))
    .await["jig"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    let text = exec(&app, "shape.text", json!({ "text": "'SN '0ddd", "font": font, "height_mm": 3, "variable": { "mode": "serial" }, "name": "SN" }))
        .await["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    exec(&app, "jig.place", json!({"jig": j, "ids": [text]})).await;
    // Pocket 2 holds no blank this time.
    exec(&app, "var.set_serial", json!({ "start": 1, "end": 100, "current": 10, "advance_by": 3, "auto_advance": true, "empty_slots": [1] })).await;
    let run = burn(&st, &app).await;

    let v = ok(&app, req("GET", &format!("/api/production/ledger?run={run}"), None)).await;
    let slots = v["entries"][0]["slots"].as_array().unwrap().clone();
    let pockets: Vec<(Value, Value, Value)> = slots.iter().map(|s| (s["pocket"]["number"].clone(), s["pocket"]["pocket"].clone(), s["marked"].clone())).collect();
    assert_eq!(pockets, [(json!(1), json!("p1"), json!(true)), (json!(2), json!("p2"), json!(false)), (json!(3), json!("p3"), json!(true))], "{v}");
    assert_eq!(slots[2]["values"][0]["shown"], "SN 011", "pocket 3 got the next value");
    let (s, csv) = send(&app, req("GET", &format!("/api/production/ledger?run={run}&format=csv"), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(csv.as_str().unwrap().contains(",Tags,p3,3"), "{csv}");

    let e = {
        let mut e = Value::Null;
        for _ in 0..500 {
            e = ok(&app, req("GET", "/api/job-log?limit=1", None)).await["entries"][0].clone();
            if e["run_id"] == json!(run) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        e
    };
    let texts = e["variables"]["texts"].as_array().unwrap();
    let got: Vec<(Value, Value)> = texts.iter().map(|t| (t["pocket"]["number"].clone(), t["value"].clone())).collect();
    assert_eq!(got, [(json!(1), json!("SN 010")), (json!(3), json!("SN 011"))], "the empty pocket burned nothing: {e}");
    assert_eq!(e["jigs"], json!([{ "id": j, "name": "Tags", "pockets_on": [1, 2, 3], "pockets": 3 }]));
    let found = ok(&app, req("GET", "/api/job-log?q=tags%20p3", None)).await;
    assert_eq!(found["entries"].as_array().unwrap().len(), 1, "the ledger search finds the pocket");
    let _ = std::fs::remove_dir_all(&dir);
}

const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Teal\" NoThickTitle=\"Colours\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Colours/Teal\" />\r\n        <maxPower Value=\"23\" />\r\n        <speed Value=\"400\" />\r\n        <frequency Value=\"450000\" />\r\n        <interval Value=\"0.01\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

#[tokio::test]
async fn example_coins_start_on_the_library_finishes() {
    let (st, app, dir) = state("links-examples").await;
    let r = Request::builder().method("POST").uri("/api/library/load?name=steel").header("host", "localhost:7878").body(Body::from(CLB)).unwrap();
    ok(&app, r).await;
    ok(&app, req("PUT", "/api/library/steel/entries/m0.e0/finish", Some(json!({ "color": "#118899" })))).await;
    exec(&app, "doc.new", json!({ "example": "two-tone-coin" })).await;
    let d = st.document();
    let ring = d.layers.iter().find(|l| l.name == "Colour ring").unwrap();
    assert_eq!((ring.color.as_str(), ring.settings.power_pct), ("#118899", 23.0), "the ring starts on the library's finish");
    let f = exec(&app, "template.fields", json!({})).await;
    let ring_field = f["fields"].as_array().unwrap().iter().find(|x| x["id"] == "ring").unwrap().clone();
    assert_eq!(ring_field["choices"][0]["label"], "Teal", "{ring_field}");
    // A caller's own finishes win, and an empty list keeps the built-in ones.
    exec(&app, "doc.new", json!({ "example": "palette-coin", "finishes": [] })).await;
    let f = exec(&app, "template.fields", json!({})).await;
    assert_eq!(f["fields"][0]["choices"][0]["label"], "Gold");
    let _ = std::fs::remove_dir_all(&dir);
}
