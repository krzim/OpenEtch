//! Hostile and degenerate inputs for the design and construction commands (`design`, `construct`, and the design
//! options of `path.optimize`, `path.fillet`, `object.transform`, `art.nest`): each either works or fails with an
//! error, never panics, and the expensive ones stay bounded.

use std::time::{Duration, Instant};

use doc::Document;
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

fn created(v: &Value) -> String {
    v["created"][0].as_str().unwrap().to_string()
}

fn path(s: &mut Session, d: &str) -> String {
    created(&ok(s, "shape.path", json!({"svg_path_data": d})))
}

/// Run and accept success or an error; a panic fails the test.
fn either(s: &mut Session, id: &str, params: Value) {
    let _ = run(s, id, params);
}

#[test]
fn true_shape_nest_with_absurd_gaps_returns_quickly() {
    let mut s = session();
    let a = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})));
    let b = created(&ok(&mut s, "shape.ellipse", json!({"rx_mm": 4, "ry_mm": 4, "center": [30, 0]})));
    let t = Instant::now();
    // A gap of 1e300 mm used to overflow the disc stamping (a panic in debug builds) or loop for ever.
    let r = ok(
        &mut s,
        "art.nest",
        json!({"ids": [a, b], "region": "field", "mode": "shapes", "spacing_mm": 1e300, "rotations": [0, 90]}),
    );
    assert_eq!(r["placed"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["unplaced"].as_array().unwrap().len(), 1, "{r}");
    let e = run(&mut s, "art.nest", json!({"ids": [a, b], "region": "field", "mode": "shapes", "edge_mm": 1e300}));
    assert!(e.unwrap_err().to_string().contains("no part fits"));
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
}

#[test]
fn warp_limits_segments_over_the_whole_selection() {
    let mut s = session();
    // Twenty zigzags of 40 lines; a wild grid at the finest tolerance splits every line about a thousand times, so
    // each object stays under the per-path limit while the selection as a whole would not.
    let mut ids = Vec::new();
    for k in 0..20 {
        let mut d = format!("M -50 {}", k * 5 - 30);
        for i in 1..=40 {
            d.push_str(&format!(" L {} {}", -50 + i * 2, k * 5 - 30 + (i % 2) * 3));
        }
        ids.push(path(&mut s, &d));
    }
    let b = s.document().bounds_of(&ids).unwrap();
    let mut grid = Vec::new();
    for r in 0..4 {
        let mut row = Vec::new();
        for c in 0..4 {
            let (x, y) = (b.x0 + b.width() * c as f64 / 3.0, b.y0 + b.height() * r as f64 / 3.0);
            let wild = if (r + c) % 2 == 0 { 1e8 } else { -1e8 };
            row.push(json!([x + wild, y - wild]));
        }
        grid.push(json!(row));
    }
    let rev = s.document().revision;
    let e = run(&mut s, "path.warp", json!({"ids": ids, "mode": "grid", "grid": grid, "tolerance_mm": 0.0005}));
    let msg = e.unwrap_err().to_string();
    assert!(msg.contains("warped selection"), "the whole-selection limit, not one path's: {msg}");
    assert_eq!(s.document().revision, rev, "nothing changed");
}

