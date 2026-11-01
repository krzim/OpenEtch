//! End-to-end tests of the geometry commands through [`Session`]: each edit is one undo step, refuses bad input and
//! never panics.

use doc::{Document, Geometry};
use kurbo::Shape;
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

fn rect(s: &mut Session, w: f64, h: f64, c: [f64; 2]) -> String {
    ok(s, "shape.rect", json!({"width_mm": w, "height_mm": h, "center": c}))["created"][0].as_str().unwrap().to_string()
}

fn path(s: &mut Session, d: &str) -> String {
    ok(s, "shape.path", json!({"svg_path_data": d}))["created"][0].as_str().unwrap().to_string()
}

fn area(s: &Session, id: &str) -> f64 {
    let o = s.document().object(id).unwrap();
    // Even-odd area of the closed outline, as jobs fill it.
    geom::Region::from_path(&o.field_path(), geom::Fill::EvenOdd).unwrap().area()
}

fn steps(s: &Session) -> usize {
    s.history().len()
}

fn ids(v: &Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn boolean_union_intersect_xor_subtract() {
    let mut s = session();
    let a = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    let b = rect(&mut s, 10.0, 10.0, [5.0, 0.0]);
    let n = steps(&s);
    let r = ok(&mut s, "path.boolean", json!({"ids": [a, b], "op": "union"}));
    assert_eq!(steps(&s), n + 1, "one undo step");
    assert_eq!(r["affected"], json!([a]));
    assert_eq!(r["deleted"], json!([b]));
    assert!((area(&s, &a) - 150.0).abs() < 1e-6);
    assert!(matches!(s.document().object(&a).unwrap().geometry, Geometry::Path { .. }));
    s.undo().unwrap();
    assert_eq!(s.document().objects.len(), 2, "undo brings both back");
    ok(&mut s, "path.boolean", json!({"ids": [a, b], "op": "intersect"}));
    assert!((area(&s, &a) - 50.0).abs() < 1e-6);
    s.undo().unwrap();
    ok(&mut s, "path.boolean", json!({"ids": [a, b], "op": "xor"}));
    assert!((area(&s, &a) - 100.0).abs() < 1e-6);
    s.undo().unwrap();
    // Subtract: the last is cut out of the first and deleted.
    let r = ok(&mut s, "path.boolean", json!({"ids": [a, b], "op": "subtract"}));
    assert_eq!(r["deleted"], json!([b]));
    assert!((area(&s, &a) - 50.0).abs() < 1e-6);
    let bb = s.document().object(&a).unwrap().bounds();
    assert!((bb.x0 + 5.0).abs() < 1e-6 && (bb.x1 - 0.0).abs() < 1e-6, "{bb:?}");
}

#[test]
fn boolean_keeps_holes_and_refuses_bad_input() {
    let mut s = session();
    // A square with a hole (even-odd), unioned with a square far away: the hole stays.
    let holed = path(&mut s, "M-10 -10 L10 -10 L10 10 L-10 10 Z M-5 -5 L5 -5 L5 5 L-5 5 Z");
    let far = rect(&mut s, 4.0, 4.0, [30.0, 0.0]);
    ok(&mut s, "path.boolean", json!({"ids": [holed, far], "op": "union"}));
    assert!((area(&s, &holed) - (300.0 + 16.0)).abs() < 1e-6, "{}", area(&s, &holed));
    // Disjoint intersection is refused and changes nothing.
    let x = rect(&mut s, 2.0, 2.0, [-60.0, 0.0]);
    let n = steps(&s);
    assert!(err(&mut s, "path.boolean", json!({"ids": [holed, x], "op": "intersect"})).contains("empty"));
    assert_eq!(steps(&s), n);
    // Open paths, images, single shapes and unknown ids are refused.
    let open = path(&mut s, "M0 0 L10 0 L10 10");
    assert!(err(&mut s, "path.boolean", json!({"ids": [holed, open], "op": "union"})).contains("not closed"));
    assert!(err(&mut s, "path.boolean", json!({"ids": [holed], "op": "union"})).contains("two"));
    assert!(matches!(run(&mut s, "path.boolean", json!({"ids": [holed, "obj-999"], "op": "union"})), Err(CommandError::NotFound { .. })));
    assert!(run(&mut s, "path.boolean", json!({"ids": [holed, x], "op": "nand"})).is_err());
    // Self-intersecting input (a bow tie) is filled even-odd.
    let bow = path(&mut s, "M0 0 L10 10 L10 0 L0 10 Z");
    let sq = rect(&mut s, 10.0, 10.0, [5.0, 5.0]);
    ok(&mut s, "path.boolean", json!({"ids": [sq, bow], "op": "subtract"}));
    assert!((area(&s, &sq) - 50.0).abs() < 1e-6);
}

#[test]
fn groups_count_as_one_shape() {
    let mut s = session();
    let a = rect(&mut s, 4.0, 4.0, [0.0, 0.0]);
    let b = rect(&mut s, 4.0, 4.0, [10.0, 0.0]);
    let g = ok(&mut s, "group.create", json!({"ids": [a, b]}))["group"].as_str().unwrap_or("group-4").to_string();
    let cutter = rect(&mut s, 30.0, 2.0, [5.0, 0.0]);
    let r = ok(&mut s, "path.boolean", json!({"ids": [g, cutter], "op": "subtract"}));
    assert_eq!(ids(&r["affected"]).len(), 2);
    assert!((area(&s, &a) - 8.0).abs() < 1e-6 && (area(&s, &b) - 8.0).abs() < 1e-6);
}

#[test]
fn weld_merges_overlaps_into_islands() {
    let mut s = session();
    // One path with two overlapping squares (self-overlap), plus another overlapping object and a separate one.
    let p = path(&mut s, "M0 0 L10 0 L10 10 L0 10 Z M5 0 L15 0 L15 10 L5 10 Z");
    let q = rect(&mut s, 10.0, 10.0, [15.0, 5.0]);
    let far = rect(&mut s, 2.0, 2.0, [50.0, 0.0]);
    let r = ok(&mut s, "path.weld", json!({"ids": [p, q, far]}));
    assert_eq!(r["islands"], json!(2));
    assert_eq!(s.document().objects.len(), 1);
    assert!((area(&s, &p) - (200.0 + 4.0)).abs() < 1e-6, "{}", area(&s, &p));
    // A letter counter (opposite winding) survives a weld.
    let o = path(&mut s, "M0 20 L10 20 L10 30 L0 30 Z M3 23 L3 27 L7 27 L7 23 Z");
    ok(&mut s, "path.weld", json!({"ids": [o]}));
    assert!((area(&s, &o) - 84.0).abs() < 1e-6);
}

#[test]
fn cut_shapes_by_layer_mode() {
    let mut s = session();
    let line_sq = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    ok(&mut s, "layer.add", json!({"name": "Fill"}));
    let fill_layer = s.document().layers[1].id.clone();
    ok(&mut s, "layer.update", json!({"id": fill_layer, "settings": {"mode": "fill"}}));
    let fill_sq = ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [0, 20], "layer": fill_layer}))["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    let untouched = rect(&mut s, 2.0, 2.0, [40.0, 40.0]);
    let tool = rect(&mut s, 10.0, 40.0, [5.0, 10.0]);
    let r = ok(&mut s, "path.cut_shapes", json!({"ids": [line_sq, fill_sq, untouched, tool]}));
    let inside = ids(&r["inside"]["ids"]);
    let outside = ids(&r["outside"]["ids"]);
    assert_eq!((inside.len(), outside.len()), (2, 2), "{r}");
    assert!(s.document().object(&tool).is_none() && s.document().object(&line_sq).is_none());
    assert!(s.document().object(&untouched).is_some(), "not crossed: left alone");
    let d = s.document();
    let closed = |id: &str| geom::flatten(&d.object(id).unwrap().field_path(), 0.01).unwrap().iter().all(|l| l.closed);
    let fill_pieces: Vec<&String> = inside.iter().chain(&outside).filter(|id| d.object(id).unwrap().layer == fill_layer).collect();
    assert_eq!(fill_pieces.len(), 2);
    assert!(fill_pieces.iter().all(|id| closed(id)), "fill-layer pieces are closed");
    let line_pieces: Vec<&String> = inside.iter().chain(&outside).filter(|id| d.object(id).unwrap().layer != fill_layer).collect();
    assert!(line_pieces.iter().all(|id| !closed(id)), "line-layer pieces stay open");
    let line_len: f64 = line_pieces.iter().map(|id| geom::flatten(&d.object(id).unwrap().field_path(), 0.01).unwrap().iter().map(|l| l.length()).sum::<f64>()).sum();
    assert!((line_len - 40.0).abs() < 1e-6, "the outline is all there: {line_len}");
    assert_eq!(d.group(r["inside"]["group"].as_str().unwrap()).unwrap().name, "Cut inside");
    // The tool must be a closed object, not a group, and must cross something.
    let a = rect(&mut s, 2.0, 2.0, [-50.0, -50.0]);
    let open = path(&mut s, "M0 0 L1 1");
    assert!(err(&mut s, "path.cut_shapes", json!({"ids": [a, open]})).contains("not closed"));
    let t2 = rect(&mut s, 1.0, 1.0, [60.0, -60.0]);
    assert!(err(&mut s, "path.cut_shapes", json!({"ids": [a, t2]})).contains("does not cross"));
    assert!(err(&mut s, "path.cut_shapes", json!({"ids": [t2]})).contains("at least one"));
}

