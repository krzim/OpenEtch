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
        ("import-hotkeys", "hotkeys.lbkeys", "Hotkeys", b"{}".to_vec()),
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
    assert!(skipped.contains("import-hotkeys") && skipped.contains("not a plain file name") && skipped.contains("missing from the bundle"), "{v}");
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
