//! End-to-end tests of the design and construction commands through [`Session`] (`design`, `construct`, and the
//! design options of `shape.polygon`, `path.optimize`, `path.fillet`, `object.transform`, `art.nest`).

use doc::{Document, FillMode, Geometry};
use kurbo::{PathSeg, Point, Shape};
use serde_json::{Value, json};

use crate::{CommandError, Invocation, Origin, Session};

fn session() -> Session {
    Session::new(Document { field_mm: Some(150.0), ..Document::default() })
}

fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, CommandError> {
    s.execute(Invocation { id: id.into(), params }, Origin::Agent("test".into())).map(|o| o.result)
}

fn ok(s: &mut Session, id: &str, params: Value) -> Value {
    run(s, id, params).unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn err(s: &mut Session, id: &str, params: Value) -> String {
    match run(s, id, params.clone()) {
        Ok(v) => panic!("{id} {params} should fail, got {v}"),
        Err(e) => e.to_string(),
    }
}

fn created(v: &Value) -> String {
    v["created"][0].as_str().unwrap().to_string()
}

fn path(s: &mut Session, d: &str) -> String {
    created(&ok(s, "shape.path", json!({"svg_path_data": d})))
}

fn ids(v: &Value) -> Vec<String> {
    v["ids"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect()
}

#[test]
fn parametric_polygon_and_shape_properties() {
    let mut s = session();
    // Default: a plain path, as before.
    let plain = created(&ok(&mut s, "shape.polygon", json!({"sides": 6, "radius_mm": 10})));
    assert!(matches!(s.document().object(&plain).unwrap().geometry, Geometry::Path { .. }));
    let p = created(&ok(
        &mut s,
        "shape.polygon",
        json!({"sides": 6, "radius_mm": 10, "center": [5, 5], "parametric": true}),
    ));
    let g = &s.document().object(&p).unwrap().geometry;
    assert_eq!(*g, Geometry::Polygon { sides: 6, radius_mm: 10.0, star_ratio: None, corner_radius_mm: 0.0 });
    let before = s.document().object(&p).unwrap().field_path().area();
    // Edit sides and make it a star; the centre stays.
    let r = ok(&mut s, "shape.update", json!({"id": p, "sides": 5, "star_ratio": 0.5}));
    assert_eq!(r["geometry"]["kind"], json!("polygon"));
    assert_eq!(r["geometry"]["sides"], json!(5));
    let o = s.document().object(&p).unwrap();
    assert!(o.field_path().area() < before);
    assert_eq!(o.transform * Point::ORIGIN, Point::new(5.0, 5.0));
    // Back to a plain polygon with rounded corners.
    ok(&mut s, "shape.update", json!({"id": p, "star_ratio": null, "corner_radius_mm": 1}));
    assert_eq!(
        s.document().object(&p).unwrap().geometry,
        Geometry::Polygon { sides: 5, radius_mm: 10.0, star_ratio: None, corner_radius_mm: 1.0 }
    );
    assert!(err(&mut s, "shape.update", json!({"id": p, "sides": 2})).contains("sides"));
    assert!(err(&mut s, "shape.update", json!({"id": p, "width_mm": 2})).contains("does not apply"));
    // Rectangles and ellipses.
    let r = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 4, "corner_radius_mm": 2})));
    ok(&mut s, "shape.update", json!({"id": r, "width_mm": 20}));
    assert_eq!(
        s.document().object(&r).unwrap().geometry,
        Geometry::Rect { width_mm: 20.0, height_mm: 4.0, corner_radius_mm: 2.0 }
    );
    // A kept corner radius shrinks with the rectangle; an explicit one must fit.
    ok(&mut s, "shape.update", json!({"id": r, "height_mm": 2}));
    assert_eq!(
        s.document().object(&r).unwrap().geometry,
        Geometry::Rect { width_mm: 20.0, height_mm: 2.0, corner_radius_mm: 1.0 }
    );
    assert!(err(&mut s, "shape.update", json!({"id": r, "corner_radius_mm": 5})).contains("half"));
    let e = created(&ok(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 3})));
    ok(&mut s, "shape.update", json!({"id": e, "ry_mm": 1.5}));
    assert_eq!(s.document().object(&e).unwrap().geometry, Geometry::Ellipse { rx_mm: 3.0, ry_mm: 1.5 });
    assert!(err(&mut s, "shape.update", json!({"id": e})).contains("nothing to change"));
    assert!(err(&mut s, "shape.update", json!({"id": plain, "sides": 4})).contains("only rectangles"));
    // A polygon converts to a path, exports to LightBurn as a path, and survives save/load.
    let saved = ok(&mut s, "doc.save", json!({}));
    assert!(saved.to_string().contains("polygon"));
    ok(&mut s, "path.convert", json!({"ids": [p]}));
    assert!(matches!(s.document().object(&p).unwrap().geometry, Geometry::Path { .. }));
}