#[test]
fn offset_directions_joins_and_open_paths() {
    let mut s = session();
    let sq = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    let r = ok(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 1, "join": "miter"}));
    let out = ids(&r["created"])[0].clone();
    assert!((area(&s, &out) - 144.0).abs() < 1e-3);
    assert_eq!(s.document().object(&out).unwrap().name, "Rectangle offset");
    s.undo().unwrap();
    let r = ok(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 1, "direction": "both", "join": "round"}));
    let c = ids(&r["created"]);
    assert_eq!(c.len(), 2);
    assert!((area(&s, &c[1]) - 64.0).abs() < 1e-3);
    s.undo().unwrap();
    // A negative distance flips outward to inward.
    let r = ok(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": -2, "join": "bevel"}));
    assert!((area(&s, &ids(&r["created"])[0]) - 36.0).abs() < 1e-3);
    s.undo().unwrap();
    // Open paths become bands outward and are skipped inward.
    let line = path(&mut s, "M20 0 L30 0");
    let r = ok(&mut s, "path.offset", json!({"ids": [line, sq], "distance_mm": 1, "direction": "inward"}));
    assert_eq!(ids(&r["created"]).len(), 1);
    assert_eq!(r["skipped"][0]["id"], json!(line));
    s.undo().unwrap();
    let r = ok(&mut s, "path.offset", json!({"ids": [line], "distance_mm": 1}));
    assert!((area(&s, &ids(&r["created"])[0]) - (20.0 + std::f64::consts::PI)).abs() < 0.05);
    s.undo().unwrap();
    // combine merges overlapping results; keep_original false deletes the sources.
    let sq2 = rect(&mut s, 10.0, 10.0, [8.0, 0.0]);
    let r = ok(&mut s, "path.offset", json!({"ids": [sq, sq2], "distance_mm": 1, "combine": true, "keep_original": false, "join": "miter"}));
    assert_eq!(ids(&r["created"]).len(), 1);
    assert_eq!(ids(&r["deleted"]), [sq.clone(), sq2.clone()]);
    s.undo().unwrap();
    // Bad input.
    assert!(run(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": "x"})).is_err());
    assert!(err(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 5000})).contains("1000"));
    assert!(err(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 1, "miter_limit": 2})).contains("miter"));
    assert!(err(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 1, "join": "miter", "miter_limit": 0.5})).contains(">= 1"));
    assert!(err(&mut s, "path.offset", json!({"ids": [sq], "distance_mm": 6, "direction": "inward"})).contains("no result"));
}

