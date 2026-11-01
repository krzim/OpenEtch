//! Route tests of the last engine gaps: project folders and locations (UX-013), material test presets (LAY-070).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::tests::test_state;
use crate::{AppState, api, pin};

fn app(st: &Arc<AppState>) -> Router {
    api::router(st.clone(), None)
}

async fn raw(app: &Router, r: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let res = app.clone().oneshot(r).await.unwrap();
    let (s, h) = (res.status(), res.headers().clone());
    (s, h, res.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn send(app: &Router, method: &str, uri: &str, body: Option<Value>, mcp: bool) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost:7878");
    if mcp {
        b = b.header("mcp-session-id", "x");
    }
    let r = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let (s, _, bytes) = raw(app, r).await;
    (s, serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())))
}

async fn configured() -> Arc<AppState> {
    let st = test_state().await;
    let _ = st.files.open(&pin::temp_dir("final-config"));
    st
}

#[tokio::test]
async fn projects_save_and_open_in_folders_and_locations() {
    let st = configured().await;
    let app = app(&st);
    let (s, v) = send(&app, "POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 4, "height_mm": 4}})), false).await;
    assert!(s.is_success() && v["ok"] == true, "{v}");
    let (s, v) = send(&app, "POST", "/api/projects/save", Some(json!({"name": "Clients/Acme/badge"})), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, list) = send(&app, "GET", "/api/projects", None, false).await;
    assert_eq!(list[0]["name"], "Clients/Acme/badge");
    assert_eq!(list[0]["folder"], "Clients/Acme");
    let (_, p) = send(&app, "GET", "/api/project", None, false).await;
    assert_eq!(p["name"], "Clients/Acme/badge");
    let (s, v) = send(&app, "POST", "/api/projects/open", Some(json!({"name": "clients/acme/badge"})), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // The thumbnail route takes the path with its slashes encoded.
    let r = Request::builder().uri("/api/projects/Clients%2FAcme%2Fbadge/thumbnail.png?width=64").header("host", "localhost:7878").body(Body::empty()).unwrap();
    let (s, _, png) = raw(&app, r).await;
    assert_eq!((s, &png[1..4]), (StatusCode::OK, &b"PNG"[..]));
    // Saving again keeps a flat-named backup.
    let (s, _) = send(&app, "POST", "/api/projects/save", Some(json!({"name": "Clients/Acme/badge"})), false).await;
    assert_eq!(s, StatusCode::OK);
    let (_, b) = send(&app, "GET", "/api/backups?project=Clients/Acme/badge", None, false).await;
    assert_eq!(b["backups"].as_array().unwrap().len(), 1, "{b}");

    // Folders: created empty, listed with the ones that hold projects.
    let (s, v) = send(&app, "POST", "/api/projects/folders", Some(json!({"name": "Jigs/Round"})), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = send(&app, "POST", "/api/projects/folders", Some(json!({"name": "templates"})), false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (_, f) = send(&app, "GET", "/api/projects/folders", None, false).await;
    assert_eq!(f["folders"], json!(["Clients", "Clients/Acme", "Jigs", "Jigs/Round"]));

    // Locations: UI only, absolute existing directories; saved and mounted as top folders.
    let nas = pin::temp_dir("final-nas");
    std::fs::create_dir_all(&nas).unwrap();
    let body = json!({"locations": [{"label": "NAS", "path": nas}]});
    let (s, _) = send(&app, "PUT", "/api/projects/locations", Some(body.clone()), true).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "never from an agent");
    let (s, _) = send(&app, "PUT", "/api/projects/locations", Some(json!({"locations": [{"label": "X", "path": "relative"}]})), false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = send(&app, "PUT", "/api/projects/locations", Some(body), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["locations"][0]["reachable"], true);
    let (s, v) = send(&app, "POST", "/api/projects/save", Some(json!({"name": "NAS/coaster"})), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(nas.join("coaster.openetch").is_file());
    let saved = std::fs::read_to_string(st.files.config_dir().unwrap().join(crate::folders::LOCATIONS_FILE)).unwrap();
    assert!(saved.contains("NAS"));
    // A restart mounts the saved locations again.
    let st2 = test_state().await;
    let notes = crate::folders::open(&st2, &st.files.config_dir().unwrap(), &[]);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(st2.projects.locations()[0].label, "NAS");
    let _ = std::fs::remove_dir_all(&nas);
}

#[tokio::test]
async fn material_test_presets_are_saved_and_trade_with_lightburn() {
    let st = configured().await;
    let app = app(&st);
    let params = json!({"columns": {"param": "frequency", "min": 100, "max": 400, "count": 4}, "rows": {"param": "power", "min": 10, "max": 40, "count": 4}, "cell_width_mm": 3});
    let (s, v) = send(&app, "PUT", "/api/material-test/presets/Steel%20colours", Some(json!({"params": params})), false).await;
    assert_eq!((s, v["replaced"].clone()), (StatusCode::OK, json!(false)), "{v}");
    let (s, v) = send(&app, "PUT", "/api/material-test/presets/Bad", Some(json!({"params": {"columns": {"param": "power", "min": 1, "max": 2, "count": 99}}})), false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (_, v) = send(&app, "GET", "/api/material-test/presets", None, false).await;
    assert_eq!(v["presets"][0]["name"], "Steel colours");

    // Export as LightBurn's .lbmt, then import it back: renamed, as the name is taken.
    let r = Request::builder().uri("/api/material-test/presets/export").header("host", "localhost:7878").body(Body::empty()).unwrap();
    let (s, h, bytes) = raw(&app, r).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h[axum::http::header::CONTENT_DISPOSITION].to_str().unwrap().contains("material_test_presets.lbmt"));
    let lbmt: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!((lbmt["Steel colours"]["XParam"].clone(), lbmt["Steel colours"]["YParam"].clone()), (json!(4), json!(1)));
    let r = Request::builder().method("POST").uri("/api/material-test/presets/import").header("host", "localhost:7878").body(Body::from(bytes)).unwrap();
    let (s, _, out) = raw(&app, r).await;
    let out: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!((s, out["imported"].clone()), (StatusCode::OK, json!(["Steel colours (2)"])), "{out}");
    let (_, v) = send(&app, "GET", "/api/material-test/presets", None, false).await;
    assert_eq!(v["presets"][1]["params"]["columns"], json!({"param": "frequency", "min": 100.0, "max": 400.0, "count": 4}));
    // The preset generates the test it describes.
    let p = v["presets"][1]["params"].clone();
    let (_, r) = send(&app, "POST", "/api/execute", Some(json!({"id": "generate.material_test", "params": p})), false).await;
    assert_eq!(r["result"]["boxes"], 16, "{r}");
    let (s, _) = send(&app, "DELETE", "/api/material-test/presets/Steel%20colours%20(2)", None, false).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = send(&app, "DELETE", "/api/material-test/presets/nothing", None, false).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let r = Request::builder().method("POST").uri("/api/material-test/presets/import").header("host", "localhost:7878").body(Body::from("not json")).unwrap();
    assert_eq!(raw(&app, r).await.0, StatusCode::BAD_REQUEST);
}

/// UX-034: LightBurn's "Allow Importing to Tool Layers". Off (the default) `layer=T1` is refused and the default
/// target skips tool layers; on, `layer=T1` imports onto T1, added in the same undo step.
#[tokio::test]
async fn imports_go_to_tool_layers_only_when_allowed() {
    let st = configured().await;
    let app = app(&st);
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10"><rect width="10" height="10"/></svg>"#;
    let post = |uri: &str| Request::builder().method("POST").uri(uri).header("host", "localhost:7878").body(Body::from(svg)).unwrap();
    let (s, _, b) = raw(&app, post("/api/document/import?format=svg&layer=T1")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&b));
    let (s, _, b) = raw(&app, post("/api/document/import?format=svg&layer=t1&tool_layers=true")).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert!(s.is_success() && v["ok"] == true, "{v}");
    let d = st.document();
    let t1 = d.layers.iter().find(|l| l.tool == Some(doc::ToolLayer::T1)).expect("T1 added").id.clone();
    assert!(d.objects.iter().all(|o| o.layer == t1), "on T1");
    // Once more onto the existing T1; an existing tool layer by id is refused without the option.
    let (s, _, _) = raw(&app, post("/api/document/import?format=svg&layer=T1&tool_layers=true")).await;
    assert!(s.is_success());
    assert_eq!(st.document().layers.iter().filter(|l| l.tool.is_some()).count(), 1);
    // A layer given by id is the caller's choice (the web UI applies the preference itself).
    let (s, _, _) = raw(&app, post(&format!("/api/document/import?format=svg&layer={t1}"))).await;
    assert!(s.is_success());
    // With T1 first in the list, an import without a layer goes to the first ordinary layer.
    st.execute(commands::Invocation { id: "layer.move".into(), params: json!({"id": t1, "to_index": 0}) }, commands::Origin::Ui).unwrap();
    assert_eq!(st.document().layers[0].id, t1);
    let (s, _, b) = raw(&app, post("/api/document/import?format=svg")).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert!(s.is_success() && v["ok"] == true, "{v}");
    let d = st.document();
    let last = d.objects.last().unwrap();
    assert!(d.layer(&last.layer).unwrap().tool.is_none());
}

/// JOB-007: markers added by generate.camera_markers, seen by the camera, align it without clicking.
#[tokio::test]
async fn the_camera_aligns_itself_on_marked_markers() {
    let st = configured().await;
    let app = app(&st);
    let (_, v) = send(&app, "POST", "/api/execute", Some(json!({"id": "generate.camera_markers", "params": {"positions": [[-40, 40], [40, 40], [40, -40], [-40, -40], [0, 0]], "size_mm": 8}})), false).await;
    assert_eq!(v["result"]["markers"].as_array().map(Vec::len), Some(5), "{v}");
    // A camera 3 px/mm over the field, slightly turned; marker 4 is covered (not drawn).
    let (sn, cs) = 4f64.to_radians().sin_cos();
    let to_img = |x: f64, y: f64| [320.0 + 3.0 * (x * cs - y * sn), 240.0 - 3.0 * (x * sn + y * cs)];
    let (w, h) = (640u32, 480u32);
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([215]));
    for (id, c) in [[-40.0, 40.0], [40.0, 40.0], [40.0, -40.0], [-40.0, -40.0]].iter().enumerate() {
        let cells = commands::camera_markers::marker_cells(id).unwrap();
        for py in 0..h {
            for px in 0..w {
                // Image pixel → field → marker cell (row 0 at the top, +Y up).
                let (u, v) = (f64::from(px) + 0.5 - 320.0, 240.0 - (f64::from(py) + 0.5));
                let (x, y) = ((u * cs + v * sn) / 3.0, (-u * sn + v * cs) / 3.0);
                let (cx, cy) = ((x - c[0] + 4.0) / 8.0 * 6.0, (c[1] + 4.0 - y) / 8.0 * 6.0);
                if (0.0..6.0).contains(&cx) && (0.0..6.0).contains(&cy) && cells[cy as usize][cx as usize] {
                    img.put_pixel(px, py, image::Luma([30]));
                }
            }
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let r = Request::builder().method("POST").uri("/api/camera/frame").header("host", "localhost:7878").body(Body::from(png)).unwrap();
    assert!(raw(&app, r).await.0.is_success());
    let (_, m) = send(&app, "POST", "/api/camera/markers", None, false).await;
    assert_eq!(m["markers"].as_array().unwrap().iter().map(|x| x["id"].as_u64().unwrap()).collect::<Vec<_>>(), [0, 1, 2, 3], "{m}");
    let (s, _) = send(&app, "POST", "/api/camera/align_markers", None, true).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "never from an agent");
    let (s, a) = send(&app, "POST", "/api/camera/align_markers", None, false).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!((a["used"].clone(), a["missing"].clone()), (json!([0, 1, 2, 3]), json!([4])));
    assert!(a["alignment"]["rms_mm"].as_f64().unwrap() < 0.2, "{a}");
    let hm: Vec<f64> = serde_json::from_value(a["alignment"]["image_to_field"].clone()).unwrap();
    let hm: [f64; 9] = hm.try_into().unwrap();
    let p = crate::camera::apply(&hm, to_img(10.0, -20.0));
    assert!((p[0] - 10.0).abs() < 0.3 && (p[1] + 20.0).abs() < 0.3, "{p:?}");
}

/// OTH-022: Adjust Image presets are saved, checked, exported and imported.
#[tokio::test]
async fn image_adjust_presets_are_kept() {
    let st = configured().await;
    let app = app(&st);
    let body = json!({"adjust": {"gamma": 1.4, "contrast": 20, "brightness": -11}, "image": {"dither": "jarvis", "interval_mm": 0.05}});
    let (s, v) = send(&app, "PUT", "/api/image-adjust/presets/Slate%20photo", Some(body), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for bad in [json!({}), json!({"adjust": {"gamma": 0}}), json!({"image": {"dither": "nope"}})] {
        let (s, v) = send(&app, "PUT", "/api/image-adjust/presets/Bad", Some(bad), false).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }
    let (_, list) = send(&app, "GET", "/api/image-adjust/presets", None, false).await;
    assert_eq!(list["presets"][0]["adjust"]["gamma"], 1.4);
    let (s, v) = send(&app, "POST", "/api/image-adjust/presets/import", Some(list.clone()), false).await;
    assert_eq!((s, v["imported"].clone()), (StatusCode::OK, json!(["Slate photo (2)"])), "{v}");
    let (s, _) = send(&app, "DELETE", "/api/image-adjust/presets/Slate%20photo", None, false).await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = send(&app, "GET", "/api/image-adjust/presets", None, false).await;
    assert_eq!(list["presets"].as_array().unwrap().len(), 1);
}

/// Whole-number query options reach the import commands as integers: `page=1` used to arrive as 1.0 and the PDF
/// import refused it.
#[tokio::test]
async fn integer_import_options_from_the_query_are_integers() {
    let st = configured().await;
    let app = app(&st);
    let (_, v) = send(&app, "POST", "/api/execute", Some(json!({"id": "shape.rect", "params": {"width_mm": 10, "height_mm": 5}})), false).await;
    assert_eq!(v["ok"], true);
    let r = Request::builder().uri("/api/export?format=pdf").header("host", "localhost:7878").body(Body::empty()).unwrap();
    let (s, _, pdf) = raw(&app, r).await;
    assert_eq!(s, StatusCode::OK);
    let r = Request::builder().method("POST").uri("/api/document/import?format=pdf&page=1&dpi=300").header("host", "localhost:7878").body(Body::from(pdf.clone())).unwrap();
    let (_, _, b) = raw(&app, r).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    // dpi is not a PDF option: refused by name, not for its number type.
    assert!(v["error"].as_str().unwrap_or_default().contains("dpi"), "{v}");
    let r = Request::builder().method("POST").uri("/api/document/import?format=pdf&page=1&width_mm=20.5").header("host", "localhost:7878").body(Body::from(pdf)).unwrap();
    let (_, _, b) = raw(&app, r).await;
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["ok"], true, "{v}");
}

/// LightBurn's Adjust Image preset file (`.lbimg`, LightBurn 2.2's layout: one object per name, float32 values) reads
/// into presets with OpenEtch's units and writes back in LightBurn's layout.
#[tokio::test]
async fn lightburn_lbimg_presets_import_and_export() {
    let st = configured().await;
    let app = app(&st);
    let lbimg = json!({"OpenEtchSample": {"Brightness": 0.10000000149011612, "Contrast": 0.25, "EnhanceAmount": 0.5, "EnhanceRadius": 3,
        "Gamma": 1.2000000476837158, "HalftoneAngle": 22.5, "HalftoneCells": 50, "ImgInterval": 0.10000000149011612, "ImgMode": "Stucki",
        "Negative": false}, "Odd": {"ImgMode": "Mystery", "Gamma": 2}, "Bad": 3});
    let (s, v) = send(&app, "POST", "/api/image-adjust/presets/import", Some(lbimg), false).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["imported"], json!(["Odd", "OpenEtchSample"]), "{v}");
    assert_eq!(v["warnings"].as_array().map(Vec::len), Some(2), "{v}");
    let (_, list) = send(&app, "GET", "/api/image-adjust/presets", None, false).await;
    let p = list["presets"].as_array().unwrap().iter().find(|p| p["name"] == "OpenEtchSample").unwrap().clone();
    assert_eq!(p["adjust"], json!({"gamma": 1.2, "contrast": 25.0, "brightness": 10.0, "enhance_radius": 3.0, "enhance_amount": 50.0, "denoise": 0.0}), "{p}");
    assert_eq!(p["image"], json!({"dither": "stucki", "interval_mm": 0.1, "negative": false, "halftone_angle_deg": 22.5, "halftone_cells_per_inch": 50.0}), "{p}");
    let r = Request::builder().uri("/api/image-adjust/presets/export?format=lbimg").header("host", "localhost:7878").body(Body::empty()).unwrap();
    let (s, h, bytes) = raw(&app, r).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h[axum::http::header::CONTENT_DISPOSITION].to_str().unwrap().contains(".lbimg"));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("{\n    \"Odd\": {\n        \"Brightness\": 0.0,"), "{text}");
    let back: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(back["OpenEtchSample"], json!({"Brightness": 0.1, "Contrast": 0.25, "EnhanceAmount": 0.5, "EnhanceRadius": 3.0, "Gamma": 1.2,
        "HalftoneAngle": 22.5, "HalftoneCells": 50.0, "ImgInterval": 0.1, "ImgMode": "Stucki", "Negative": false}));
    // The owner's own export (OPENETCH_LBRN_SAMPLES, see the samples' README) reads the same.
    if let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES")
        && let Some(f) = std::env::split_paths(&dirs).map(|d| d.join("adjust-preset.lbimg")).find(|p| p.is_file())
    {
        let v: Value = serde_json::from_slice(&std::fs::read(f).unwrap()).unwrap();
        let (presets, warnings) = crate::test_presets::lbimg_to_presets(&v).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(presets.len(), 1);
        assert_eq!(serde_json::to_value(&presets[0]).unwrap(), p);
    }
    let (s, _) = send(&app, "GET", "/api/image-adjust/presets/export?format=xml", None, false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
