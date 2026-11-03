//! Parity of the browser backend (`webcore::backend`, the design-only build) with the engine: the same scripted
//! requests go to the engine's axum router (on the simulated board with the B6 profile) and to the backend, and the
//! answers must match: status, JSON (after dropping what is volatile by nature: change-set epochs, file paths,
//! timestamps), file bytes or their headers where the engine stamps the time, and the events each request sends.
//!
//! Values that differ between the two only by name (the change-set epoch, an image's object id is the same on both)
//! are captured from earlier answers per side (`{name}` in a step's path, body or `since`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use webcore::backend::{Backend, Init};

const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
const SHAPES: &str = include_str!("../../lbrn/tests/fixtures/shapes.lbrn2");
const SETTINGS: &str = include_str!("../../lbrn/tests/fixtures/settings.lbrn2");
const GANTRY: &str = include_str!("../../machine/tests/fixtures/gantry.lbdev");
/// A hand-written LightBurn material library (as the engine's library tests use).
const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Engrave\" NoThickTitle=\"Finish\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Finish/Engrave\" />\r\n        <minPower Value=\"7\" />\r\n        <maxPower Value=\"38\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"25000\" />\r\n        <interval Value=\"0.025\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="20mm" viewBox="0 0 40 20"><rect x="2" y="2" width="30" height="12" fill="none" stroke="#000"/><circle cx="34" cy="10" r="4"/></svg>"##;
const DXF: &str = "0\nSECTION\n2\nENTITIES\n0\nLINE\n8\n0\n10\n0.0\n20\n0.0\n30\n0.0\n11\n25.0\n21\n10.0\n31\n0.0\n0\nCIRCLE\n8\n0\n10\n10.0\n20\n10.0\n30\n0.0\n40\n5.0\n0\nENDSEC\n0\nEOF\n";

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(8, 6, |x, y| if (x + y) % 2 == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn temp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("openetch-parity-{tag}-{}-{nanos}", std::process::id()))
}

/// The engine on the simulated B6, with a config directory (preferences, presets, font favourites).
async fn engine() -> (Router, Arc<engine::AppState>) {
    let profile = machine::MachineProfile::from_toml(B6).unwrap();
    let field = profile.field_mm;
    let m = Arc::new(engine::hw::Machine::simulated(profile.clone(), None, machine::PollConfig::immediate()));
    m.connect().await.unwrap();
    let approval = engine::pin::ApprovalConfig { pin: engine::pin::PinStore::new(&temp("pin")), confirm: None };
    let device = machine::Device::builtin(profile);
    let doc = appcore::layout::new_document_for(&device, field);
    let st = engine::AppState::new(
        commands::Session::new(doc),
        m,
        engine::security::SecurityConfig::default(),
        approval,
        engine::projects::ProjectDir::new(temp("projects")),
    );
    let cfg = temp("config");
    std::fs::create_dir_all(&cfg).unwrap();
    st.files.open(&cfg);
    st.libs.open(&cfg);
    (engine::api::router(st.clone(), None), st)
}

fn core() -> Backend {
    let profile = machine::MachineProfile::from_toml(B6).unwrap();
    Backend::with_profiles(Init::default(), vec![profile])
}

/// One answer, from either side.
#[derive(Debug)]
struct Got {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    events: Vec<Value>,
}

impl Got {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn engine_send(app: &Router, st: &engine::AppState, method: &str, path: &str, query: &str, body: &[u8], since: Option<&str>) -> Got {
    let mut rx = st.subscribe();
    let uri = if query.is_empty() { path.to_owned() } else { format!("{path}?{query}") };
    let mut req = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    if let Some(s) = since {
        req = req.header("x-openetch-since", s);
    }
    let res = app.clone().oneshot(req.body(Body::from(body.to_vec())).unwrap()).await.unwrap();
    let status = res.status().as_u16();
    let headers = res.headers().iter().map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap_or_default().to_owned())).collect();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(serde_json::to_value(&e).unwrap());
    }
    Got { status, headers, body, events }
}

fn core_send(b: &mut Backend, method: &str, path: &str, query: &str, body: &[u8], since: Option<&str>) -> Got {
    let r = b.handle(method, path, query, body.to_vec(), since);
    Got {
        status: r.status,
        headers: r.headers.into_iter().map(|(k, v)| (k.to_ascii_lowercase(), v)).collect(),
        body: r.body,
        events: r.events.iter().map(|e| serde_json::from_str(e).unwrap()).collect(),
    }
}