#[test]
fn offset_keeps_circles_close_to_round() {
    let mut s = session();
    let c = ok(&mut s, "shape.circle", json!({"diameter_mm": 20}))["created"][0].as_str().unwrap().to_string();
    let r = ok(&mut s, "path.offset", json!({"ids": [c], "distance_mm": 2}));
    let b = s.document().object(&ids(&r["created"])[0]).unwrap().bounds();
    assert!((b.width() - 24.0).abs() < 0.03, "{b:?}");
}

#[test]
fn break_apart_modes() {
    let mut s = session();
    let sq = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    let r = ok(&mut s, "path.break_apart", json!({"ids": [sq]}));
    assert_eq!(ids(&r["created"]).len(), 3, "a rectangle becomes four lines");
    s.undo().unwrap();
    let letter = path(&mut s, "M0 0 L10 0 L10 10 L0 10 Z M3 3 L3 7 L7 7 L7 3 Z M20 0 L30 0 L30 10 L20 10 Z");
    let r = ok(&mut s, "path.break_apart", json!({"ids": [letter], "mode": "islands"}));
    assert_eq!(ids(&r["created"]).len(), 1, "the o keeps its counter");
    assert!((area(&s, &letter) - 84.0).abs() < 1e-6);
    s.undo().unwrap();
    let r = ok(&mut s, "path.break_apart", json!({"ids": [letter]}));
    assert_eq!(ids(&r["created"]).len(), 2, "auto: three subpaths");
    s.undo().unwrap();
    let line = path(&mut s, "M0 0 L1 0");
    assert!(err(&mut s, "path.break_apart", json!({"ids": [line]})).contains("nothing to break"));
}

#[test]
fn auto_join_close_and_reverse() {
    let mut s = session();
    let a = path(&mut s, "M0 0 L10 0");
    let b = path(&mut s, "M10 10 L10 0.03");
    let c = path(&mut s, "M10 10 L0 10 L0 0");
    let far = path(&mut s, "M50 50 L60 50");
    let r = ok(&mut s, "path.auto_join", json!({"ids": [a, b, c, far]}));
    assert_eq!(r["closed"], json!(1));
    assert_eq!(ids(&r["deleted"]), [b.clone(), c.clone()]);
    assert!((area(&s, &a) - 100.0).abs() < 0.5);
    assert!(s.document().object(&far).is_some());
    s.undo().unwrap();
    assert!(err(&mut s, "path.auto_join", json!({"ids": [a, far]})).contains("no open path ends"));
    assert!(err(&mut s, "path.auto_join", json!({"ids": [a], "tolerance_mm": 50})).contains("at most 10"));
    // Close with and without tolerance.
    let u = path(&mut s, "M0 20 L10 20 L10 30 L0 30.5");
    assert!(err(&mut s, "path.close", json!({"ids": [u], "tolerance_mm": 1})).contains("could be closed"));
    let r = ok(&mut s, "path.close", json!({"ids": [u], "tolerance_mm": 11}));
    assert_eq!(r["closed"], json!(1));
    s.undo().unwrap();
    ok(&mut s, "path.close", json!({"ids": [u], "mode": "move_ends"}));
    assert!((area(&s, &u) - 50.0).abs() < 1e-6, "the ends met at their midpoint");
    // Reverse flips the direction and keeps the outline.
    let sq = rect(&mut s, 4.0, 4.0, [30.0, 30.0]);
    let before = s.document().object(&sq).unwrap().field_path().area();
    ok(&mut s, "path.reverse", json!({"ids": [sq]}));
    let after = s.document().object(&sq).unwrap().field_path().area();
    assert!((before + after).abs() < 1e-9 && before != 0.0);
}

#[test]
fn delete_duplicates_objects_and_segments() {
    let mut s = session();
    let a = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    let b = path(&mut s, "M5 5 L-5 5 L-5 -5 L5 -5 Z"); // same square, other start and direction
    let c = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    ok(&mut s, "object.lock", json!({"ids": [c], "locked": true}));
    let other = rect(&mut s, 10.0, 10.0, [1.0, 0.0]);
    let r = ok(&mut s, "path.delete_duplicates", json!({}));
    assert_eq!(ids(&r["deleted"]), std::slice::from_ref(&b), "locked duplicates stay");
    assert!(s.document().object(&a).is_some() && s.document().object(&other).is_some());
    s.undo().unwrap();
    // Shared edge between two parts.
    let mut s = session();
    let left = path(&mut s, "M0 0 L10 0 L10 10 L0 10 Z");
    let right = path(&mut s, "M10 0 L20 0 L20 10 L10 10 Z");
    let r = ok(&mut s, "path.delete_duplicates", json!({"ids": [left, right], "segments": true}));
    assert_eq!(r["segments_removed"], json!(1));
    assert_eq!(ids(&r["affected"]), std::slice::from_ref(&right));
    assert!(err(&mut s, "path.delete_duplicates", json!({"ids": [left]})).contains("no duplicates"));
}

