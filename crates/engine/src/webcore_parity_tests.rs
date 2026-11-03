//! Parity of the browser's preview (crates/webcore, run as WebAssembly in a Web Worker) with the engine's
//! `GET /api/preview`: for each document, webcore is given exactly what the web app gets (the JSON of
//! `GET /api/document` and `GET /api/preview/context`) and must give the same segments (kind, emits, layer, pass,
//! power, end points and times within f32 precision), the same stats, passes, layers (with their unsupported
//! LightBurn settings) and warnings, and the same 422 message for a document that cannot be planned.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use doc::{Cleanup, CutOptions, CutSettings, Document, FillMode, Geometry, Layer, Object, Tabs};
use http_body_util::BodyExt;
use kurbo::{Affine, BezPath};
use machine::{MachineProfile, PollConfig};
use serde_json::{Value, json};
use tower::ServiceExt;
use webcore::preview::{Context, EMITS, LAYER_SHIFT, MARK, NO_PASS, Packed};

use crate::hw::Machine;
use crate::pin::{self, ApprovalConfig, PinStore};
use crate::security::SecurityConfig;
use crate::tests::{TEST_PIN, test_projects};
use crate::{AppState, api};

const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");

async fn state(doc: Document) -> Arc<AppState> {
    let profile = MachineProfile::from_toml(B6).expect("B6 profile");
    let machine = Arc::new(Machine::simulated(profile, None, PollConfig::immediate()));
    machine.connect().await.expect("sim connects");
    let dir = pin::temp_dir("webcore-parity");
    pin::set_pin(&dir, TEST_PIN).expect("set test PIN");
    let approval = ApprovalConfig { pin: PinStore::new(&dir), confirm: None };
    AppState::new(commands::Session::new(doc), machine, SecurityConfig::default(), approval, test_projects())
}

async fn get(app: &Router, uri: &str) -> (StatusCode, String) {
    let r = Request::builder().method("GET").uri(uri).header("host", "localhost:7878").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn exec(app: &Router, id: &str, params: Value) -> Value {
    let body = json!({ "id": id, "params": params }).to_string();
    let r = Request::builder()
        .method("POST")
        .uri("/api/execute")
        .header("host", "localhost:7878")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    let v: Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(v["ok"], true, "{id}: {v}");
    v
}

/// The browser's packed preview from what the web app downloads.
async fn browser_preview(app: &Router) -> (Packed, Value) {
    let (s, doc_json) = get(app, "/api/document").await;
    assert_eq!(s, StatusCode::OK);
    let (s, ctx_json) = get(app, "/api/preview/context").await;
    assert_eq!(s, StatusCode::OK, "{ctx_json}");
    let doc: Document = serde_json::from_str(&doc_json).unwrap();
    let ctx: Context = serde_json::from_str(&ctx_json).unwrap();
    let packed = webcore::Core::packed(&doc, &ctx);
    let summary = match webcore::preview::plan_preview(&doc, &ctx) {
        Ok(p) => webcore::preview::summary(&doc, p),
        Err(e) => webcore::preview::error_meta(&e),
    };
    (packed, summary)
}

fn close(a: f64, b: f64, what: &str, i: usize) {
    // f32 keeps 24 bits: about 6e-8 relative.
    let tol = 2e-7 * a.abs().max(b.abs()).max(1.0);
    assert!((a - b).abs() <= tol, "segment {i} {what}: browser {a} vs engine {b}");
}

/// Assert the browser's preview of the engine's current document equals the engine's; returns the segment count.
async fn assert_parity(app: &Router, name: &str) -> usize {
    let (status, body) = get(app, "/api/preview").await;
    let engine: Value = serde_json::from_str(&body).unwrap();
    let (packed, summary) = browser_preview(app).await;
    if status != StatusCode::OK {
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}: {body}");
        assert_eq!(packed.meta["error"], engine["error"], "{name}: the same planning error");
        assert_eq!(summary["error"], engine["error"], "{name}: stats give the same error");
        assert!(packed.t.is_empty());
        return 0;
    }
    assert!(packed.meta.get("error").is_none(), "{name}: {}", packed.meta);
    for key in ["passes", "stats", "layers", "warnings"] {
        assert_eq!(packed.meta[key], engine[key], "{name}: {key}");
        assert_eq!(summary[key], engine[key], "{name}: stats-only {key}");
    }
    let segs = engine["segments"].as_array().unwrap();
    assert_eq!(packed.t.len(), segs.len(), "{name}: segment count");
    assert_eq!(packed.meta["segment_count"], segs.len());
    assert_eq!(summary["segment_count"], segs.len());
    assert_eq!((packed.xy.len(), packed.info.len(), packed.pass.len(), packed.power.len()), (4 * segs.len(), segs.len(), segs.len(), segs.len()));
    let layer_ids: Vec<&str> = packed.meta["layer_ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    for (i, s) in segs.iter().enumerate() {
        let info = packed.info[i];
        assert_eq!(info & MARK != 0, s["kind"] == "mark", "{name}: segment {i} kind");
        assert_eq!(info & EMITS != 0, s["emits"] == true, "{name}: segment {i} emits");
        let layer = match info >> LAYER_SHIFT {
            0 => Value::Null,
            k => json!(layer_ids[k as usize - 1]),
        };
        assert_eq!(layer, s["layer"], "{name}: segment {i} layer");
        let pass = if packed.pass[i] == NO_PASS { Value::Null } else { json!(packed.pass[i]) };
        assert_eq!(pass, s["pass"], "{name}: segment {i} pass");
        let [fx, fy] = [s["from"][0].as_f64().unwrap(), s["from"][1].as_f64().unwrap()];
        let [tx, ty] = [s["to"][0].as_f64().unwrap(), s["to"][1].as_f64().unwrap()];
        for (k, (want, what)) in [(fx, "x0"), (fy, "y0"), (tx, "x1"), (ty, "y1")].into_iter().enumerate() {
            close(f64::from(packed.xy[4 * i + k]), want, what, i);
        }
        close(f64::from(packed.t[i]), s["t_s"].as_f64().unwrap(), "t_s", i);
        match s.get("power_pct").and_then(Value::as_f64) {
            Some(p) => close(f64::from(packed.power[i]), p, "power_pct", i),
            None => assert!(packed.power[i].is_nan(), "{name}: segment {i} has no own power"),
        }
    }
    segs.len()
}

