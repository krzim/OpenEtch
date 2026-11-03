//! Route tests of the MOPA materials (`api_materials`, `finish`): the finish sidecar beside a `.clb` (saved, reopened,
//! renamed, downloaded, imported, the `.clb` itself untouched), result photos and colours, the result colour a layer
//! takes from its preset, the palette, the hue step model (UI only), bracket test and pick, and colour separation.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::{Invocation, Origin, Session};
use http_body_util::BodyExt;
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Gold\" NoThickTitle=\"Colours\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Colours/Gold\" />\r\n        <maxPower Value=\"40\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"600000\" />\r\n        <interval Value=\"0.01\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Blue\" NoThickTitle=\"Colours\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Colours/Blue\" />\r\n        <maxPower Value=\"55\" />\r\n        <speed Value=\"600\" />\r\n        <frequency Value=\"200000\" />\r\n        <interval Value=\"0.01\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

async fn state(tag: &str) -> (Arc<AppState>, std::path::PathBuf) {
    let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir(tag);
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    let st = AppState::new(Session::new(doc::Document::default()), machine, SecurityConfig::default(), approval, test_projects());
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

async fn raw(app: &Router, r: Request<Body>) -> (StatusCode, Vec<u8>) {
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    (status, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn send(app: &Router, r: Request<Body>) -> (StatusCode, Value) {
    let (s, bytes) = raw(app, r).await;
    (s, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

async fn ok(app: &Router, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (s, v) = send(app, req(method, uri, body)).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
    v
}

fn jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(32, 24, image::Rgb(rgb));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).unwrap();
    out
}

#[tokio::test]
async fn finishes_live_in_a_sidecar_beside_an_untouched_clb() {
    let (st, dir) = state("mat-sidecar").await;
    let app = api::router(st.clone(), None);
    let (s, _) = raw(&app, Request::builder().method("POST").uri("/api/library/load?name=steel").header("host", "localhost:7878").body(Body::from(CLB)).unwrap()).await;
    assert_eq!(s, StatusCode::OK);

    // A photo sets the colour (its average); notes, lot, surface.
    let photo = STANDARD.encode(jpeg([200, 150, 40]));
    let v = ok(&app, "PUT", "/api/library/steel/entries/m0.e0/finish", Some(json!({ "photo_base64": photo, "photo_mime": "image/jpeg", "notes": "Lot A", "lot": "A", "surface": "brushed" }))).await;
    let c = v["finish"]["color"].as_str().unwrap().to_string();
    assert!(fileio::colorsep::delta_e(fileio::colorsep::parse_hex(&c).unwrap(), [200, 150, 40]) < 3.0, "{c}");
    assert_eq!(v["finish"]["photo"]["width"], 32);
    let (s, bytes) = raw(&app, req("GET", "/api/library/steel/entries/m0.e0/photo", None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(&bytes[..2], &[0xff, 0xd8], "the JPEG as sent");
    let (s, _) = send(&app, req("GET", "/api/library/steel/entries/m0.e1/photo", None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(&app, req("PUT", "/api/library/steel/entries/m0.e0/finish", Some(json!({ "date": "yesterday" })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    ok(&app, "PUT", "/api/library/steel/entries/m0.e1/finish", Some(json!({ "color": "#2040c0", "surface": "polished" }))).await;
    let tree = ok(&app, "GET", "/api/library", None).await;
    assert_eq!(tree["libraries"][0]["dirty"], true);
    assert_eq!(tree["tree"]["materials"][0]["groups"][0]["entries"][1]["finish"]["color"], "#2040c0");

    // Palette by surface.
    let p = ok(&app, "GET", "/api/materials/palette?surface=polished", None).await;
    assert_eq!(p["swatches"].as_array().unwrap().len(), 1);
    assert_eq!(p["swatches"][0]["description"], "Blue");

    // Save writes the sidecar; the .clb stays LightBurn's (byte for byte, nothing of ours in it).
    ok(&app, "POST", "/api/library/save", Some(json!({ "name": "steel" }))).await;
    let libdir = dir.join(crate::library::LIBRARIES_DIR);
    assert_eq!(std::fs::read(libdir.join("steel.clb")).unwrap(), CLB.as_bytes());
    assert!(libdir.join("steel.oefinish.json").is_file());
    let (again, notes) = crate::library::LibraryStore::open(&dir);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(again.finish_of("steel", "Steel/Colours/Gold").unwrap().notes, "Lot A");

    // Rename carries the sidecar; Edit Description carries the finish to the new path.
    ok(&app, "POST", "/api/library/rename", Some(json!({ "name": "steel", "new_name": "ss304" }))).await;
    assert!(libdir.join("ss304.oefinish.json").is_file() && !libdir.join("steel.oefinish.json").exists());
    let r = ok(&app, "PUT", "/api/library/ss304/entries/m0.e1", Some(json!({ "meta": { "material": "Steel", "title": "Colours", "description": "Deep blue" } }))).await;
    assert_eq!(r["path"], "Steel/Colours/Deep blue");
    let f = ok(&app, "GET", "/api/library/ss304/entries/m0.e1/finish", None).await;
    assert_eq!(f["finish"]["color"], "#2040c0");

    // Download and import the sidecar (a path the library lacks is skipped).
    let (s, side) = raw(&app, req("GET", "/api/library/ss304/finish/download", None)).await;
    assert_eq!(s, StatusCode::OK);
    let mut parsed: Value = serde_json::from_slice(&side).unwrap();
    assert_eq!(parsed["format"], "openetch-finish");
    parsed["entries"]["Steel/Colours/Gold"]["notes"] = json!("imported");
    parsed["entries"]["Brass/X/Y"] = json!({ "color": "#ffcc00" });
    let (s, v) = send(&app, Request::builder().method("POST").uri("/api/library/ss304/finish/import").header("host", "localhost:7878").body(Body::from(parsed.to_string())).unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["imported"].as_u64(), v["skipped"].as_u64()), (Some(2), Some(1)));
    let f = ok(&app, "GET", "/api/library/ss304/entries/m0.e0/finish", None).await;
    assert_eq!(f["finish"]["notes"], "imported");

    // Assign gives the layer the preset's result colour; a preset without one clears it.
    let layer = st.document().layers[0].id.clone();
    ok(&app, "POST", "/api/library/ss304/entries/m0.e1/apply", Some(json!({ "layer": layer }))).await;
    assert_eq!(st.document().layers[0].extras.get(commands::finish::RESULT_COLOR_KEY).map(String::as_str), Some("#2040c0"));
    ok(&app, "PUT", "/api/library/ss304/entries/m0.e1/finish", Some(json!({ "color": null }))).await;
    ok(&app, "POST", "/api/library/ss304/entries/m0.e1/apply", Some(json!({ "layer": layer, "link": true }))).await;
    assert!(!st.document().layers[0].extras.contains_key(commands::finish::RESULT_COLOR_KEY));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn hue_model_is_ui_only_and_bracket_boxes_calibrate_it() {
    let (st, dir) = state("mat-hue").await;
    let app = api::router(st.clone(), None);
    let (s, _) = raw(&app, Request::builder().method("POST").uri("/api/library/load?name=steel").header("host", "localhost:7878").body(Body::from(CLB)).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    let m = ok(&app, "GET", "/api/materials/hue-model", None).await;
    assert_eq!((m["model"]["power_pct"].as_f64(), m["calibrated"].as_bool()), (Some(0.5), Some(false)));
    let mcp = Request::builder().method("PUT").uri("/api/materials/hue-model").header("host", "localhost:7878").header("mcp-session-id", "x").header("content-type", "application/json");
    let (s, _) = send(&app, mcp.body(Body::from(json!({ "model": null }).to_string())).unwrap()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = send(&app, req("PUT", "/api/materials/hue-model", Some(json!({ "model": { "power_pct": 0 } })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Nudge through the route (compute only).
    let n = ok(&app, "POST", "/api/materials/nudge", Some(json!({ "library": "steel", "entry": "m0.e1", "steps": -2 }))).await;
    assert_eq!((n["param"].as_str(), n["settings"]["power_pct"].as_f64()), (Some("power"), Some(56.0)));

    // A bracket test (new document), then calibrate from two boxes of the same column.
    let b = ok(&app, "POST", "/api/materials/bracket", Some(json!({ "library": "steel", "entry": "Steel/Colours/Blue" }))).await;
    assert!(b["document"].is_string());
    let doc = st.document();
    assert_eq!(doc.name, "Bracket Blue");
    let cell = |c: &str| doc.layers.iter().find(|l| l.extras.get(commands::bracket::CELL_KEY).map(String::as_str) == Some(c)).unwrap().id.clone();
    // Same speed (row 0 %), power -5 % (52.25) came out one hue warmer than power 0 % (55).
    let v = ok(&app, "POST", "/api/materials/hue-model/calibrate", Some(json!({ "best": cell("0,0"), "other": cell("0,-5"), "hues": 1 }))).await;
    assert_eq!((v["model"]["power_pct"].as_f64(), v["model"]["source"].as_str(), v["calibrated"].as_bool()), (Some(2.75), Some("bracket"), Some(true)));
    let (s, _) = send(&app, req("POST", "/api/materials/hue-model/calibrate", Some(json!({ "best": cell("0,0"), "other": cell("5,5") })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "two settings differ");
    // The model is kept per device across restarts.
    let (again, _) = crate::library::LibraryStore::open(&dir);
    assert_eq!(again.hue_model(&st.active_device().id).power_pct, 2.75);

    // Pick writes the box back (and dates the settings, so an older photo shows as stale).
    ok(&app, "PUT", "/api/library/steel/entries/m0.e1/finish", Some(json!({ "notes": "before the lot change" }))).await;
    let p = ok(&app, "POST", "/api/materials/bracket/pick", Some(json!({ "layer": cell("5,-10") }))).await;
    assert_eq!(p["path"], "Steel/Colours/Blue");
    {
        let s = crate::libs::lock(&st.libs.library);
        let preset = crate::library::preset_of(s.entry("steel", (0, 1)).unwrap().1).unwrap();
        assert!((preset.settings.speed_mm_s - 630.0).abs() < 1e-6 && (preset.settings.power_pct - 49.5).abs() < 1e-6, "{:?}", preset.settings);
        assert!(s.finish_of("steel", "Steel/Colours/Blue").unwrap().settings_unix_s.is_some());
    }
    let (s, _) = send(&app, req("POST", "/api/materials/bracket/pick", Some(json!({ "layer": st.document().layers.iter().find(|l| !l.extras.contains_key(commands::bracket::CELL_KEY)).unwrap().id })))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "not a box");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn colour_separation_links_layers_to_library_colours() {
    let (st, dir) = state("mat-sep").await;
    let app = api::router(st.clone(), None);
    let (s, _) = raw(&app, Request::builder().method("POST").uri("/api/library/load?name=steel").header("host", "localhost:7878").body(Body::from(CLB)).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    ok(&app, "PUT", "/api/library/steel/entries/m0.e0/finish", Some(json!({ "color": "#d0a030" }))).await;
    ok(&app, "PUT", "/api/library/steel/entries/m0.e1/finish", Some(json!({ "color": "#2030c0" }))).await;
    // A gold disc and a blue bar on white.
    let img = image::RgbaImage::from_fn(60, 30, |x, y| {
        let (dx, dy) = (f64::from(x) - 15.0, f64::from(y) - 15.0);
        if dx * dx + dy * dy < 100.0 {
            image::Rgba([210, 165, 50, 255])
        } else if (35..55).contains(&x) && (10..20).contains(&y) {
            image::Rgba([30, 50, 190, 255])
        } else {
            image::Rgba([255, 255, 255, 255])
        }
    });
    let png = fileio::bitmap::png(&image::DynamicImage::ImageRgba8(img), None).unwrap();
    let r = st.execute(Invocation { id: "art.import_image".into(), params: json!({ "data_base64": STANDARD.encode(png), "mime": "image/png", "width_mm": 30.0 }) }, Origin::Ui).unwrap();
    let image = r.result["created"][0].as_str().unwrap().to_string();
    let pv = ok(&app, "POST", "/api/materials/separate", Some(json!({ "image": image, "colors": 3, "preview": true }))).await;
    assert_eq!(pv["colors"].as_array().unwrap().len(), 3);
    let revision = st.document().revision;
    let v = ok(&app, "POST", "/api/materials/separate", Some(json!({ "image": image, "colors": 3 }))).await;
    assert!(st.document().revision > revision);
    let layers = v["layers"].as_array().unwrap();
    assert_eq!(layers.len(), 2);
    let doc = st.document();
    let mut paths: Vec<String> = layers.iter().map(|l| doc.layer(l["layer"].as_str().unwrap()).unwrap().link.as_ref().unwrap().path.clone()).collect();
    paths.sort();
    assert_eq!(paths, ["Steel/Colours/Blue", "Steel/Colours/Gold"]);
    let gold = doc.layers.iter().find(|l| l.name == "Gold").unwrap();
    assert_eq!(gold.settings.power_pct, 40.0);
    assert_eq!(gold.extras[commands::finish::RESULT_COLOR_KEY], "#d0a030");
    // Without a library: unlinked fill layers in the image's colours.
    st.undo(Origin::Ui).unwrap();
    let v = ok(&app, "POST", "/api/materials/separate", Some(json!({ "image": image, "colors": 3, "library": "" }))).await;
    assert!(v["layers"].as_array().unwrap().iter().all(|l| l["swatch"].is_null()));
    let _ = std::fs::remove_dir_all(&dir);
}