#[test]
fn convert_optimize_fillet() {
    let mut s = session();
    let e = ok(&mut s, "shape.ellipse", json!({"rx_mm": 5, "ry_mm": 3}))["created"][0].as_str().unwrap().to_string();
    ok(&mut s, "path.convert", json!({"ids": [e]}));
    assert!(matches!(s.document().object(&e).unwrap().geometry, Geometry::Path { .. }));
    assert!(err(&mut s, "path.convert", json!({"ids": [e]})).contains("nothing to convert"));
    // Optimize removes collinear nodes.
    let p = path(&mut s, "M0 0 L5 0 L10 0 L10 10 L0 10 Z");
    let r = ok(&mut s, "path.optimize", json!({"ids": [p], "tolerance_mm": 0.01}));
    assert_eq!((r["nodes_before"].as_u64(), r["nodes_after"].as_u64()), (Some(5), Some(4)));
    assert!(err(&mut s, "path.optimize", json!({"ids": [p], "tolerance_mm": 0.01})).contains("nothing to optimize"));
    assert!(run(&mut s, "path.optimize", json!({"ids": [p], "tolerance_mm": -1})).is_err());
    // A rectangle stays a rectangle with a corner radius.
    let r0 = rect(&mut s, 10.0, 6.0, [30.0, 0.0]);
    ok(&mut s, "path.fillet", json!({"ids": [r0], "radius_mm": 2}));
    assert!(matches!(s.document().object(&r0).unwrap().geometry, Geometry::Rect { corner_radius_mm, .. } if corner_radius_mm == 2.0));
    // Path corners by index; one too big to fit is skipped.
    let tri = path(&mut s, "M0 30 L20 30 L0 50 Z");
    let r = ok(&mut s, "path.fillet", json!({"ids": [tri], "radius_mm": 1, "corners": [0, 9]}));
    assert_eq!(r["corners"][&tri]["filleted"], json!([0]));
    assert_eq!(r["corners"][&tri]["skipped"][0]["node"], json!(9));
    assert!(err(&mut s, "path.fillet", json!({"ids": [e], "radius_mm": 1})).contains("no corner"));
    assert!(err(&mut s, "path.fillet", json!({"ids": [tri, e], "radius_mm": 1, "corners": [0]})).contains("exactly one"));
    assert!(run(&mut s, "path.fillet", json!({"ids": [tri], "radius_mm": 0})).is_err());
}

#[test]
fn unfillet_by_click_and_all() {
    let mut s = session();
    // A filleted triangle: click one rounded corner, then undo the rest.
    let tri = path(&mut s, "M0 0 L20 0 L0 20 Z");
    let before = area(&s, &tri);
    ok(&mut s, "path.fillet", json!({"ids": [tri], "radius_mm": 1}));
    let rounded = area(&s, &tri);
    assert!(rounded < before);
    // The corner at (0, 0) is rounded: its arc passes about (0.29, 0.29).
    let r = ok(&mut s, "path.unfillet", json!({"ids": [tri], "at": [0.3, 0.3], "pick_mm": 0.5}));
    let c = &r["corners"][&tri]["unrounded"][0];
    assert!(c["corner"][0].as_f64().unwrap().abs() < 1e-6 && c["corner"][1].as_f64().unwrap().abs() < 1e-6, "{c}");
    assert!((c["radius_mm"].as_f64().unwrap() - 1.0).abs() < 1e-3);
    assert!(area(&s, &tri) > rounded);
    // Clicking a straight side is refused; far away too.
    assert!(err(&mut s, "path.unfillet", json!({"ids": [tri], "at": [10, 0]})).contains("no rounded corner"));
    assert!(err(&mut s, "path.unfillet", json!({"ids": [tri], "at": [100, 100]})).contains("within"));
    let r = ok(&mut s, "path.unfillet", json!({"ids": [tri]}));
    assert_eq!(r["corners"][&tri]["unrounded"].as_array().unwrap().len(), 2);
    assert!((area(&s, &tri) - before).abs() < 1e-6);
    assert!(err(&mut s, "path.unfillet", json!({"ids": [tri]})).contains("no corner was made sharp"));
    // One undo step each.
    s.undo().unwrap();
    assert!(area(&s, &tri) < before);
    // A rounded rectangle stays a rectangle; a single corner click turns it into a path.
    let r0 = rect(&mut s, 10.0, 6.0, [30.0, 0.0]);
    ok(&mut s, "path.fillet", json!({"ids": [r0], "radius_mm": 2}));
    ok(&mut s, "path.unfillet", json!({"ids": [r0]}));
    assert!(matches!(s.document().object(&r0).unwrap().geometry, Geometry::Rect { corner_radius_mm, .. } if corner_radius_mm == 0.0));
    ok(&mut s, "path.fillet", json!({"ids": [r0], "radius_mm": 2}));
    let r = ok(&mut s, "path.unfillet", json!({"ids": [r0], "at": [35, 3]}));
    assert_eq!(r["corners"][&r0]["unrounded"][0]["corner"], json!([35.0, 3.0]));
    assert!(matches!(s.document().object(&r0).unwrap().geometry, Geometry::Path { .. }));
    let want = 60.0 - 3.0 * 4.0 * (1.0 - std::f64::consts::PI / 4.0);
    assert!((area(&s, &r0) - want).abs() < 0.1, "{} {want}", area(&s, &r0));
    // Bad input.
    assert!(err(&mut s, "path.unfillet", json!({"ids": [tri, r0], "at": [0, 0]})).contains("exactly one"));
    assert!(run(&mut s, "path.unfillet", json!({"ids": [tri], "at": [0, 0], "pick_mm": -1})).is_err());
    let e = ok(&mut s, "shape.ellipse", json!({"rx_mm": 5, "ry_mm": 2}))["created"][0].as_str().unwrap().to_string();
    assert!(err(&mut s, "path.unfillet", json!({"ids": [e]})).contains("no corner"));
}

#[test]
fn trim_between_crossings() {
    let mut s = session();
    let h = path(&mut s, "M0 0 L30 0");
    path(&mut s, "M10 -5 L10 5");
    path(&mut s, "M20 -5 L20 5");
    let r = ok(&mut s, "path.trim", json!({"id": h, "at": [15, 0.2]}));
    assert_eq!(r["cut_at"].as_array().unwrap().len(), 2);
    let len: f64 = geom::flatten(&s.document().object(&h).unwrap().field_path(), 0.01).unwrap().iter().map(|l| l.length()).sum();
    assert!((len - 20.0).abs() < 1e-6);
    assert!(err(&mut s, "path.trim", json!({"id": h, "at": [15, 20]})).contains("within"));
    // A line with no crossings goes entirely.
    let lone = path(&mut s, "M0 50 L10 50");
    let r = ok(&mut s, "path.trim", json!({"id": lone, "at": [5, 50]}));
    assert_eq!(r["deleted"], json!([lone]));
}