#[test]
fn polygon_exports_to_lightburn_as_a_path() {
    let mut doc = Document { field_mm: Some(150.0), ..Document::default() };
    let layer = doc.layers[0].id.clone();
    let g = Geometry::Polygon { sides: 6, radius_mm: 10.0, star_ratio: None, corner_radius_mm: 0.0 };
    doc.objects.push(doc::Object::new("obj-9".into(), "hex", layer, g, kurbo::Affine::IDENTITY));
    let (project, _warnings) = lbrn::from_document(&doc, 150.0).unwrap();
    let text = lbrn::write(&project);
    assert!(text.contains("Type=\"Path\""), "{text}");
    assert!(!text.contains("Polygon"));
    let back = lbrn::to_document(&lbrn::parse(&text).unwrap(), 150.0).unwrap();
    assert_eq!(back.objects.len(), 1);
    let o = &back.objects[0];
    assert!(matches!(o.geometry, Geometry::Path { .. }));
    assert!((o.field_path().area() - 1.5 * 3f64.sqrt() * 100.0).abs() < 1e-3);
}

#[test]
fn selection_queries() {
    let mut s = session();
    let open = path(&mut s, "M 0 0 L 10 0 L 10 10");
    let closed = created(&ok(&mut s, "shape.rect", json!({"width_mm": 40, "height_mm": 40})));
    let inner = created(&ok(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 3, "center": [5, 5]})));
    let outside = created(&ok(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 3, "center": [60, 0]})));
    let rev = s.document().revision;
    assert_eq!(ids(&ok(&mut s, "select.open_shapes", json!({}))), vec![open.clone()]);
    assert_eq!(ids(&ok(&mut s, "select.open_shapes", json!({"ids": [closed]}))), Vec::<String>::new());
    // Open shapes on fill layers only.
    assert!(ids(&ok(&mut s, "select.open_on_fill", json!({}))).is_empty());
    ok(&mut s, "layer.add", json!({"name": "Fill"}));
    let fill_layer = s.document().layers[1].id.clone();
    ok(&mut s, "layer.update", json!({"id": fill_layer, "settings": {"mode": "fill"}}));
    assert_eq!(s.document().layers[1].settings.mode, FillMode::Fill);
    ok(&mut s, "object.set_layer", json!({"ids": [open], "layer": fill_layer}));
    assert_eq!(ids(&ok(&mut s, "select.open_on_fill", json!({}))), vec![open.clone()]);
    // Contained: the selection first, then what lies inside it (the open path lies inside the rectangle too).
    let rev2 = s.document().revision;
    let c = ids(&ok(&mut s, "select.contained", json!({"ids": [closed]})));
    assert_eq!(c[0], closed);
    assert!(c.contains(&inner) && c.contains(&open) && !c.contains(&outside), "{c:?}");
    assert_eq!(s.document().revision, rev2, "queries change nothing");
    assert!(rev2 > rev);
}

#[test]
fn page_positions_and_fixed_spacing() {
    let mut s = session();
    let a = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 4})));
    let b = created(&ok(&mut s, "shape.rect", json!({"width_mm": 6, "height_mm": 6, "center": [30, 0]})));
    let r = ok(&mut s, "object.move_to_page_position", json!({"ids": [a, b], "anchor": "top_left", "margin_mm": 5}));
    assert_eq!(r["bounds"]["min"][0], json!(-70.0));
    assert_eq!(r["bounds"]["max"][1], json!(70.0));
    let r = ok(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "bottom_right"}));
    assert_eq!(r["bounds"]["max"][0], json!(75.0));
    assert_eq!(r["bounds"]["min"][1], json!(-75.0));
    ok(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "center"}));
    assert_eq!(s.document().object(&a).unwrap().bounds().center(), Point::new(0.0, 0.0));
    assert!(
        err(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "center", "reference": "stock"}))
            .contains("stock")
    );
    assert!(
        err(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "center", "margin_mm": 100}))
            .contains("margin")
    );
    // Move together with a fixed gap (DES-006).
    ok(&mut s, "object.distribute", json!({"ids": [a, b], "axis": "x", "by": "spacing", "spacing_mm": 2}));
    let (ba, bb) = (s.document().object(&a).unwrap().bounds(), s.document().object(&b).unwrap().bounds());
    // b sits left of a (it went to the top left with it), so a follows b 2 mm on.
    assert!((ba.x0 - bb.x1 - 2.0).abs() < 1e-9, "{ba:?} {bb:?}");
}

