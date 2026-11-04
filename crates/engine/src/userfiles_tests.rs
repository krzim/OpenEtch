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
    // An import is held to the rules of PUT /api/prefs: console macros pass the console policy, saved Move positions
    // belong to a GRBL gantry's bed (the active device here is a galvo) and have sane names.
    let id = st.active_device().id;
    for d in [
        json!({"console_macros": [{"name": "Fire", "lines": ["M3 S1000"]}]}),
        json!({"move_positions": [{"name": "Jig", "x_mm": 10.0, "y_mm": 10.0}]}),
    ] {
        let mut devices = serde_json::Map::new();
        devices.insert(id.clone(), d);
        let file = json!({"format": "openetch-prefs", "version": 1, "preferences": {"devices": devices}});
        let (s, _, b) = raw(&app, req("POST", "/api/prefs/import", Body::from(file.to_string()), true)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&b));
        assert_eq!(crate::libs::lock(&st.libs.prefs).prefs, before);
    }
    // A device this engine does not know keeps its positions if their names are sane.
    let unknown = |name: &str| json!({"format": "openetch-prefs", "version": 1, "preferences": {"devices": {"elsewhere": {"move_positions": [{"name": name, "x_mm": 10.0, "y_mm": 10.0}]}}}});
    let (s, _, _) = raw(&app, req("POST", "/api/prefs/import", Body::from(unknown(" ").to_string()), true)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _, b) = raw(&app, req("POST", "/api/prefs/import", Body::from(unknown("Jig").to_string()), true)).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
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