#[test]
fn node_editing_under_a_transform() {
    let mut s = session();
    let sq = rect(&mut s, 10.0, 10.0, [100.0 - 80.0, 0.0]);
    ok(&mut s, "object.transform", json!({"ids": [sq], "rotate_deg": 90}));
    let n = steps(&s);
    let l = ok(&mut s, "node.list", json!({"id": sq}));
    assert_eq!(steps(&s), n, "listing is a query");
    assert_eq!(l["nodes"].as_array().unwrap().len(), 4);
    assert_eq!(l["converts_on_edit"], json!(true));
    // Points are in field mm (rotated).
    let p0 = l["nodes"][0]["point"].clone();
    ok(&mut s, "node.move", json!({"id": sq, "index": 0, "to": [0, 0]}));
    let l = ok(&mut s, "node.list", json!({"id": sq}));
    assert_eq!(l["nodes"][0]["point"], json!([0.0, 0.0]));
    assert_ne!(p0, json!([0.0, 0.0]));
    assert!(matches!(s.document().object(&sq).unwrap().geometry, Geometry::Path { .. }));
    let r = ok(&mut s, "node.insert", json!({"id": sq, "segment": 1, "t": 0.5}));
    assert_eq!(r["index"], json!(2));
    ok(&mut s, "node.segment_type", json!({"id": sq, "segment": 0, "type": "curve"}));
    let l = ok(&mut s, "node.list", json!({"id": sq}));
    assert_eq!(l["segments"][0]["type"], json!("curve"));
    assert!(l["nodes"][0]["handle_out"].is_array());
    ok(&mut s, "node.move", json!({"id": sq, "index": 0, "to": [3, -3], "handle": "out"}));
    assert!(err(&mut s, "node.move", json!({"id": sq, "index": 2, "to": [3, -3], "handle": "out"})).contains("line"));
    ok(&mut s, "node.set_type", json!({"id": sq, "indices": [3], "type": "smooth"}));
    ok(&mut s, "node.delete", json!({"id": sq, "indices": [2]}));
    let r = ok(&mut s, "node.break", json!({"id": sq, "index": 1}));
    assert_eq!(r["ends"].as_array().unwrap().len(), 2);
    let l = ok(&mut s, "node.list", json!({"id": sq}));
    assert_eq!(l["subpaths"][0]["closed"], json!(false));
    let a = r["ends"][0].as_u64().unwrap();
    let b = r["ends"][1].as_u64().unwrap();
    ok(&mut s, "node.join", json!({"id": sq, "a": a, "b": b}));
    let l = ok(&mut s, "node.list", json!({"id": sq}));
    assert_eq!(l["subpaths"][0]["closed"], json!(true));
    // Bad input never panics.
    assert!(err(&mut s, "node.move", json!({"id": sq, "index": 99, "to": [0, 0]})).contains("no node"));
    assert!(run(&mut s, "node.move", json!({"id": sq, "index": 0, "to": [f64::MAX, 0]})).is_err() || s.document().numeric_problem().is_none());
    assert!(err(&mut s, "node.insert", json!({"id": sq, "segment": 0, "t": 1.5})).contains("between 0 and 1"));
    assert!(err(&mut s, "node.delete", json!({"id": sq, "indices": []})).contains("at least one"));
    assert!(err(&mut s, "node.delete", json!({"id": sq, "indices": [0, 1, 2, 3]})).contains("whole shape"));
    assert!(err(&mut s, "node.join", json!({"id": sq, "a": 0, "b": 1})).contains("not an end"));
}

#[test]
fn generators() {
    let mut s = session();
    let r = ok(&mut s, "shape.polygon", json!({"sides": 6, "radius_mm": 10, "center": [5, 5]}));
    let h = ids(&r["created"])[0].clone();
    assert!((area(&s, &h) - 1.5 * 3f64.sqrt() * 100.0).abs() < 1e-6);
    let r = ok(&mut s, "shape.polygon", json!({"sides": 5, "width_mm": 20, "star_inner_ratio": 0.5, "corner_radius_mm": 0.5}));
    let st = ids(&r["created"])[0].clone();
    let w = s.document().object(&st).unwrap().bounds().width();
    assert!(w < 20.0 && w > 17.0, "rounded tips are a little narrower: {w}");
    assert_eq!(s.document().object(&st).unwrap().name, "Star");
    assert!(err(&mut s, "shape.polygon", json!({"sides": 2, "radius_mm": 1})).contains("sides"));
    assert!(err(&mut s, "shape.polygon", json!({"sides": 5, "radius_mm": 1, "width_mm": 2})).contains("exactly one"));
    assert!(err(&mut s, "shape.polygon", json!({"sides": 100000, "radius_mm": 1})).contains("sides"));
    let r = ok(&mut s, "shape.line", json!({"points": [[0, 0], [10, 0], [10, 10]], "closed": true}));
    let t = ids(&r["created"])[0].clone();
    assert!((area(&s, &t) - 50.0).abs() < 1e-9);
    assert_eq!(r["bounds"]["min"], json!([0.0, 0.0]));
    assert!(err(&mut s, "shape.line", json!({"points": [[0, 0]]})).contains("2 distinct"));
    let r = ok(&mut s, "shape.circle", json!({"diameter_mm": 8, "center": [1, 2]}));
    let c = ids(&r["created"])[0].clone();
    assert!(matches!(s.document().object(&c).unwrap().geometry, Geometry::Ellipse { rx_mm, ry_mm } if rx_mm == 4.0 && ry_mm == 4.0));
    assert!(run(&mut s, "shape.circle", json!({"diameter_mm": -1})).is_err());
}

