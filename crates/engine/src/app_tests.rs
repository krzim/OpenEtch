//! Route tests of the app batch: several open documents (UX-017), the Art Library (DES-074 / UX-045), simulator
//! inputs (JOB-053 / JOB-057, door and framing switch), device details (IO bits, red dot), LightBurn settings that
//! will not be executed (LAY-034) and tool layers (LAY-042), on the simulated board.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use commands::Session;
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::mcp::GalvoMcp;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::projects::ProjectDir;
use crate::runs::{Run, RunState};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, Event, api};

async fn state_with(profile: MachineProfile, projects: ProjectDir) -> Arc<AppState> {
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("apptests");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, projects);
    st.job.open(dir.join(crate::job::COUNTERS_FILE));
    st
}

async fn state() -> Arc<AppState> {
    state_with(MachineProfile::generic_lmc_fiber(), test_projects()).await
}

fn req(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

fn raw_req(method: &str, uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder().method(method).uri(uri).header("host", "localhost:7878").body(Body::from(body)).unwrap()
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
    if x != 0.0 {
        exec(app, "object.transform", json!({"ids": [id], "translate": [x, 0]})).await;
    }
    id
}

async fn objects(app: &Router) -> usize {
    let (_, d) = send(app, req("GET", "/api/document", None)).await;
    d["objects"].as_array().unwrap().len()
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

async fn request_run(app: &Router, body: Value) -> String {
    let (s, v) = send(app, req("POST", "/api/runs", Some(body))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn approve(app: &Router, id: &str) -> (StatusCode, Value) {
    send(app, req("POST", &format!("/api/runs/{id}/approve"), Some(json!({"pin": TEST_PIN})))).await
}

/// Drain the events received so far.
fn drain(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// The JSON text of a successful MCP tool result.
async fn mcp_call(st: &Arc<AppState>, name: &str, args: Value) -> Value {
    let r = GalvoMcp::new(st.clone()).call(name, obj(args)).await;
    let v = serde_json::to_value(&r).unwrap();
    assert_ne!(v["isError"], true, "{name}: {v}");
    let text = v["content"][0]["text"].as_str().unwrap_or_default().to_owned();
    serde_json::from_str(&text).unwrap()
}

// ---------------------------------------------------------------- UX-017: several open documents

#[tokio::test]
async fn documents_open_switch_and_keep_their_own_history() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let (_, v) = send(&app, req("GET", "/api/documents", None)).await;
    assert_eq!((v["active"].as_str(), v["documents"].as_array().unwrap().len()), (Some("default"), 1), "{v}");
    assert_eq!(v["documents"][0]["objects"], 1);
    let default_rev = st.revision();

    // New Window: a new, empty, active document; single-document routes now use it.
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let second = v["id"].as_str().unwrap().to_owned();
    assert_eq!((second.as_str(), v["active"].as_str()), ("doc-2", Some("doc-2")));
    let evs = drain(&mut events);
    assert!(evs.contains(&Event::DocumentsChanged { active: "doc-2".into() }), "{evs:?}");
    assert!(
        evs.iter().any(|e| matches!(e, Event::DocumentChanged { document_id, .. } if document_id == "doc-2")),
        "single-document clients see a document change: {evs:?}"
    );
    assert!(evs.iter().any(|e| matches!(e, Event::HistoryChanged { document_id, steps: 0, .. } if document_id == "doc-2")));
    assert_eq!(objects(&app).await, 0);
    assert!(st.revision() > default_rev, "revisions stay unique across documents");
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(h["steps"].as_array().unwrap().len(), 0, "its own timeline");

    // Edits and undo are per document; events carry the document id.
    rect(&app, 0.0).await;
    rect(&app, 20.0).await;
    let evs = drain(&mut events);
    assert!(evs.iter().all(|e| match e {
        Event::DocumentChanged { document_id, .. } | Event::HistoryChanged { document_id, .. } => document_id == "doc-2",
        _ => true,
    }));
    send(&app, req("POST", "/api/undo", None)).await;
    assert_eq!(objects(&app).await, 2, "undo in doc-2 undid its translate, not a rect");
    let (_, v) = send(&app, req("POST", "/api/documents/default/activate", Some(json!({})))).await;
    assert_eq!((v["ok"].as_bool(), v["active"].as_str()), (Some(true), Some("default")), "{v}");
    assert_eq!(objects(&app).await, 1);
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!(h["steps"].as_array().unwrap().len(), 1, "the default document's own step");
    let (_, u) = send(&app, req("POST", "/api/undo", None)).await;
    assert_eq!(u["ok"], true);
    assert_eq!(objects(&app).await, 0);
    send(&app, req("POST", "/api/redo", None)).await;
    assert_eq!(objects(&app).await, 1);
    // doc-2 kept its state while inactive.
    send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    let (_, h) = send(&app, req("GET", "/api/history", None)).await;
    assert_eq!((h["marker"].as_u64(), h["steps"].as_array().unwrap().len()), (Some(2), 3), "{h}");

    // A pending run of another document is stale after a switch, and stays so after switching back.
    let run = request_run(&app, json!({})).await;
    send(&app, req("POST", "/api/documents/default/activate", None)).await;
    assert_eq!(approve(&app, &run).await.0, StatusCode::CONFLICT);
    send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    assert_eq!(approve(&app, &run).await.0, StatusCode::CONFLICT, "stale: the document was switched");
    // Runs and previews use the active document.
    let (_, p) = send(&app, req("GET", "/api/preview", None)).await;
    assert!(p["segments"].as_array().is_some_and(|s| !s.is_empty()), "{p}");

    // Unknown documents 404; switching to the active one changes nothing.
    assert_eq!(send(&app, req("POST", "/api/documents/nope/activate", None)).await.0, StatusCode::NOT_FOUND);
    let (_, v) = send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    assert_eq!(v["changed"], false);
}

#[tokio::test]
async fn documents_hold_projects_and_close_like_windows() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let (s, v) = send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Alpha"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A project already open activates its document instead of opening it twice.
    send(&app, req("POST", "/api/documents", None)).await;
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({"project": "alpha"})))).await;
    assert_eq!((s, v["existing"].as_bool(), v["id"].as_str()), (StatusCode::OK, Some(true), Some("default")), "{v}");
    // Switching back and forth keeps a saved document clean.
    let (_, docs) = send(&app, req("GET", "/api/documents", None)).await;
    let row = |id: &str| docs["documents"].as_array().unwrap().iter().find(|d| d["id"] == id).cloned().unwrap();
    assert_eq!((row("default")["name"].as_str(), row("default")["dirty"].as_bool()), (Some("Alpha"), Some(false)), "{docs}");
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (Some("Alpha"), Some(false)), "{p}");

    // Save doc-2 as Beta, close it, open Beta into a new document.
    send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    rect(&app, 0.0).await;
    rect(&app, 20.0).await;
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Beta"})))).await;
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/documents/doc-2/close", None)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["closed"].as_str(), v["active"].as_str()), (Some("doc-2"), Some("default")));
    assert!(drain(&mut events).contains(&Event::DocumentsChanged { active: "default".into() }));
    assert_eq!(objects(&app).await, 1, "the default document is active again");
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({"project": "Beta"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let third = v["id"].as_str().unwrap().to_owned();
    assert_eq!(third, "doc-3");
    assert_eq!(objects(&app).await, 2);
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (Some("Beta"), Some(false)), "{p}");
    // An unknown project makes no document.
    let (s, _) = send(&app, req("POST", "/api/documents", Some(json!({"project": "Nope"})))).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, docs) = send(&app, req("GET", "/api/documents", None)).await;
    assert_eq!(docs["documents"].as_array().unwrap().len(), 2, "{docs}");

    // Unsaved changes need discard; the last document cannot be closed.
    rect(&app, 40.0).await;
    let (s, v) = send(&app, req("POST", "/api/documents/doc-3/close", Some(json!({})))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, v) = send(&app, req("POST", "/api/documents/doc-3/close", Some(json!({"discard": true})))).await;
    assert_eq!((s, v["active"].as_str()), (StatusCode::OK, Some("default")), "{v}");
    let (s, _) = send(&app, req("POST", "/api/documents/default/close", None)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    // An inactive new document (activate: false) and its close.
    let (_, v) = send(&app, req("POST", "/api/documents", Some(json!({"activate": false})))).await;
    assert_eq!((v["active"].as_str(), v["id"].as_str()), (Some("default"), Some("doc-4")), "{v}");
    let (s, _) = send(&app, req("POST", "/api/documents", Some(json!({"activate": false, "project": "Beta"})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(send(&app, req("POST", "/api/documents/doc-4/close", None)).await.0, StatusCode::OK);
    assert_eq!(send(&app, req("POST", "/api/documents/doc-4/close", None)).await.0, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, req("POST", "/api/documents", Some(json!({"bogus": 1})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn mcp_lists_and_switches_documents() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    send(&app, req("POST", "/api/documents", None)).await;
    rect(&app, 0.0).await;
    let v = mcp_call(&st, "documents_list", json!({})).await;
    assert_eq!((v["active"].as_str(), v["documents"].as_array().unwrap().len()), (Some("doc-2"), 2), "{v}");
    let v = mcp_call(&st, "document_activate", json!({"id": "default"})).await;
    assert_eq!(v["active"], "default");
    assert_eq!(st.active_document_id(), "default");
    assert_eq!(objects(&app).await, 0);
    // The edit lands in the active document, tagged as the agent's.
    mcp_call(&st, "execute", json!({"id": "shape.rect", "params": {"width_mm": 5, "height_mm": 5}})).await;
    assert_eq!(objects(&app).await, 1);
    let r = GalvoMcp::new(st.clone()).call("document_activate", obj(json!({"id": "nope"}))).await;
    assert_eq!(serde_json::to_value(&r).unwrap()["isError"], true);
}

// ---------------------------------------------------------------- DES-074 / UX-045: Art Library

const STAR_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="20mm" viewBox="0 0 20 20"><path d="M10 0 L13 7 L20 7 L14 12 L16 20 L10 15 L4 20 L6 12 L0 7 L7 7 Z" fill="none" stroke="black"/></svg>"#;

#[tokio::test]
async fn art_library_add_insert_import_and_share() {
    let projects = test_projects();
    let st = state_with(MachineProfile::generic_lmc_fiber(), projects.clone()).await;
    let app = api::router(st.clone(), None);
    let (_, v) = send(&app, req("GET", "/api/art/libraries", None)).await;
    assert_eq!(v["libraries"], json!([]), "{v}");
    assert_eq!(v["formats"], json!(["oeart", "lbart"]), "{v}");
    let mut events = st.subscribe();
    let (s, v) = send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "Shapes"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert!(drain(&mut events).contains(&Event::ArtLibraryChanged { library: None }));
    assert_eq!(send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "shapes"})))).await.0, StatusCode::CONFLICT);
    assert_eq!(send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "../x"})))).await.0, StatusCode::BAD_REQUEST);

    // Add Selection to Library: a group of two shapes.
    let a = rect(&app, 0.0).await;
    let b = exec(&app, "shape.ellipse", json!({"rx_mm": 4, "ry_mm": 4})).await["created"][0].as_str().unwrap().to_owned();
    exec(&app, "object.transform", json!({"ids": [b], "translate": [20, 0]})).await;
    let g = exec(&app, "group.create", json!({"ids": [a, b], "name": "Pair"})).await["group"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", "/api/art/libraries/Shapes/items", Some(json!({"ids": [g], "name": "Pair"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let item = v["item"]["id"].as_str().unwrap().to_owned();
    assert_eq!((v["item"]["objects"].as_u64(), v["item"]["size_mm"][0].as_f64()), (Some(2), Some(29.0)), "{v}");
    assert!(drain(&mut events).contains(&Event::ArtLibraryChanged { library: Some("Shapes".into()) }));
    let (_, lib) = send(&app, req("GET", "/api/art/libraries/shapes", None)).await;
    assert_eq!((lib["name"].as_str(), lib["items"].as_array().unwrap().len()), (Some("Shapes"), 1), "{lib}");
    let thumb = lib["items"][0]["thumbnail"].as_str().unwrap().to_owned();
    let (s, png) = raw(&app, req("GET", &thumb, None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(png.starts_with(b"\x89PNG"), "a PNG thumbnail");
    assert_eq!(send(&app, req("POST", "/api/art/libraries/Shapes/items", Some(json!({"ids": []})))).await.0, StatusCode::BAD_REQUEST);

    // Insert into another document, at a drop point (Add Graphic to Project).
    send(&app, req("POST", "/api/documents", None)).await;
    let (s, v) = send(&app, req("POST", &format!("/api/art/libraries/Shapes/items/{item}/insert"), Some(json!({"center": [5, 5]})))).await;
    assert_eq!((s, v["ok"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
    let created = v["result"]["created"].as_array().unwrap();
    assert_eq!(created.len(), 2);
    let (_, d) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(d["objects"].as_array().unwrap().len(), 2);
    assert_eq!(d["groups"][0]["name"], "Pair");
    let bounds = &v["result"]["bounds"];
    assert!((bounds["center"][0].as_f64().unwrap() - 5.0).abs() < 1e-6, "{bounds}");
    send(&app, req("POST", "/api/undo", None)).await;
    assert_eq!(objects(&app).await, 0, "one undo step");
    let (_, v) = send(&app, req("POST", &format!("/api/art/libraries/Shapes/items/{item}/insert"), Some(json!({"layer": "nope"})))).await;
    assert_eq!(v["ok"], false, "{v}");

    // Import a file as an item; rename it; move it to another library.
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/Shapes/import?format=svg&name=star.svg", STAR_SVG.as_bytes().to_vec())).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let star = v["item"]["id"].as_str().unwrap().to_owned();
    assert_eq!(v["item"]["name"], "star");
    let (s, v) = send(&app, req("PUT", &format!("/api/art/libraries/Shapes/items/{star}"), Some(json!({"name": "Star"})))).await;
    assert_eq!((s, v["item"]["name"].as_str()), (StatusCode::OK, Some("Star")), "{v}");
    send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "Other"})))).await;
    let (s, v) = send(&app, req("POST", &format!("/api/art/libraries/Shapes/items/{star}/copy"), Some(json!({"to": "Other", "move": true})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, lib) = send(&app, req("GET", "/api/art/libraries/Shapes", None)).await;
    assert_eq!(lib["items"].as_array().unwrap().len(), 1, "moved out");
    let (_, other) = send(&app, req("GET", "/api/art/libraries/Other", None)).await;
    assert_eq!(other["items"][0]["name"], "Star");

    // Shared by every client: another engine on the same projects directory sees the libraries.
    let again = state_with(MachineProfile::generic_lmc_fiber(), projects.clone()).await;
    let app2 = api::router(again.clone(), None);
    let (_, v) = send(&app2, req("GET", "/api/art/libraries", None)).await;
    let names: Vec<&str> = v["libraries"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Other", "Shapes"], "{v}");
    assert!(projects.root().join("art-libraries").join("Shapes.oeart").is_file());

    // Download, Load under another name; a damaged .lbart is refused with the reason.
    let (s, file) = raw(&app, req("GET", "/api/art/libraries/Shapes/download", None)).await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/load?name=Copy.oeart", file.clone())).await;
    assert_eq!((s, v["items"].as_u64()), (StatusCode::CREATED, Some(1)), "{v}");
    assert_eq!(send(&app, raw_req("POST", "/api/art/libraries/load?name=Copy.oeart", file)).await.0, StatusCode::CONFLICT);
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/load?name=Clip.lbart", vec![0, 1, 2, 3, 0xff])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["error"].as_str().unwrap().contains(".lbart"), "{v}");
    let (s, _) = send(&app, raw_req("POST", "/api/art/libraries/load?name=Bad.oeart", b"{\"format\":\"x\"}".to_vec())).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = send(&app, raw_req("POST", "/api/art/libraries/Shapes/import?format=lbart", vec![1, 2])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Rename and delete.
    let (s, _) = send(&app, req("POST", "/api/art/libraries/Copy/rename", Some(json!({"to": "Shapes"})))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, v) = send(&app, req("POST", "/api/art/libraries/Copy/rename", Some(json!({"to": "Kept"})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(send(&app, req("GET", "/api/art/libraries/Copy", None)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, req("DELETE", &format!("/api/art/libraries/Kept/items/{item}"), None)).await.0, StatusCode::OK);
    assert_eq!(send(&app, req("DELETE", &format!("/api/art/libraries/Kept/items/{item}"), None)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(send(&app, req("DELETE", "/api/art/libraries/Kept", None)).await.0, StatusCode::OK);
    assert_eq!(send(&app, req("GET", "/api/art/libraries/Kept", None)).await.0, StatusCode::NOT_FOUND);
}

/// LightBurn shapes XML as an Art Library item holds it (structure of LightBurn 2.2's `.lbart` art).
fn lbart_xml(shape: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnShapes FormatVersion=\"1\" MirrorX=\"False\" \
         MirrorY=\"False\">\n    {shape}\n</LightBurnShapes>\n"
    )
}

#[tokio::test]
async fn art_library_reads_and_writes_lightburn_lbart() {
    let projects = test_projects();
    let st = state_with(MachineProfile::generic_lmc_fiber(), projects.clone()).await;
    let app = api::router(st.clone(), None);
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 4 4"><rect width="4" height="4" fill="#fff"/></svg>"##;
    let png = crate::render::svg_to_png(svg, 256).unwrap();
    let lib = fileio::lbart::Lbart {
        items: vec![
            fileio::lbart::Item::new(
                "Circle15",
                png.clone(),
                lbart_xml("<Shape Type=\"Ellipse\" CutIndex=\"0\" Rx=\"7.5000014\" Ry=\"7.5\">\n        <XForm>1 0 0 1 71 127.5</XForm>\n    </Shape>"),
                [15.0, 15.0],
            ),
            fileio::lbart::Item::new(
                "Rect20x10",
                png.clone(),
                lbart_xml("<Shape Type=\"Rect\" CutIndex=\"2\" W=\"20\" H=\"10\" Cr=\"0\">\n        <XForm>1 0 0 1 34 130</XForm>\n    </Shape>"),
                [20.0, 10.0],
            ),
        ],
    };
    let bytes = fileio::lbart::write(&lib).unwrap();
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/load?name=Sample.lbart", bytes.clone())).await;
    assert_eq!((s, v["items"].as_u64()), (StatusCode::CREATED, Some(2)), "{v}");
    assert_eq!(v["warnings"], json!([]), "{v}");
    let (_, view) = send(&app, req("GET", "/api/art/libraries/Sample", None)).await;
    let names: Vec<&str> = view["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Circle15", "Rect20x10"]);
    assert_eq!(view["items"][1]["size_mm"], json!([20.0, 10.0]));
    // LightBurn's thumbnail is served as is.
    let (_, thumb) = raw(&app, req("GET", view["items"][0]["thumbnail"].as_str().unwrap(), None)).await;
    assert_eq!(thumb, png);
    // Unchanged, it downloads as the same .lbart; renamed, as one LightBurn reads with the new name.
    let (s, back) = raw(&app, req("GET", "/api/art/libraries/Sample/download?format=lbart", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(back, bytes, "a loaded .lbart downloads byte for byte");
    let id = view["items"][1]["id"].as_str().unwrap().to_owned();
    send(&app, req("PUT", &format!("/api/art/libraries/Sample/items/{id}"), Some(json!({"name": "Box"})))).await;
    let (_, back) = raw(&app, req("GET", "/api/art/libraries/Sample/download?format=lbart", None)).await;
    let read = fileio::lbart::read(&back).unwrap();
    assert_eq!(read.items[1].name, "Box");
    assert_eq!(read.items[1].shapes_xml, lib.items[1].shapes_xml);
    // Insert: the rectangle lands as a rectangle.
    let (_, v) = send(&app, req("POST", &format!("/api/art/libraries/Sample/items/{id}/insert"), Some(json!({"center": [0, 0]})))).await;
    assert_eq!(v["ok"], true, "{v}");
    let (_, d) = send(&app, req("GET", "/api/document", None)).await;
    assert_eq!(d["objects"][0]["geometry"]["kind"], "rect", "{d}");

    // Import adds a library's items to another library.
    send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "Mine"})))).await;
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/Mine/import?format=lbart&name=Sample.lbart", bytes.clone())).await;
    assert_eq!((s, v["items"].as_array().map(Vec::len)), (StatusCode::CREATED, Some(2)), "{v}");

    // An OpenEtch-made library downloads as an .lbart in LightBurn's layout: 256 px thumbnails, shapes XML.
    let a = rect(&app, 0.0).await;
    send(&app, req("POST", "/api/art/libraries/Mine/items", Some(json!({"ids": [a], "name": "Made here"})))).await;
    let (_, out) = raw(&app, req("GET", "/api/art/libraries/Mine/download?format=lbart", None)).await;
    let read = fileio::lbart::read(&out).unwrap();
    assert_eq!(read.items.len(), 3);
    let made = &read.items[2];
    assert_eq!(made.name, "Made here");
    let img = fileio::image::load_from_memory(&made.thumbnail_png).unwrap();
    assert_eq!((img.width(), img.height()), (256, 256));
    let shapes = lbrn::parse_shapes(&made.shapes_xml).unwrap();
    assert_eq!(shapes.shapes.len(), 1);
    assert!(made.shapes_xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnShapes FormatVersion=\"1\""));
    assert_eq!(send(&app, req("GET", "/api/art/libraries/Mine/download?format=zip", None)).await.0, StatusCode::BAD_REQUEST);
}

/// The owner's LightBurn libraries (`OPENETCH_LBRN_SAMPLES`; see the samples' README): known contents, and a
/// byte-for-byte download.
#[tokio::test]
async fn real_lbart_samples_load_with_their_contents() {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
        return;
    };
    let Some(path) = std::env::split_paths(&dirs).map(|d| d.join("art-sample-1.lbart")).find(|p| p.is_file()) else {
        println!("no art-sample-1.lbart among the samples; skipping");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let st = state_with(MachineProfile::generic_lmc_fiber(), test_projects()).await;
    let app = api::router(st.clone(), None);
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/load?name=art-sample-1.lbart", bytes.clone())).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["warnings"], json!([]), "{v}");
    let (_, view) = send(&app, req("GET", "/api/art/libraries/art-sample-1", None)).await;
    let rows: Vec<(&str, u64)> =
        view["items"].as_array().unwrap().iter().map(|i| (i["name"].as_str().unwrap(), i["objects"].as_u64().unwrap())).collect();
    assert_eq!(rows, [("Circle15", 1), ("Rect20x10", 1), ("TextAB", 5)]);
    let sizes = [(15.0, 15.0), (20.0, 10.0), (32.057, 17.896)];
    for (k, (w, h)) in sizes.into_iter().enumerate() {
        let size = &view["items"][k]["size_mm"];
        assert!((size[0].as_f64().unwrap() - w).abs() < 1e-3 && (size[1].as_f64().unwrap() - h).abs() < 1e-3, "{size}");
        // Inserted, each graphic has LightBurn's size.
        let id = view["items"][k]["id"].as_str().unwrap();
        let (_, v) = send(&app, req("POST", &format!("/api/art/libraries/art-sample-1/items/{id}/insert"), Some(json!({"center": [0, 0]})))).await;
        let b = &v["result"]["bounds"];
        // (LightBurn's stored size of the text is 0.002 mm smaller than its curves' exact extent.)
        assert!((b["size"][0].as_f64().unwrap() - w).abs() < 3e-3 && (b["size"][1].as_f64().unwrap() - h).abs() < 3e-3, "{v}");
    }
    // On the black C00 layer.
    let (_, d) = send(&app, req("GET", "/api/document", None)).await;
    let layer = d["objects"][0]["layer"].as_str().unwrap();
    let l = d["layers"].as_array().unwrap().iter().find(|l| l["id"] == layer).unwrap();
    assert_eq!((l["name"].as_str(), l["color"].as_str()), (Some("C00"), Some("#000000")), "{l}");
    let (_, back) = raw(&app, req("GET", "/api/art/libraries/art-sample-1/download?format=lbart", None)).await;
    assert_eq!(back, bytes, "art-sample-1.lbart does not download byte for byte");
}

// ---------------------------------------------------------------- JOB-053 / JOB-057: simulator inputs

fn pedal_profile() -> MachineProfile {
    MachineProfile { start_mark_input_bit: Some(14), ..MachineProfile::generic_lmc_fiber() }
}

async fn set_inputs(app: &Router, bits: u16) {
    let (s, v) = send(app, req("POST", "/api/sim/inputs", Some(json!({"bits": bits})))).await;
    assert_eq!((s, v["bits"].as_u64()), (StatusCode::OK, Some(u64::from(bits))), "{v}");
}

async fn job_state(app: &Router) -> String {
    let (_, job) = send(app, req("GET", "/api/job", None)).await;
    job["state"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn sim_inputs_only_on_the_simulator() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    let (_, v) = send(&app, req("GET", "/api/sim/inputs", None)).await;
    assert_eq!(v, json!({"simulated": true, "bits": 0}));
    set_inputs(&app, 0x4008).await;
    assert_eq!(st.machine.sim_counters().unwrap().inputs(), 0x4008);
    let mut r = req("POST", "/api/sim/inputs", Some(json!({"bits": 1})));
    r.headers_mut().insert("mcp-session-id", "x".parse().unwrap());
    assert_eq!(send(&app, r).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, req("POST", "/api/sim/inputs", Some(json!({"bits": 70000})))).await.0, StatusCode::BAD_REQUEST);
    // A real board: refused.
    let usb = Arc::new(Machine::usb(MachineProfile::generic_lmc_fiber(), None));
    let dir = pin::temp_dir("apptests-usb");
    let st = AppState::new(
        Session::new(doc::Document::default()),
        usb,
        SecurityConfig::default(),
        ApprovalConfig { pin: PinStore::new(&dir), confirm: None },
        test_projects(),
    );
    let app = api::router(st, None);
    let (s, v) = send(&app, req("POST", "/api/sim/inputs", Some(json!({"bits": 1})))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    let (_, v) = send(&app, req("GET", "/api/sim/inputs", None)).await;
    assert_eq!(v, json!({"simulated": false, "bits": null}));
}

/// JOB-053 end to end through the API: the pedal (start marking input) pressed with `POST /api/sim/inputs` starts
/// each iteration; the job reports `waiting_for_start_input`, and the machine event of the same name is sent.
#[tokio::test]
async fn the_pedal_starts_each_iteration_through_the_api() {
    let st = state_with(pedal_profile(), test_projects()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let mut events = st.subscribe();
    let id = request_run(&app, json!({"repeat": 2, "start_input": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(job_state(&app).await, "waiting_for_start_input");
    let evs = drain(&mut events);
    assert!(evs.contains(&Event::machine("waiting_for_start_input", Some(id.clone()))), "{evs:?}");
    let counters = st.machine.sim_counters().unwrap().clone();
    assert_eq!(counters.emitting_packets(), 0, "nothing fires before the pedal");
    for n in 1..=2u32 {
        set_inputs(&app, 1 << 14).await;
        for _ in 0..400 {
            if st.runs().get(&id).is_none_or(|r| r.completed >= n || r.state.is_final()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        set_inputs(&app, 0).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let run = wait_final(&st, &id).await;
    assert_eq!((run.state, run.completed), (RunState::Finished, 2), "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);
}

/// JOB-057: with Require Full Pulse (the default) a pedal already held at approval never starts the job; with
/// `full_pulse: false` a held pedal starts it at once.
#[tokio::test]
async fn full_pulse_decides_whether_a_held_pedal_starts() {
    let st = state_with(pedal_profile(), test_projects()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();

    set_inputs(&app, 1 << 14).await;
    let id = request_run(&app, json!({"start_input": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(job_state(&app).await, "waiting_for_start_input", "held since before approval: no full pulse");
    assert_eq!(counters.emitting_packets(), 0);
    // Release, then press: that is a full pulse.
    set_inputs(&app, 0).await;
    tokio::time::sleep(Duration::from_millis(60)).await;
    set_inputs(&app, 1 << 14).await;
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);

    // full_pulse: false, pedal still held: starts without a release.
    let before = counters.emitting_packets();
    let id = request_run(&app, json!({"start_input": true, "full_pulse": false})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > before);
    set_inputs(&app, 0).await;
}

/// JOB-053: an active-low pedal (LightBurn StartMarkBitHigh = false) is pressed while its input reads 0: the input
/// resting high never starts the job, pulling it low does (after being seen released, the full pulse).
#[tokio::test]
async fn an_active_low_pedal_starts_on_a_low_input() {
    let p = MachineProfile { start_mark_active_low: true, ..pedal_profile() };
    let st = state_with(p, test_projects()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let counters = st.machine.sim_counters().unwrap().clone();
    // Released: the input rests high.
    set_inputs(&app, 1 << 14).await;
    let id = request_run(&app, json!({"start_input": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(job_state(&app).await, "waiting_for_start_input", "high is released for an active-low pedal");
    assert_eq!(counters.emitting_packets(), 0);
    set_inputs(&app, 0).await;
    let run = wait_final(&st, &id).await;
    assert_eq!(run.state, RunState::Finished, "{:?}", run.error);
    assert!(counters.emitting_packets() > 0);

    // A pedal held low since before approval is no full pulse.
    let before = counters.emitting_packets();
    let id = request_run(&app, json!({"start_input": true})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(counters.emitting_packets(), before, "held low since approval");
    // Released long enough for the wait to see it even on a busy test machine, then pressed.
    set_inputs(&app, 1 << 14).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    set_inputs(&app, 0).await;
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

/// Door Protect through the simulated inputs: an open door refuses approval; closed, the run goes.
#[tokio::test]
async fn the_door_input_refuses_approval_while_open() {
    let mut p = MachineProfile::generic_lmc_fiber();
    p.io.door_protect = Some(machine::InputPort { bit: 3, active_low: false });
    let st = state_with(p, test_projects()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    set_inputs(&app, 1 << 3).await;
    let id = request_run(&app, json!({})).await;
    let (s, v) = approve(&app, &id).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("door"), "{v}");
    set_inputs(&app, 0).await;
    let id = request_run(&app, json!({})).await;
    assert_eq!(approve(&app, &id).await.0, StatusCode::OK);
    assert_eq!(wait_final(&st, &id).await.state, RunState::Finished);
}

/// The Start Framing switch through the simulated inputs: released then pressed, it starts framing.
#[tokio::test]
async fn the_framing_switch_frames_through_the_api() {
    let mut p = MachineProfile::generic_lmc_fiber();
    p.io.start_framing = Some(machine::InputPort { bit: 5, active_low: false });
    let st = state_with(p, test_projects()).await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let watch = crate::libs::spawn_io_watch(st.clone());
    set_inputs(&app, 0).await;
    tokio::time::sleep(crate::libs::IO_POLL * 2).await;
    assert!(!st.machine.is_framing());
    set_inputs(&app, 1 << 5).await;
    let mut framing = false;
    for _ in 0..100 {
        if st.machine.is_framing() {
            framing = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(framing, "the switch started framing");
    send(&app, req("POST", "/api/machine/frame/stop", None)).await;
    set_inputs(&app, 0).await;
    watch.abort();
}

// ---------------------------------------------------------------- device details (IO bits, red dot: JOB-028)

#[tokio::test]
async fn device_details_report_every_io_bit_and_the_red_dot() {
    let mut p = pedal_profile();
    p.stop_mark_input_bit = Some(13);
    p.focus_light_port_bit = Some(9);
    p.io.door_protect = Some(machine::InputPort { bit: 3, active_low: true });
    p.io.busy_light = Some(5);
    p.red_dot.offset_x_mm = 0.4;
    let st = state_with(p, test_projects()).await;
    let app = api::router(st.clone(), None);
    let id = st.active_device().id;
    let (s, d) = send(&app, req("GET", &format!("/api/devices/{id}"), None)).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let prof = &d["profile"];
    assert_eq!((prof["start_mark_input_bit"].as_u64(), prof["stop_mark_input_bit"].as_u64()), (Some(14), Some(13)), "{prof}");
    assert_eq!(prof["laser_enable_port_bit"], 0);
    assert_eq!(prof["io"]["door_protect"], json!({"bit": 3, "active_low": true}));
    assert_eq!(
        prof["inputs"],
        json!([
            {"role": "start_marking", "bit": 14, "active_low": false},
            {"role": "stop_marking", "bit": 13, "active_low": false},
            {"role": "door_protect", "bit": 3, "active_low": true},
        ])
    );
    let outputs: Vec<(&str, u64)> =
        prof["outputs"].as_array().unwrap().iter().map(|o| (o["role"].as_str().unwrap(), o["bit"].as_u64().unwrap())).collect();
    assert_eq!(outputs, [("laser_enable", 0), ("red_light", 8), ("focus_light", 9), ("busy_light", 5)]);
    assert_eq!(prof["red_dot"]["offset_x_mm"], 0.4, "the red-dot values the editor shows (JOB-028)");
}

// ---------------------------------------------------------------- LAY-034 and LAY-042

const TABS_LBRN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.7.08" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="Outline"/>
        <maxPower Value="20"/>
        <speed Value="500"/>
        <tabsEnabled Value="1"/>
        <priority Value="0"/>
    </CutSetting>
    <CutSetting type="Tool">
        <index Value="30"/>
        <name Value="T1"/>
        <priority Value="1"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 75 75</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="30" W="40" H="40" Cr="0">
        <XForm>1 0 0 1 75 75</XForm>
    </Shape>
</LightBurnProject>"#;

#[tokio::test]
async fn unexecuted_lightburn_settings_are_reported_per_layer() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    let r = exec(&app, "doc.import_lbrn2", json!({"content": TABS_LBRN, "field_mm": 150})).await;
    let warnings = r["warnings"].to_string();
    assert!(warnings.contains("Tabs is on in the file but will not be executed"), "{warnings}");
    assert_eq!(r["unsupported_settings"][0]["unsupported"][0]["tag"], "tabsEnabled", "{r}");
    let (_, u) = send(&app, req("GET", "/api/layers/unsupported", None)).await;
    assert_eq!(u["layers"].as_array().unwrap().len(), 1, "only the Outline layer (T1 is a tool layer): {u}");
    assert_eq!(u["layers"][0]["name"], "Outline");
    let lid = u["layers"][0]["layer_id"].as_str().unwrap().to_owned();
    let (_, p) = send(&app, req("GET", "/api/preview", None)).await;
    let layer = p["layers"].as_array().unwrap().iter().find(|l| l["layer_id"] == lid.as_str()).cloned().unwrap();
    assert_eq!(layer["unsupported"][0]["setting"], "Tabs", "{layer}");
    assert_eq!(layer["unsupported"][0]["active"], true);
    // Turning OpenEtch's tabs on clears it.
    exec(&app, "layer.update", json!({"id": lid, "settings": {"options": {"tabs": {"count": 4, "size_mm": 0.5}}}})).await;
    let (_, u) = send(&app, req("GET", "/api/layers/unsupported", None)).await;
    assert_eq!(u["layers"], json!([]));
}

#[tokio::test]
async fn tool_layers_never_output_frame_with_the_job_and_export_as_tool_layers() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    exec(&app, "doc.import_lbrn2", json!({"content": TABS_LBRN, "field_mm": 150})).await;
    let d = st.document();
    let t1 = d.layers.iter().find(|l| l.tool == Some(doc::ToolLayer::T1)).expect("the import made T1 a tool layer").id.clone();
    assert!(!d.layer(&t1).unwrap().output);
    // Output cannot be turned on.
    let (_, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "layer.update", "params": {"id": t1, "output": true}})))).await;
    assert_eq!(v["ok"], false, "{v}");
    // The job frame includes the 40 mm tool rectangle; the plan does not mark it.
    let outline = crate::frame::outline_for(&d, &crate::frame::FrameRequest::default()).unwrap();
    let xs: Vec<f64> = outline[0].iter().map(|p| p.0).collect();
    assert!(xs.iter().cloned().fold(f64::MIN, f64::max) >= 19.9, "framed with the tool layer: {xs:?}");
    let (_, p) = send(&app, req("GET", "/api/preview", None)).await;
    let marks: Vec<f64> = p["segments"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["kind"] == "mark")
        .flat_map(|s| [s["from"][0].as_f64().unwrap(), s["to"][0].as_f64().unwrap()])
        .collect();
    assert!(marks.iter().all(|x| x.abs() <= 5.01), "only the 10 mm rectangle marks: {marks:?}");
    // A native tool layer exports as LightBurn's T2.
    let r = exec(&app, "layer.tool", json!({"tool": "T2"})).await;
    let t2 = r["id"].as_str().unwrap().to_owned();
    let (_, v) = send(&app, req("POST", "/api/execute", Some(json!({"id": "layer.tool", "params": {"tool": "T2"}})))).await;
    assert!(v["error"].as_str().unwrap().contains(&t2), "already there: {v}");
    let c = exec(&app, "shape.circle", json!({"diameter_mm": 6, "center": [0, 0], "layer": t2})).await;
    assert!(c["created"].as_array().is_some_and(|a| a.len() == 1), "{c}");
    let x = exec(&app, "doc.export_lbrn2", json!({})).await;
    let xml = x["content"].as_str().unwrap();
    assert!(xml.contains(r#"<CutSetting type="Tool">"#), "{xml}");
    assert!(xml.contains(r#"CutIndex="31""#) && xml.contains(r#"CutIndex="30""#), "{xml}");
    // And reads back as T2.
    exec(&app, "doc.import_lbrn2", json!({"content": xml, "field_mm": 150})).await;
    assert!(st.document().layers.iter().any(|l| l.tool == Some(doc::ToolLayer::T2)));
}

// ---------------------------------------------------------------- review: documents, runs and hostile libraries

/// A run carries the document it was made from, and approval checks it besides the revision: even a run whose
/// revision happens to match is never approved while another document is active.
#[tokio::test]
async fn a_run_is_only_approved_while_its_own_document_is_active() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    let run = request_run(&app, json!({})).await;
    let (_, m) = send(&app, req("GET", "/api/machine", None)).await;
    assert_eq!(m["pending_runs"][0]["document_id"], "default", "{m}");
    // Another document becomes active; the run is made to look current by revision alone.
    send(&app, req("POST", "/api/documents", None)).await;
    rect(&app, 0.0).await;
    {
        let mut store = st.runs();
        store.get_mut(&run).unwrap().revision = st.revision();
    }
    let (s, v) = approve(&app, &run).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().unwrap().contains("'default'"), "{v}");
    assert_eq!(st.runs().get(&run).unwrap().state, RunState::Cancelled);
    assert_eq!(st.machine.sim_counters().unwrap().laser_singles(), 0, "nothing marked");
    // A run of the active document still goes through.
    let own = request_run(&app, json!({})).await;
    assert_eq!(st.runs().get(&own).unwrap().document_id, "doc-2");
    let (s, v) = approve(&app, &own).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(wait_final(&st, &own).await.state, RunState::Finished);
}

/// Opening a project into a new document loads it into that document before it is activated, so the project can
/// never replace whatever document another client made active in the meantime.
#[tokio::test]
async fn a_project_opened_into_a_new_document_never_lands_in_another() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    rect(&app, 0.0).await;
    rect(&app, 20.0).await;
    send(&app, req("POST", "/api/projects/save", Some(json!({"name": "Alpha"})))).await;
    send(&app, req("POST", "/api/documents", None)).await;
    rect(&app, 0.0).await;
    let (_, v) = send(&app, req("POST", "/api/documents", Some(json!({"activate": false})))).await;
    let inactive = v["id"].as_str().unwrap().to_owned();
    let before = st.document();
    // The load itself, with doc-2 active: doc-2 is untouched.
    let r = st.load_project_into(&inactive, "alpha", commands::Origin::Ui).await.unwrap();
    assert_eq!(r["name"], "Alpha", "{r}");
    assert_eq!(st.document().objects, before.objects);
    assert_eq!((st.active_document_id(), st.revision()), ("doc-2".to_owned(), before.revision));
    let (_, docs) = send(&app, req("GET", "/api/documents", None)).await;
    let row = docs["documents"].as_array().unwrap().iter().find(|d| d["id"] == inactive.as_str()).cloned().unwrap();
    assert_eq!((row["project"].as_str(), row["objects"].as_u64(), row["dirty"].as_bool()), (Some("Alpha"), Some(2), Some(false)), "{docs}");
    // Activated, it is the clean project.
    send(&app, req("POST", &format!("/api/documents/{inactive}/activate"), None)).await;
    let (_, p) = send(&app, req("GET", "/api/project", None)).await;
    assert_eq!((p["name"].as_str(), p["dirty"].as_bool()), (Some("Alpha"), Some(false)), "{p}");
    // Not into the active document, nor a closed one.
    assert_eq!(st.load_project_into(&inactive, "Alpha", commands::Origin::Ui).await.unwrap_err().status(), StatusCode::CONFLICT);
    assert_eq!(st.load_project_into("doc-99", "Alpha", commands::Origin::Ui).await.unwrap_err().status(), StatusCode::CONFLICT);
    // The route: the new document holds the project and is active; the others kept their content.
    send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    send(&app, req("POST", &format!("/api/documents/{inactive}/close"), None)).await;
    assert_eq!(send(&app, req("POST", "/api/documents/default/close", None)).await.0, StatusCode::OK, "Alpha closed");
    let (s, v) = send(&app, req("POST", "/api/documents", Some(json!({"project": "Alpha"})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!((v["active"].as_str(), v["result"]["name"].as_str()), (v["id"].as_str(), Some("Alpha")), "{v}");
    assert_eq!(v["revision"], st.revision());
    assert_eq!(objects(&app).await, 2);
    send(&app, req("POST", "/api/documents/doc-2/activate", None)).await;
    assert_eq!(objects(&app).await, 1, "doc-2 kept its own content");
}

/// A hand-made `.oeart` is held to the rules of the engine's own: unusable item ids and names, and items a project
/// file could not hold (groups inside each other), are refused; a thumbnail that is not a PNG is never served as one.
#[tokio::test]
async fn hostile_art_libraries_are_refused_or_defused() {
    let st = state().await;
    let app = api::router(st.clone(), None);
    send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "Lib"})))).await;
    let a = rect(&app, 0.0).await;
    let b = rect(&app, 20.0).await;
    let c = rect(&app, 40.0).await;
    let g1 = exec(&app, "group.create", json!({"ids": [a, b], "name": "In"})).await["group"].as_str().unwrap().to_owned();
    let g2 = exec(&app, "group.create", json!({"ids": [g1, c], "name": "Out"})).await["group"].as_str().unwrap().to_owned();
    let (s, v) = send(&app, req("POST", "/api/art/libraries/Lib/items", Some(json!({"ids": [g2]})))).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (_, file) = raw(&app, req("GET", "/api/art/libraries/Lib/download", None)).await;
    let good: Value = serde_json::from_slice(&file).unwrap();
    let load = |v: Value, name: &str| raw_req("POST", &format!("/api/art/libraries/load?name={name}.oeart"), v.to_string().into_bytes());

    let mut cyclic = good.clone();
    let groups = cyclic["items"][0]["content"]["groups"].as_array_mut().unwrap();
    let ids: Vec<Value> = groups.iter().map(|g| g["id"].clone()).collect();
    groups[0]["parent"] = ids[1].clone();
    groups[1]["parent"] = ids[0].clone();
    let (s, v) = send(&app, load(cyclic, "Cyclic")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    for (field, value) in [("id", json!("../../x")), ("id", json!("")), ("name", json!("")), ("name", json!("a\u{1b}b"))] {
        let mut bad = good.clone();
        bad["items"][0][field] = value;
        assert_eq!(send(&app, load(bad, "Bad")).await.0, StatusCode::BAD_REQUEST, "{field}");
    }
    // A thumbnail holding something else (here a web page) is replaced by a rendered PNG.
    let mut html = good.clone();
    html["items"][0]["thumbnail_png_base64"] = json!("PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==");
    assert_eq!(send(&app, load(html, "Html")).await.0, StatusCode::CREATED);
    let (_, lib) = send(&app, req("GET", "/api/art/libraries/Html", None)).await;
    let (s, png) = raw(&app, req("GET", lib["items"][0]["thumbnail"].as_str().unwrap(), None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{:?}", &png[..png.len().min(16)]);
    // Add Selection of one object whose name is too long for an item still makes one.
    let d = rect(&app, 60.0).await;
    exec(&app, "object.rename", json!({"id": d, "name": "x".repeat(150)})).await;
    let (s, v) = send(&app, req("POST", "/api/art/libraries/Lib/items", Some(json!({"ids": [d]})))).await;
    assert_eq!((s, v["item"]["name"].as_str()), (StatusCode::CREATED, Some("Art")), "{v}");
}

/// A big library (2000 items) is read once for its window, not once per thumbnail, and a library file changed on
/// disk by something else is read again.
#[tokio::test]
async fn big_libraries_are_parsed_once_and_outside_changes_are_seen() {
    let projects = test_projects();
    let st = state_with(MachineProfile::generic_lmc_fiber(), projects.clone()).await;
    let app = api::router(st.clone(), None);
    send(&app, req("POST", "/api/art/libraries", Some(json!({"name": "Seed"})))).await;
    let a = rect(&app, 0.0).await;
    send(&app, req("POST", "/api/art/libraries/Seed/items", Some(json!({"ids": [a]})))).await;
    let (_, file) = raw(&app, req("GET", "/api/art/libraries/Seed/download", None)).await;
    let mut f: Value = serde_json::from_slice(&file).unwrap();
    let item = f["items"][0].clone();
    let items: Vec<Value> = (1..=2000)
        .map(|i| {
            let mut it = item.clone();
            it["id"] = json!(format!("art-{i}"));
            it
        })
        .collect();
    f["items"] = json!(items);
    f["next_id"] = json!(2001);
    let (s, v) = send(&app, raw_req("POST", "/api/art/libraries/load?name=Huge.oeart", f.to_string().into_bytes())).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let started = std::time::Instant::now();
    for i in 1..=200 {
        let (s, png) = raw(&app, req("GET", &format!("/api/art/libraries/Huge/items/art-{i}/thumbnail.png"), None)).await;
        assert!(s == StatusCode::OK && png.starts_with(b"\x89PNG"), "{s}");
    }
    // Parsing the 2.3 MB file per thumbnail took ~140 ms each (debug build): 200 of them ~28 s.
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    // Changed on disk by something else (another engine on the same folder, a copy): seen at once.
    f["items"] = json!([item]);
    let path = projects.root().join("art-libraries").join("Huge.oeart");
    std::fs::write(&path, f.to_string()).unwrap();
    let (_, lib) = send(&app, req("GET", "/api/art/libraries/Huge", None)).await;
    assert_eq!(lib["items"].as_array().unwrap().len(), 1, "{lib}");
    assert_eq!(send(&app, req("GET", "/api/art/libraries/Huge/items/art-2/thumbnail.png", None)).await.0, StatusCode::NOT_FOUND);
}