/// Drop what differs by nature: change-set epochs, file paths and timestamps.
fn normalise(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for k in ["epoch", "path", "modified_unix_s", "saved_unix_s", "created_unix_s"] {
                m.remove(k);
            }
            m.values_mut().for_each(normalise);
        }
        Value::Array(a) => a.iter_mut().for_each(normalise),
        _ => {}
    }
}

/// How a step's answers are compared.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Check {
    /// Status and normalised JSON.
    Json,
    /// Status and the body's bytes, and the file headers.
    Bytes,
    /// Status and the file headers only (the file carries a time stamp: PDF).
    Headers,
}

struct Step {
    method: &'static str,
    path: String,
    query: String,
    body: Vec<u8>,
    since: Option<String>,
    check: Check,
    /// `(name, JSON pointer)`: values to capture from the answer, per side.
    capture: Vec<(&'static str, &'static str)>,
}

fn step(method: &'static str, path: &str, query: &str, body: impl Into<Vec<u8>>, check: Check) -> Step {
    Step { method, path: path.into(), query: query.into(), body: body.into(), since: None, check, capture: Vec::new() }
}

fn get(path: &str) -> Step {
    step("GET", path, "", Vec::new(), Check::Json)
}

fn post(path: &str, body: Value) -> Step {
    step("POST", path, "", body.to_string(), Check::Json)
}

fn put(path: &str, body: Value) -> Step {
    step("PUT", path, "", body.to_string(), Check::Json)
}

impl Step {
    fn capture(mut self, name: &'static str, pointer: &'static str) -> Step {
        self.capture.push((name, pointer));
        self
    }

    fn since(mut self, s: &str) -> Step {
        self.since = Some(s.into());
        self
    }
}