#[test]
fn circular_array_and_along_path() {
    let mut s = session();
    let a = rect(&mut s, 2.0, 2.0, [10.0, 0.0]);
    let r = ok(&mut s, "art.circular_array", json!({"ids": [a], "count": 4, "center": [0, 0]}));
    let copies: Vec<Vec<String>> = serde_json::from_value(r["copies"].clone()).unwrap();
    assert_eq!(copies.len(), 3);
    let c = s.document().object(&copies[0][0]).unwrap().bounds().center();
    assert!((c.x).abs() < 1e-9 && (c.y - 10.0).abs() < 1e-9, "{c:?}");
    s.undo().unwrap();
    // A half-circle sweep puts the last copy at the end; unrotated copies keep their orientation.
    let r = ok(&mut s, "art.circular_array", json!({"ids": [a], "count": 3, "center": [0, 0], "angle_deg": 180, "rotate_copies": false}));
    let last = r["copies"][1][0].as_str().unwrap();
    let o = s.document().object(last).unwrap();
    assert!((o.bounds().center().x + 10.0).abs() < 1e-9);
    assert!(o.transform.as_coeffs()[1].abs() < 1e-12, "not rotated");
    assert!(err(&mut s, "art.circular_array", json!({"ids": [a], "count": 1, "center": [0, 0]})).contains("count"));
    assert!(err(&mut s, "art.circular_array", json!({"ids": [a], "count": 10001, "center": [0, 0]})).contains("count"));
    assert!(err(&mut s, "art.circular_array", json!({"ids": [a], "count": 3, "center": [0, 0], "angle_deg": 0})).contains("angle"));
    // Along an open path.
    let mut s = session();
    let p = path(&mut s, "M0 0 L30 0");
    let dot = ok(&mut s, "shape.circle", json!({"diameter_mm": 1}))["created"][0].as_str().unwrap().to_string();
    let r = ok(&mut s, "art.along_path", json!({"ids": [dot], "path_id": p, "count": 4}));
    let last = r["copies"][2][0].as_str().unwrap();
    assert!((s.document().object(last).unwrap().bounds().center().x - 30.0).abs() < 1e-9);
    s.undo().unwrap();
    let r = ok(&mut s, "art.along_path", json!({"ids": [dot], "path_id": p, "spacing_mm": 7}));
    assert_eq!(r["copies"].as_array().unwrap().len(), 4);
    s.undo().unwrap();
    assert!(err(&mut s, "art.along_path", json!({"ids": [dot], "path_id": p})).contains("exactly one"));
    assert!(err(&mut s, "art.along_path", json!({"ids": [dot, p], "path_id": p, "count": 3})).contains("cannot also"));
    assert!(err(&mut s, "art.along_path", json!({"ids": [dot], "path_id": p, "spacing_mm": 0.000001})).contains("more than"));
    // Around a closed square, rotating with it.
    let sq = rect(&mut s, 20.0, 20.0, [50.0, 0.0]);
    let r = ok(&mut s, "art.along_path", json!({"ids": [dot], "path_id": sq, "count": 4, "rotate_to_path": true}));
    assert_eq!(r["copies"].as_array().unwrap().len(), 3);
}

#[test]
fn nest_into_a_region() {
    let mut s = session();
    let a = rect(&mut s, 20.0, 10.0, [0.0, 0.0]);
    let b = rect(&mut s, 20.0, 10.0, [0.0, 0.0]);
    let c = rect(&mut s, 10.0, 30.0, [0.0, 0.0]);
    let huge = rect(&mut s, 100.0, 100.0, [0.0, 0.0]);
    let r = ok(&mut s, "art.nest", json!({"ids": [a, b, c, huge], "region": {"min": [0, 0], "max": [50, 50]}, "spacing_mm": 1, "rotations": [0, 90]}));
    assert_eq!(r["unplaced"], json!([huge]));
    let d = s.document();
    let boxes: Vec<kurbo::Rect> = [&a, &b, &c].iter().map(|id| d.object(id).unwrap().bounds()).collect();
    for (k, x) in boxes.iter().enumerate() {
        assert!(x.x0 >= -1e-9 && x.y0 >= -1e-9 && x.x1 <= 50.0 + 1e-9 && x.y1 <= 50.0 + 1e-9, "{x:?}");
        for y in &boxes[k + 1..] {
            let o = x.intersect(*y);
            assert!(o.width() <= 1e-9 || o.height() <= 1e-9);
        }
    }
    assert!(err(&mut s, "art.nest", json!({"ids": [huge], "region": "field", "spacing_mm": 1, "rotations": []})).contains("rotations"));
    assert!(err(&mut s, "art.nest", json!({"ids": [a], "region": "stock"})).contains("stock"));
    ok(&mut s, "stock.define", json!({"shape": {"kind": "circle", "diameter_mm": 50}, "thickness_mm": 1}));
    ok(&mut s, "art.nest", json!({"ids": [a], "region": "stock"}));
}

#[test]
fn nest_multipage_overflows_onto_new_pages() {
    for mode in ["boxes", "shapes"] {
        let mut s = session();
        // A 30 x 30 container takes four 13 x 13 squares (spacing 1); ten squares need three sheets.
        let sheet = rect(&mut s, 30.0, 30.0, [0.0, 0.0]);
        let parts: Vec<String> = (0..10).map(|k| rect(&mut s, 13.0, 13.0, [60.0 + k as f64 * 20.0, 60.0])).collect();
        let huge = rect(&mut s, 40.0, 40.0, [-60.0, -60.0]);
        let mut ids = parts.clone();
        ids.push(huge.clone());
        let params = json!({"ids": ids, "region": {"object": sheet}, "spacing_mm": 1, "mode": mode, "multipage": true, "resolution_mm": if mode == "shapes" { json!(0.25) } else { Value::Null }});
        let mut params = params;
        if mode == "boxes" {
            params.as_object_mut().unwrap().remove("resolution_mm");
        }
        let r = ok(&mut s, "art.nest", params);
        assert_eq!(r["unplaced"], json!([huge]), "{mode}: {r}");
        let pages = r["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 2, "{mode}: {r}");
        assert_eq!(pages.iter().map(|p| p["parts"].as_u64().unwrap()).sum::<u64>(), 6);
        // Every page has a copy of the container; the original page keeps four parts, the container and the huge one.
        let d = s.document();
        assert_eq!(d.objects.len(), 6);
        assert_eq!(d.pages.len(), 3);
        for (k, p) in pages.iter().enumerate() {
            let page = d.pages.iter().find(|x| x.id == p["page"].as_str().unwrap()).unwrap();
            assert_eq!(page.name, format!("Page {}", k + 2));
            let copy = p["container"][0].as_str().unwrap();
            let c = page.objects.iter().find(|o| o.id == copy).unwrap().bounds();
            assert!((c.x0 + 15.0).abs() < 1e-9 && (c.y1 - 15.0).abs() < 1e-9);
            for o in page.objects.iter().filter(|o| o.id != copy) {
                let b = o.bounds();
                assert!(b.x0 >= c.x0 - 1e-6 && b.y0 >= c.y0 - 1e-6 && b.x1 <= c.x1 + 1e-6 && b.y1 <= c.y1 + 1e-6, "{mode} {b:?}");
            }
        }
        // One undo step takes it all back.
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 12);
        assert!(s.document().pages.is_empty());
        // Without multipage the rest stays; max_pages caps the pages.
        let r = ok(&mut s, "art.nest", json!({"ids": parts, "region": {"object": sheet}, "spacing_mm": 1}));
        assert_eq!(r["unplaced"].as_array().unwrap().len(), 6);
        assert!(r["pages"].as_array().unwrap().is_empty());
        s.undo().unwrap();
        let r = ok(&mut s, "art.nest", json!({"ids": parts, "region": {"object": sheet}, "spacing_mm": 1, "multipage": true, "max_pages": 1}));
        assert_eq!(r["unplaced"].as_array().unwrap().len(), 2);
        assert!(err(&mut s, "art.nest", json!({"ids": parts, "region": "field", "max_pages": 2})).contains("only for multipage"));
        assert!(err(&mut s, "art.nest", json!({"ids": parts, "region": "field", "multipage": true, "max_pages": 0})).contains("max_pages"));
    }
}

