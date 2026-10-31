//! Tests over the hand-written fixtures in `tests/fixtures/` and synthetic documents.

use doc::{CutSettings, Document, FillMode, Geometry, Layer, Object, Stock, StockShape};
use kurbo::{Affine, BezPath, ParamCurve, Point, Rect};
use lbrn::{LbrnError, LbrnProject, Prims, ShapeKind, from_document, parse, to_document, write};

const SHAPES: &str = include_str!("fixtures/shapes.lbrn2");
const SETTINGS: &str = include_str!("fixtures/settings.lbrn2");
const FIELD: f64 = 150.0;

fn import(text: &str) -> (Vec<Layer>, Vec<Object>, Vec<String>) {
    to_document(&parse(text).unwrap(), FIELD).unwrap()
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn rect_close(a: Rect, b: Rect, tol: f64) -> bool {
    close(a.x0, b.x0, tol) && close(a.y0, b.y0, tol) && close(a.x1, b.x1, tol) && close(a.y1, b.y1, tol)
}

fn obj<'a>(objects: &'a [Object], name: &str) -> &'a Object {
    objects.iter().find(|o| o.name == name).unwrap_or_else(|| panic!("no object {name}"))
}

#[test]
fn parses_the_shapes_fixture() {
    let p = parse(SHAPES).unwrap();
    assert_eq!(p.attribute("AppVersion"), Some("1.4.00"));
    assert_eq!(p.cut_settings.len(), 5);
    assert!(p.cut_settings[3].image && p.cut_settings[3].kind == "Image");
    assert_eq!(p.cut_settings[0].params.frequency, Some(100_000.0));
    assert_eq!(p.cut_settings[0].params.other.get("tabCount").map(String::as_str), Some("1"));
    assert_eq!(p.shapes.len(), 11);
    // The thumbnail is dropped; the other root elements are kept.
    let names: Vec<&str> = p.extra_elements.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["VariableText", "UIPrefs", "Notes"]);
    // The second path resolved both shared lists.
    match (&p.shapes[2].kind, &p.shapes[3].kind) {
        (ShapeKind::Path(a), ShapeKind::Path(b)) => {
            assert_eq!(a, b);
            assert_eq!(a.vertices[1].c0, Some(Point::new(5.0, 3.0)));
            assert_eq!(a.vertices[1].c1, None, "c1x1 is the no-control flag");
        }
        other => panic!("{other:?}"),
    }
    match &p.shapes[4].kind {
        ShapeKind::Path(d) => assert_eq!(d.prims, Prims::LineClosed),
        other => panic!("{other:?}"),
    }
    let text = &p.shapes[6];
    assert_eq!(text.attribute("Str"), Some("Hi\nthere"));
    assert!(matches!(&text.kind, ShapeKind::Text { backup: Some(_) }));
    let bitmap = &p.shapes[8];
    assert!(matches!(bitmap.kind, ShapeKind::Bitmap { w, h } if w == 10.0 && h == 10.0));
    assert!(bitmap.attribute("Data").is_some_and(|d| d.starts_with("iVBOR")));
    let star = &p.shapes[9];
    assert_eq!(star.type_name(), "Star");
    assert_eq!(star.attribute("N"), Some("5"));
    assert_eq!(star.children.len(), 1, "unknown children are kept");
}

#[test]
fn write_then_parse_is_identity() {
    for text in [SHAPES, SETTINGS] {
        let p = parse(text).unwrap();
        let out = write(&p);
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnProject AppVersion="));
        assert!(!out.contains("Thumbnail"));
        let back = parse(&out).unwrap();
        assert_eq!(back, p);
        assert_eq!(write(&back), out, "writing is deterministic");
    }
}