#[test]
fn guides_are_saved_with_the_document() {
    let mut s = session();
    let g = ok(&mut s, "guide.add", json!({"axis": "x", "position_mm": 12.5}))["id"].as_str().unwrap().to_string();
    ok(&mut s, "guide.add", json!({"axis": "y", "position_mm": -3}));
    assert_eq!(s.document().guides.len(), 2);
    let r = ok(&mut s, "guide.move", json!({"id": g, "position_mm": 20, "axis": "y"}));
    assert_eq!(r["guides"][0]["position_mm"], json!(20.0));
    assert_eq!(r["guides"][0]["axis"], json!("y"));
    assert!(err(&mut s, "guide.move", json!({"id": "guide-404", "position_mm": 1})).contains("no guide"));
    assert!(err(&mut s, "guide.add", json!({"axis": "x", "position_mm": 1e9})).contains("within"));
    let saved = ok(&mut s, "doc.save", json!({}));
    assert!(saved.to_string().contains("guides"));
    // Undo brings a deleted guide back.
    ok(&mut s, "guide.delete", json!({"ids": [g]}));
    assert_eq!(s.document().guides.len(), 1);
    s.undo().unwrap();
    assert_eq!(s.document().guides.len(), 2);
    ok(&mut s, "guide.delete", json!({}));
    assert!(s.document().guides.is_empty());
    assert!(err(&mut s, "guide.delete", json!({})).contains("no guides"));
}

#[test]
fn curve_tolerance_setting() {
    let mut s = session();
    ok(&mut s, "doc.set_curve_tolerance", json!({"tolerance_mm": 0.05}));
    assert_eq!(s.document().curve_tolerance_mm, Some(0.05));
    assert!(err(&mut s, "doc.set_curve_tolerance", json!({"tolerance_mm": 2})).contains("0.001..=1"));
    ok(&mut s, "doc.set_curve_tolerance", json!({"tolerance_mm": null}));
    assert_eq!(s.document().curve_tolerance_mm, None);
}

#[test]
fn extend_to_the_nearest_line() {
    let mut s = session();
    let line = path(&mut s, "M 0 0 L 5 0");
    path(&mut s, "M 10 -5 L 10 5");
    path(&mut s, "M 20 -5 L 20 5");
    let r = ok(&mut s, "node.extend", json!({"id": line, "at": [5, 0]}));
    assert_eq!(r["to"], json!([10.0, 0.0]));
    assert_eq!(s.document().object(&line).unwrap().bounds().x1, 10.0);
    // Again: now it reaches the next wall.
    let r = ok(&mut s, "node.extend", json!({"id": line, "at": [10, 0]}));
    assert_eq!(r["to"], json!([20.0, 0.0]));
    assert!(err(&mut s, "node.extend", json!({"id": line, "at": [0, 0]})).contains("nothing to extend"));
    assert!(err(&mut s, "node.extend", json!({"id": line, "at": [7, 3]})).contains("no open end"));
    let rect = created(&ok(&mut s, "shape.rect", json!({"width_mm": 2, "height_mm": 2, "center": [0, 30]})));
    assert!(err(&mut s, "node.extend", json!({"id": rect, "at": [1, 31]})).contains("open ends"));
}

#[test]
fn warp_perspective_envelope_and_grid() {
    let mut s = session();
    let r = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [5, 5]})));
    let corners = json!([[0, 0], [10, 0], [8, 6], [2, 6]]);
    ok(&mut s, "path.warp", json!({"ids": [r], "mode": "perspective", "corners": corners}));
    let o = s.document().object(&r).unwrap();
    assert!(matches!(o.geometry, Geometry::Path { .. }));
    // A trapezoid: (10 + 6) / 2 * 6.
    assert!((o.field_path().area().abs() - 48.0).abs() < 1e-6, "{}", o.field_path().area());
    s.undo().unwrap();
    // Envelope bends the circle; grid with the identity handles changes nothing.
    let c = created(&ok(&mut s, "shape.ellipse", json!({"rx_mm": 4, "ry_mm": 4, "center": [30, 0]})));
    let area = s.document().object(&c).unwrap().field_path().area();
    let grid: Vec<Vec<[f64; 2]>> = (0..4)
        .map(|row| (0..4).map(|col| [26.0 + 8.0 * col as f64 / 3.0, -4.0 + 8.0 * row as f64 / 3.0]).collect())
        .collect();
    ok(&mut s, "path.warp", json!({"ids": [c], "mode": "grid", "grid": grid}));
    let after = s.document().object(&c).unwrap().field_path().area();
    assert!((after - area).abs() < 1e-3, "{area} {after}");
    ok(&mut s, "path.warp", json!({"ids": [c], "mode": "envelope", "corners": [[26, -4], [34, -4], [38, 4], [22, 4]]}));
    assert!(s.document().object(&c).unwrap().field_path().area() > area);
    assert!(err(&mut s, "path.warp", json!({"ids": [c], "mode": "perspective"})).contains("corners"));
    assert!(
        err(
            &mut s,
            "path.warp",
            json!({"ids": [c], "mode": "perspective", "corners": [[0, 0], [10, 10], [10, 0], [0, 10]]})
        )
        .contains("convex")
    );
}