#[test]
fn order_lock_and_two_point_transforms() {
    let mut s = session();
    let a = rect(&mut s, 2.0, 2.0, [0.0, 0.0]);
    let b = rect(&mut s, 2.0, 2.0, [5.0, 0.0]);
    let c = rect(&mut s, 2.0, 2.0, [10.0, 0.0]);
    let r = ok(&mut s, "object.order", json!({"ids": [a], "to": "front"}));
    assert_eq!(ids(&r["order"]), [b.clone(), c.clone(), a.clone()]);
    let r = ok(&mut s, "object.order", json!({"ids": [a], "to": "backward"}));
    assert_eq!(ids(&r["order"]), [b.clone(), a.clone(), c.clone()]);
    ok(&mut s, "object.order", json!({"ids": [a], "to": "back"}));
    assert!(err(&mut s, "object.order", json!({"ids": [a], "to": "back"})).contains("already"));
    let r = ok(&mut s, "object.order", json!({"ids": [a, b], "to": "forward"}));
    assert_eq!(ids(&r["order"]), [c.clone(), a.clone(), b.clone()]);
    // Locks refuse moves, transforms, deletes and reshaping; copying still works.
    ok(&mut s, "object.lock", json!({"ids": [a], "locked": true}));
    assert!(s.document().is_locked(&a));
    for (cmd, params) in [
        ("object.transform", json!({"ids": [a], "translate": [1, 0]})),
        ("object.move", json!({"ids": [a], "from_point": [0, 0], "to_point": [1, 1]})),
        ("object.delete", json!({"ids": [a, b]})),
        ("object.set_layer", json!({"ids": [a], "layer": "layer-1"})),
        ("path.convert", json!({"ids": [a]})),
        ("node.move", json!({"id": a, "index": 0, "to": [0, 0]})),
        ("object.align", json!({"ids": [a, b], "x": "left"})),
        ("object.rotate_to", json!({"ids": [a], "pivot": [0, 0], "from": [1, 0], "to": [0, 1]})),
    ] {
        assert!(err(&mut s, cmd, params).contains("locked"), "{cmd}");
    }
    assert_eq!(s.document().objects.len(), 3);
    ok(&mut s, "object.duplicate", json!({"ids": [a]}));
    // A locked object can be the alignment reference.
    ok(&mut s, "object.align", json!({"ids": [b, a], "x": "left"}));
    ok(&mut s, "object.lock", json!({"ids": [a], "locked": false}));
    ok(&mut s, "object.transform", json!({"ids": [a], "translate": [1, 0]}));
    // Two-point rotate and scale.
    let r = ok(&mut s, "object.rotate_to", json!({"ids": [c], "pivot": [0, 0], "from": [10, 0], "to": [0, 10]}));
    assert_eq!(r["angle_deg"], json!(90.0));
    let bc = s.document().object(&c).unwrap().bounds().center();
    assert!(bc.x.abs() < 1e-9 && (bc.y - 10.0).abs() < 1e-9);
    let r = ok(&mut s, "object.scale_to", json!({"ids": [c], "pivot": [0, 0], "from": [0, 10], "to": [0, 20]}));
    assert_eq!(r["factor"], json!(2.0));
    assert!((s.document().object(&c).unwrap().bounds().width() - 4.0).abs() < 1e-9);
    assert!(err(&mut s, "object.scale_to", json!({"ids": [c], "pivot": [0, 0], "from": [0, 0], "to": [0, 20]})).contains("differ"));
    assert!(err(&mut s, "object.rotate_to", json!({"ids": [c], "pivot": [0, 0], "from": [1, 0], "to": [0, 1], "rotate": true})).contains("only"));
}