#[test]
fn written_layout_mirrors_lightburn() {
    let out = write(&parse(SHAPES).unwrap());
    let pos = |s: &str| out.find(s).unwrap_or_else(|| panic!("missing {s}"));
    assert!(pos("<VariableText>") < pos("<CutSetting type=\"Scan\">"));
    assert!(pos("<CutSetting type=\"Scan\">") < pos("<Shape Type=\"Rect\""));
    assert!(pos("<Shape Type=\"Rect\"") < pos("<Notes "));
    assert!(out.contains("        <index Value=\"0\"/>\n        <name Value=\"Fill\"/>\n        <minPower Value=\"12\"/>"));
    assert!(out.contains("<Shape Type=\"Rect\" CutIndex=\"0\" W=\"20\" H=\"10\" Cr=\"2\">\n        <XForm>1 0 0 1 75 75</XForm>"));
    assert!(out.contains("<VertList>V-5 0c0x1c1x1V5 0c0x5c0y3c1x1V0 8c0x1c1x2c1y8</VertList>"));
    assert!(out.contains("<PrimList>L0 1B1 2L2 0</PrimList>"));
    assert!(out.contains("Str=\"Hi&#10;there\""));
    assert!(out.contains("<CutSetting_Img type=\"Image\">"));
    // Every path gets its own ids, so no list is shared.
    assert!(out.contains("VertID=\"0\" PrimID=\"0\"") && out.contains("VertID=\"1\" PrimID=\"1\""));
}

#[test]
fn imports_layers_in_priority_order_with_settings() {
    let (layers, _, _) = import(SHAPES);
    let names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Fill", "Outline", "Guide", "Photo", "T1"]);
    let ids: Vec<&str> = layers.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["layer-1", "layer-2", "layer-3", "layer-4", "layer-5"]);
    let fill = &layers[0];
    assert_eq!(fill.color, "#000000");
    assert_eq!(fill.settings.mode, FillMode::Fill);
    assert_eq!(fill.settings.power_pct, 30.0);
    assert_eq!(fill.settings.speed_mm_s, 2000.0);
    assert_eq!(fill.settings.frequency_khz, 100.0);
    assert_eq!(fill.settings.pulse_width_ns, Some(200.0));
    assert!(fill.settings.hatch.cross_hatch && fill.settings.hatch.bidirectional);
    assert_eq!(fill.settings.hatch.interval_mm, 0.03);
    assert_eq!(fill.settings.hatch.angle_deg, 90.0);
    let extras: Vec<(&str, &str)> = fill.extras.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(extras, [("lbrn2.index", "0"), ("lbrn2.minPower", "12"), ("lbrn2.tabCount", "1")]);
    let outline = &layers[1];
    assert_eq!(outline.color, "#0000ff");
    assert_eq!(outline.settings.mode, FillMode::Line);
    assert_eq!(outline.settings.passes, 2);
    assert!(!outline.settings.hatch.bidirectional);
    assert_eq!(outline.settings.pulse_width_ns, None);
    let guide = &layers[2];
    assert!(!guide.visible && !guide.output);
    let photo = &layers[3];
    assert_eq!(photo.settings.mode, FillMode::Fill);
    assert_eq!(photo.extras.get("lbrn2.type").map(String::as_str), Some("Image"));
    assert_eq!(photo.extras.get("lbrn2.ditherMode").map(String::as_str), Some("dithered"));
    let tool = &layers[4];
    assert!(!tool.output);
    assert_eq!(tool.color, "#f36926");
}