#[test]
fn tangent_circles_rubber_band_and_toolpaths() {
    let mut s = session();
    let a = created(&ok(&mut s, "shape.circle", json!({"diameter_mm": 2, "center": [0, 0]})));
    let b = created(&ok(&mut s, "shape.circle", json!({"diameter_mm": 4, "center": [10, 0]})));
    let c = created(&ok(&mut s, "shape.circle", json!({"diameter_mm": 3, "center": [4, 8]})));
    let rev = s.document().revision;
    let cand = ok(&mut s, "art.tangent_circle_candidates", json!({"ids": [a, b, c]}));
    assert_eq!(cand["candidates"].as_array().unwrap().len(), 8);
    assert_eq!(s.document().revision, rev);
    // The small one in the middle, picked by a click near its outline.
    let small = &cand["candidates"][0];
    let (cx, cy, r) = (
        small["center"][0].as_f64().unwrap(),
        small["center"][1].as_f64().unwrap(),
        small["radius_mm"].as_f64().unwrap(),
    );
    let made = ok(&mut s, "art.tangent_circle", json!({"ids": [a, b, c], "near": [cx + r, cy]}));
    let t = created(&made);
    let o = s.document().object(&t).unwrap();
    assert!(matches!(o.geometry, Geometry::Ellipse { rx_mm, ry_mm } if (rx_mm - r).abs() < 1e-3 && rx_mm == ry_mm));
    let all = ok(&mut s, "art.tangent_circle", json!({"ids": [a, b, c], "pick": "all"}));
    assert_eq!(all["created"].as_array().unwrap().len(), 8);
    assert!(err(&mut s, "art.tangent_circle", json!({"ids": [a, b, c], "pick": [9]})).contains("no candidate"));
    let rect = created(&ok(&mut s, "shape.rect", json!({"width_mm": 2, "height_mm": 2})));
    assert!(err(&mut s, "art.tangent_circle_candidates", json!({"ids": [a, b, rect]})).contains("not a circle"));
    assert!(err(&mut s, "art.tangent_circle_candidates", json!({"ids": [a, b]})).contains("exactly three"));

    // Rubber band around two circles: a stadium-like hull.
    let band = created(&ok(&mut s, "art.rubber_band", json!({"ids": [a, b]})));
    let bb = s.document().object(&band).unwrap().bounds();
    assert!((bb.x0 + 1.0).abs() < 0.02 && (bb.x1 - 12.0).abs() < 0.02, "{bb:?}");

    // Toolpath of a filled rectangle: its hatch lines as an open path.
    let mut s = session();
    s.execute(
        Invocation {
            id: "layer.update".into(),
            params: json!({"id": "layer-1", "settings": {"mode": "fill", "hatch": {"interval_mm": 1}}}),
        },
        Origin::Ui,
    )
    .unwrap();
    let sq = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})));
    let r = ok(&mut s, "art.toolpath_to_path", json!({"ids": [sq]}));
    let tp = created(&r);
    let o = s.document().object(&tp).unwrap();
    assert!(o.name.starts_with("Toolpath"));
    let subs = geom::split(&o.field_path());
    assert!(subs.len() >= 9 && subs.iter().all(|s| !s.closed), "{}", subs.len());
    assert_eq!(s.document().objects.len(), 2, "the original stays");
}