#[test]
fn make_same_and_transform_extensions() {
    let mut s = session();
    let a = rect(&mut s, 4.0, 2.0, [0.0, 0.0]);
    let b = rect(&mut s, 10.0, 5.0, [20.0, 0.0]);
    let r = ok(&mut s, "object.make_same", json!({"ids": [a, b], "dimension": "width", "keep_aspect": true}));
    assert_eq!(r["reference"], json!(b));
    let ba = s.document().object(&a).unwrap().bounds();
    assert!((ba.width() - 10.0).abs() < 1e-9 && (ba.height() - 5.0).abs() < 1e-9);
    assert!((ba.center().x).abs() < 1e-9, "scaled about its centre");
    s.undo().unwrap();
    // A single locked object is the reference wherever it is in the list.
    ok(&mut s, "object.lock", json!({"ids": [a], "locked": true}));
    let r = ok(&mut s, "object.make_same", json!({"ids": [a, b], "dimension": "both"}));
    assert_eq!(r["reference"], json!(a));
    assert!((s.document().object(&b).unwrap().bounds().width() - 4.0).abs() < 1e-9);
    assert!(err(&mut s, "object.make_same", json!({"ids": [a, b], "dimension": "both", "keep_aspect": true})).contains("keep_aspect"));
    // Shear and mirror across a line.
    let mut s = session();
    let q = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    ok(&mut s, "object.transform", json!({"ids": [q], "shear_deg": [45, 0]}));
    let b = s.document().object(&q).unwrap().bounds();
    assert!((b.width() - 20.0).abs() < 1e-9 && (b.height() - 10.0).abs() < 1e-9, "{b:?}");
    s.undo().unwrap();
    ok(&mut s, "object.transform", json!({"ids": [q], "mirror_line": {"a": [10, 0], "b": [10, 1]}}));
    let b = s.document().object(&q).unwrap().bounds();
    assert!((b.center().x - 20.0).abs() < 1e-9);
    s.undo().unwrap();
    ok(&mut s, "object.transform", json!({"ids": [q], "mirror_line": {"a": [0, 0], "b": [1, 1]}, "translate": [0, 0]}));
    assert!(err(&mut s, "object.transform", json!({"ids": [q], "shear_deg": [90, 0]})).contains("89"));
    assert!(err(&mut s, "object.transform", json!({"ids": [q], "mirror_line": {"a": [1, 1], "b": [1, 1]}})).contains("two different"));
}

#[test]
fn huge_and_degenerate_inputs_are_bounded() {
    let mut s = session();
    let a = rect(&mut s, 1.0, 1.0, [0.0, 0.0]);
    let r = run(&mut s, "art.circular_array", json!({"ids": [a], "count": 4_000_000_000u32, "center": [0, 0]}));
    assert!(r.is_err());
    // Many tiny segments: break apart is capped.
    let mut d = String::from("M0 0");
    for k in 1..=10_050 {
        d.push_str(&format!(" L{} 0", f64::from(k) * 0.001));
    }
    let p = path(&mut s, &d);
    assert!(err(&mut s, "path.break_apart", json!({"ids": [p], "mode": "segments"})).contains("10000"));
    // Zero-area closed shapes enclose nothing.
    let flat = path(&mut s, "M0 0 L10 0 L20 0 Z");
    assert!(err(&mut s, "path.boolean", json!({"ids": [a, flat], "op": "union"})).contains("encloses no area") ||
        run(&mut s, "path.boolean", json!({"ids": [a, flat], "op": "union"})).is_err());
    assert!(run(&mut s, "path.weld", json!({"ids": []})).is_err());
    assert!(run(&mut s, "node.list", json!({"id": "nope"})).is_err());
    assert!(s.document().numeric_problem().is_none());
}

#[test]
fn images_are_refused_by_path_tools() {
    let mut s = session();
    // A 1x1 PNG.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==";
    let r = run(&mut s, "art.import_image", json!({"data_base64": png, "mime": "image/png", "width_mm": 5}));
    let Ok(r) = r else { return };
    let img = ids(&r["created"])[0].clone();
    let sq = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    assert!(err(&mut s, "path.boolean", json!({"ids": [img, sq], "op": "union"})).contains("image"));
    assert!(err(&mut s, "path.offset", json!({"ids": [img], "distance_mm": 1})).contains("image"));
    assert!(err(&mut s, "node.list", json!({"id": img})).contains("image"));
}

/// Found in review: inputs that crashed the process, and no-op edits that left empty undo steps.
#[test]
fn review_crashes_and_no_op_edits_are_refused() {
    let mut s = session();
    // A huge circle used to ask for a 60 GB outline and abort the process.
    assert!(err(&mut s, "shape.circle", json!({"diameter_mm": 1e308})).contains("at most"));
    ok(&mut s, "shape.circle", json!({"diameter_mm": 2e6}));
    s.undo().unwrap();
    // A node dragged absurdly far: the polygon engine refuses it instead of panicking (an abort on wasm).
    let a = rect(&mut s, 10.0, 10.0, [0.0, 0.0]);
    let b = rect(&mut s, 10.0, 10.0, [5.0, 0.0]);
    ok(&mut s, "node.move", json!({"id": a, "index": 0, "to": [1e300, 1e300]}));
    let n = steps(&s);
    for (cmd, p) in [
        ("path.boolean", json!({"ids": [a, b], "op": "union"})),
        ("path.weld", json!({"ids": [a, b]})),
        ("path.offset", json!({"ids": [a], "distance_mm": 1})),
        ("path.cut_shapes", json!({"ids": [a, b]})),
        ("path.cut_shapes", json!({"ids": [b, a]})),
    ] {
        let e = err(&mut s, cmd, p);
        assert!(e.contains("from the origin"), "{cmd}: {e}");
    }
    assert_eq!(steps(&s), n);
    // Locking what is locked (or unlocking what is not) changes nothing and is refused.
    assert!(err(&mut s, "object.lock", json!({"ids": [b], "locked": false})).contains("already unlocked"));
    ok(&mut s, "object.lock", json!({"ids": [b], "locked": true}));
    assert!(err(&mut s, "object.lock", json!({"ids": [b], "locked": true})).contains("already locked"));
    ok(&mut s, "object.lock", json!({"ids": [a, b], "locked": true}));
    // Node and segment types that are already set.
    let c = rect(&mut s, 10.0, 10.0, [40.0, 0.0]);
    let n = steps(&s);
    assert!(err(&mut s, "node.set_type", json!({"id": c, "indices": [0], "type": "corner"})).contains("nothing to change"));
    assert!(err(&mut s, "node.segment_type", json!({"id": c, "segment": 0, "type": "line"})).contains("already a line"));
    assert_eq!(steps(&s), n);
    ok(&mut s, "node.segment_type", json!({"id": c, "segment": 0, "type": "curve"}));
    assert!(err(&mut s, "node.segment_type", json!({"id": c, "segment": 0, "type": "curve"})).contains("already a curve"));
}