fn fill(text: &str, vars: &HashMap<&'static str, String>) -> String {
    let mut out = text.to_owned();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

fn fill_bytes(body: &[u8], vars: &HashMap<&'static str, String>) -> Vec<u8> {
    match std::str::from_utf8(body) {
        Ok(t) if t.contains('{') => fill(t, vars).into_bytes(),
        _ => body.to_vec(),
    }
}

fn script() -> Vec<Step> {
    let b6 = machine::Device::builtin(machine::MachineProfile::from_toml(B6).unwrap()).id;
    vec![
        // Commands, the empty document, the project, the documents, the timeline, devices, the planning context.
        get("/api/commands"),
        get("/api/document"),
        get("/api/project"),
        get("/api/documents"),
        get("/api/history"),
        get("/api/devices"),
        get(&format!("/api/devices/{b6}")),
        get("/api/preview/context"),
        get("/api/layers/unsupported"),
        get("/api/document/sync").capture("epoch", "/epoch").capture("rev", "/revision"),
        // Edits.
        post("/api/execute", json!({"id": "shape.rect", "params": {"width_mm": 30, "height_mm": 15, "center": [-20, -10]}})),
        post("/api/batch", json!({"commands": [
            {"id": "shape.ellipse", "params": {"rx_mm": 8, "ry_mm": 5, "center": [20, 15]}},
            {"id": "layer.add", "params": {"name": "Fill"}},
        ]})),
        post("/api/execute", json!({"id": "shape.text", "params": {"text": "OpenEtch", "height_mm": 6}})),
        get("/api/document/sync").since("unused"),
        step("GET", "/api/document/sync", "since={rev}&epoch={epoch}", Vec::new(), Check::Json),
        post("/api/execute", json!({"id": "shape.polygon", "params": {"radius_mm": 6, "sides": 6, "center": [0, -30]}})).since("{epoch}:{rev}"),
        get("/api/history"),
        post("/api/undo", json!({})),
        post("/api/redo", json!({"origin": "ui"})),
        post("/api/undo", json!({})),
        post("/api/history/goto", json!({"marker": 1})),
        post("/api/history/goto", json!({"marker": 99})),
        post("/api/history/goto", json!({"marker": 3})),
        post("/api/history/delete", json!({"index": 2})),
        get("/api/history"),
        get("/api/document"),
        get("/api/project"),
        // Previews.
        get("/api/preview"),
        step("GET", "/api/preview", "layers=layer-1", Vec::new(), Check::Json),
        get("/api/preview/stats"),
        step("GET", "/api/preview.svg", "", Vec::new(), Check::Bytes),
        step("GET", "/api/preview.svg", "jumps=false", Vec::new(), Check::Bytes),
        get("/api/fill_regions"),
        // Uploads and imports.
        step("POST", "/api/document/upload", "format=svg&name=Logo%20%26%20mark", SVG, Check::Json),
        step("POST", "/api/document/upload", "format=image&name=dot", png(), Check::Json).capture("image", "/result/created/0"),
        step("POST", "/api/document/import", "format=dxf&name=part.dxf", DXF, Check::Json),
        step("POST", "/api/document/upload", "format=lbrn2&mode=merge&name=shapes.lbrn2", SHAPES, Check::Json),
        step("POST", "/api/document/upload", "format=pdf", "x", Check::Json),
        step("POST", "/api/document/upload", "format=svg&field_mm=110", SVG, Check::Json),
        get("/api/document"),
        get("/api/layers/unsupported"),
        step("GET", "/api/images/{image}/file", "", Vec::new(), Check::Bytes),
        step("GET", "/api/images/{image}/processed.png", "", Vec::new(), Check::Bytes),
        // Downloads, exports and print.
        step("GET", "/api/document/download", "format=lbrn2", Vec::new(), Check::Bytes),
        step("GET", "/api/document/download", "format=openetch", Vec::new(), Check::Bytes).capture("saved", ""),
        step("GET", "/api/export", "format=svg", Vec::new(), Check::Bytes),
        step("GET", "/api/export", "format=dxf&colors=black", Vec::new(), Check::Bytes),
        step("GET", "/api/export", "format=pdf", Vec::new(), Check::Headers),
        step("GET", "/api/print", "format=svg&colors=black", Vec::new(), Check::Bytes),
        step("GET", "/api/export", "", Vec::new(), Check::Json),
        // A LightBurn project replacing the document, then the project state and its unsupported settings.
        step("POST", "/api/document/upload", "format=lbrn2&mode=replace&name=settings.lbrn2", SETTINGS, Check::Json),
        get("/api/project"),
        get("/api/layers/unsupported"),
        get("/api/preview"),
        step("GET", "/api/document/download", "format=lbrn2", Vec::new(), Check::Bytes),
        step("POST", "/api/document/import", "format=lbrn2&mode=replace&name=shapes.lbrn2", SHAPES, Check::Json),
        get("/api/project"),
        // Open documents.
        post("/api/documents", json!({"activate": false})),
        get("/api/documents"),
        post("/api/documents/doc-2/activate", json!({})),
        post("/api/execute", json!({"id": "shape.rect", "params": {"width_mm": 5, "height_mm": 5}})),
        post("/api/documents/doc-2/close", json!({})),
        post("/api/documents/doc-2/close", json!({"discard": true})),
        get("/api/documents"),
        post("/api/documents/nope/activate", json!({})),
        // Preferences and defaults.
        get("/api/prefs"),
        put("/api/prefs", json!({"require_framing": true, "checklist": {"enabled": true, "items": ["Door closed"]}, "load_defaults_on_new": true})),
        put("/api/prefs", json!({"checklist": {"enabled": true, "items": []}})),
        put("/api/prefs", json!({"play_sound": true, "origin": "agent"})),
        post("/api/layers/layer-1/default", json!({})),
        post("/api/layers/layer-1/reset", json!({"builtin": true})),
        post("/api/layers/layer-1/reset", json!({})),
        post("/api/optimization/default", json!({})),
        post("/api/optimization/reset", json!({"builtin": true})),
        step("DELETE", "/api/optimization/default", "", Vec::new(), Check::Json),
        post("/api/execute", json!({"id": "layer.add", "params": {"name": "Red", "color": "#d62728"}})),
        step("GET", "/api/prefs/export", "", Vec::new(), Check::Bytes),
        get("/api/files/settings"),
        put("/api/files/settings", json!({"autosave_minutes": 2})),
        put("/api/files/settings", json!({"autosave_keep": 0})),
        get("/api/fonts/prefs"),
        put("/api/fonts/prefs", json!({"favourites": [{"family": " Hershey Sans ", "kind": "stroke"}, {"family": "hershey sans", "kind": "stroke"}]})),
        get("/api/fonts/prefs"),
        // Presets.
        put("/api/material-test/presets/Steel%20grid", json!({"params": {"rows": {"param": "frequency", "min": 20, "max": 200, "count": 4}, "columns": {"param": "power", "min": 10, "max": 60, "count": 3}}})),
        get("/api/material-test/presets"),
        step("GET", "/api/material-test/presets/export", "", Vec::new(), Check::Bytes),
        step("DELETE", "/api/material-test/presets/Steel%20grid", "", Vec::new(), Check::Json),
        step("DELETE", "/api/material-test/presets/Steel%20grid", "", Vec::new(), Check::Json),
        put("/api/image-adjust/presets/Photo", json!({"adjust": {"brightness": 10, "contrast": 5}})),
        get("/api/image-adjust/presets"),
        step("GET", "/api/image-adjust/presets/export", "format=lbimg", Vec::new(), Check::Bytes),
        post("/api/image-adjust/presets/import", json!({"presets": [{"name": "Photo", "adjust": {"brightness": 20}}]})),
        get("/api/image-adjust/presets"),
        // Material libraries: a LightBurn .clb loaded, entries made, edited, applied (linked), saved, renamed, merged.
        step("POST", "/api/library/load", "name=steel", CLB, Check::Json),
        get("/api/library"),
        get("/api/library/steel"),
        post("/api/library/steel/entries", json!({"layer": "layer-1", "material": "Brass", "title": "Mark", "description": "Fast"})),
        post("/api/library/steel/entries/m0.e0/apply", json!({"layer": "layer-1", "link": true})),
        put("/api/library/steel/entries/m0.e0", json!({"settings": {"power_pct": 42.0, "speed_mm_s": 900.0}})),
        put("/api/library/steel/entries/m0.e0", json!({"meta": {"material": "Steel", "title": "Finish", "description": "Engrave deep"}})),
        post("/api/library/steel/entries/m0.e0/duplicate", json!({})),
        get("/api/document"),
        post("/api/library/save", json!({"name": "steel", "as": "steel copy"})),
        step("GET", "/api/library/steel/download", "", Vec::new(), Check::Bytes),
        post("/api/library/select", json!({"name": "steel"})),
        post("/api/library/rename", json!({"name": "steel copy", "new_name": "brass"})),
        post("/api/library/merge", json!({"name": "steel", "from": "brass"})),
        step("DELETE", "/api/library/steel/entries/m0.e1", "", Vec::new(), Check::Json),
        post("/api/library/unload", json!({"name": "steel"})),
        post("/api/library/unload", json!({"name": "steel", "discard": true})),
        post("/api/library/new", json!({"name": "steel"})),
        post("/api/library/new", json!({"name": "fresh"})),
        get("/api/library"),
        get("/api/prefs"),
        // Devices: a LightBurn gantry imported, made active (design-only: no toolpath), removed.
        step("POST", "/api/devices/import", "", GANTRY, Check::Json).capture("gantry", "/added/0"),
        get("/api/devices"),
        get("/api/devices/{gantry}"),
        post("/api/devices/{gantry}/activate", json!({})),
        get("/api/preview"),
        post("/api/execute", json!({"id": "doc.new", "params": {}})),
        get("/api/document"),
        post(&format!("/api/devices/{b6}/activate"), json!({})),
        step("DELETE", "/api/devices/{gantry}", "", Vec::new(), Check::Json),
        get("/api/devices"),
        // Refusals.
        post("/api/execute", json!({"id": "shape.nope"})),
        post("/api/execute", json!({"id": "shape.rect", "params": {"width_mm": "wide"}})),
        post("/api/batch", json!({"commands": []})),
        step("POST", "/api/execute", "", "{not json", Check::Json),
        step("POST", "/api/document/upload", "format=svg", Vec::new(), Check::Json),
        step("POST", "/api/document/upload", "", SVG, Check::Json),
        step("GET", "/api/document/download", "format=svg", Vec::new(), Check::Json),
        get("/api/nope"),
        // The document round trip through the native file.
        step("POST", "/api/document/upload", "format=openetch&mode=replace&name=Badge.openetch", "{saved}", Check::Json),
        get("/api/project"),
        get("/api/history"),
    ]
}

#[tokio::test]
async fn the_browser_backend_answers_like_the_engine() {
    let (app, st) = engine().await;
    let mut b = core();
    let mut ev: HashMap<&'static str, String> = HashMap::new();
    let mut cv: HashMap<&'static str, String> = HashMap::new();
    let steps = script();
    assert!(steps.len() >= 60, "{} requests", steps.len());
    let mut failures = Vec::new();
    for (n, s) in steps.iter().enumerate() {
        let e = engine_send(
            &app,
            &st,
            s.method,
            &fill(&s.path, &ev),
            &fill(&s.query, &ev),
            &fill_bytes(&s.body, &ev),
            s.since.as_deref().map(|t| fill(t, &ev)).as_deref(),
        )
        .await;
        let c = core_send(
            &mut b,
            s.method,
            &fill(&s.path, &cv),
            &fill(&s.query, &cv),
            &fill_bytes(&s.body, &cv),
            s.since.as_deref().map(|t| fill(t, &cv)).as_deref(),
        );
        for (name, ptr) in &s.capture {
            let val = |g: &Got| match g.json().pointer(ptr) {
                Some(Value::String(x)) => x.clone(),
                Some(v) => v.to_string(),
                None => panic!("step {n} {} {}: nothing at {ptr} in {}", s.method, s.path, String::from_utf8_lossy(&g.body)),
            };
            ev.insert(name, val(&e));
            cv.insert(name, val(&c));
        }
        let what = format!("step {n}: {} {}?{}", s.method, fill(&s.path, &ev), s.query);
        if std::env::var_os("PARITY_TRACE").is_some() {
            let ok = e.json().get("ok").cloned().unwrap_or(Value::Null);
            let err = e.json().get("error").cloned().unwrap_or(Value::Null);
            eprintln!("{what} -> {} ok={ok} {}B {} events {err}", e.status, e.body.len(), e.events.len());
        }
        if e.status != c.status {
            failures.push(format!("{what}: status {} (engine) vs {} (core)\n  engine: {}\n  core:   {}", e.status, c.status, String::from_utf8_lossy(&e.body), String::from_utf8_lossy(&c.body)));
            continue;
        }
        match s.check {
            Check::Json => {
                let (mut ej, mut cj) = (e.json(), c.json());
                normalise(&mut ej);
                normalise(&mut cj);
                if ej != cj {
                    failures.push(format!("{what}: JSON differs\n  engine: {}\n  core:   {}", short(&ej), short(&cj)));
                }
            }
            Check::Bytes | Check::Headers => {
                if s.check == Check::Bytes && e.body != c.body {
                    failures.push(format!("{what}: {} bytes (engine) vs {} (core)", e.body.len(), c.body.len()));
                }
                for h in ["content-type", "content-disposition", "x-openetch-warnings"] {
                    if e.headers.get(h) != c.headers.get(h) {
                        failures.push(format!("{what}: header {h}: {:?} (engine) vs {:?} (core)", e.headers.get(h), c.headers.get(h)));
                    }
                }
            }
        }
        let (mut ee, mut ce) = (Value::Array(e.events.clone()), Value::Array(c.events.clone()));
        normalise(&mut ee);
        normalise(&mut ce);
        if ee != ce {
            failures.push(format!("{what}: events differ\n  engine: {ee}\n  core:   {ce}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} requests differ:\n{}", failures.len(), steps.len(), failures.join("\n"));
}

fn short(v: &Value) -> String {
    let t = v.to_string();
    if t.len() > 1500 { format!("{}...", &t[..1500]) } else { t }
}

/// The store records a backend writes bring it back as it was (the worker shell's restart).
#[test]
fn store_writes_restore_the_backend() {
    let mut b = core();
    let mut records = Vec::new();
    fn send(b: &mut Backend, records: &mut Vec<appcore::stores::StoreWrite>, method: &str, path: &str, body: Vec<u8>) -> Value {
        let r = b.handle(method, path, "", body, None);
        assert!(r.status < 300, "{method} {path}: {}", String::from_utf8_lossy(&r.body));
        records.extend(r.writes.iter().cloned());
        r.json()
    }
    let j = |v: Value| v.to_string().into_bytes();
    send(&mut b, &mut records, "PUT", "/api/prefs", j(json!({"require_framing": true})));
    send(&mut b, &mut records, "PUT", "/api/files/settings", j(json!({"autosave_minutes": 7})));
    send(&mut b, &mut records, "PUT", "/api/fonts/prefs", j(json!({"hidden": [{"family": "Hershey Script", "kind": "stroke"}]})));
    send(&mut b, &mut records, "PUT", "/api/material-test/presets/Grid", j(json!({"params": {"rows": {"param": "frequency", "min": 20, "max": 200, "count": 4}, "columns": {"param": "power", "min": 10, "max": 60, "count": 3}}})));
    let added = send(&mut b, &mut records, "POST", "/api/devices/import", GANTRY.as_bytes().to_vec());
    let id = added["added"][0].as_str().unwrap().to_owned();
    send(&mut b, &mut records, "POST", &format!("/api/devices/{id}/activate"), j(json!({})));
    // The last write of each record is what the shell keeps.
    let mut kept: Vec<webcore::backend::Record> = Vec::new();
    for w in records {
        kept.retain(|r| !(r.store == w.store && r.key == w.key));
        if let Some(v) = w.value {
            kept.push(webcore::backend::Record { store: w.store, key: w.key, value: v });
        }
    }
    let profile = machine::MachineProfile::from_toml(B6).unwrap();
    let mut again = Backend::with_profiles(Init { stores: kept, epoch: None }, vec![profile]);
    assert!(again.notes.is_empty(), "{:?}", again.notes);
    for path in ["/api/prefs", "/api/files/settings", "/api/fonts/prefs", "/api/material-test/presets", "/api/devices"] {
        let a = b.handle("GET", path, "", Vec::new(), None).json();
        let c = again.handle("GET", path, "", Vec::new(), None).json();
        assert_eq!(a, c, "{path}");
    }
    assert_eq!(again.handle("GET", "/api/preview/context", "", Vec::new(), None).json()["design_only"].as_str().map(|s| s.contains("design-only")), Some(true));
}

/// The shell routes: a project opened from its text belongs to that project; a save recorded after the download
/// makes it clean; machine routes are stubs.
#[test]
fn shell_routes_open_and_save_projects() {
    let mut b = core();
    let r = b.handle("POST", "/api/execute", "", json!({"id": "shape.rect", "params": {"width_mm": 4, "height_mm": 4}}).to_string().into_bytes(), None);
    assert_eq!(r.status, 200);
    let d = b.handle("GET", "/api/document/download", "format=openetch", Vec::new(), None);
    let rev = d.header("x-openetch-revision").unwrap().to_owned();
    let doc_id = d.header("x-openetch-document").unwrap().to_owned();
    let s = b.handle("POST", "/core/saved", &format!("name=Coaster&revision={rev}&document={doc_id}"), Vec::new(), None);
    assert_eq!(s.json()["ok"], true);
    assert!(s.events[0].contains("project_saved"), "{:?}", s.events);
    let p = b.handle("GET", "/api/project", "", Vec::new(), None).json();
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (Some("Coaster"), Some(false)));
    let o = b.handle("POST", "/core/open", "kind=project&name=Badge", d.body.clone(), None);
    assert_eq!(o.json()["name"], "Badge", "{}", String::from_utf8_lossy(&o.body));
    let p = b.handle("GET", "/api/project", "", Vec::new(), None).json();
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (Some("Badge"), Some(false)));
    let t = b.handle("POST", "/core/open", "kind=template", d.body.clone(), None);
    assert_eq!(t.status, 200);
    let p = b.handle("GET", "/api/project", "", Vec::new(), None).json();
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (None, Some(true)));
    let svg = b.handle("POST", "/core/thumbnail.svg", "width=64", d.body, None);
    assert!(String::from_utf8_lossy(&svg.body).starts_with("<svg"));
    let m = b.handle("GET", "/api/machine", "", Vec::new(), None).json();
    assert_eq!((m["connected"].as_bool(), m["design_only"].as_bool()), (Some(false), Some(true)));
    let j = b.handle("POST", "/api/machine/frame", "", Vec::new(), None);
    assert_eq!((j.status, j.json()["code"].as_str()), (404, Some("design_only")));
    let runs = b.handle("POST", "/api/runs", "", Vec::new(), None);
    assert_eq!(runs.status, 404);
}
