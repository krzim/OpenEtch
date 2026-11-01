//! Routes of `userfiles`: exports, print, imports (also as a new project), backups, autosave, thumbnails,
//! templates, preference export/import/backups and user bundles.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::tests::test_state;
use crate::{AppState, api, userfiles};

fn app(st: &Arc<AppState>) -> Router {
    api::router(st.clone(), None)
}

fn req(method: &str, uri: &str, body: Body, json_body: bool) -> Request<Body> {
    let b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    let b = if json_body { b.header("content-type", "application/json") } else { b };
    b.body(body).unwrap()
}

/// Status, headers and body bytes.
async fn raw(app: &Router, r: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(r).await.unwrap();
    let (s, h) = (res.status(), res.headers().clone());
    (s, h, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn json(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (b, j) = match body {
        Some(v) => (Body::from(v.to_string()), true),
        None => (Body::empty(), false),
    };
    let (s, _, bytes) = raw(app, req(method, uri, b, j)).await;
    (s, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

async fn rect(app: &Router) {
    let (s, v) = json(app, "POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 5}}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true, "{v}");
}

#[tokio::test]
async fn exports_and_print_download_with_the_document_name() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app).await;
    let (s, h, body) = raw(&app, req("GET", "/api/export?format=svg&colors=black", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-disposition"].to_str().unwrap().contains("Untitled.svg"));
    assert!(String::from_utf8(body).unwrap().contains("<svg"));
    let (s, _, body) = raw(&app, req("GET", "/api/export?format=dxf", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains("AC1009"));
    let (s, h, body) = raw(&app, req("GET", "/api/print?paper=210,297", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-disposition"].to_str().unwrap().starts_with("inline"));
    assert!(body.starts_with(b"%PDF"));
    let (s, _, _) = raw(&app, req("GET", "/api/export?format=svg&bogus=1", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = json(&app, "GET", "/api/export?format=svg&ids=obj-99", None).await;
    assert_ne!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn imports_merge_or_open_as_a_new_project() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app).await;
    let hpgl = "IN;SP1;PU0,0;PD400,0,400,400;";
    let (s, v) = json(&app, "POST", "/api/document/import?format=hpgl", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "empty body: {v}");
    let (_, _, b) = raw(&app, req("POST", "/api/document/import?format=hpgl&width_mm=20", Body::from(hpgl), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().objects.len(), 2, "merged");
    // Open as a new project: the document is replaced and unsaved, named after the file.
    let (_, _, b) = raw(&app, req("POST", "/api/document/import?format=plt&mode=replace&name=plot.plt", Body::from(hpgl), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().objects.len(), 1);
    let p = st.project_status();
    assert_eq!((p["name"].clone(), p["display_name"].clone(), p["dirty"].clone()), (Value::Null, json!("plot"), json!(true)));
    // A refused file is 200 {ok: false}.
    let (_, _, b) = raw(&app, req("POST", "/api/document/import?format=eps", Body::from("not eps"), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["ok"], false, "{v}");
    // Legacy .lbrn through the LightBurn import.
    let lbrn = r#"<?xml version="1.0"?><LightBurnProject AppVersion="0.8" FormatVersion="0"><CutSetting type="Cut"><index Value="0"/></CutSetting><Shape Type="Path" CutIndex="0"><XForm>1 0 0 1 75 75</XForm><V vx="0" vy="0"/><V vx="10" vy="0"/><P T="L" p0="0" p1="1"/></Shape></LightBurnProject>"#;
    let (_, _, b) = raw(&app, req("POST", "/api/document/import?format=lbrn&mode=replace&name=old.lbrn", Body::from(lbrn), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(st.document().objects.len(), 1);
}

#[tokio::test]
async fn saves_make_backups_autosave_rotates_and_restores() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app).await;
    let (s, v) = json(&app, "POST", "/api/projects/save", Some(json!({"name": "Tag"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    rect(&app).await;
    json(&app, "POST", "/api/projects/save", Some(json!({"name": "Tag"}))).await;
    let (_, v) = json(&app, "GET", "/api/backups?project=Tag", None).await;
    assert_eq!(v["backups"].as_array().unwrap().len(), 1, "{v}");
    // Autosave writes only while dirty, and rotates.
    assert!(userfiles::autosave_now(&st, false).unwrap().is_none(), "saved: nothing to autosave");
    rect(&app).await;
    assert!(userfiles::autosave_now(&st, false).unwrap().is_some());
    assert!(userfiles::autosave_now(&st, false).unwrap().is_none(), "unchanged since");
    for _ in 0..12 {
        rect(&app).await;
        userfiles::autosave_now(&st, true).unwrap();
    }
    let (_, v) = json(&app, "GET", "/api/backups", None).await;
    let autosaves = v["autosaves"].as_array().unwrap();
    assert_eq!(autosaves.len(), 10, "rotation keeps 10");
    let file = autosaves[0]["file"].as_str().unwrap().to_string();
    let n = st.document().objects.len();
    json(&app, "POST", "/api/execute", Some(json!({"id": "doc.new", "params": {}}))).await;
    let (s, v) = json(&app, "POST", "/api/backups/restore", Some(json!({"kind": "autosave", "file": file}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(st.document().objects.len(), n);
    let (s, _) = json(&app, "POST", "/api/backups/restore", Some(json!({"kind": "autosave", "file": "../x.openetch"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // Settings.
    let (s, v) = json(&app, "PUT", "/api/files/settings", Some(json!({"autosave_minutes": 0}))).await;
    assert_eq!((s, v["autosave_minutes"].clone()), (StatusCode::OK, json!(0)));
    let (s, _) = json(&app, "PUT", "/api/files/settings", Some(json!({"autosave_keep": 0}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // Thumbnails of saved projects.
    let (s, h, png) = raw(&app, req("GET", "/api/projects/Tag/thumbnail.png?width=64", Body::empty(), false)).await;
    assert_eq!((s, h["content-type"].to_str().unwrap()), (StatusCode::OK, "image/png"));
    assert!(png.starts_with(b"\x89PNG"));
    let (s, _, _) = raw(&app, req("GET", "/api/projects/Nope/thumbnail.png", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn templates_open_as_untitled_copies() {
    let st = test_state().await;
    let app = app(&st);
    rect(&app).await;
    let (s, v) = json(&app, "POST", "/api/templates/save", Some(json!({"name": "Jig"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _) = json(&app, "POST", "/api/templates/save", Some(json!({"name": "jig"}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let lbt = r#"<?xml version="1.0"?><LightBurnProject AppVersion="1.4.00" FormatVersion="1"><CutSetting type="Cut"><index Value="0"/></CutSetting></LightBurnProject>"#;
    let (_, _, b) = raw(&app, req("POST", "/api/templates/upload?name=Coaster", Body::from(lbt), false)).await;
    assert!(String::from_utf8_lossy(&b).contains("\"ok\":true"), "{}", String::from_utf8_lossy(&b));
    let (_, v) = json(&app, "GET", "/api/templates", None).await;
    let names: Vec<(&str, &str)> = v.as_array().unwrap().iter().map(|t| (t["name"].as_str().unwrap(), t["kind"].as_str().unwrap())).collect();
    assert_eq!(names, [("Coaster", "lbt"), ("Jig", "openetch")]);
    json(&app, "POST", "/api/execute", Some(json!({"id": "doc.new", "params": {}}))).await;
    let (s, v) = json(&app, "POST", "/api/templates/open", Some(json!({"name": "Jig"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(st.document().objects.len(), 1);
    assert_eq!(st.project_status()["name"], Value::Null, "an untitled copy");
    let (s, v) = json(&app, "POST", "/api/templates/open", Some(json!({"name": "Coaster"}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn prefs_export_import_backups_and_bundles() {
    let st = test_state().await;
    let dir = crate::pin::temp_dir("userfiles");
    st.libs.open(&dir);
    st.files.open(&dir);
    let app = app(&st);
    let (s, _, body) = raw(&app, req("GET", "/api/prefs/export", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    let mut file: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(file["format"], "openetch-prefs");
    file["preferences"]["debug_log"] = json!(true);
    file["files"]["autosave_minutes"] = json!(7);
    let (_, _, b) = raw(&app, req("POST", "/api/prefs/import", Body::from(file.to_string()), true)).await;
    assert!(String::from_utf8_lossy(&b).contains("\"ok\":true"), "{}", String::from_utf8_lossy(&b));
    assert!(crate::libs::lock(&st.libs.prefs).prefs.debug_log);
    assert_eq!(st.files.settings().autosave_minutes, 7);
    // MCP clients cannot import preferences.
    let r = Request::builder().method("POST").uri("/api/prefs/import").header("host", "localhost:7878").header("mcp-session-id", "x").body(Body::from(file.to_string())).unwrap();
    let (s, _, _) = raw(&app, r).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, v) = json(&app, "GET", "/api/prefs/backups", None).await;
    assert!(!v.as_array().unwrap().is_empty(), "the import backed the old preferences up: {v}");
    let backup = v[0]["file"].as_str().unwrap().to_string();
    let (s, v) = json(&app, "POST", "/api/prefs/backups/restore", Some(json!({"file": backup}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // A bundle of preferences, devices and libraries imports into another engine.
    let (s, _, zip) = raw(&app, req("GET", "/api/bundle/export", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    let names = fileio::zip::names(&zip).unwrap();
    assert!(names.contains(&"bundle.json".to_string()) && names.contains(&"prefs.oeprefs".to_string()), "{names:?}");
    let other = test_state().await;
    let dir2 = crate::pin::temp_dir("userfiles2");
    other.libs.open(&dir2);
    other.files.open(&dir2);
    let app2 = crate::api::router(other.clone(), None);
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import?parts=prefs", Body::from(zip.clone()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["imported"]["prefs"], true, "{v}");
    let (s, _, _) = raw(&app2, req("POST", "/api/bundle/import?parts=bogus", Body::from(zip), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let bad = fileio::zip::write(&[fileio::zip::Entry { name: "x".into(), data: vec![1] }]).unwrap();
    let (s, _, _) = raw(&app2, req("POST", "/api/bundle/import", Body::from(bad), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// A bundle from elsewhere cannot write outside the config directory, point a device at any file, raise a device's
/// limits above this machine's, overwrite an unloaded library or bring in a checklist that cannot be ticked.
#[tokio::test]
async fn hostile_bundles_are_contained() {
    let st = test_state().await;
    let dir = crate::pin::temp_dir("userfiles-hostile");
    st.libs.open(&dir);
    st.files.open(&dir);
    let app = app(&st);
    let manifest = fileio::zip::Entry { name: "bundle.json".into(), data: br#"{"format":"openetch-bundle","version":1}"#.to_vec() };
    let device = |name: &str, edit: &dyn Fn(&mut machine::MachineProfile)| {
        let mut d = st.devices().list()[0].clone();
        d.display_name = name.into();
        edit(d.profile.as_mut().unwrap());
        fileio::zip::Entry { name: format!("devices/{name}.toml"), data: d.to_toml().unwrap().into_bytes() }
    };
    let entries = vec![
        manifest.clone(),
        // `C:x.cor` joined onto the devices directory is drive C's current folder on Windows.
        fileio::zip::Entry { name: "devices/C:escape.cor".into(), data: b"x".to_vec() },
        fileio::zip::Entry { name: "devices/..\\up.cor".into(), data: b"x".to_vec() },
        fileio::zip::Entry { name: "devices/CON.cor".into(), data: b"x".to_vec() },
        device("Raised", &|p| p.max_jump_speed_mm_s *= 10.0),
        device("Share", &|p| p.cor_file = Some(r"\\evil\share\lens.cor".into())),
        device("Fine", &|_| {}),
    ];
    let zip = fileio::zip::write(&entries).unwrap();
    let (_, _, b) = raw(&app, req("POST", "/api/bundle/import?parts=devices", Body::from(zip), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    let skipped = v["skipped"].to_string();
    assert_eq!(v["imported"]["devices"].as_array().unwrap().len(), 1, "{v}");
    assert!(skipped.contains("C:escape.cor") && skipped.contains("CON.cor") && skipped.contains("Raised") && skipped.contains("Share"), "{v}");
    assert!(!std::path::Path::new("escape.cor").exists() && !dir.join("up.cor").exists());
    // An unloaded library's file is kept: the bundle's copy loads as "<name> (bundle)".
    let clb = r#"<?xml version="1.0" encoding="UTF-8"?><LightBurnLibrary><Material name="Steel"><Entry Thickness="-1.0000" Desc="Engrave" NoThickTitle="Finish"><CutSetting type="Scan"><index Value="0"/><name Value=""/><maxPower Value="38"/><speed Value="1000"/></CutSetting></Entry></Material></LightBurnLibrary>"#;
    let libs = dir.join(crate::library::LIBRARIES_DIR);
    std::fs::create_dir_all(&libs).unwrap();
    std::fs::write(libs.join("Steel.clb"), "mine").unwrap();
    let zip = fileio::zip::write(&[manifest.clone(), fileio::zip::Entry { name: "libraries/Steel.clb".into(), data: clb.as_bytes().to_vec() }]).unwrap();
    let (_, _, b) = raw(&app, req("POST", "/api/bundle/import?parts=libraries", Body::from(zip), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["imported"]["libraries"], json!(["Steel (bundle)"]), "{v}");
    assert_eq!(std::fs::read_to_string(libs.join("Steel.clb")).unwrap(), "mine");
    // Preferences with an enabled, empty Job Checklist (nothing to tick) are refused whole, the current ones kept.
    let before = crate::libs::lock(&st.libs.prefs).prefs.clone();
    let prefs = json!({"format": "openetch-prefs", "version": 1, "preferences": {"devices": {"b6": {"require_framing": false, "checklist": {"enabled": true, "items": []}}}}});
    let zip = fileio::zip::write(&[manifest, fileio::zip::Entry { name: "prefs.oeprefs".into(), data: prefs.to_string().into_bytes() }]).unwrap();
    let (s, _, _) = raw(&app, req("POST", "/api/bundle/import?parts=prefs", Body::from(zip), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(crate::libs::lock(&st.libs.prefs).prefs, before);
    let bad_files = json!({"format": "openetch-prefs", "version": 1, "preferences": {}, "files": {"autosave_keep": 0}});
    let (s, _, _) = raw(&app, req("POST", "/api/prefs/import", Body::from(bad_files.to_string()), true)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(crate::libs::lock(&st.libs.prefs).prefs, before, "nothing changed by a half-valid file");
}

#[tokio::test]
async fn images_download_and_processed_bitmap() {
    let st = test_state().await;
    let app = app(&st);
    let img = image::GrayImage::from_pixel(4, 4, image::Luma([0]));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    let (_, v) = json(&app, "POST", "/api/execute", Some(json!({"id": "art.import_image", "params": {"data_base64": data, "mime": "image/png", "width_mm": 4}}))).await;
    let id = v["result"]["created"][0].as_str().unwrap().to_string();
    let (s, _, body) = raw(&app, req("GET", &format!("/api/images/{id}/file"), Body::empty(), false)).await;
    assert_eq!((s, body), (StatusCode::OK, png));
    let (s, h, body) = raw(&app, req("GET", &format!("/api/images/{id}/processed.png"), Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-disposition"].to_str().unwrap().contains("processed.png"));
    assert!(body.starts_with(b"\x89PNG"));
}