#[test]
fn optimize_fits_arcs_and_smooths() {
    let mut s = session();
    // A segmented quarter circle of radius 10.
    let mut d = String::from("M 10 0");
    for i in 1..=30 {
        let a = std::f64::consts::FRAC_PI_2 * i as f64 / 30.0;
        d.push_str(&format!(" L {} {}", 10.0 * a.cos(), 10.0 * a.sin()));
    }
    let p = path(&mut s, &d);
    let r = ok(&mut s, "path.optimize", json!({"ids": [p], "arcs_mm": 0.02}));
    assert_eq!(r["arcs_fitted"], json!(1));
    assert!(r["nodes_after"].as_u64().unwrap() < 5, "{r}");
    let o = s.document().object(&p).unwrap();
    assert!(geom::split(&o.field_path())[0].segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))));
    s.undo().unwrap();
    let r = ok(&mut s, "path.optimize", json!({"ids": [p], "smooth_mm": 0.05}));
    assert_eq!(r["runs_smoothed"], json!(1));
    // Lines only, as before.
    s.undo().unwrap();
    ok(&mut s, "path.optimize", json!({"ids": [p], "tolerance_mm": 0.5}));
    assert!(err(&mut s, "path.optimize", json!({"ids": [p]})).contains("at least one"));
    assert!(err(&mut s, "path.optimize", json!({"ids": [p], "arcs_mm": 20})).contains("at most 10"));
}

#[test]
fn fillet_rounds_line_curve_corners() {
    let mut s = session();
    let p = path(&mut s, "M 0 0 L 10 0 C 10 5 15 10 20 10");
    let r = ok(&mut s, "path.fillet", json!({"ids": [p], "radius_mm": 2}));
    assert_eq!(r["corners"][&p]["filleted"], json!([1]));
    // A D shape: a line and a half circle meeting at two sharp corners.
    let dshape = path(&mut s, "M 0 -5 L 0 5 C 7 5 7 -5 0 -5 Z");
    let r = ok(&mut s, "path.fillet", json!({"ids": [dshape], "radius_mm": 0.5}));
    assert_eq!(r["corners"][&dshape]["filleted"].as_array().unwrap().len(), 2, "{r}");
}

#[test]
fn transform_keep_copy_mirrors_a_copy() {
    let mut s = session();
    let a = created(&ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2, "center": [5, 0]})));
    let r = ok(
        &mut s,
        "object.transform",
        json!({"ids": [a], "mirror_line": {"a": [0, -1], "b": [0, 1]}, "keep_copy": true}),
    );
    let copy = created(&r);
    assert_eq!(r["copies"][&a], json!(copy));
    assert_eq!(s.document().object(&a).unwrap().bounds().center(), Point::new(5.0, 0.0), "the original stays");
    assert!(s.document().object(&copy).unwrap().bounds().center().distance(Point::new(-5.0, 0.0)) < 1e-9);
    assert_eq!(s.document().objects.len(), 2);
    // A locked original can still be copied.
    ok(&mut s, "object.lock", json!({"ids": [a], "locked": true}));
    ok(&mut s, "object.transform", json!({"ids": [a], "translate": [0, 10], "keep_copy": true}));
    assert!(run(&mut s, "object.transform", json!({"ids": [a], "translate": [0, 10]})).is_err());
}

#[test]
fn nest_true_shapes_interlock() {
    let mut s = session();
    let l1 = path(&mut s, "M 0 0 L 20 0 L 20 5 L 5 5 L 5 20 L 0 20 Z");
    let l2 = path(&mut s, "M 40 40 L 20 40 L 20 35 L 35 35 L 35 20 L 40 20 Z");
    // 23 x 30 mm: two 20 x 20 boxes cannot both fit; the Ls interlock.
    let region = json!({"min": [0, 0], "max": [23, 30]});
    let boxes = run(&mut s, "art.nest", json!({"ids": [l1, l2], "region": region, "spacing_mm": 0.5}));
    assert!(boxes.is_ok_and(|r| !r["unplaced"].as_array().unwrap().is_empty()));
    s.undo().ok();
    let r = ok(&mut s, "art.nest", json!({"ids": [l1, l2], "region": region, "spacing_mm": 0.5, "mode": "shapes"}));
    assert_eq!(r["unplaced"], json!([]), "{r}");
    assert_eq!(r["mode"], json!("shapes"));
    let (a, b) = (s.document().object(&l1).unwrap().field_path(), s.document().object(&l2).unwrap().field_path());
    let ra = geom::Region::from_path(&a, geom::Fill::EvenOdd).unwrap();
    let rb = geom::Region::from_path(&b, geom::Fill::EvenOdd).unwrap();
    assert!(ra.boolean(&rb, geom::BoolOp::Intersect).unwrap().is_empty(), "no overlap");
    for p in [&a, &b] {
        let bb = p.bounding_box();
        assert!(bb.x0 >= -1e-9 && bb.y0 >= -1e-9 && bb.x1 <= 23.0 + 1e-9 && bb.y1 <= 30.0 + 1e-9, "{bb:?}");
    }
    assert!(
        err(&mut s, "art.nest", json!({"ids": [l1], "region": region, "resolution_mm": 0.1})).contains("only for mode")
    );
}
