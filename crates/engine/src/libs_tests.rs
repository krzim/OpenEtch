//! Route tests of material libraries, defaults, preferences (Require Framing, Job Checklist, Play Sound), the job
//! log, the door interlock, queued runs, Repeat Marking, diagnostics, the device wizard and the camera, on the
//! simulated board.

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
use crate::{AppState, Event, api};

const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Engrave\" NoThickTitle=\"Finish\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Finish/Engrave\" />\r\n        <maxPower Value=\"38\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"25000\" />\r\n        <interval Value=\"0.025\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

async fn state_with(profile: MachineProfile, tag: &str) -> (Arc<AppState>, std::path::PathBuf) {
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir(tag);
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
    st.job.open(dir.join(crate::job::COUNTERS_FILE));
    assert!(st.libs.open(&dir).is_empty());
    (st, dir)
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

fn raw_req(method: &str, uri: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder().method(method).uri(uri).header("host", "localhost:7878").body(Body::from(bytes)).unwrap()
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
    let (s, v) = send(app, req("POST", "/api/execute", Some(json!({"id": id, "params": params})))).await;
    assert!(s == StatusCode::OK && v["ok"] == true, "{id}: {v}");
    v["result"].clone()
}

async fn rect(app: &Router) -> String {
    let r = exec(app, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).await;
    r["created"][0].as_str().unwrap().to_owned()
}

async fn request(app: &Router, body: Value) -> String {
    let (s, v) = send(app, req("POST", "/api/runs", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn approve(app: &Router, id: &str, extra: Value) -> (StatusCode, Value) {
    let mut b = json!({"pin": TEST_PIN});
    if let (Some(m), Value::Object(e)) = (b.as_object_mut(), extra) {
        m.extend(e);
    }
    send(app, req("POST", &format!("/api/runs/{id}/approve"), Some(b))).await
}

async fn wait_final(st: &AppState, id: &str) -> Run {
    for _ in 0..2000 {
        let run = st.runs().get(id).cloned().expect("run exists");
        if run.state.is_final() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run {id} did not finish: {:?}", st.runs().get(id).map(|r| r.state));
}

#[tokio::test]
async fn library_load_assign_link_and_follow_edits() {
    let (st, dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-a").await;
    let app = api::router(st.clone(), None);
    let v = ok(&app, raw_req("POST", "/api/library/load?name=steel", CLB.as_bytes().to_vec())).await;
    assert_eq!(v["selected"], "steel");
    assert_eq!(v["tree"]["materials"][0]["groups"][0]["entries"][0]["path"], "Steel/Finish/Engrave");
    let (s, bytes) = raw(&app, req("GET", "/api/library/steel/download", None)).await;
    assert_eq!((s, bytes.as_slice()), (StatusCode::OK, CLB.as_bytes()), "downloaded byte for byte");

    // Assign: settings copied, no link.
    ok(&app, req("POST", "/api/library/steel/entries/m0.e0/apply", Some(json!({"layer": "layer-1"})))).await;
    let d = st.document();
    assert_eq!((d.layers[0].settings.power_pct, d.layers[0].settings.frequency_khz), (38.0, 25.0));
    assert!(d.layers[0].link.is_none());
    // Link: the layer follows the preset.
    ok(&app, req("POST", "/api/library/steel/entries/m0.e0/apply", Some(json!({"layer": "layer-1", "link": true})))).await;
    assert_eq!(st.document().layers[0].link.as_ref().unwrap().path, "Steel/Finish/Engrave");
    let mut edited = st.document().layers[0].settings.clone();
    edited.power_pct = 44.0;
    let v = ok(&app, req("PUT", "/api/library/steel/entries/m0.e0", Some(json!({"settings": edited})))).await;
    assert_eq!(v["updated_layers"], json!(["layer-1"]));
    assert_eq!(st.document().layers[0].settings.power_pct, 44.0, "linked layer updated");
    // Edit Description: the link follows the new path.
    let meta = json!({"meta": {"material": "Steel", "title": "Finish", "description": "Deep"}});
    ok(&app, req("PUT", "/api/library/steel/entries/m0.e0", Some(meta))).await;
    assert_eq!(st.document().layers[0].link.as_ref().unwrap().path, "Steel/Finish/Deep");
    // Create from a layer, duplicate, delete (a deleted preset unlinks its layers).
    let v = ok(&app, req("POST", "/api/library/steel/entries", Some(json!({"layer": "layer-1", "material": "Brass", "description": "Copy", "title": "Finish"})))).await;
    assert_eq!(v["path"], "Brass/Finish/Copy");
    let v = ok(&app, req("POST", "/api/library/steel/entries/m1.e0/duplicate", None)).await;
    assert_eq!(v["id"], "m1.e1");
    let v = ok(&app, req("DELETE", "/api/library/steel/entries/m0.e0", None)).await;
    assert_eq!(v["unlinked_layers"], json!(["layer-1"]));
    assert!(st.document().layers[0].link.is_none());
    ok(&app, req("POST", "/api/library/save", Some(json!({"name": "steel"})))).await;
    let saved = std::fs::read_to_string(dir.join("libraries/steel.clb")).unwrap();
    assert!(saved.contains("Brass") && !saved.contains("Desc=\"Deep\""), "{saved}");
    // Manage: new, merge, rename, unload.
    ok(&app, req("POST", "/api/library/new", Some(json!({"name": "other"})))).await;
    let v = ok(&app, req("POST", "/api/library/merge", Some(json!({"name": "other", "from": "steel"})))).await;
    assert_eq!(v["added"], 1, "the duplicate is the same entry: merged once");
    ok(&app, req("POST", "/api/library/rename", Some(json!({"name": "other", "new_name": "merged"})))).await;
    let (s, _) = send(&app, req("POST", "/api/library/unload", Some(json!({"name": "merged"})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "unsaved changes");
    ok(&app, req("POST", "/api/library/unload", Some(json!({"name": "merged", "discard": true})))).await;
    let (s, _) = send(&app, raw_req("POST", "/api/library/load?name=bad", b"<LightBurnProject/>".to_vec())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn layer_and_optimization_defaults() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-b").await;
    let app = api::router(st.clone(), None);
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 61, "frequency_khz": 90}})).await;
    let v = ok(&app, req("POST", "/api/layers/layer-1/default", None)).await;
    assert_eq!(v["color"], "#1f77b4");
    // A new layer of that colour starts from the default; another colour does not.
    let r = exec(&app, "layer.add", json!({"name": "Same", "color": "#1F77B4"})).await;
    let same = r["id"].as_str().unwrap().to_owned();
    assert_eq!(st.document().layer(&same).unwrap().settings.power_pct, 61.0);
    let r = exec(&app, "layer.add", json!({"name": "Red", "color": "#d62728"})).await;
    let red = r["id"].as_str().unwrap().to_owned();
    assert_eq!(st.document().layer(&red).unwrap().settings.power_pct, 20.0);
    // Make Default for All, then Reset to Default.
    exec(&app, "layer.update", json!({"id": red, "settings": {"power_pct": 33}})).await;
    ok(&app, req("POST", &format!("/api/layers/{red}/default"), Some(json!({"all": true})))).await;
    exec(&app, "layer.update", json!({"id": same, "settings": {"power_pct": 5}})).await;
    ok(&app, req("POST", &format!("/api/layers/{same}/reset"), None)).await;
    assert_eq!(st.document().layer(&same).unwrap().settings.power_pct, 33.0);
    ok(&app, req("POST", &format!("/api/layers/{same}/reset"), Some(json!({"builtin": true})))).await;
    assert_eq!(st.document().layer(&same).unwrap().settings.power_pct, 20.0);
    // Optimization defaults and "load defaults on New".
    exec(&app, "job.set_optimization", json!({"optimization": {"inner_first": false, "cuts_last": true}})).await;
    ok(&app, req("POST", "/api/optimization/default", None)).await;
    ok(&app, req("PUT", "/api/prefs", Some(json!({"load_defaults_on_new": true})))).await;
    exec(&app, "doc.new", json!({})).await;
    let d = st.document();
    assert!(d.optimization.cuts_last && !d.optimization.inner_first);
    assert_eq!(d.layers[0].settings.power_pct, 33.0, "Make Default for All applies to the new layer's colour");
    ok(&app, req("POST", "/api/optimization/reset", Some(json!({"builtin": true})))).await;
    assert!(!st.document().optimization.cuts_last);
}

#[tokio::test]
async fn require_framing_checklist_job_log_and_sound() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-c").await;
    let app = api::router(st.clone(), None);
    let mut events = st.subscribe();
    rect(&app).await;
    let prefs = json!({"require_framing": true, "play_sound": true, "checklist": {"enabled": true, "items": ["Door closed", "Eyewear on"]}});
    ok(&app, req("PUT", "/api/prefs", Some(prefs))).await;
    let mut agent = req("PUT", "/api/prefs", Some(json!({"require_framing": false})));
    agent.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, agent).await.0, StatusCode::FORBIDDEN, "agents cannot turn safety options off");
    let id = request(&app, json!({})).await;
    let (s, v) = approve(&app, &id, json!({"checklist": [true, true]})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("frame"), "{v}");
    ok(&app, req("POST", "/api/machine/frame", Some(json!({"continuous": false})))).await;
    let (s, v) = approve(&app, &id, json!({"checklist": [true, false]})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Checklist"), "{v}");
    let (s, v) = approve(&app, &id, json!({"checklist": [true, true]})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    let mut sound = false;
    while let Ok(e) = events.try_recv() {
        if let Event::Machine { event, .. } = e {
            sound |= event == "job_finished_sound";
        }
    }
    assert!(sound, "the finished-sound event was sent");
    let v = ok(&app, req("GET", "/api/job-log", None)).await;
    let e = &v["entries"][0];
    assert_eq!((e["run_id"].as_str(), e["state"].as_str(), e["origin"].as_str()), (Some(id.as_str()), Some("finished"), Some("ui")));
    let (s, csv) = raw(&app, req("GET", "/api/job-log?format=csv", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(csv).unwrap().starts_with("time_unix_s,run_id"));
    // An edit after the frame needs a new frame.
    rect(&app).await;
    let id = request(&app, json!({})).await;
    assert_eq!(approve(&app, &id, json!({"checklist": [true, true]})).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn door_interlock_blocks_and_status_lights() {
    let mut p = MachineProfile::generic_lmc_fiber();
    p.io.door_protect = Some(machine::InputPort { bit: 3, active_low: false });
    p.io.busy_light = Some(5);
    let (st, _dir) = state_with(p, "libs-d").await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    st.machine.sim_counters().unwrap().set_inputs(1 << 3);
    let id = request(&app, json!({})).await;
    let (s, v) = approve(&app, &id, json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("door"), "{v}");
    st.machine.sim_counters().unwrap().set_inputs(0);
    assert_eq!(approve(&app, &id, json!({})).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

#[tokio::test]
async fn queued_projects_run_with_one_approval() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-e").await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    ok(&app, req("POST", "/api/projects/save", Some(json!({"name": "queue-a"})))).await;
    exec(&app, "shape.ellipse", json!({"rx_mm": 5, "ry_mm": 3})).await;
    ok(&app, req("POST", "/api/projects/save", Some(json!({"name": "queue-b"})))).await;
    let (s, v) = send(&app, req("POST", "/api/runs", Some(json!({"queue": ["queue-a", "queue-b"]})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(v["run"]["summary"].as_str().unwrap().starts_with("queue of 2 projects (\"queue-a\", \"queue-b\")"), "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    assert_eq!(approve(&app, &id, json!({})).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"queue": ["nope"]})))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, req("POST", "/api/runs", Some(json!({"queue": []})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repeat_marking_moves_the_table() {
    let mut p = MachineProfile::generic_lmc_fiber();
    p.axis = Some(machine::ExtensionAxis { role: machine::AxisRole::RotaryTable, steps_per_mm: 10.0, min_mm: 0.0, max_mm: 0.0, invert: false });
    let (st, _dir) = state_with(p, "libs-f").await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = request(&app, json!({"repeat": 3, "table": {"increment": 90, "settle_ms": 1}})).await;
    assert_eq!(approve(&app, &id, json!({})).await.0, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 3), "{:?}", run.error);
    // Without a table axis the option is refused.
    let (st2, _d) = state_with(MachineProfile::generic_lmc_fiber(), "libs-g").await;
    let app2 = api::router(st2, None);
    rect(&app2).await;
    let (s, _) = send(&app2, req("POST", "/api/runs", Some(json!({"repeat": 2, "table": {"increment": 5}})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn diagnostics_updates_scan_and_wizard() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-h").await;
    let app = api::router(st.clone(), None);
    crate::diagnostics::push_line("GET /api/events?token=hunter2".into());
    crate::diagnostics::push_line(r#"INFO engine: listening token="quotedsecret""#.into());
    st.libs.append_job_log(&crate::joblog::JobLogEntry {
        time_unix_s: 1,
        run_id: "run-1".into(),
        state: "finished".into(),
        origin: "ui".into(),
        device: "b6".into(),
        project: Some("Customer Smith ring".into()),
        summary: "layer Engrave Smith".into(),
        duration_s: 1.0,
        completed: 1,
        error: None,
    });
    let (s, bytes) = raw(&app, req("GET", "/api/diagnostics/report", None)).await;
    assert_eq!(s, StatusCode::OK);
    let text = String::from_utf8(bytes).unwrap();
    let report: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(report["versions"]["openetch"], env!("CARGO_PKG_VERSION"));
    assert!(!text.contains("hunter2") && text.contains("token=***"));
    assert!(!text.contains("quotedsecret"), "tracing's quoted fields are masked too");
    assert!(!text.contains("Smith"), "no project names or summaries: {text}");
    assert_eq!(report["job_log"][0]["state"], "finished");
    assert!(report["machine"]["lightburn"].is_boolean(), "only whether a LightBurn device matched, no prefs.ini values");
    // (The debug flag itself is process-wide and other tests set it too; the stored preference is checked.)
    let v = ok(&app, req("PUT", "/api/prefs", Some(json!({"debug_log": true})))).await;
    assert_eq!(v["debug_log"], true);
    let v = ok(&app, req("PUT", "/api/prefs", Some(json!({"debug_log": false})))).await;
    assert_eq!(v["debug_log"], false);
    let v = ok(&app, req("GET", "/api/updates", None)).await;
    assert_eq!(v["enabled"], false);
    assert_eq!(send(&app, req("POST", "/api/updates/check", None)).await.0, StatusCode::CONFLICT, "opt-in");
    let v = ok(&app, req("GET", "/api/devices/scan", None)).await;
    assert_eq!(v["usb_support"], cfg!(feature = "usb"));
    use base64::Engine as _;
    let markcfg = base64::engine::general_purpose::STANDARD.encode(include_bytes!("../../../fixtures/markcfg7.ini"));
    let cor = base64::engine::general_purpose::STANDARD.encode(include_bytes!("../../../fixtures/jcz150.cor"));
    // This test engine has no config directory for correction files: the device made from the markcfg is taken
    // back when its correction file cannot be stored.
    let devices = |st: &AppState| st.devices().list().len();
    let before = devices(&st);
    let body = json!({"name": "My B6", "markcfg_base64": markcfg, "cor_name": "jcz150.cor", "cor_base64": cor});
    let (s, v) = send(&app, req("POST", "/api/devices/wizard", Some(body))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(devices(&st), before, "rolled back");
    let body = json!({"name": "My B6", "markcfg_base64": markcfg});
    let (s, v) = send(&app, req("POST", "/api/devices/wizard", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(v["device"]["id"].as_str().is_some());
    let mut agent = req("POST", "/api/devices/wizard", Some(json!({"markcfg_base64": markcfg})));
    agent.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, agent).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn camera_frame_alignment_and_overlay() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-i").await;
    let app = api::router(st.clone(), None);
    let mut img = image::RgbImage::new(64, 48);
    for (x, _, p) in img.enumerate_pixels_mut() {
        *p = image::Rgb([(x * 4) as u8, 0, 0]);
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let v = ok(&app, raw_req("POST", "/api/camera/frame", png.into_inner())).await;
    assert_eq!((v["width"].as_u64(), v["height"].as_u64()), (Some(64), Some(48)));
    assert_eq!(send(&app, req("GET", "/api/camera/overlay.png", None)).await.0, StatusCode::CONFLICT, "not aligned");
    let align = json!({"image": [[0, 0], [64, 0], [64, 48], [0, 48]], "field": [[-40, 30], [40, 30], [40, -30], [-40, -30]]});
    let v = ok(&app, req("POST", "/api/camera/align", Some(align))).await;
    assert!(v["alignment"]["rms_mm"].as_f64().unwrap() < 1e-6);
    let (s, bytes) = raw(&app, req("GET", "/api/camera/overlay.png?px_per_mm=1", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(&bytes[1..4], b"PNG");
    let v = ok(&app, req("GET", "/api/camera", None)).await;
    assert!(v["calibration"]["alignment"].is_object());
    let v = ok(&app, req("POST", "/api/camera/detect", Some(json!({"cols": 4, "rows": 3})))).await;
    assert_eq!(v["found"], false, "no checkerboard in a gradient");
    let (s, _) = send(&app, req("POST", "/api/camera/capture", Some(json!({})))).await;
    assert_eq!(s, if cfg!(feature = "camera") { s } else { StatusCode::BAD_REQUEST });
}

#[tokio::test]
async fn image_bounds_frames_the_content() {
    // A 20 x 20 mm image whose left half is transparent and right half black.
    let mut img = image::RgbaImage::new(20, 20);
    for (x, _, p) in img.enumerate_pixels_mut() {
        *p = if x >= 10 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) };
    }
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let mut d = doc::Document::default();
    let geometry = doc::Geometry::Image { width_mm: 20.0, height_mm: 20.0, mime: "image/png".into(), data_base64: data.as_str().into(), adjust: doc::ImageAdjust::default() };
    d.objects.push(doc::Object::new("obj-1".into(), "img", "layer-1".into(), geometry, kurbo::Affine::IDENTITY));
    let whole = crate::frame::outline_for(&d, &crate::frame::FrameRequest::default()).unwrap();
    let content = crate::frame::outline_for(&d, &crate::frame::FrameRequest { image_bounds: true, ..Default::default() }).unwrap();
    let x_min = |p: &Vec<Vec<(f64, f64)>>| p.iter().flatten().map(|q| q.0).fold(f64::INFINITY, f64::min);
    assert!((x_min(&whole) + 10.0).abs() < 1e-9);
    assert!(x_min(&content).abs() < 1e-9, "the frame starts where the ink does");
}

#[tokio::test]
async fn door_check_never_waits_behind_a_running_job() {
    // Before: with Door Protect on, approving while another job held the controller waited for that whole job,
    // then started this run with nobody having pressed anything since.
    let mut p = MachineProfile::generic_lmc_fiber();
    p.io.door_protect = Some(machine::InputPort { bit: 3, active_low: false });
    let (st, _dir) = state_with(p, "libs-j").await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    let id = request(&app, json!({})).await;
    // Another run holds the machine and the controller for 3 s.
    assert!(st.machine.try_begin_run());
    let holder = tokio::spawn({
        let m = st.machine.clone();
        async move { m.blocking(|_, _| std::thread::sleep(Duration::from_secs(3))).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    let (s, v) = approve(&app, &id, json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(started.elapsed() < Duration::from_millis(1500), "answered at once, not after the job: {:?}", started.elapsed());
    assert_eq!(st.runs().get(&id).unwrap().state, RunState::Pending, "still pending, never started");
    holder.await.unwrap();
    st.machine.end_run();
    assert_eq!(approve(&app, &id, json!({})).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

#[tokio::test]
async fn require_framing_counts_frames_of_this_device_only() {
    let (st, _dir) = state_with(MachineProfile::generic_lmc_fiber(), "libs-k").await;
    let app = api::router(st.clone(), None);
    rect(&app).await;
    ok(&app, req("PUT", "/api/prefs", Some(json!({"require_framing": true})))).await;
    let id = request(&app, json!({})).await;
    // A frame of this revision made with another device (machine generation) does not count.
    let generation = st.machine.generation();
    st.libs.framed(generation + 1, st.revision(), &crate::frame::FrameRequest::default());
    let (s, v) = approve(&app, &id, json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("Framing"), "{v}");
    st.libs.framed(generation, st.revision(), &crate::frame::FrameRequest::default());
    assert_eq!(approve(&app, &id, json!({})).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}