#[test]
fn imports_shapes_in_the_field_frame() {
    let (layers, objects, warnings) = import(SHAPES);
    let names: Vec<&str> = objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        ["Rectangle 1", "Ellipse 1", "Path 1", "Path 2", "Path 3", "Group 1.1.1", "Group 1.1.2", "Hi", "Rectangle 2"]
    );
    assert_eq!(objects[0].id, "obj-6", "ids continue after the five layers");
    // Rect at LightBurn (75, 75) is the field centre.
    let r = obj(&objects, "Rectangle 1");
    assert!(matches!(r.geometry, Geometry::Rect { width_mm, height_mm, corner_radius_mm }
        if width_mm == 20.0 && height_mm == 10.0 && corner_radius_mm == 2.0));
    assert!(rect_close(r.bounds(), Rect::new(-10.0, -5.0, 10.0, 5.0), 1e-9));
    assert_eq!(r.layer, layers[0].id);
    // The ellipse is rotated 90 degrees: its 5 mm radius now runs along y.
    let e = obj(&objects, "Ellipse 1");
    assert!(rect_close(e.bounds(), Rect::new(17.0, -5.0, 23.0, 5.0), 1e-2), "{:?}", e.bounds());
    // Shared lists, different placement.
    let (p1, p2) = (obj(&objects, "Path 1"), obj(&objects, "Path 2"));
    assert_eq!(p1.geometry, p2.geometry);
    assert!(close(p1.bounds().x0 - p2.bounds().x0, 20.0, 1e-9));
    assert!(close(p1.bounds().y0, -25.0, 1e-9) && close(p1.bounds().y1, -17.0, 1e-9));
    // Nested groups compose: scale 2 about (100, 100), then +5 mm in x.
    let g = obj(&objects, "Group 1.1.1");
    assert!(rect_close(g.bounds(), Rect::new(33.0, 23.0, 37.0, 27.0), 1e-9), "{:?}", g.bounds());
    assert_eq!(g.layer, layers[2].id);
    let gp = obj(&objects, "Group 1.1.2");
    assert!(rect_close(gp.bounds(), Rect::new(35.0, 35.0, 37.0, 35.0), 1e-9), "{:?}", gp.bounds());
    // The text became its outline.
    let t = obj(&objects, "Hi");
    assert!(matches!(t.geometry, Geometry::Path { .. }));
    assert!(rect_close(t.bounds(), Rect::new(-1.0, -47.0, 1.0, -43.0), 1e-9));
    assert_eq!(obj(&objects, "Rectangle 2").layer, layers[4].id, "tool shape on the tool layer");

    let has = |s: &str| warnings.iter().any(|w| w.contains(s));
    assert!(has("outline (BackupPath)"), "{warnings:?}");
    assert!(has("\"No outline\" was skipped"), "{warnings:?}");
    assert!(has("bitmap was skipped"), "{warnings:?}");
    assert!(has("'Star' shape was skipped"), "{warnings:?}");
    assert!(has("Photo") || has("image (bitmap) settings"), "{warnings:?}");
    assert!(!has("outside"), "{warnings:?}");
}

#[test]
fn imports_sub_layers_and_reports_bad_values() {
    let (layers, objects, warnings) = import(SETTINGS);
    let names: Vec<&str> = layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["deep", "Offset", "C07"]);
    let deep = &layers[0];
    assert_eq!(deep.settings.mode, FillMode::FillThenLine);
    assert_eq!(deep.settings.power_pct, 100.0);
    assert_eq!(deep.settings.speed_mm_s, CutSettings::default().speed_mm_s);
    assert_eq!(deep.settings.frequency_khz, CutSettings::default().frequency_khz);
    assert_eq!(deep.settings.passes, 1000);
    assert_eq!(deep.settings.hatch.angle_increment_deg, 90.0);
    assert_eq!(deep.settings.hatch.angle_deg, -135.0);
    assert_eq!(deep.sub_layers.len(), 2);
    let s1 = &deep.sub_layers[0];
    assert_eq!((s1.mode.clone(), s1.power_pct, s1.speed_mm_s, s1.frequency_khz), (FillMode::Fill, 50.0, 3000.0, 80.0));
    assert_eq!((s1.passes, s1.hatch.cross_hatch, s1.hatch.angle_deg), (3, true, 180.0));
    assert_eq!(deep.sub_layers[1].mode, FillMode::Line);
    let ex = |k: &str| deep.extras.get(k).map(String::as_str);
    assert_eq!(ex("lbrn2.subname"), Some("deep"));
    assert_eq!(ex("lbrn2.wobbleEnable"), Some("1"));
    assert_eq!(ex("lbrn2.sub1.subname"), Some("clean"));
    assert_eq!(ex("lbrn2.speed"), None, "an unreadable value is dropped with a warning");
    assert_eq!(layers[1].settings.mode, FillMode::Line);
    assert_eq!(layers[1].extras.get("lbrn2.type").map(String::as_str), Some("Offset Fill"));
    assert_eq!(objects.len(), 2);
    let r = objects.iter().find(|o| matches!(o.geometry, Geometry::Rect { .. })).unwrap();
    assert!(matches!(r.geometry, Geometry::Rect { corner_radius_mm, .. } if corner_radius_mm == 2.5));

    let has = |s: &str| warnings.iter().any(|w| w.contains(s));
    for w in [
        "MirrorY=True was not applied",
        "power 150 % clamped",
        "speed value 'fast' could not be read",
        "frequency 0 Hz is not usable",
        "5000 passes clamped to 1000",
        "cut type 'Offset Fill' is not supported",
        "C07: used by shapes but has no cut settings",
        "corner radius 9 mm was clamped to 2.5 mm",
        "1 object(s) are partly or wholly outside the 150 mm field",
    ] {
        assert!(has(w), "missing warning '{w}' in {warnings:#?}");
    }
}