#[test]
fn degenerate_geometry_never_panics() {
    let mut s = session();
    let cusp = path(&mut s, "M 0 0 C 10 0 0 0 10 0");
    let back = path(&mut s, "M 0 0 L 5 0 L 0 0 L 5 0 Z");
    let lens = path(&mut s, "M 0 0 C 5 5 15 5 20 0 C 15 -5 5 -5 0 0 Z");
    let tiny = path(&mut s, "M 0 0 L 0.000001 0 L 0.000001 0.000001");
    let line = path(&mut s, "M -10 3 L 10 3");
    let spiral = {
        let mut d = String::from("M 0 0");
        for i in 1..2000 {
            let a = i as f64 * 0.05;
            d.push_str(&format!(" L {:.4} {:.4}", a.cos() * a, a.sin() * a));
        }
        path(&mut s, &d)
    };
    for id in [&cusp, &back, &lens, &tiny, &line, &spiral] {
        for r in [0.5, -0.5, 1e6, 1e-9] {
            either(&mut s, "path.fillet", json!({"ids": [id], "radius_mm": r, "corners": "all"}));
        }
        for opt in [json!({"smooth_mm": 10}), json!({"arcs_mm": 10}), json!({"smooth_mm": 0}), json!({"arcs_mm": 0.0001})] {
            let mut p = opt.as_object().unwrap().clone();
            p.insert("ids".into(), json!([id]));
            either(&mut s, "path.optimize", Value::Object(p));
        }
        either(&mut s, "node.extend", json!({"id": id, "at": [0, 0], "pick_mm": 1e6}));
        either(&mut s, "node.extend", json!({"id": id, "at": [10, 0], "pick_mm": 1e6, "max_mm": 1e300}));
        either(&mut s, "art.rubber_band", json!({"ids": [id]}));
        either(&mut s, "select.contained", json!({"ids": [id]}));
        let corners = json!([[0, 0], [0, 0], [0, 0], [0, 0]]);
        either(&mut s, "path.warp", json!({"ids": [id], "mode": "envelope", "corners": corners}));
        either(&mut s, "path.warp", json!({"ids": [id], "mode": "perspective", "corners": corners}));
    }
    // A horizontal line has no height to warp by; refused, not divided by zero.
    let e = run(&mut s, "path.warp", json!({"ids": [line], "mode": "envelope", "corners": [[0, 0], [1, 0], [1, 1], [0, 1]]}));
    assert!(e.is_err());
    // Three identical, three concentric and three collinear equal circles.
    let circle = |s: &mut Session, r: f64, c: [f64; 2]| {
        created(&ok(s, "shape.ellipse", json!({"rx_mm": r, "ry_mm": r, "center": c})))
    };
    let same = [circle(&mut s, 3.0, [0.0, 0.0]), circle(&mut s, 3.0, [0.0, 0.0]), circle(&mut s, 3.0, [0.0, 0.0])];
    either(&mut s, "art.tangent_circle", json!({"ids": same}));
    let conc = [circle(&mut s, 1.0, [9.0, 9.0]), circle(&mut s, 2.0, [9.0, 9.0]), circle(&mut s, 3.0, [9.0, 9.0])];
    either(&mut s, "art.tangent_circle", json!({"ids": conc}));
    let row = [circle(&mut s, 1.0, [0.0, 20.0]), circle(&mut s, 1.0, [5.0, 20.0]), circle(&mut s, 1.0, [10.0, 20.0])];
    let r = ok(&mut s, "art.tangent_circle_candidates", json!({"ids": row}));
    for c in r["candidates"].as_array().unwrap() {
        assert!(c["radius_mm"].as_f64().unwrap().is_finite(), "{r}");
    }
    let tiny_far = [circle(&mut s, 1e-6, [0.0, 0.0]), circle(&mut s, 1e-6, [1e5, 0.0]), circle(&mut s, 1e-6, [0.0, 1e5])];
    either(&mut s, "art.tangent_circle", json!({"ids": tiny_far}));
    // Extreme polygons.
    let p = created(&ok(&mut s, "shape.polygon", json!({"sides": 1000, "radius_mm": 1e6, "parametric": true})));
    either(&mut s, "shape.update", json!({"id": p, "star_ratio": 1e-12, "corner_radius_mm": 1e12}));
    either(&mut s, "path.fillet", json!({"ids": [p], "radius_mm": 1, "corners": "all"}));
    // The document still saves and loads.
    let saved = ok(&mut s, "doc.save", json!({}));
    ok(&mut s, "doc.load", json!({"content": saved["content"]}));
}

#[test]
fn page_position_margins_and_missing_references() {
    let mut s = session();
    let a = created(&ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})));
    for m in [1e300, -1.0, f64::MAX] {
        assert!(run(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "top_left", "margin_mm": m})).is_err());
    }
    assert!(run(&mut s, "object.move_to_page_position", json!({"ids": [a], "anchor": "center", "reference": "stock"})).is_err());
    // A selection larger than the page still moves (centred or flush), as LightBurn does.
    let big = created(&ok(&mut s, "shape.rect", json!({"width_mm": 400, "height_mm": 10})));
    ok(&mut s, "object.move_to_page_position", json!({"ids": [big], "anchor": "left"}));
    let b = s.document().object(&big).unwrap().bounds();
    assert!((b.x0 + 75.0).abs() < 1e-9, "{b:?}");
}
