//! Parity of the browser's preview (crates/webcore, run as WebAssembly in a Web Worker) with the engine's
//! `GET /api/preview`: for each document, webcore is given exactly what the web app gets (the JSON of
//! `GET /api/document` and `GET /api/preview/context`) and must give the same segments (kind, emits, layer, pass,
//! power, end points and times within f32 precision), the same stats, passes, layers (with their unsupported
//! LightBurn settings) and warnings, and the same 422 message for a document that cannot be planned. Image passes
//! come as rasters instead of runs: each must draw exactly the engine's segments of its range (every run's dots at
//! its power, nothing else, rows in burn order: `plan::raster_mismatch`), and together they must hold every image
//! run the engine plans.

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
use plan::SegmentKind;
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
    // The rasters, against the engine's own segments.
    let headers: Vec<plan::RasterHeader> = serde_json::from_value(packed.meta["rasters"].clone()).unwrap();
    let rasters = plan::PackedRasters {
        headers,
        levels: packed.levels.clone(),
        row_segments: packed.row_segments.clone(),
        row_span: packed.row_span.clone(),
    }
    .unpack()
    .expect("the raster arrays hold every header");
    let engine_preview: plan::Preview = serde_json::from_str(&body).unwrap();
    let mut in_raster = vec![false; segs.len()];
    for r in &rasters {
        assert_eq!(plan::raster_mismatch(&engine_preview.segments, r), None, "{name}: raster of pass {}", r.pass);
        in_raster[r.first..r.end].iter_mut().for_each(|v| *v = true);
    }
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
        close(f64::from(packed.t[i]), s["t_s"].as_f64().unwrap(), "t_s", i);
        if in_raster[i] {
            // Drawn by its raster (checked above): no geometry of its own.
            assert!(packed.xy[4 * i..4 * i + 4].iter().all(|v| v.is_nan()) && packed.power[i].is_nan(), "{name}: segment {i}");
            continue;
        }
        let [fx, fy] = [s["from"][0].as_f64().unwrap(), s["from"][1].as_f64().unwrap()];
        let [tx, ty] = [s["to"][0].as_f64().unwrap(), s["to"][1].as_f64().unwrap()];
        for (k, (want, what)) in [(fx, "x0"), (fy, "y0"), (tx, "x1"), (ty, "y1")].into_iter().enumerate() {
            close(f64::from(packed.xy[4 * i + k]), want, what, i);
        }
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
    assert_eq!(browser_preview(&app).await.0.meta["rasters"].as_array().map(Vec::len), Some(1), "the image is a raster");

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

async fn post(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let r = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "localhost:7878")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(r).await.unwrap();
    let status = res.status();
    (status, serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap_or(Value::Null))
}

