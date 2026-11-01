//! Tests over the hand-written fixtures in `tests/fixtures/` and synthetic documents.

use doc::{CutSettings, Document, FillMode, Geometry, Layer, Object, Stock, StockShape};
use kurbo::{Affine, BezPath, ParamCurve, Point, Rect};
use lbrn::{LbrnError, LbrnProject, Prims, ShapeKind, from_document, parse, to_document, write};

const SHAPES: &str = include_str!("fixtures/shapes.lbrn2");
const SETTINGS: &str = include_str!("fixtures/settings.lbrn2");
const FIELD: f64 = 150.0;

fn import(text: &str) -> (Vec<Layer>, Vec<Object>, Vec<String>) {
    tuple(to_document(&parse(text).unwrap(), FIELD).unwrap())
}

fn tuple(i: lbrn::Imported) -> (Vec<Layer>, Vec<Object>, Vec<String>) {
    (i.layers, i.objects, i.warnings)
}

/// The whole document an import makes.
fn import_document(text: &str) -> Document {
    let i = to_document(&parse(text).unwrap(), FIELD).unwrap();
    Document {
        layers: i.layers,
        objects: i.objects,
        groups: i.groups,
        extras: i.extras,
        field_mm: Some(FIELD),
        next_id: 100,
        ..Document::default()
    }
}