/// LightBurn's .lbprefs / prefs.ini: a preview changes nothing; an import sets autosave and the active device's
/// Optimization defaults (backing the old preferences up) and returns the browser-side settings; MCP cannot import.
#[tokio::test]
async fn lightburn_prefs_import() {
    let st = test_state().await;
    let dir = crate::pin::temp_dir("lbprefs");
    st.libs.open(&dir);
    st.files.open(&dir);
    let app = app(&st);
    let file = json!({
        "AutoSaveSeconds": 600, "Optimize_ByLayer": 1, "Optimize_ByPriority": 0, "Optimize_ByGroup": -1,
        "Optimize_InnerToOuter": true, "GridSpacing": 5, "NormalMoveIncrement": 2, "NestMultisheet": true,
        "LicenseKey": "SECRET-1", "DeviceList": [{"name": "SECRET-2"}]
    })
    .to_string();
    let (s, _, b) = raw(&app, req("POST", "/api/prefs/import_lightburn?preview=1", Body::from(file.clone()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["preview"].as_bool(), v["other_keys"].as_u64()), (Some(true), Some(2)));
    assert_eq!(v["engine"]["autosave_minutes"], 10);
    assert_eq!(v["engine"]["optimization"]["order_by"], json!(["priority", "layer"]));
    assert_eq!(v["browser"]["prefs"], json!({"gridMm": 5.0, "nudge": {"normal": 2.0}}));
    assert_eq!(v["browser"]["dialogs"]["nest"]["multipage"], true);
    assert!(!String::from_utf8_lossy(&b).contains("SECRET"));
    assert_ne!(st.files.settings().autosave_minutes, 10, "a preview changes nothing");
    let device = st.active_device().id;
    let (s, _, b) = raw(&app, req("POST", "/api/prefs/import_lightburn", Body::from(file.clone()), false)).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    assert_eq!(st.files.settings().autosave_minutes, 10);
    let o = crate::libs::lock(&st.libs.prefs).device(&device).optimization_defaults.unwrap();
    assert!(o.inner_first);
    assert_eq!(o.order_by, vec![doc::OrderBy::Priority, doc::OrderBy::Layer]);
    let saved = std::fs::read_to_string(dir.join(crate::prefs::PREFS_FILE)).unwrap_or_default();
    assert!(!saved.contains("SECRET"));
    let (_, v) = json(&app, "GET", "/api/prefs/backups", None).await;
    assert!(!v.as_array().unwrap().is_empty(), "{v}");
    let r = Request::builder().method("POST").uri("/api/prefs/import_lightburn").header("host", "localhost:7878").header("mcp-session-id", "x").body(Body::from(file)).unwrap();
    let (s, _, _) = raw(&app, r).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = raw(&app, req("POST", "/api/prefs/import_lightburn?preview=1", Body::from("[1]"), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// A state with its own config directory (preferences, presets, devices).
async fn configured_state(tag: &str) -> Arc<AppState> {
    let st = test_state().await;
    let dir = crate::pin::temp_dir(tag);
    st.libs.open(&dir);
    st.files.open(&dir);
    st
}

/// A LightBurn bundle laid out as LightBurn 2.2 writes one: the parts plus `LightBurnSoftware.json`, last.
fn lightburn_bundle(parts: &[(&str, &str, &str, Vec<u8>)]) -> Vec<u8> {
    let items: Vec<Value> = parts
        .iter()
        .map(|(action, file, name, data)| {
            json!({"action": action, "filename": file, "group": "Application", "icon": "filetype:x", "name": name, "size": data.len(), "source": file})
        })
        .collect();
    let mut entries: Vec<fileio::zip::Entry> =
        parts.iter().filter(|(_, f, _, _)| !f.contains("..") && !f.starts_with("missing")).map(|(_, f, _, d)| fileio::zip::Entry { name: (*f).into(), data: d.clone() }).collect();
    entries.push(fileio::zip::Entry { name: crate::lbzip::MANIFEST.into(), data: crate::lbzip::manifest_text(&items).into_bytes() });
    fileio::zip::write(&entries).unwrap()
}

/// LightBurn's `.lbzip` bundles: settings through the whitelist, devices, material test presets and Art Libraries
/// imported; unknown actions and unsafe names skipped; OpenEtch's own `.lbzip` export reads back.
#[tokio::test]
async fn lightburn_bundles_import_and_export() {
    let st = configured_state("lbzip-a").await;
    let app = app(&st);
    // Parts: a preset (made here, exported as LightBurn's .lbmt), a device, settings, an art library.
    let params = json!({"columns": {"param": "frequency", "min": 100, "max": 400, "count": 4}, "rows": {"param": "power", "min": 10, "max": 40, "count": 4}});
    let (s, v) = json(&app, "PUT", "/api/material-test/presets/Steel", Some(json!({"params": params}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, _, lbmt) = raw(&app, req("GET", "/api/material-test/presets/export", Body::empty(), false)).await;
    let lbdev = include_str!("../../machine/tests/fixtures/gantry.lbdev").as_bytes().to_vec();
    let png = crate::render::svg_to_png(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"><rect width="1" height="1" fill="#fff"/></svg>"##, 256).unwrap();
    let art_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnShapes FormatVersion=\"1\" MirrorX=\"False\" MirrorY=\"False\">\n    <Shape Type=\"Rect\" CutIndex=\"0\" W=\"20\" H=\"10\" Cr=\"0\">\n        <XForm>1 0 0 1 34 130</XForm>\n    </Shape>\n</LightBurnShapes>\n";
    let lbart = fileio::lbart::write(&fileio::lbart::Lbart { items: vec![fileio::lbart::Item::new("Rect20x10", png, art_xml.into(), [20.0, 10.0])] }).unwrap();
    let prefs = br#"{"AutoSaveSeconds": 600, "LicenseKey": "never read", "RecentFiles": ["C:/secret.lbrn2"]}"#.to_vec();
    let bundle = lightburn_bundle(&[
        ("import-settings", "settings.lbprefs", "Settings", prefs),
        ("import-materialtestpresets", "material_test_presets.lbmt", "Material Test Presets", lbmt),
        ("import-device", "aBcD1234.lbdev", "Bed", lbdev),
        ("import-artlib", "clips.lbart", "clips", lbart),
        ("import-hotkeys", "hotkeys.json", "Hotkeys", br#"{"actionGroup": "Ctrl+Shift+G"}"#.to_vec()),
        ("import-artlib", "../escape.lbart", "escape", b"x".to_vec()),
        ("import-artlib", "missing.lbart", "missing", b"x".to_vec()),
    ]);

    let other = configured_state("lbzip-b").await;
    let app2 = app_of(&other);
    let (s, _, b) = raw(&app2, req("POST", "/api/bundle/import", Body::from(bundle.clone()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["lightburn"], true, "{v}");
    assert_eq!(v["imported"]["prefs"], true, "{v}");
    assert_eq!(other.files.settings().autosave_minutes, 10, "AutoSaveSeconds 600 → 10 minutes");
    assert_eq!(v["imported"]["presets"], json!(["Steel"]), "{v}");
    assert_eq!(v["imported"]["devices"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["imported"]["art"], json!(["clips"]), "{v}");
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("not a plain file name") && skipped.contains("missing from the bundle"), "{v}");
    assert_eq!(v["hotkeys"], json!({"actionGroup": "Ctrl+Shift+G"}), "the hotkeys go to the browser: {v}");
    assert!(!v.to_string().contains("secret") && !v.to_string().contains("never read"), "other keys are never echoed: {v}");
    // Again: the art library's name is taken, so it loads as "<name> (bundle)"; parts can be chosen.
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import?parts=art", Body::from(bundle), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!((v["imported"]["art"].clone(), v["imported"]["prefs"].clone()), (json!(["clips (bundle)"]), json!(false)), "{v}");

    // OpenEtch's .lbzip: LightBurn's layout, its manifest last; it imports into a third engine.
    let (s, h, zip) = raw(&app2, req("GET", "/api/bundle/export?format=lbzip", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h["content-disposition"].to_str().unwrap().contains(".lbzip"));
    let names = fileio::zip::names(&zip).unwrap();
    assert_eq!(names.last().map(String::as_str), Some("LightBurnSoftware.json"), "{names:?}");
    assert!(names.contains(&"material_test_presets.lbmt".to_string()) && names.contains(&"clips.lbart".to_string()), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with(".lbdev")), "{names:?}");
    let manifest: Value = serde_json::from_slice(&fileio::zip::read_one(&zip, "LightBurnSoftware.json").unwrap().unwrap()).unwrap();
    let m = &manifest["LightBurnSoftware"];
    assert_eq!(m["version"], json!({"Bundle": "1.0", "LightBurn": "2.2.00"}));
    for it in m["manifest"].as_array().unwrap() {
        let data = fileio::zip::read_one(&zip, it["filename"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(it["size"].as_u64(), Some(data.len() as u64), "{it}");
        assert_eq!(it["source"], it["filename"]);
    }
    let actions: Vec<&str> = m["manifest"].as_array().unwrap().iter().map(|i| i["action"].as_str().unwrap()).collect();
    assert!(actions.contains(&"import-artlib") && actions.contains(&"import-device") && actions.contains(&"import-materialtestpresets"), "{actions:?}");
    let third = configured_state("lbzip-c").await;
    let (_, _, b) = raw(&app_of(&third), req("POST", "/api/bundle/import?parts=presets,art", Body::from(zip), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["imported"]["presets"], json!(["Steel"]), "{v}");
    assert_eq!(v["imported"]["art"].as_array().unwrap().len(), 2, "{v}");
    let (s, _, _) = raw(&app2, req("GET", "/api/bundle/export?format=rar", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

/// UX-068: material libraries (`import-cutlib`) and Adjust Image presets (`import-imagepresets`) travel in `.lbzip`
/// bundles both ways, with the manifest fields LightBurn 2.2 writes (`crate::lbzip`'s docs); a library name taken
/// loads as "<name> (bundle)"; tool libraries are skipped with their reason.
#[tokio::test]
async fn lightburn_bundles_carry_material_libraries_and_image_presets() {
    let st = configured_state("lbzip-lib-a").await;
    let app = app(&st);
    let clb = r#"<?xml version="1.0" encoding="UTF-8"?><LightBurnLibrary><Material name="Steel"><Entry Thickness="-1.0000" Desc="Engrave" NoThickTitle="Finish"><CutSetting type="Scan"><index Value="0"/><name Value=""/><maxPower Value="38"/><speed Value="1000"/></CutSetting></Entry></Material></LightBurnLibrary>"#;
    crate::libs::lock(&st.libs.library).load("Fiber 60W", clb.as_bytes()).unwrap();
    let body = json!({"adjust": {"gamma": 1.4, "contrast": 20, "brightness": -11}, "image": {"dither": "jarvis", "interval_mm": 0.05}});
    let (s, v) = json(&app, "PUT", "/api/image-adjust/presets/Slate%20photo", Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Export: LightBurn's items, Application first, then devices, then libraries.
    let (s, _, zip) = raw(&app, req("GET", "/api/bundle/export?format=lbzip", Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    let manifest: Value = serde_json::from_slice(&fileio::zip::read_one(&zip, "LightBurnSoftware.json").unwrap().unwrap()).unwrap();
    let items = manifest["LightBurnSoftware"]["manifest"].as_array().unwrap().clone();
    let pick = |i: &Value| json!([i["action"], i["filename"], i["group"], i["icon"], i["name"]]);
    assert!(items.iter().any(|i| pick(i) == json!(["import-imagepresets", "image_presets.lbimg", "Application", "filetype:image-presets", "Image Presets"])), "{items:?}");
    assert!(items.iter().any(|i| pick(i) == json!(["import-cutlib", "Fiber 60W.clb", "Material Libraries", "filetype:material-library", "Fiber 60W"])), "{items:?}");
    let groups: Vec<&str> = items.iter().map(|i| i["group"].as_str().unwrap()).collect();
    let rank = |g: &str| ["Application", "Devices", "Material Libraries", "Art Libraries"].iter().position(|x| *x == g).unwrap();
    assert!(groups.windows(2).all(|w| rank(w[0]) <= rank(w[1])), "{groups:?}");
    for it in &items {
        let data = fileio::zip::read_one(&zip, it["filename"].as_str().unwrap()).unwrap().unwrap();
        assert_eq!(it["size"].as_u64(), Some(data.len() as u64), "{it}");
    }

    // Import into another engine: the library's content and the preset come back.
    let other = configured_state("lbzip-lib-b").await;
    let app2 = app_of(&other);
    let (s, _, b) = raw(&app2, req("POST", "/api/bundle/import?parts=libraries,images", Body::from(zip.clone()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["imported"]["libraries"].clone(), v["imported"]["images"].clone()), (json!(["Fiber 60W"]), json!(["Slate photo"])), "{v}");
    let text = |st: &Arc<AppState>, n: &str| crate::libs::lock(&st.libs.library).text(n).unwrap().1;
    assert_eq!(text(&other, "Fiber 60W"), text(&st, "Fiber 60W"));
    let (_, mine) = json(&app, "GET", "/api/image-adjust/presets", None).await;
    let (_, theirs) = json(&app2, "GET", "/api/image-adjust/presets", None).await;
    assert_eq!(theirs["presets"][0]["adjust"], mine["presets"][0]["adjust"], "{theirs}");
    assert_eq!(theirs["presets"][0]["image"]["dither"], "jarvis", "{theirs}");
    assert_eq!(theirs["presets"][0]["image"]["interval_mm"], 0.05, "{theirs}");
    // Again: the same library is already here; the preset's name is taken.
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import?parts=libraries,images", Body::from(zip), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!((v["imported"]["libraries"].clone(), v["imported"]["images"].clone()), (json!([]), json!(["Slate photo (2)"])), "{v}");
    assert!(v["skipped"].to_string().contains("already here"), "{v}");

    // A different library of a taken name loads as "<name> (bundle)"; a name that is not a plain one falls back to the
    // file's; a tool library is skipped with its reason.
    let clb2 = clb.replace("Steel", "Brass");
    let bundle = lightburn_bundle(&[
        ("import-cutlib", "Fiber 60W.clb", "Fiber 60W", clb2.clone().into_bytes()),
        ("import-cutlib", "Other.clb", "../evil", clb2.into_bytes()),
        ("import-hotkeys", "hotkeys.json", "Hotkeys", b"{}".to_vec()),
        ("import-toollib", "tools.lbtools", "Tools", b"{}".to_vec()),
    ]);
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import", Body::from(bundle), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["imported"]["libraries"], json!(["Fiber 60W (bundle)", "Other"]), "{v}");
    let skipped = v["skipped"].to_string();
    assert!(skipped.contains("gantry/CNC tool library"), "{v}");
    assert_eq!(v["hotkeys"], json!({}), "{v}");
    assert!(text(&other, "Fiber 60W").contains("Steel") && text(&other, "Fiber 60W (bundle)").contains("Brass"));
}

/// Percent-encode a query value.
fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// UX-068: a device's linked material library (`link-filename` / `link-name`, as LightBurn 2.2 writes them) becomes
/// the device's library on import, under the name the library got, and is written on export; LightBurn's
/// `hotkeys.json` goes to the browser (with the prefs part) and the browser's comes back in an export, checked.
#[tokio::test]
async fn lightburn_bundles_carry_hotkeys_and_device_library_links() {
    let clb = r#"<?xml version="1.0" encoding="UTF-8"?><LightBurnLibrary><Material name="Steel"><Entry Thickness="-1.0000" Desc="Mark" NoThickTitle="Finish"><CutSetting type="Scan"><index Value="0"/><name Value=""/><maxPower Value="20"/><speed Value="1000"/></CutSetting></Entry></Material></LightBurnLibrary>"#;
    let lbdev = include_str!("../../machine/tests/fixtures/gantry.lbdev").as_bytes().to_vec();
    let hotkeys = b"{\n    \"actionDistribute_Horizontal_Centered\": \"Ctrl+Alt+Shift+D\"\n}\n".to_vec();
    let item = |action: &str, file: &str, group: &str, name: &str, size: usize| {
        json!({"action": action, "filename": file, "group": group, "icon": "filetype:x", "name": name, "size": size, "source": file})
    };
    let mut device = item("import-device", "dev1.lbdev", "Devices", "Bed", lbdev.len());
    device["link-filename"] = json!("Mats.clb");
    device["link-name"] = json!("Mats");
    let items = vec![
        item("import-hotkeys", "hotkeys.json", "Application", "Hotkeys", hotkeys.len()),
        device,
        item("import-cutlib", "Mats.clb", "Material Libraries", "Mats", clb.len()),
    ];
    let bundle = |items: &[Value]| {
        fileio::zip::write(&[
            fileio::zip::Entry { name: "hotkeys.json".into(), data: hotkeys.clone() },
            fileio::zip::Entry { name: "dev1.lbdev".into(), data: lbdev.clone() },
            fileio::zip::Entry { name: "Mats.clb".into(), data: clb.as_bytes().to_vec() },
            fileio::zip::Entry { name: crate::lbzip::MANIFEST.into(), data: crate::lbzip::manifest_text(items).into_bytes() },
        ])
        .unwrap()
    };

    let st = configured_state("lbzip-link-a").await;
    let app = app(&st);
    // A library of the same name already here: the bundle's loads as "Mats (bundle)", and the device links that.
    crate::libs::lock(&st.libs.library).load("Mats", clb.replace("Steel", "Brass").as_bytes()).unwrap();
    let (s, _, b) = raw(&app, req("POST", "/api/bundle/import", Body::from(bundle(&items)), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["hotkeys"], json!({"actionDistribute_Horizontal_Centered": "Ctrl+Alt+Shift+D"}), "{v}");
    assert_eq!(v["imported"]["libraries"], json!(["Mats (bundle)"]), "{v}");
    let id = v["imported"]["devices"][0].as_str().unwrap().to_owned();
    assert_eq!(v["imported"]["links"], json!([{"device": id, "library": "Mats (bundle)"}]), "{v}");
    assert_eq!(crate::libs::lock(&st.libs.prefs).device(&id).library.as_deref(), Some("Mats (bundle)"));

    // Without the prefs part no hotkeys; without the libraries part no link.
    let other = configured_state("lbzip-link-b").await;
    let app2 = app_of(&other);
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import?parts=devices", Body::from(bundle(&items)), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!((v["hotkeys"].clone(), v["imported"]["links"].clone()), (Value::Null, json!([])), "{v}");
    // A hotkeys file that is not LightBurn's is refused whole and reported.
    let mut bad = items.clone();
    bad[0]["filename"] = json!("hotkeys.json");
    let evil = fileio::zip::write(&[
        fileio::zip::Entry { name: "hotkeys.json".into(), data: br#"{"rm -rf": "Ctrl+A"}"#.to_vec() },
        fileio::zip::Entry { name: crate::lbzip::MANIFEST.into(), data: crate::lbzip::manifest_text(&bad[..1]).into_bytes() },
    ])
    .unwrap();
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import", Body::from(evil), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["hotkeys"], Value::Null, "{v}");
    assert!(v["skipped"].to_string().contains("is not a LightBurn action name"), "{v}");

    // Export: the browser's hotkeys first (Qt's JSON), the device linked to its library in the bundle.
    let keys = r#"{"actionGroup":"Ctrl+Shift+G","actionUngroup":""}"#;
    let uri = format!("/api/bundle/export?format=lbzip&parts=prefs,devices,libraries&hotkeys={}", enc(keys));
    let (s, _, zip) = raw(&app, req("GET", &uri, Body::empty(), false)).await;
    assert_eq!(s, StatusCode::OK);
    let file = String::from_utf8(fileio::zip::read_one(&zip, "hotkeys.json").unwrap().unwrap()).unwrap();
    assert_eq!(file, "{\n    \"actionGroup\": \"Ctrl+Shift+G\",\n    \"actionUngroup\": \"\"\n}\n");
    let manifest: Value = serde_json::from_slice(&fileio::zip::read_one(&zip, "LightBurnSoftware.json").unwrap().unwrap()).unwrap();
    let list = manifest["LightBurnSoftware"]["manifest"].as_array().unwrap().clone();
    assert_eq!(json!([list[0]["action"], list[0]["icon"], list[0]["name"]]), json!(["import-hotkeys", "filetype:hotkeys", "Hotkeys"]));
    let dev = list.iter().find(|i| i["action"] == "import-device" && i["filename"].as_str().unwrap().starts_with(&id)).unwrap();
    assert_eq!((dev["link-filename"].clone(), dev["link-name"].clone()), (json!("Mats (bundle).clb"), json!("Mats (bundle)")), "{dev}");
    assert!(list.iter().any(|i| i["action"] == "import-cutlib" && i["filename"] == "Mats (bundle).clb"), "{list:?}");
    // And back: the export imports with its link and hotkeys.
    let (_, _, b) = raw(&app2, req("POST", "/api/bundle/import", Body::from(zip), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["hotkeys"], serde_json::from_str::<Value>(keys).unwrap(), "{v}");
    assert!(v["imported"]["links"].as_array().is_some_and(|l| l.iter().any(|x| x["library"] == "Mats (bundle)")), "{v}");
    // Hotkeys are refused in an .oebundle, when they are not JSON or not LightBurn's.
    for uri in [
        format!("/api/bundle/export?hotkeys={}", enc(keys)),
        "/api/bundle/export?format=lbzip&hotkeys=%7B".to_string(),
        format!("/api/bundle/export?format=lbzip&hotkeys={}", enc(r#"{"group": "Ctrl+G"}"#)),
        format!("/api/bundle/export?format=lbzip&hotkeys={}", enc(r#"{"actionGroup": 3}"#)),
    ] {
        let (s, _, _) = raw(&app, req("GET", &uri, Body::empty(), false)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{uri}");
    }
}

/// The owner's own LightBurn bundle (`OPENETCH_LBRN_SAMPLES`, see the samples' README), imported into a scratch
/// engine. Only counts are checked; nothing of the user's settings is printed.
#[tokio::test]
async fn real_lightburn_bundle_imports() {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
        return;
    };
    let Some(path) = std::env::split_paths(&dirs).map(|d| d.join("bundle.lbzip")).find(|p| p.is_file()) else {
        println!("no bundle.lbzip among the samples; skipping");
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let st = configured_state("lbzip-real").await;
    let app = app(&st);
    let (s, _, b) = raw(&app, req("POST", "/api/bundle/import", Body::from(bytes), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["imported"]["prefs"], true);
    assert_eq!(v["imported"]["devices"].as_array().map(Vec::len), Some(1));
    assert!(v["imported"]["presets"].as_array().is_some_and(|p| !p.is_empty()));
    assert_eq!(v["imported"]["art"], json!(["art-sample-1"]));
    println!(
        "imported: {} preset(s), {} device(s), art {}; {} skipped",
        v["imported"]["presets"].as_array().map_or(0, Vec::len),
        v["imported"]["devices"].as_array().map_or(0, Vec::len),
        v["imported"]["art"],
        v["skipped"].as_array().map_or(0, Vec::len)
    );
    // LightBurn's full export (every kind of item): its hotkeys, image preset, library and the device's link.
    let Some(full) = std::env::split_paths(&dirs).map(|d| d.join("bundle-full.lbzip")).find(|p| p.is_file()) else { return };
    let st = configured_state("lbzip-real-full").await;
    let (s, _, b) = raw(&app_of(&st), req("POST", "/api/bundle/import", Body::from(std::fs::read(full).unwrap()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["hotkeys"].as_object().map(serde_json::Map::len), Some(1));
    assert_eq!(v["imported"]["libraries"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["imported"]["images"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["imported"]["links"].as_array().map(Vec::len), Some(1));
}

/// A hostile `.lbzip` whose manifest lists one file many times imports it once (the rest are reported as skipped).
#[tokio::test]
async fn lightburn_bundle_with_a_repeated_manifest_item_imports_it_once() {
    let st = configured_state("lbzip-dup-a").await;
    let app = app(&st);
    let params = json!({"columns": {"param": "frequency", "min": 100, "max": 400, "count": 4}, "rows": {"param": "power", "min": 10, "max": 40, "count": 4}});
    let (s, v) = json(&app, "PUT", "/api/material-test/presets/Steel", Some(json!({"params": params}))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, _, lbmt) = raw(&app, req("GET", "/api/material-test/presets/export", Body::empty(), false)).await;
    let item = json!({"action": "import-materialtestpresets", "filename": "material_test_presets.lbmt", "name": "Material Test Presets"});
    let entries = vec![
        fileio::zip::Entry { name: "material_test_presets.lbmt".into(), data: lbmt },
        fileio::zip::Entry { name: crate::lbzip::MANIFEST.into(), data: crate::lbzip::manifest_text(&vec![item; 500]).into_bytes() },
    ];
    let bundle = fileio::zip::write(&entries).unwrap();
    let other = configured_state("lbzip-dup-b").await;
    let (s, _, b) = raw(&app_of(&other), req("POST", "/api/bundle/import?parts=presets", Body::from(bundle), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["imported"]["presets"], json!(["Steel"]), "{v}");
    let skipped = v["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 499, "{v}");
    assert!(skipped[0].as_str().unwrap().contains("listed more than once"), "{v}");
}

fn app_of(st: &Arc<AppState>) -> Router {
    api::router(st.clone(), None)
}

/// Hostile material libraries and image presets in a `.lbzip`: bad XML and JSON are skipped with their reasons; one
/// image presets file is read; at most `MAX_LOADED` libraries are tried; a LightBurn-written `.clb` (BOM, CRLF) is
/// "already here" on a second import; names at the length limit still get " (bundle)" and " (2)" within it.
#[tokio::test]
async fn lightburn_bundle_with_hostile_libraries_and_image_presets() {
    let st = configured_state("lbzip-hostile").await;
    let app = app(&st);
    let laughs = r#"<?xml version="1.0"?><!DOCTYPE l [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">]><LightBurnLibrary><Material name="&b;"/></LightBurnLibrary>"#;
    let lb = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Engrave\" NoThickTitle=\"Finish\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <maxPower Value=\"38\" />\r\n        <speed Value=\"1000\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";
    let long = "L".repeat(crate::projects::MAX_NAME_CHARS);
    let lbimg = |name: &str| serde_json::to_vec(&json!({ name: {"Gamma": 1.3, "ImgMode": "Jarvis"}, "a\nb": {"Gamma": 1.0}, "Q": {"Gamma": 1e30}, "R": "x" })).unwrap();
    let preset = "P".repeat(crate::test_presets::MAX_NAME_CHARS);
    let bundle = lightburn_bundle(&[
        ("import-cutlib", "laughs.clb", "Laughs", laughs.as_bytes().to_vec()),
        ("import-cutlib", "notxml.clb", "NotXml", b"\xff\xfe garbage".to_vec()),
        ("import-cutlib", "wrong.clb", "Wrong", b"<LightBurnProject/>".to_vec()),
        ("import-cutlib", "lb.clb", "LB", lb.as_bytes().to_vec()),
        ("import-cutlib", "long.clb", &long, lb.replace("Steel", "Brass").into_bytes()),
        ("import-imagepresets", "image_presets.lbimg", "Image Presets", lbimg(&preset)),
        ("import-imagepresets", "again.lbimg", "Image Presets", lbimg("Again")),
    ]);
    let import = |bundle: Vec<u8>| {
        let app = app.clone();
        async move {
            let (s, _, b) = raw(&app, req("POST", "/api/bundle/import?parts=libraries,images", Body::from(bundle), false)).await;
            let v: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(s, StatusCode::OK, "{v}");
            v
        }
    };
    let v = import(bundle).await;
    assert_eq!(v["imported"]["libraries"], json!(["LB", long]), "{v}");
    // One image presets file is read (its bad presets left out with warnings), a second is not.
    assert_eq!(v["imported"]["images"], json!([preset]), "{v}");
    let skipped = v["skipped"].to_string();
    for why in ["Laughs: not valid XML", "NotXml: the library is not UTF-8", "Wrong: ", "a\\nb: a preset name", "Q: adjust: adjust.gamma", "\\\"R\\\" is not a preset", "a bundle holds one images file"] {
        assert!(skipped.contains(why), "{why}: {v}");
    }
    for (data, why) in [(b"{not json".to_vec(), "not JSON"), (vec![b' '; crate::test_presets::MAX_LBIMG_BYTES + 1], "at most 2 MB"), (b"[1]".to_vec(), "expected a JSON object")] {
        let v = import(lightburn_bundle(&[("import-imagepresets", "image_presets.lbimg", "Image Presets", data)])).await;
        assert!(v["imported"]["images"] == json!([]) && v["skipped"].to_string().contains(why), "{why}: {v}");
    }

    // Again: the LightBurn-written library is already here; the long name loads as "<shortened> (bundle)"; the preset
    // is renamed within the name limit.
    let lb2 = lb.replace("Steel", "Copper");
    let v = import(lightburn_bundle(&[
        ("import-cutlib", "lb.clb", "LB", lb.as_bytes().to_vec()),
        ("import-cutlib", "long.clb", &long, lb2.clone().into_bytes()),
        ("import-imagepresets", "image_presets.lbimg", "Image Presets", lbimg(&preset)),
        ("import-imagepresets", "again.lbimg", "Image Presets", lbimg("Again")),
    ]))
    .await;
    let bundled = format!("{} (bundle)", &long[..crate::projects::MAX_NAME_CHARS - 9]);
    assert_eq!(v["imported"]["libraries"], json!([bundled]), "{v}");
    assert!(v["skipped"].to_string().contains("LB: already here"), "{v}");
    // The same preset again is renamed within the name limit, and so can be saved under its name and read back.
    let renamed = format!("{} (2)", &preset[..crate::test_presets::MAX_NAME_CHARS - 4]);
    assert_eq!(v["imported"]["images"], json!([renamed]), "{v}");
    let (s, v) = json(&app, "PUT", &format!("/api/image-adjust/presets/{}", renamed.replace(' ', "%20")), Some(json!({"adjust": {"gamma": 1.4}}))).await;
    assert_eq!((s, v["replaced"].clone()), (StatusCode::OK, json!(true)), "{v}");

    // A bundle repeating one library name past MAX_LOADED: only MAX_LOADED are tried.
    let clb = lb.as_bytes().to_vec();
    let many: Vec<(String, Vec<u8>)> = (0..crate::library::MAX_LOADED + 5).map(|i| (format!("m{i}.clb"), clb.clone())).collect();
    let items: Vec<(&str, &str, &str, Vec<u8>)> = many.iter().map(|(f, d)| ("import-cutlib", f.as_str(), "LB", d.clone())).collect();
    let v = import(lightburn_bundle(&items)).await;
    let skipped = v["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), crate::library::MAX_LOADED + 5, "{v}");
    assert_eq!(skipped.iter().filter(|s| s.as_str().unwrap().contains("at most 32 material libraries")).count(), 5, "{v}");
    // The same in an OpenEtch .oebundle (the libraries' import is shared).
    let manifest = json!({"format": "openetch-bundle", "version": 1});
    let mut entries = vec![fileio::zip::Entry { name: "bundle.json".into(), data: manifest.to_string().into_bytes() }];
    entries.extend((0..crate::library::MAX_LOADED + 5).map(|i| fileio::zip::Entry { name: format!("libraries/LB{i}.clb"), data: clb.clone() }));
    let (s, _, b) = raw(&app, req("POST", "/api/bundle/import?parts=libraries", Body::from(fileio::zip::write(&entries).unwrap()), false)).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["skipped"].as_array().unwrap().iter().filter(|s| s.as_str().unwrap().contains("at most 32 material libraries")).count(), 5, "{v}");
}