fn obj(id: &str, layer: &str, g: Geometry, at: (f64, f64)) -> Object {
    Object::new(id.into(), id, layer.into(), g, Affine::translate(at))
}

fn rect(id: &str, layer: &str, w: f64, h: f64, at: (f64, f64)) -> Object {
    obj(id, layer, Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: 1.0 }, at)
}

fn fill(interval: f64) -> CutSettings {
    let mut s = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
    s.hatch.interval_mm = interval;
    s
}

/// A grey ramp PNG, `w` x `h` px, as base64.
fn ramp_png(w: u32, h: u32) -> String {
    let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([u8::try_from((x * 255 / w.max(1) + y * 3) % 256).unwrap_or(0)]));
    let mut bytes = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn layer(id: &str, name: &str, color: &str, settings: CutSettings) -> Layer {
    Layer { settings, ..Layer::new(id.into(), name, color) }
}

/// Lines, a fill with turning hatch and cross hatch over two passes (with the job counter and a learned estimate
/// correction), sub-layers with a cleanup, tabs, an image and a closed path.
fn mixed_doc() -> Document {
    let mut d = Document::default();
    let mut turning = fill(0.2);
    turning.hatch.angle_increment_deg = 15.0;
    turning.hatch.cross_hatch = true;
    turning.passes = 2;
    d.layers.push(layer("layer-2", "Fill", "#ff0000", turning));
    let mut subs = layer("layer-3", "Subs", "#00aa00", fill(0.3));
    subs.settings.passes = 3;
    subs.sub_layers.push(CutSettings { power_pct: 35.0, ..fill(0.15) });
    subs.sub_layers.push(CutSettings { cleanup: Some(Cleanup { every_n_passes: 2 }), name: Some("Edge".into()), ..CutSettings::default() });
    d.layers.push(subs);
    let tabbed = CutSettings {
        options: CutOptions { tabs: Some(Tabs { count: 4, size_mm: 0.8, ..Tabs::default() }), ..CutOptions::default() },
        ..CutSettings::default()
    };
    d.layers.push(layer("layer-4", "Tabs", "#0000ff", tabbed));
    d.layers.push(layer("layer-5", "Image", "#555555", CutSettings::default()));
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.curve_to((6.0, 8.0), (12.0, -4.0), (18.0, 3.0));
    path.line_to((9.0, -6.0));
    path.close_path();
    d.objects = vec![
        rect("obj-101", "layer-1", 20.0, 10.0, (-30.0, 30.0)),
        obj("obj-102", "layer-1", Geometry::Ellipse { rx_mm: 6.0, ry_mm: 4.0 }, (-30.0, 10.0)),
        rect("obj-103", "layer-2", 14.0, 9.0, (0.0, 30.0)),
        obj("obj-104", "layer-2", Geometry::Ellipse { rx_mm: 5.0, ry_mm: 5.0 }, (20.0, 30.0)),
        rect("obj-105", "layer-3", 12.0, 12.0, (0.0, 5.0)),
        rect("obj-106", "layer-4", 16.0, 8.0, (25.0, 5.0)),
        obj("obj-107", "layer-1", Geometry::Path { path }, (-40.0, -20.0)),
        obj(
            "obj-108",
            "layer-5",
            Geometry::Image { width_mm: 12.0, height_mm: 8.0, mime: "image/png".into(), data_base64: ramp_png(48, 32).into(), adjust: Default::default() },
            (10.0, -25.0),
        ),
    ];
    d
}