/// Every segment of `a` and `b` (field frame) agrees within `tol` at several parameters.
fn assert_same_geometry(a: &Object, b: &Object, tol: f64) {
    let (sa, sb): (Vec<_>, Vec<_>) = (a.field_path().segments().collect(), b.field_path().segments().collect());
    assert_eq!(sa.len(), sb.len(), "{} vs {}", a.name, b.name);
    for (x, y) in sa.iter().zip(&sb) {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let d = (x.eval(t) - y.eval(t)).hypot();
            assert!(d <= tol, "{}: {x:?} vs {y:?} differ by {d} at t={t}", a.name);
        }
    }
    match (&a.geometry, &b.geometry) {
        (Geometry::Path { .. }, Geometry::Path { .. }) => {}
        (ga, gb) => assert_eq!(ga, gb),
    }
}

fn layer(d: &mut Document, name: &str, color: &str, f: impl FnOnce(&mut Layer)) -> String {
    let id = d.alloc_id("layer");
    let mut l = Layer::new(id.clone(), name, color);
    f(&mut l);
    d.layers.push(l);
    id
}

fn object(d: &mut Document, layer: &str, geometry: Geometry, transform: Affine) {
    let id = d.alloc_id("obj");
    d.objects.push(Object { id: id.clone(), name: id, layer: layer.into(), geometry, transform });
}

fn sample_document() -> Document {
    let mut d = Document { field_mm: Some(FIELD), ..Document::default() };
    d.layers[0].color = "#000000".into();
    let l1 = d.layers[0].id.clone();
    let l2 = layer(&mut d, "Deep", "#ff0000", |l| {
        l.settings.mode = FillMode::FillThenLine;
        l.settings.power_pct = 55.5;
        l.settings.speed_mm_s = 850.0;
        l.settings.frequency_khz = 233.0;
        l.settings.pulse_width_ns = Some(10.0);
        l.settings.passes = 3;
        l.settings.hatch.interval_mm = 0.003;
        l.settings.hatch.angle_deg = 45.0;
        l.settings.hatch.angle_increment_deg = 30.0;
        l.settings.hatch.cross_hatch = true;
        l.settings.hatch.bidirectional = false;
        l.sub_layers.push(CutSettings { mode: FillMode::Fill, power_pct: 10.0, frequency_khz: 80.0, ..Default::default() });
    });
    let l3 = layer(&mut d, "Guides", "#123456", |l| {
        l.output = false;
        l.visible = false;
    });
    object(
        &mut d,
        &l1,
        Geometry::Rect { width_mm: 30.0, height_mm: 12.5, corner_radius_mm: 3.0 },
        Affine::translate((-20.0, 15.0)) * Affine::rotate(0.3),
    );
    object(&mut d, &l2, Geometry::Ellipse { rx_mm: 8.0, ry_mm: 4.0 }, Affine::translate((40.0, -30.0)));
    let mut p = BezPath::new();
    p.move_to((0.0, 0.0));
    p.curve_to((5.0, 10.0), (15.0, 10.0), (20.0, 0.0));
    p.line_to((10.0, -5.0));
    p.close_path();
    p.move_to((30.0, 0.0));
    p.quad_to((35.0, 5.0), (40.0, 0.0));
    p.line_to((40.0, -10.0));
    p.move_to((50.0, 0.0));
    p.line_to((55.0, 0.0));
    p.line_to((55.0, 5.0));
    p.line_to((50.0, 0.0));
    p.close_path();
    object(&mut d, &l2, Geometry::Path { path: p }, Affine::translate((-50.0, -50.0)) * Affine::scale(1.5));
    object(&mut d, &l3, Geometry::Ellipse { rx_mm: 2.0, ry_mm: 2.0 }, Affine::IDENTITY);
    d
}