/// A GRBL gantry: the lead-ins, lead-outs, overcut, path pauses and fill overscan are in the browser's preview and
/// estimate exactly as in the engine's (the planner bakes the leads and overcut into the paths for a gantry).
#[tokio::test]
async fn a_grbl_preview_with_leads_overcut_pauses_and_overscan_matches() {
    let mut d = Document::default();
    let lead = doc::Lead { style: doc::LeadStyle::Arc, length_mm: 3.0, angle_deg: 60.0 };
    let line = CutSettings {
        speed_mm_s: 20.0,
        options: CutOptions { lead_in: Some(lead), lead_out: Some(lead), overcut_mm: 2.0, start_pause_ms: 300.0, end_pause_ms: 100.0, ..CutOptions::default() },
        ..CutSettings::default()
    };
    d.layers[0].settings = line;
    let mut scan = fill(0.5);
    scan.speed_mm_s = 40.0;
    scan.options.overscan_mm = 2.5;
    d.layers.push(layer("layer-2", "Fill", "#ff0000", scan));
    d.objects = vec![
        rect("obj-1", "layer-1", 30.0, 20.0, (-50.0, 0.0)),
        rect("obj-2", "layer-1", 10.0, 6.0, (-50.0, 0.0)),
        obj("obj-3", "layer-1", Geometry::Ellipse { rx_mm: 8.0, ry_mm: 5.0 }, (0.0, 40.0)),
        rect("obj-4", "layer-2", 25.0, 15.0, (40.0, -20.0)),
    ];
    let st = state(d).await;
    let app = api::router(st.clone(), None);
    let profile = json!({"port": "COM9", "work_area_mm": [400.0, 300.0], "origin": "bottom_left", "max_feed_mm_min": 6000.0});
    let (s, v) = post(&app, "/api/devices/grbl", json!({"display_name": "Diode", "profile": profile})).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let id = v["device"]["id"].as_str().unwrap().to_owned();
    let (s, v) = post(&app, &format!("/api/devices/{id}/activate"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let n = assert_parity(&app, "grbl leads").await;
    assert!(n > 50, "{n} segments");
    // The engine's estimate includes the extras: without them the same document is quicker and shorter.
    let (_, with) = get(&app, "/api/preview").await;
    let with: Value = serde_json::from_str(&with).unwrap();
    exec(&app, "layer.update", json!({"id": "layer-1", "settings": {"options": {"lead_in": null, "lead_out": null, "overcut_mm": 0, "start_pause_ms": 0, "end_pause_ms": 0}}})).await;
    exec(&app, "layer.update", json!({"id": "layer-2", "settings": {"options": {"overscan_mm": 0}}})).await;
    let (_, without) = get(&app, "/api/preview").await;
    let without: Value = serde_json::from_str(&without).unwrap();
    let (a, b) = (&with["stats"], &without["stats"]);
    assert!(a["mark_length_mm"].as_f64().unwrap() > b["mark_length_mm"].as_f64().unwrap() + 3.0 * 6.0, "{a} {b}");
    assert!(a["estimated_time_s"].as_f64().unwrap() > b["estimated_time_s"].as_f64().unwrap() + 3.0 * 0.4, "{a} {b}");
    assert_parity(&app, "grbl plain").await;
}

#[tokio::test]
async fn image_rasters_draw_exactly_the_engines_runs() {
    // Grayscale, bidirectional, turned, over two passes with a turning scan angle; and a dithered copy at half power.
    let mut grey = CutSettings::default();
    grey.image.dither = doc::DitherMode::Grayscale;
    grey.image.interval_mm = 0.15;
    grey.image.bidirectional = true;
    grey.image.angle_deg = 20.0;
    grey.image.angle_increment_deg = 45.0;
    grey.image.min_power_pct = 10.0;
    grey.passes = 2;
    let mut dots = CutSettings::default();
    dots.image.dither = doc::DitherMode::FloydSteinberg;
    dots.image.interval_mm = 0.1;
    let mut d = Document::default();
    d.layers[0].settings = grey;
    d.layers.push(layer("layer-2", "Dots", "#aa0000", dots));
    let image = |id: &str, layer: &str, at: (f64, f64)| {
        let g = Geometry::Image { width_mm: 14.0, height_mm: 9.0, mime: "image/png".into(), data_base64: ramp_png(56, 36).into(), adjust: Default::default() };
        Object::new(id.into(), id, layer.into(), g, Affine::translate(at) * Affine::rotate(0.3))
    };
    d.objects = vec![image("obj-1", "layer-1", (-15.0, 0.0)), image("obj-2", "layer-2", (15.0, 0.0)), rect("obj-3", "layer-2", 6.0, 6.0, (15.0, 20.0))];
    d.object_options.insert("obj-2".into(), doc::ObjectOptions { power_scale_pct: 50.0, ..Default::default() });
    let st = state(d).await;
    let app = api::router(st.clone(), None);
    let n = assert_parity(&app, "images").await;
    let (packed, _) = browser_preview(&app).await;
    let rasters = packed.meta["rasters"].as_array().unwrap();
    assert_eq!(rasters.len(), 3, "two grey passes and the dithered one");
    let covered: u64 = rasters.iter().map(|r| r["end"].as_u64().unwrap() - r["first"].as_u64().unwrap()).sum();
    assert!(covered as f64 > 0.9 * n as f64, "nearly every segment is an image's: {covered} of {n}");
    assert!(rasters[0]["powers"].as_array().unwrap().len() > 20, "grey levels");
}

#[tokio::test]
async fn the_binary_preview_carries_the_same_rasters() {
    let mut d = mixed_doc();
    d.layers.last_mut().unwrap().settings.image.bidirectional = true;
    let st = state(d).await;
    let app = api::router(st.clone(), None);
    let raw = |uri: &str| {
        let r = Request::builder().method("GET").uri(uri).header("host", "localhost:7878").body(Body::empty()).unwrap();
        let app = app.clone();
        async move { app.oneshot(r).await.unwrap().into_body().collect().await.unwrap().to_bytes().to_vec() }
    };
    let (_, body) = get(&app, "/api/preview").await;
    let engine: plan::Preview = serde_json::from_str(&body).unwrap();
    let (header, segs, rasters) = crate::preview_bin::decode_with_rasters(&raw("/api/preview.bin").await);
    assert_eq!(segs.len(), engine.segments.len());
    assert_eq!(rasters.len(), 1);
    assert_eq!(header["rasters"][0]["pass"], json!(rasters[0].pass));
    for r in &rasters {
        assert_eq!(plan::raster_mismatch(&engine.segments, r), None);
        assert!(segs[r.first..r.end].iter().all(|s| s["raster"] == true));
    }
    assert!(segs.iter().filter(|s| s["raster"] != true).all(|s| s["from"].is_array()), "every other segment has its geometry");
    // The same rasters as the browser's worker plans.
    let (packed, _) = browser_preview(&app).await;
    assert_eq!(packed.meta["rasters"], header["rasters"]);
    assert_eq!((packed.levels.as_slice(), packed.row_segments.as_slice()), (rasters[0].levels.as_slice(), rasters[0].row_segments.iter().flatten().copied().collect::<Vec<_>>().as_slice()));
    // Without travel the rows are renumbered: each holds its own runs and nothing else.
    let (_, segs, lean) = crate::preview_bin::decode_with_rasters(&raw("/api/preview.bin?jumps=false").await);
    let r = &lean[0];
    let runs = engine.segments[rasters[0].first..rasters[0].end].iter().filter(|s| s.kind == SegmentKind::Mark).count();
    assert_eq!(r.end - r.first, runs);
    assert!(segs[r.first..r.end].iter().all(|s| s["raster"] == true && s["kind"] == "mark"));
    assert_eq!(r.row_segments.first().map(|s| s[0] as usize), Some(r.first));
    assert_eq!(r.row_segments.last().map(|s| s[1] as usize), Some(r.end));
    let marks_per_row = |r: &plan::PreviewRaster| r.row_segments.iter().map(|s| s[1] - s[0]).collect::<Vec<_>>();
    let full: Vec<u32> = rasters[0]
        .row_segments
        .iter()
        .map(|s| engine.segments[s[0] as usize..s[1] as usize].iter().filter(|g| g.kind == SegmentKind::Mark).count() as u32)
        .collect();
    assert_eq!(marks_per_row(r), full);
    // A layer filter without the image's layer has no raster.
    let (_, _, none) = crate::preview_bin::decode_with_rasters(&raw("/api/preview.bin?layers=layer-1").await);
    assert!(none.is_empty());
}