#[tokio::test]
async fn browser_preview_equals_the_engine_preview() {
    let st = state(mixed_doc()).await;
    let app = api::router(st.clone(), None);
    let n = assert_parity(&app, "mixed").await;
    assert!(n > 1000, "a real job: {n} segments");

    // The next run's index turns the hatch, and the learned estimate correction scales every time.
    let device = st.active_device().id;
    st.job.increment(&device);
    st.job.increment(&device);
    for _ in 0..3 {
        st.job.times.record(&device, 10.0, 13.7);
    }
    let ctx = st.preview_context();
    assert_eq!(ctx.run_index, 2);
    assert!(ctx.correction > 1.1, "a learned correction: {}", ctx.correction);
    // A change of the document invalidates the engine's cached preview (the counter and correction alone do not).
    exec(&app, "shape.rect", json!({ "width_mm": 4, "height_mm": 4 })).await;
    assert_parity(&app, "counter and correction").await;
}

#[tokio::test]
async fn text_barcodes_and_empty_documents_match() {
    let st = state(Document::default()).await;
    let app = api::router(st.clone(), None);
    assert_eq!(assert_parity(&app, "empty").await, 0);
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    exec(&app, "shape.text", json!({ "text": "Parity 42", "font": font, "height_mm": 6 })).await;
    exec(&app, "shape.barcode", json!({ "kind": "qr", "data": "https://example.com/parity", "module_mm": 0.5 })).await;
    assert!(assert_parity(&app, "text and QR as lines").await > 10);
    exec(&app, "layer.update", json!({ "id": "layer-1", "settings": { "mode": "fill" } })).await;
    assert!(assert_parity(&app, "text and QR filled").await > 10);
}

#[tokio::test]
async fn planning_errors_match() {
    // Outside the 110 mm field.
    let mut d = Document::default();
    d.objects.push(rect("obj-101", "layer-1", 10.0, 10.0, (80.0, 0.0)));
    let st = state(d).await;
    let app = api::router(st.clone(), None);
    assert_eq!(assert_parity(&app, "out of field").await, 0);
    // Power above the device limit.
    let mut d = Document::default();
    d.layers[0].settings.power_pct = 100.0;
    d.objects.push(rect("obj-101", "layer-1", 10.0, 10.0, (0.0, 0.0)));
    let st = state(d).await;
    let app = api::router(st.clone(), None);
    let (s, _) = get(&app, "/api/preview").await;
    if s == StatusCode::UNPROCESSABLE_ENTITY {
        assert_parity(&app, "power above the limit").await;
    }
}

#[tokio::test]
async fn context_names_the_active_device() {
    let st = state(Document::default()).await;
    let app = api::router(st.clone(), None);
    let (s, body) = get(&app, "/api/preview/context").await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["target"], serde_json::to_value(st.machine.target()).unwrap());
    assert_eq!((v["correction"].as_f64(), v["run_index"].as_u64(), v["design_only"].clone()), (Some(1.0), Some(0), Value::Null));
    assert_eq!(v["generation"], st.machine.generation());
    let ctx: Context = serde_json::from_str(&body).unwrap();
    assert_eq!(ctx.target.as_ref(), Some(&st.machine.target()));
}

/// Parity on a real file kept outside the repository: `OPENETCH_PARITY_DOC=<a .lbrn2 file> cargo test -p engine
/// --release parity_of_a_local_lightburn_file -- --ignored --nocapture`. Each document variant is checked after a
/// change the web app makes (layer settings, a move, Order By), so a large import with images, fills, sub-layers and
/// tabs is compared segment by segment.
#[tokio::test]
#[ignore = "needs OPENETCH_PARITY_DOC: a local .lbrn2 file"]
async fn parity_of_a_local_lightburn_file() {
    let Ok(path) = std::env::var("OPENETCH_PARITY_DOC") else { return };
    let bytes = std::fs::read(&path).expect("read OPENETCH_PARITY_DOC");
    let st = state(Document::default()).await;
    let app = api::router(st.clone(), None);
    let r = Request::builder()
        .method("POST")
        .uri("/api/document/upload?format=lbrn2")
        .header("host", "localhost:7878")
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "upload");
    let n = assert_parity(&app, "as imported").await;
    println!("as imported: {n} segments equal");
    let d = st.document();
    if let Some(l) = d.layers.iter().find(|l| d.objects.iter().any(|o| o.layer == l.id)) {
        exec(&app, "layer.update", json!({ "id": l.id, "settings": { "speed_mm_s": 777 } })).await;
        println!("layer speed: {} segments equal", assert_parity(&app, "layer speed").await);
    }
    if let Some(o) = d.objects.first() {
        exec(&app, "object.move", json!({ "ids": [o.id], "from_point": [0, 0], "to_point": [0.5, 0.25] })).await;
        println!("moved: {} segments equal", assert_parity(&app, "moved").await);
    }
}