/// Extras without the bookkeeping keys (`lbrn2._*`).
fn plain_extras(l: &Layer) -> Vec<(&str, &str)> {
    l.extras.iter().filter(|(k, _)| !k.starts_with("lbrn2._")).map(|(k, v)| (k.as_str(), v.as_str())).collect()
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
    // Every other root element is kept, the thumbnail included.
    let names: Vec<&str> = p.extra_elements.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Thumbnail", "VariableText", "UIPrefs", "Notes"]);
    // The second path resolved both shared lists, and remembers it only referred to them.
    match (&p.shapes[2].kind, &p.shapes[3].kind) {
        (ShapeKind::Path(a), ShapeKind::Path(b)) => {
            assert_eq!((&a.vertices, &a.prims), (&b.vertices, &b.prims));
            let (ea, eb) = (a.encoded.as_ref().unwrap(), b.encoded.as_ref().unwrap());
            assert_eq!((ea.vert_id.as_deref(), ea.prim_id.as_deref()), (Some("1"), Some("2")));
            assert!(ea.vert_list.is_some() && eb.vert_list.is_none() && eb.prim_list.is_none());
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
        assert_eq!(out.contains("<Thumbnail Source="), text.contains("<Thumbnail Source="));
        // Only the comments are gone.
        let without_comments: String =
            text.lines().filter(|l| !l.trim_start().starts_with("<!--")).map(|l| format!("{l}\n")).collect();
        assert_eq!(out, without_comments);
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
    // Path ids and the sharing of lists are kept: the second user of lists 1 and 2 carries none.
    assert_eq!(out.matches("VertID=\"1\" PrimID=\"2\"").count(), 2);
    assert_eq!(out.matches("<VertList>").count(), 4);
}

#[test]
fn new_paths_get_ids_above_the_kept_ones() {
    let mut p = parse(SHAPES).unwrap();
    let mut b = BezPath::new();
    b.move_to((0.0, 0.0));
    b.line_to((1.0, 1.0));
    p.shapes.push(lbrn::Shape::new(ShapeKind::Path(lbrn::PathData::from_bez(&b)), Some(0), Affine::IDENTITY));
    let out = write(&p);
    assert!(out.contains("VertID=\"6\" PrimID=\"6\""), "one above the largest kept id (5)");
    assert_eq!(parse(&out).unwrap().shapes.len(), p.shapes.len());
}

#[test]
fn layout_comes_back_as_read() {
    let crlf = SHAPES.replace("<!-- Hand-written", "<!--").replace('\n', "\r\n");
    let p = parse(&crlf).unwrap();
    assert!(p.format.crlf && p.format.final_newline);
    let out = write(&p);
    assert!(out.contains("\r\n") && !out.replace("\r\n", "").contains('\n'), "CRLF throughout");
    let script = "<?xml version='1.0' encoding='UTF-8'?>\n<LightBurnProject AppVersion=\"2.1.04\">\n    \
        <CutSetting type=\"Cut\">\n        <index Value=\"0\" />\n        <name Value=\"C00\" />\n        \
        <maxPower Value=\"15.8750\" />\n    </CutSetting>\n</LightBurnProject>";
    let p = parse(script).unwrap();
    assert!(p.format.space_before_slash && !p.format.final_newline);
    assert_eq!(write(&p), script, "declaration, empty-tag style and number spelling are kept");
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
    assert_eq!(plain_extras(fill), [("lbrn2.index", "0"), ("lbrn2.minPower", "12"), ("lbrn2.tabCount", "1")]);
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
    assert_eq!(photo.extras.get("lbrn2.ditherMode"), None, "mapped, not kept");
    assert_eq!(photo.settings.image.dither, doc::DitherMode::FloydSteinberg);
    assert_eq!(photo.settings.image.interval_mm, 0.05);
    let tool = &layers[4];
    assert!(!tool.output);
    assert_eq!(tool.color, "#f36926");
}

#[test]
fn imports_shapes_in_the_field_frame() {
    let imported = to_document(&parse(SHAPES).unwrap(), FIELD).unwrap();
    let groups = imported.groups.clone();
    let (layers, objects, warnings) = tuple(imported);
    let names: Vec<&str> = objects.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        ["Rectangle 1", "Ellipse 1", "Path 1", "Path 2", "Path 3", "Rectangle 2", "Path 4", "Hi", "dot.png", "Rectangle 3"]
    );
    assert_eq!(objects[0].id, "obj-6", "ids continue after the five layers");
    // LightBurn's nested groups are kept.
    let group_names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(group_names, ["Group 1", "Group 2"]);
    assert_eq!(groups[0].parent, None);
    assert_eq!(groups[1].parent.as_deref(), Some(groups[0].id.as_str()));
    assert_eq!(obj(&objects, "Rectangle 2").group.as_deref(), Some(groups[1].id.as_str()));
    assert_eq!(obj(&objects, "Path 4").group.as_deref(), Some(groups[1].id.as_str()));
    assert_eq!(obj(&objects, "Path 3").group, None);
    assert_eq!(groups[0].extras.get("lbrn2._xform").map(String::as_str), Some("2 0 0 2 100 100"));
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
    let g = obj(&objects, "Rectangle 2");
    assert!(rect_close(g.bounds(), Rect::new(33.0, 23.0, 37.0, 27.0), 1e-9), "{:?}", g.bounds());
    assert_eq!(g.layer, layers[2].id);
    let gp = obj(&objects, "Path 4");
    assert!(rect_close(gp.bounds(), Rect::new(35.0, 35.0, 37.0, 35.0), 1e-9), "{:?}", gp.bounds());
    // The text is drawn as its outline and keeps what LightBurn needs to show it as text.
    let t = obj(&objects, "Hi");
    assert!(matches!(t.geometry, Geometry::Path { .. }));
    assert!(rect_close(t.bounds(), Rect::new(-1.0, -47.0, 1.0, -43.0), 1e-9));
    let tx = |k: &str| t.extras.get(k).map(String::as_str);
    assert_eq!(tx("lbrn2._type"), Some("Text"));
    assert_eq!(tx("lbrn2.Str"), Some("Hi\nthere"));
    assert_eq!(tx("lbrn2.Font"), Some("Arial,-1,100,5,50,0,0,0,0,0"));
    assert_eq!(tx("lbrn2._attrs"), Some("CutIndex Font Str H LS LnS Ah Av Weld HasBackupPath"));
    // The text without an outline is kept, after the object before it.
    assert!(tx("lbrn2._after.0").is_some_and(|x| x.contains("Str=\"No outline\"")), "{:?}", t.extras);
    // The bitmap is an image, with its LightBurn attributes and exact data.
    let image = obj(&objects, "dot.png");
    assert!(matches!(&image.geometry, Geometry::Image { width_mm, height_mm, mime, data_base64, adjust }
        if *width_mm == 10.0 && *height_mm == 10.0 && mime == "image/png" && data_base64.starts_with("iVBOR")
            && data_base64.ends_with("Jggg==") && adjust.is_default()));
    assert!(rect_close(image.bounds(), Rect::new(-40.0, -40.0, -30.0, -30.0), 1e-9));
    assert_eq!(image.layer, layers[3].id);
    assert_eq!(image.extras.get("lbrn2.Gamma").map(String::as_str), Some("1"));
    assert!(image.extras.get("lbrn2._after.0").is_some_and(|x| x.contains("Type=\"Star\"")));
    assert_eq!(obj(&objects, "Rectangle 3").layer, layers[4].id, "tool shape on the tool layer");

    let has = |s: &str| warnings.iter().any(|w| w.contains(s));
    assert!(has("1 text object(s) are shown and burned as LightBurn's saved outline"), "{warnings:?}");
    assert!(has("text \"No outline\" has no saved outline"), "{warnings:?}");
    assert!(has("'Star' shape was kept for LightBurn export"), "{warnings:?}");
    assert!(!has("image"), "images engrave now: {warnings:?}");
    assert!(!has("outside"), "{warnings:?}");
}

#[test]
fn an_unchanged_import_exports_as_it_was_read() {
    let d = import_document(SHAPES);
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    // The export is the source as parsed (only the comments, which the model does not keep, are gone).
    assert_eq!(write(&p), write(&parse(SHAPES).unwrap()));
    // Also through a saved .openetch file.
    let loaded = doc::file::from_openetch_json(&doc::file::try_to_openetch_json(&d).unwrap()).unwrap();
    assert_eq!(write(&from_document(&loaded, FIELD).unwrap().0), write(&p));
}

#[test]
fn edits_export_with_everything_else_kept() {
    let mut d = import_document(SHAPES);
    let id = |d: &Document, name: &str| d.objects.iter().position(|o| o.name == name).unwrap();
    // Move the text and a grouped rectangle, change a layer, add a layer and an image.
    let t = id(&d, "Hi");
    d.objects[t].transform = Affine::translate((10.0, 0.0)) * d.objects[t].transform;
    let r = id(&d, "Rectangle 2");
    d.objects[r].transform = Affine::translate((0.0, 1.0)) * d.objects[r].transform;
    d.layers[0].settings.power_pct = 31.0;
    let extra = layer(&mut d, "New", "#00a000", |_| {});
    let image = Geometry::Image {
        width_mm: 4.0,
        height_mm: 2.0,
        mime: "image/png".into(),
        data_base64: "iVBORw0KGgo=".into(),
        adjust: Default::default(),
    };
    object(&mut d, &extra, image, Affine::translate((5.0, 5.0)));
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let out = write(&p);

    // The text is still LightBurn text, moved: its XForm and its outline's are the new placement.
    assert!(out.contains("<Shape Type=\"Text\" CutIndex=\"0\" Font=\"Arial,-1,100,5,50,0,0,0,0,0\" Str=\"Hi&#10;there\""));
    assert_eq!(out.matches("<XForm>1 0 0 1 85 30</XForm>").count(), 2, "{out}");
    // The grouped rectangle moved inside its groups: 1 mm in the field is 0.5 in the outer group's scale 2.
    assert!(out.contains("<Shape Type=\"Rect\" CutIndex=\"3\" W=\"2\" H=\"2\" Cr=\"0\">\n                        <XForm>1 0 0 1 0 0.5</XForm>"), "{out}");
    // Only the changed value differs in the layer; the rest keeps the source order and spelling.
    assert!(out.contains("<name Value=\"Fill\"/>\n        <minPower Value=\"12\"/>\n        <maxPower Value=\"31\"/>"));
    // The new layer comes after the kept settings, the new image is a bitmap on it.
    let new_setting = out.find("<name Value=\"New\"/>").unwrap();
    assert!(out.find("<CutSetting type=\"Tool\">").unwrap() < new_setting && new_setting < out.find("<Shape").unwrap());
    assert!(out.contains("<Shape Type=\"Bitmap\" CutIndex=\"11\" W=\"4\" H=\"2\" Gamma=\"1\""), "{out}");
    assert!(out.contains("Data=\"iVBORw0KGgo=\">\n        <XForm>1 0 0 -1 80 80</XForm>"), "a top-down image is stored bottom-up: {out}");
    // Everything else is untouched: thumbnail, notes, shared lists, the kept Star and text.
    for kept in ["<Thumbnail Source=\"iVBORw0KGgo=\"/>", "Notes=\"hand-written fixture\"", "Type=\"Star\"", "Str=\"No outline\""] {
        assert!(out.contains(kept), "{kept}");
    }
    assert_eq!(out.matches("VertID=\"1\" PrimID=\"2\"").count(), 2);
    // And it reads back as the edited document.
    let back = import_document(&out);
    assert_eq!(back.objects.len(), d.objects.len());
    assert_eq!(back.groups.len(), d.groups.len());
    for (a, b) in d.objects.iter().zip(&back.objects) {
        assert!(rect_close(a.bounds(), b.bounds(), 1e-9), "{}", a.name);
    }
}

#[test]
fn groups_made_in_openetch_export_as_lightburn_groups() {
    let mut d = sample_document();
    let gid = d.alloc_id("group").unwrap();
    d.groups.push(doc::Group::new(gid.clone(), "Pair", None));
    d.objects[0].group = Some(gid.clone());
    d.objects[2].group = Some(gid);
    let (p, _) = from_document(&d, FIELD).unwrap();
    assert_eq!(p.shapes.len(), 3, "the group takes the place of its first member");
    match &p.shapes[0].kind {
        ShapeKind::Group(children) => {
            assert_eq!(children.len(), 2);
            assert!(matches!(children[0].kind, ShapeKind::Rect { .. }));
            assert!(matches!(children[1].kind, ShapeKind::Path(_)));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(p.shapes[0].xform, Affine::IDENTITY);
    let back = import_document(&write(&p));
    assert_eq!(back.groups.len(), 1);
    assert_eq!(back.objects.iter().filter(|o| o.group.is_some()).count(), 2);
    // The group's members are drawn together, where the first one was.
    for (k, b) in [0, 2, 1, 3].into_iter().zip(&back.objects) {
        assert_same_geometry(&d.objects[k], b, 1e-6);
    }
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
    assert_eq!(s1.name.as_deref(), Some("clean"), "a sub-layer's subname is its name");
    assert_eq!(ex("lbrn2.sub1.subname"), None);
    assert_eq!((s1.cleanup.as_ref(), deep.sub_layers[1].name.as_ref()), (None, None));
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
    let id = d.alloc_id("layer").unwrap();
    let mut l = Layer::new(id.clone(), name, color);
    f(&mut l);
    d.layers.push(l);
    id
}

fn object(d: &mut Document, layer: &str, geometry: Geometry, transform: Affine) {
    let id = d.alloc_id("obj").unwrap();
    d.objects.push(Object::new(id.clone(), id, layer.into(), geometry, transform));
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
    assert_eq!(write(&reparsed), text);
    let (layers, objects, warnings) = tuple(to_document(&reparsed, FIELD).unwrap());
    assert!(warnings.is_empty(), "{warnings:?}");

    assert_eq!(layers.len(), d.layers.len());
    for (a, b) in d.layers.iter().zip(&layers) {
        assert_eq!(a.name, b.name);
        assert_eq!((a.visible, a.output), (b.visible, b.output), "{}", a.name);
        assert_eq!(a.settings, b.settings, "{}", a.name);
        assert_eq!(a.sub_layers, b.sub_layers, "{}", a.name);
        let mut extras = b.extras.clone();
        assert!(extras.remove("lbrn2.index").is_some());
        extras.retain(|k, _| !k.starts_with("lbrn2._") && !k.contains("._src"));
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
    let d = import_document(SHAPES);
    let (layers, objects) = (d.layers.clone(), d.objects.clone());
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
    let (layers2, objects2, _) = tuple(to_document(&parse(&write(&p)).unwrap(), FIELD).unwrap());
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
    let (layers, objects, _) = tuple(to_document(&p, FIELD).unwrap());
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
    let (layers, objects, _) = tuple(to_document(&parse("<LightBurnProject/>").unwrap(), FIELD).unwrap());
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

/// A project whose first path defines a `PrimList` of `prims` segments and `reuses` more paths that only refer to it.
fn shared_list_project(prims: usize, reuses: usize) -> String {
    let mut s = String::from("<LightBurnProject>\n<Shape Type=\"Path\" CutIndex=\"0\" VertID=\"1\" PrimID=\"1\">");
    s.push_str("<XForm>1 0 0 1 75 75</XForm><VertList>V0 0c0x1c1x1V1 0c0x1c1x1</VertList><PrimList>");
    for _ in 0..prims {
        s.push_str("L0 1");
    }
    s.push_str("</PrimList></Shape>\n");
    for _ in 0..reuses {
        s.push_str("<Shape Type=\"Path\" CutIndex=\"0\" VertID=\"1\" PrimID=\"1\">");
        s.push_str("<XForm>1 0 0 1 75 75</XForm></Shape>\n");
    }
    s.push_str("</LightBurnProject>");
    s
}

#[test]
fn reusing_a_shared_prim_list_counts_every_use() {
    // 20k segments used 120 times is 2.4 M segments from a file of about 90 kB: refused before it is expanded.
    let text = shared_list_project(20_000, 119);
    assert!(text.len() < 100_000);
    let e = parse(&text).unwrap_err();
    assert!(matches!(&e, LbrnError::TooLarge(m) if m.contains("path segments")), "{e}");
    // A few uses are fine, and each one is its own object.
    let (_, objects, _) = import(&shared_list_project(1_000, 3));
    assert_eq!(objects.len(), 4);
    // LineOpen counts its vertices too.
    let mut verts = String::new();
    for i in 0..100_000 {
        verts.push_str(&format!("V{i} 0"));
    }
    let mut big = format!(
        "<LightBurnProject><Shape Type=\"Path\" VertID=\"1\" PrimID=\"1\"><VertList>{verts}</VertList>\
         <PrimList>LineOpen</PrimList></Shape>"
    );
    for _ in 0..25 {
        big.push_str("<Shape Type=\"Path\" VertID=\"1\" PrimID=\"1\"/>");
    }
    big.push_str("</LightBurnProject>");
    assert!(matches!(parse(&big), Err(LbrnError::TooLarge(_))));
}

#[test]
fn many_distinct_warnings_are_capped_and_fast() {
    let mut s = String::from("<LightBurnProject>");
    for i in 0..20_000 {
        s.push_str(&format!("<Shape Type=\"Text\" CutIndex=\"0\" Str=\"t{i}\"><XForm>1 0 0 1 75 75</XForm></Shape>"));
    }
    s.push_str("</LightBurnProject>");
    let start = std::time::Instant::now();
    let (_, objects, warnings) = import(&s);
    assert!(start.elapsed().as_secs() < 10, "took {:?}", start.elapsed());
    assert!(objects.is_empty());
    assert_eq!(warnings.len(), 201, "200 distinct warnings and a summary line");
    assert!(warnings[0].contains("\"t0\" has no saved outline"), "{}", warnings[0]);
    assert!(warnings[200].contains("19800 more warning"), "{}", warnings[200]);
}

#[test]
fn hostile_extras_and_names_still_export_valid_xml() {
    let mut d = sample_document();
    let evil = "lbrn2.evil a=\"1\"/><Shape Type=\"Rect\" CutIndex=\"0\" W=\"5\" H=\"5\"/><x";
    d.layers[0].extras.insert(evil.into(), "1".into());
    d.layers[0].extras.insert("lbrn2.".into(), "2".into());
    d.layers[0].extras.insert("lbrn2.sub1.bad tag".into(), "3".into());
    d.layers[0].sub_layers.push(CutSettings::default());
    d.layers[0].extras.insert("lbrn2.ok".into(), "a\u{1}b\u{FFFE}<&>".into());
    d.layers[1].name = "De\u{1}ep\u{0}".into();
    d.objects[0].name = "obj\u{7}".into();
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    let skipped = warnings.iter().filter(|w| w.contains("is not a LightBurn tag name")).count();
    assert_eq!(skipped, 3, "{warnings:?}");
    let text = write(&p);
    assert!(!text.contains("evil") && !text.contains('\u{1}') && !text.contains('\u{0}'), "{text}");
    assert!(!text.contains("< Value"), "an empty tag name is never written");
    let back = parse(&text).expect("valid XML");
    assert_eq!(back.shapes.len(), p.shapes.len(), "no injected shape");
    assert_eq!(back.shapes.len(), d.objects.len());
    assert_eq!(back.cut_settings[1].params.name.as_deref(), Some("Deep"));
    assert_eq!(back.cut_settings[0].params.other.get("ok").map(String::as_str), Some("ab<&>"));
    let (layers, _, _) = tuple(to_document(&back, FIELD).unwrap());
    assert_eq!(layers[1].name, "Deep");
    assert!(layers.iter().all(|l| l.extras.keys().all(|k| doc::is_extras_key(k))), "{layers:?}");
}

#[test]
fn imported_names_and_tags_load_back_from_openetch() {
    // Tags and names LightBurn files can hold but an .openetch file may not are cleaned on import, so a saved import
    // always loads again.
    let text = "<LightBurnProject><CutSetting type=\"Cut\"><index Value=\"0\"/><name Value=\"a&#9;b&#10;c\"/>\
        <gr\u{f6}\u{df}e Value=\"1\"/><frequency Value=\"20000\"/></CutSetting>\
        <Shape Type=\"Text\" CutIndex=\"0\" Str=\"x&#9;y\"><BackupPath Type=\"Path\"><VertList>V0 0V1 1</VertList>\
        <PrimList>L0 1</PrimList></BackupPath><XForm>1 0 0 1 75 75</XForm></Shape></LightBurnProject>";
    let (layers, objects, warnings) = import(text);
    assert_eq!(layers[0].name, "a b c");
    assert_eq!(objects[0].name, "x y");
    assert!(warnings.iter().any(|w| w.contains("gr\u{f6}\u{df}e") && w.contains("dropped")), "{warnings:?}");
    let d = Document { layers, objects, ..Document::default() };
    doc::file::from_openetch_json(&doc::file::try_to_openetch_json(&d).unwrap()).unwrap();
}

#[test]
fn overflowing_or_singular_placements_are_skipped() {
    let text = "<LightBurnProject>\
        <Shape Type=\"Group\"><XForm>1e200 0 0 1e200 0 0</XForm><Children>\
          <Shape Type=\"Group\"><XForm>1e200 0 0 1e200 0 0</XForm><Children>\
            <Shape Type=\"Rect\" CutIndex=\"0\" W=\"1\" H=\"1\"><XForm>1 0 0 1 0 0</XForm></Shape>\
          </Children></Shape></Children></Shape>\
        <Shape Type=\"Rect\" CutIndex=\"0\" W=\"1e308\" H=\"1\"><XForm>10 0 0 1 75 75</XForm></Shape>\
        <Shape Type=\"Ellipse\" CutIndex=\"0\" Rx=\"1\" Ry=\"1\"><XForm>1 0 1 0 75 75</XForm></Shape>\
        <Shape Type=\"Rect\" CutIndex=\"0\" W=\"2\" H=\"2\"><XForm>1 0 0 1 75 75</XForm></Shape>\
        </LightBurnProject>";
    let imported = to_document(&parse(text).unwrap(), FIELD).unwrap();
    assert!(imported.groups.is_empty(), "groups left without members are dropped");
    let (_, objects, warnings) = tuple(imported);
    assert_eq!(objects.len(), 1, "only the sound rectangle is kept");
    assert!(objects[0].bounds().x0.is_finite());
    let skipped: Vec<_> = warnings.iter().filter(|w| w.contains("kept for LightBurn export but not imported")).collect();
    assert!(!skipped.is_empty() && skipped.iter().any(|w| w.contains("cannot be inverted")), "{warnings:?}");
    let d = Document { objects, ..Document::default() };
    assert_eq!(d.numeric_problem(), None);
}

#[test]
fn subname_and_library_settings_round_trip() {
    let clb = r#"<LightBurnLibrary><Material name="Steel"><Entry Thickness="-1" NoThickTitle="Mark">
      <CutSetting type="Scan">
        <index Value="0"/><name Value=""/><maxPower Value="37"/><speed Value="158"/><frequency Value="40000"/>
        <subname Value="dark"/>
        <SubLayer type="Scan" index="1">
          <maxPower Value="50"/><speed Value="3000"/><frequency Value="80000"/><subname Value="clean"/>
        </SubLayer>
        <SubLayer type="Cut" index="2"><maxPower Value="10"/><speed Value="500"/><subname Value="edge"/></SubLayer>
      </CutSetting></Entry></Material></LightBurnLibrary>"#;
    let entry = &lbrn::parse_library(clb).unwrap()[0];
    let (l, _) = lbrn::cut_setting_to_layer(&entry.setting, "layer-1");
    assert_eq!(l.extras.get("lbrn2.subname").map(String::as_str), Some("dark"));
    let d = Document { layers: vec![l.clone()], ..Document::default() };
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let cs = &p.cut_settings[0];
    assert_eq!(cs.params.subname.as_deref(), Some("dark"), "the layer's own subname is exported");
    assert_eq!(cs.sub_layers[0].params.subname.as_deref(), Some("clean"));
    assert_eq!(cs.sub_layers[1].params.subname.as_deref(), Some("edge"));
    assert_eq!(cs.sub_layers[1].params.frequency, None, "the sub-layer had no frequency");
    let back = parse_library_entry_of(&write(&p));
    let (back_layer, _) = lbrn::cut_setting_to_layer(&back, "layer-1");
    assert_eq!(back_layer.extras, l.extras);
    assert_eq!((back_layer.settings, back_layer.sub_layers), (l.settings, l.sub_layers));
}

/// The first cut setting of a written project.
fn parse_library_entry_of(project: &str) -> lbrn::CutSetting {
    parse(project).unwrap().cut_settings.remove(0)
}

#[test]
fn a_missing_frequency_stays_missing_until_changed() {
    let text = "<LightBurnProject><CutSetting type=\"Cut\"><index Value=\"0\"/><maxPower Value=\"20\"/>\
        <speed Value=\"100\"/></CutSetting>\
        <Shape Type=\"Rect\" CutIndex=\"0\" W=\"2\" H=\"2\"><XForm>1 0 0 1 75 75</XForm></Shape></LightBurnProject>";
    let (layers, objects, warnings) = import(text);
    assert!(!warnings.iter().any(|w| w.contains("frequency")), "LightBurn's default is no cause for a warning: {warnings:?}");
    assert_eq!(layers[0].extras.get(lbrn::FREQUENCY_ABSENT_KEY).map(String::as_str), Some("1"));
    let mut d = Document { layers, objects, ..Document::default() };
    let out = write(&from_document(&d, FIELD).unwrap().0);
    assert!(!out.contains("<frequency") && !out.contains("frequency_absent"), "{out}");
    // Once the frequency is set (layer.update removes the marker), it is written.
    d.layers[0].extras.remove(lbrn::FREQUENCY_ABSENT_KEY);
    d.layers[0].settings.frequency_khz = 45.0;
    let out = write(&from_document(&d, FIELD).unwrap().0);
    assert!(out.contains("<frequency Value=\"45000\"/>"), "{out}");
}

#[test]
fn shapes_on_layer_indices_lightburn_lacks_are_reported() {
    let text = "<LightBurnProject><Shape Type=\"Rect\" CutIndex=\"40\" W=\"2\" H=\"2\"><XForm>1 0 0 1 75 75</XForm>\
        </Shape></LightBurnProject>";
    let (layers, objects, warnings) = import(text);
    assert_eq!(objects.len(), 1);
    assert!(!layers[0].output);
    assert!(warnings.iter().any(|w| w.contains("layer index 40") && w.contains("non-output")), "{warnings:?}");
    // T1 and T2 are real LightBurn layers: no such warning.
    let t2 = text.replace("CutIndex=\"40\"", "CutIndex=\"31\"");
    assert!(!import(&t2).2.iter().any(|w| w.contains("not a LightBurn layer")));
}

#[test]
fn a_second_image_setting_stays_with_its_layer() {
    let text = "<LightBurnProject>\
        <CutSetting type=\"Scan\"><index Value=\"0\"/><name Value=\"C00\"/><maxPower Value=\"20\"/></CutSetting>\
        <CutSetting_Img type=\"Image\"><index Value=\"0\"/><name Value=\"C00\"/><ditherMode Value=\"stucki\"/></CutSetting_Img>\
        <Shape Type=\"Rect\" CutIndex=\"0\" W=\"2\" H=\"2\"><XForm>1 0 0 1 75 75</XForm></Shape></LightBurnProject>";
    let (layers, objects, warnings) = import(text);
    assert_eq!(layers.len(), 1);
    assert!(!warnings.iter().any(|w| w.contains("ignored")), "{warnings:?}");
    assert!(layers[0].extras.get("lbrn2._img").is_some_and(|x| x.contains("stucki")));
    // Even without the file's own layout (e.g. merged into another document), the image setting follows the layer.
    let d = Document { layers, objects, ..Document::default() };
    let out = write(&from_document(&d, FIELD).unwrap().0);
    let (cs, img, shape) = (out.find("<CutSetting ").unwrap(), out.find("<CutSetting_Img").unwrap(), out.find("<Shape").unwrap());
    assert!(cs < img && img < shape, "{out}");
    let p = parse(&out).unwrap();
    assert_eq!(p.cut_settings.iter().filter(|c| c.image).count(), 1);
}

/// A layer with both vector and image settings (as `.lbt` templates have): the image tags are read into the layer's
/// image settings, the kept `CutSetting_Img` is written back as it was until they change, then with them.
#[test]
fn a_kept_image_setting_follows_image_edits() {
    let text = "<LightBurnProject>        <CutSetting type=\"Scan\"><index Value=\"0\"/><name Value=\"C00\"/><maxPower Value=\"20\"/></CutSetting>        <CutSetting_Img type=\"Image\"><index Value=\"0\"/><name Value=\"C00\"/><maxPower Value=\"55\"/>        <interval Value=\"0.08\"/><ditherMode Value=\"stucki\"/></CutSetting_Img>        <Shape Type=\"Rect\" CutIndex=\"0\" W=\"2\" H=\"2\"><XForm>1 0 0 1 75 75</XForm></Shape></LightBurnProject>";
    let (layers, objects, warnings) = import(text);
    let img = &layers[0].settings.image;
    assert_eq!((img.dither, img.interval_mm), (doc::DitherMode::Stucki, 0.08));
    assert_eq!(layers[0].settings.hatch.interval_mm, 0.05, "the vector settings keep their own");
    assert!(warnings.iter().any(|w| w.contains("own power")), "{warnings:?}");
    let mut d = Document { layers, objects, ..Document::default() };
    let out = write(&from_document(&d, FIELD).unwrap().0);
    assert!(out.contains("<ditherMode Value=\"stucki\"/>") && !out.contains("dpi"), "{out}");
    d.layers[0].settings.image.dither = doc::DitherMode::Threshold;
    d.layers[0].settings.image.negative = true;
    let out = write(&from_document(&d, FIELD).unwrap().0);
    let p = parse(&out).unwrap();
    let img = p.cut_settings.iter().find(|c| c.image).unwrap();
    assert_eq!(img.params.other.get("ditherMode").map(String::as_str), Some("threshold"));
    assert_eq!(img.params.other.get("negative").map(String::as_str), Some("1"));
    assert_eq!(img.params.max_power, Some(55.0), "everything else as kept");
    assert_eq!(img.params.interval, Some(0.08));
}

/// Image layers and Adjust Image values round-trip through the model; layers made in OpenEtch that hold images get
/// a `CutSetting_Img` with their image settings.
#[test]
fn image_settings_and_adjust_round_trip() {
    let text = "<LightBurnProject>        <CutSetting_Img type=\"Image\"><index Value=\"0\"/><name Value=\"C00\"/><minPower Value=\"10\"/>        <maxPower Value=\"60\"/><bidir Value=\"0\"/><dotWidth Value=\"0.025\"/><useDotCorrection Value=\"1\"/>        <interval Value=\"0.0651\"/><angle Value=\"180\"/><halftoneAngle Value=\"22\"/><negative Value=\"1\"/>        <ditherMode Value=\"grayscale\"/><dpi Value=\"390\"/></CutSetting_Img>        <Shape Type=\"Bitmap\" CutIndex=\"0\" W=\"4\" H=\"2\" Gamma=\"1.5\" Contrast=\"0.2\" Brightness=\"-0.11\"         EnhanceAmount=\"0\" EnhanceRadius=\"0\" EnhanceDenoise=\"0\" File=\"a.png\" SourceHash=\"0\" Data=\"iVBORw0KGgo=\">        <XForm>1 0 0 1 75 75</XForm></Shape></LightBurnProject>";
    let mut d = import_document(text);
    let s = &d.layers[0].settings;
    assert_eq!((s.image.dither, s.image.min_power_pct, s.image.dot_width_mm), (doc::DitherMode::Grayscale, 10.0, 0.025));
    assert_eq!((s.image.angle_deg, s.image.bidirectional, s.image.negative), (180.0, false, true));
    let Geometry::Image { adjust, .. } = &d.objects[0].geometry else { panic!() };
    assert_eq!((adjust.gamma, adjust.contrast, adjust.brightness), (1.5, 20.0, -11.0));
    // LightBurn's rows are bottom-up: the object flips them, so the image file's top row is at the top.
    assert_eq!(d.objects[0].transform, Affine::translate((0.0, 0.0)) * Affine::FLIP_Y);
    assert_eq!(write(&from_document(&d, FIELD).unwrap().0), write(&parse(text).unwrap()), "unchanged comes back as read");
    // Edits come back as LightBurn values.
    d.layers[0].settings.image.interval_mm = 0.1;
    let Geometry::Image { adjust, .. } = &mut d.objects[0].geometry else { panic!() };
    adjust.contrast = 35.0;
    let out = write(&from_document(&d, FIELD).unwrap().0);
    assert!(out.contains("<interval Value=\"0.1\"/>") && out.contains("<dpi Value=\"254\"/>"), "{out}");
    assert!(out.contains("Contrast=\"0.35\" Brightness=\"-0.11\""), "{out}");
    // A layer made in OpenEtch holding an image gets image settings LightBurn reads back the same.
    let mut fresh = Document::default();
    fresh.layers[0].settings.image.dither = doc::DitherMode::Atkinson;
    fresh.layers[0].settings.image.interval_mm = 0.08;
    fresh.objects.push(d.objects[0].clone());
    fresh.objects[0].layer = fresh.layers[0].id.clone();
    let (p, warnings) = from_document(&fresh, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let back = import_document(&write(&p));
    assert_eq!(back.layers[0].settings.image.dither, doc::DitherMode::Atkinson);
    assert_eq!(back.layers[0].settings.image.interval_mm, 0.08);
}

/// A 3D-sliced cleanup as LightBurn writes it (synthetic, after the shape of a real sample): the parent's
/// `cleanupPass` enables it, the sub-layer's `isCleanup` marks it and its `numPasses` is "Clean after # of passes".
#[test]
fn cleanup_sub_layers_map_to_cleanup_and_round_trip() {
    let text = r#"<LightBurnProject>
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="C01"/>
        <maxPower Value="90"/>
        <speed Value="3000"/>
        <numPasses Value="25"/>
        <subname Value="Slice"/>
        <cleanupPass Value="1"/>
        <SubLayer type="Scan" index="1">
            <maxPower Value="30"/>
            <speed Value="2500"/>
            <numPasses Value="10"/>
            <isCleanup Value="1"/>
            <subname Value="Cleanup"/>
        </SubLayer>
    </CutSetting>
    <Shape Type="Rect" CutIndex="1" W="2" H="2"><XForm>1 0 0 1 75 75</XForm></Shape>
</LightBurnProject>
"#;
    let (layers, objects, _) = import(text);
    let l = &layers[0];
    let c = &l.sub_layers[0];
    assert_eq!(c.name.as_deref(), Some("Cleanup"));
    assert_eq!(c.cleanup, Some(doc::Cleanup { every_n_passes: 10 }));
    assert_eq!(c.passes, 1, "each cleanup is one pass");
    let moved = ["lbrn2.sub1.subname", "lbrn2.sub1.isCleanup", "lbrn2.sub1.numPasses"];
    assert!(moved.iter().all(|k| !l.extras.contains_key(*k)), "{:?}", l.extras);
    let d = Document { layers: layers.clone(), objects, ..Document::default() };
    let (p, warnings) = from_document(&d, FIELD).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let out = write(&p);
    for tag in ["<numPasses Value=\"10\"/>", "<isCleanup Value=\"1\"/>", "<subname Value=\"Cleanup\"/>", "<cleanupPass Value=\"1\"/>"] {
        assert!(out.contains(tag), "{tag} in {out}");
    }

    // Edited: a new interval is written; without a cleanup sub-layer the parent's flag goes too.
    let mut d2 = d.clone();
    d2.layers[0].sub_layers[0].cleanup = Some(doc::Cleanup { every_n_passes: 4 });
    d2.layers[0].sub_layers[0].name = Some("Clean".into());
    let out = write(&from_document(&d2, FIELD).unwrap().0);
    assert!(out.contains("<numPasses Value=\"4\"/>") && out.contains("<subname Value=\"Clean\"/>"), "{out}");
    d2.layers[0].sub_layers[0].cleanup = None;
    let out = write(&from_document(&d2, FIELD).unwrap().0);
    assert!(!out.contains("isCleanup") && !out.contains("cleanupPass"), "{out}");

    // Made in OpenEtch: tags in LightBurn's order, `numPasses` left out at 1.
    let mut fresh = Document::default();
    let mut s = CutSettings { name: Some("Cleanup".into()), cleanup: Some(doc::Cleanup { every_n_passes: 1 }), ..CutSettings::default() };
    s.hatch.angle_deg = 45.0;
    fresh.layers[0].sub_layers.push(s);
    let out = write(&from_document(&fresh, FIELD).unwrap().0);
    assert!(out.contains("<cleanupPass Value=\"1\"/>"), "{out}");
    let sub = &out[out.find("<SubLayer").unwrap()..out.find("</SubLayer>").unwrap()];
    assert!(!sub.contains("numPasses"), "{sub}");
    let at = |t: &str| sub.find(t).unwrap_or_else(|| panic!("{t} in {sub}"));
    assert!(at("isCleanup") < at("angle") && at("angle") < at("subname"), "{sub}");
    let (back, _, _) = import(&out);
    assert_eq!(back[0].sub_layers, fresh.layers[0].sub_layers);
}