#[test]
fn document_round_trips_through_lbrn2() {
    let d = sample_document();
    let (project, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let text = write(&project);
    let reparsed: LbrnProject = parse(&text).unwrap();
    assert_eq!(reparsed, project);
    let (layers, objects, warnings) = to_document(&reparsed, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    assert_eq!(layers.len(), d.layers.len());
    for (a, b) in d.layers.iter().zip(&layers) {
        assert_eq!(a.name, b.name);
        assert_eq!((a.visible, a.output), (b.visible, b.output), "{}", a.name);
        assert_eq!(a.settings, b.settings, "{}", a.name);
        assert_eq!(a.sub_layers, b.sub_layers, "{}", a.name);
        let mut extras = b.extras.clone();
        assert!(extras.remove("lbrn2.index").is_some());
        assert_eq!(a.extras, extras, "{}", a.name);
    }
    // Palette colours survive; others are replaced by the colour of the free index they got.
    assert_eq!(layers[0].color, "#000000");
    assert_eq!(layers[1].color, "#ff0000");
    assert_eq!(layers[2].color, "#0000ff");
    assert_eq!(layers[1].extras["lbrn2.index"], "2");

    assert_eq!(objects.len(), d.objects.len());
    for (a, b) in d.objects.iter().zip(&objects) {
        let la = d.layers.iter().position(|l| l.id == a.layer);
        let lb = layers.iter().position(|l| l.id == b.layer);
        assert_eq!(la, lb, "{}", a.name);
        assert_same_geometry(a, b, 1e-6);
    }
}

#[test]
fn round_trip_keeps_imported_extras_and_indices() {
    let (layers, objects, _) = import(SHAPES);
    let d = Document { layers: layers.clone(), objects: objects.clone(), ..Document::default() };
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let indices: Vec<u32> = p.cut_settings.iter().map(|c| c.index).collect();
    assert_eq!(indices, [0, 1, 3, 5, 30]);
    assert!(p.cut_settings[3].image, "image settings are written back as CutSetting_Img");
    assert_eq!(p.cut_settings[4].kind, "Tool");
    assert_eq!(p.cut_settings[2].params.do_output, Some(false));
    assert_eq!(p.cut_settings[2].params.hide, Some(true));
    assert_eq!(p.cut_settings[0].params.min_power, Some(12.0));
    assert_eq!(p.cut_settings[0].params.other.get("tabCount").map(String::as_str), Some("1"));
    let (layers2, objects2, _) = to_document(&parse(&write(&p)).unwrap(), FIELD).unwrap();
    assert_eq!(layers2, layers);
    assert_eq!(objects2.len(), objects.len());
    for (a, b) in objects.iter().zip(&objects2) {
        assert_same_geometry(a, b, 1e-6);
    }
}

#[test]
fn stock_is_exported_on_tool_layer_t1() {
    let mut d = sample_document();
    d.stock = Some(Stock {
        name: "tag".into(),
        shape: StockShape::Circle { diameter_mm: 40.0 },
        thickness_mm: 1.0,
        material: None,
        center: [5.0, -5.0],
        rotation_deg: 0.0,
    });
    let (p, _) = from_document(&d, FIELD).unwrap();
    let tool = p.shapes.iter().find(|s| s.cut_index == Some(30)).expect("T1 shape");
    assert!(matches!(tool.kind, ShapeKind::Ellipse { rx, ry } if rx == 20.0 && ry == 20.0));
    assert_eq!(tool.xform.translation(), kurbo::Vec2::new(80.0, 70.0));
    let cs = p.cut_setting(30).expect("T1 settings");
    assert_eq!((cs.kind.as_str(), cs.params.name.as_deref()), ("Tool", Some("T1")));
    let (layers, objects, _) = to_document(&p, FIELD).unwrap();
    let t1 = layers.iter().find(|l| l.name == "T1").unwrap();
    assert!(!t1.output);
    let stock_obj = objects.iter().find(|o| o.layer == t1.id).unwrap();
    assert!(rect_close(stock_obj.bounds(), Rect::new(-15.0, -25.0, 25.0, 15.0), 1e-2));
}

#[test]
fn index_assignment_and_limits() {
    let mut d = Document::default();
    d.layers[0].color = "#00e000".into(); // palette C03
    layer(&mut d, "kept", "#000000", |l| {
        l.extras.insert("lbrn2.index".into(), "7".into());
    });
    layer(&mut d, "clash", "#00e000", |_| {});
    layer(&mut d, "tool", "#000000", |l| {
        l.output = false;
        l.extras.insert("lbrn2.index".into(), "31".into());
    });
    layer(&mut d, "not a tool", "#000000", |l| {
        l.extras.insert("lbrn2.index".into(), "31".into());
    });
    let (p, _) = from_document(&d, FIELD).unwrap();
    let idx: Vec<u32> = p.cut_settings.iter().map(|c| c.index).collect();
    assert_eq!(idx, [3, 7, 0, 31, 1]);
    assert_eq!(p.cut_settings[3].kind, "Tool");

    let mut d = Document::default();
    for k in 0..30 {
        layer(&mut d, &format!("L{k}"), "#000000", |_| {});
    }
    let e = from_document(&d, FIELD).unwrap_err();
    assert!(e.to_string().contains("more layers"), "{e}");
    for bad in [0.0, -1.0, f64::NAN, 1e9] {
        assert!(matches!(from_document(&Document::default(), bad), Err(LbrnError::Invalid(_))));
        assert!(matches!(to_document(&parse(SHAPES).unwrap(), bad), Err(LbrnError::Invalid(_))));
    }
}

#[test]
fn exported_values_are_lightburn_units() {
    let d = sample_document();
    let (p, _) = from_document(&d, FIELD).unwrap();
    let deep = p.cut_setting(2).unwrap();
    assert_eq!(deep.kind, "Scan+Cut");
    let v = &deep.params;
    assert_eq!((v.max_power, v.speed, v.frequency, v.q_pulse_width), (Some(55.5), Some(850.0), Some(233_000.0), Some(10.0)));
    assert_eq!((v.num_passes, v.priority, v.bidir, v.cross_hatch), (Some(3), Some(1), Some(false), Some(true)));
    assert_eq!(v.angle_per_pass, Some(30.0));
    assert_eq!(deep.sub_layers.len(), 1);
    assert_eq!((deep.sub_layers[0].kind.as_str(), deep.sub_layers[0].index), ("Scan", 1));
    assert_eq!(deep.sub_layers[0].params.frequency, Some(80_000.0));
    let text = write(&p);
    assert!(text.contains("<frequency Value=\"233000\"/>"), "{text}");
    let guides = p.cut_settings.iter().find(|c| c.params.name.as_deref() == Some("Guides")).unwrap();
    assert_eq!((guides.params.do_output, guides.params.hide), (Some(false), Some(true)));
}

#[test]
fn refuses_bad_input() {
    let bad = |t: &str| parse(t).unwrap_err();
    assert!(matches!(bad("not xml"), LbrnError::Xml(_)));
    assert!(matches!(bad("<LightBurnLibrary/>"), LbrnError::NotProject(m) if m.contains(".clb")));
    assert!(matches!(bad("<svg/>"), LbrnError::NotProject(_)));
    assert!(matches!(bad("<!DOCTYPE x [<!ENTITY a \"b\">]><LightBurnProject/>"), LbrnError::Xml(_)));
    assert!(matches!(bad("<LightBurnProject><CutSetting type=\"Cut\"/></LightBurnProject>"), LbrnError::Malformed(_)));
    let path = |attrs: &str, body: &str| format!("<LightBurnProject><Shape Type=\"Path\" {attrs}>{body}</Shape></LightBurnProject>");
    assert!(matches!(bad(&path("VertID=\"9\" PrimID=\"9\"", "")), LbrnError::Malformed(m) if m.contains("VertID 9")));
    assert!(matches!(bad(&path("", "<VertList>V0 0V1 1</VertList><PrimList>L0 2</PrimList>")), LbrnError::Malformed(_)));
    assert!(matches!(bad(&path("", "<XForm>1 0 0 1 0</XForm><VertList>V0 0</VertList><PrimList></PrimList>")), LbrnError::Malformed(_)));
    assert!(matches!(bad("<LightBurnProject><Shape Type=\"Rect\" W=\"1\"/></LightBurnProject>"), LbrnError::Malformed(_)));
    assert!(matches!(bad("<LightBurnProject><Shape Type=\"Rect\" CutIndex=\"-1\" W=\"1\" H=\"1\"/></LightBurnProject>"), LbrnError::Malformed(_)));
    let mut deep = String::from("<LightBurnProject>");
    for _ in 0..70 {
        deep.push_str("<Shape Type=\"Group\"><Children>");
    }
    for _ in 0..70 {
        deep.push_str("</Children></Shape>");
    }
    deep.push_str("</LightBurnProject>");
    assert!(matches!(bad(&deep), LbrnError::TooLarge(_)));
    // An empty project imports as one default layer and no objects.
    let (layers, objects, _) = to_document(&parse("<LightBurnProject/>").unwrap(), FIELD).unwrap();
    assert_eq!((layers.len(), objects.len()), (1, 0));
}

#[test]
fn library_entries_and_sub_layers() {
    let clb = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnLibrary>
  <Material name="Steel">
    <Entry Thickness="-1.0000" Desc="Black" NoThickTitle="Mark">
      <CutSetting type="Scan">
        <index Value="0" />
        <name Value="" />
        <LinkPath Value="Steel/Mark/Black" />
        <maxPower Value="37" />
        <speed Value="158" />
        <frequency Value="40000" />
        <JumpSpeed Value="4000" />
        <numPasses Value="2" />
        <subname Value="dark" />
        <SubLayer type="Scan" index="1">
          <maxPower Value="50" />
          <speed Value="3000" />
          <frequency Value="80000" />
          <subname Value="clean" />
        </SubLayer>
      </CutSetting>
    </Entry>
  </Material>
</LightBurnLibrary>"#;
    let entries = lbrn::parse_library(clb).unwrap();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!((e.material.as_str(), e.title.as_str(), e.description.as_str()), ("Steel", "Mark", "Black"));
    let (l, warnings) = lbrn::cut_setting_to_layer(&e.setting, "layer-1");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(l.name, "dark");
    assert_eq!(l.settings.passes, 2);
    assert_eq!(l.sub_layers.len(), 1);
    assert_eq!(l.sub_layers[0].frequency_khz, 80.0);
    assert_eq!(l.extras.get("lbrn2.JumpSpeed").map(String::as_str), Some("4000"));
    assert!(lbrn::parse_library("<LightBurnProject/>").is_err());
    assert!(parse(clb).is_err());
}

#[test]
fn objects_on_missing_layers_and_empty_paths_are_reported() {
    let mut d = Document::default();
    object(&mut d, "layer-404", Geometry::Ellipse { rx_mm: 1.0, ry_mm: 1.0 }, Affine::IDENTITY);
    object(&mut d, "layer-1", Geometry::Path { path: BezPath::new() }, Affine::IDENTITY);
    object(&mut d, "layer-1", Geometry::Rect { width_mm: 10.0, height_mm: 10.0, corner_radius_mm: 0.0 }, Affine::translate((80.0, 0.0)));
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert_eq!(p.shapes.len(), 1);
    assert!(warnings.iter().any(|w| w.contains("missing layer")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("empty path")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("outside the 150 mm field")), "{warnings:?}");
}
