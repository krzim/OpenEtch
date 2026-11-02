//! `path.fill_region` and `path.fill_region_preview` through [`Session`]: one undo step for many points, errors that
//! change nothing, boundary choices, and the preview reading only.

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

fn err(s: &mut Session, id: &str, params: Value) -> String {
    match run(s, id, params.clone()) {
        Ok(v) => panic!("{id} {params} should fail, got {v}"),
        Err(e) => e.to_string(),
    }
}

fn created(v: &Value) -> Vec<String> {
    serde_json::from_value(v["created"].clone()).unwrap()
}

fn layer(s: &mut Session, name: &str) -> String {
    ok(s, "layer.add", json!({"name": name}))["created"][0].as_str().unwrap().to_string()
}

/// A ghost-like eye: an elliptical outline with a round pupil in it, on separate layers, inside a body outline.
fn ghost(s: &mut Session) -> (String, String, String) {
    let body_layer = layer(s, "Body");
    let eye_layer = layer(s, "Eyes");
    let frost = layer(s, "Frost");
    ok(s, "shape.rect", json!({"width_mm": 40, "height_mm": 40, "center": [0, 0], "layer": body_layer}));
    let eye = ok(s, "shape.ellipse", json!({"rx_mm": 4, "ry_mm": 3, "center": [-6, 5], "layer": eye_layer}))["created"][0].clone();
    let pupil = ok(s, "shape.ellipse", json!({"rx_mm": 1.2, "ry_mm": 1.2, "center": [-5, 5], "layer": eye_layer}))["created"][0].clone();
    (frost, eye.as_str().unwrap().into(), pupil.as_str().unwrap().into())
}

fn region_area(s: &Session, id: &str) -> f64 {
    let o = s.document().object(id).unwrap();
    geom::Region::from_path(&o.field_path(), geom::Fill::EvenOdd).unwrap().area()
}

#[test]
fn eye_white_is_a_ring_on_the_chosen_layer() {
    let mut s = session();
    let (frost, _, _) = ghost(&mut s);
    let n = s.history().len();
    let objects = s.document().objects.len();
    let r = ok(&mut s, "path.fill_region", json!({"points": [[-8.5, 5]], "layer": frost}));
    assert_eq!(s.history().len(), n + 1);
    let ids = created(&r);
    assert_eq!(ids.len(), 1);
    let o = s.document().object(&ids[0]).unwrap();
    assert_eq!(o.layer, frost);
    assert_eq!(o.name, "Fill region");
    let expect = std::f64::consts::PI * (12.0 - 1.44);
    assert!((region_area(&s, &ids[0]) - expect).abs() < 0.3, "{}", region_area(&s, &ids[0]));
    assert_eq!(r["faces"][0]["holes"], json!(1));
    assert_eq!(r["faces"][0]["points"], json!([0]));
    s.undo().unwrap();
    assert_eq!(s.document().objects.len(), objects);
    s.redo().unwrap();
    assert_eq!(s.document().objects.len(), objects + 1);
}

#[test]
fn several_points_are_one_step_and_share_faces() {
    let mut s = session();
    let (frost, _, _) = ghost(&mut s);
    let n = s.history().len();
    let r = ok(
        &mut s,
        "path.fill_region",
        json!({"points": [[-8.5, 5], [-5, 5], [-3, 3.5], [10, -10]], "layer": frost, "name": "Frost fill", "holes": false}),
    );
    assert_eq!(s.history().len(), n + 1, "one undo step");
    let ids = created(&r);
    assert_eq!(ids.len(), 3, "eye white (twice), pupil, body: {r}");
    assert_eq!(r["faces"][0]["points"], json!([0, 2]));
    assert_eq!(r["faces"][0]["holes"], json!(0), "holes: false fills over the pupil");
    assert_eq!(s.document().object(&ids[1]).unwrap().name, "Frost fill");
    s.undo().unwrap();
    assert!(ids.iter().all(|id| s.document().object(id).is_none()));
}

#[test]
fn a_point_outside_fails_and_changes_nothing() {
    let mut s = session();
    let (frost, _, _) = ghost(&mut s);
    let n = s.history().len();
    let rev = s.document().revision;
    let e = err(&mut s, "path.fill_region", json!({"points": [[-8.5, 5], [60, 60]], "layer": frost}));
    assert!(e.contains("point 2 (60, 60)") && e.contains("no closed area"), "{e}");
    assert!(!e.contains("point 1"), "{e}");
    assert_eq!(s.history().len(), n);
    assert_eq!(s.document().revision, rev);
    // On a line.
    let e = err(&mut s, "path.fill_region", json!({"points": [[20, 0]]}));
    assert!(e.contains("on a line"), "{e}");
    // Bad input.
    assert!(err(&mut s, "path.fill_region", json!({"points": []})).contains("at least one"));
    assert!(err(&mut s, "path.fill_region", json!({"points": [[0, 0]], "gap_mm": -1})).contains("gap_mm"));
    assert!(err(&mut s, "path.fill_region", json!({"points": [[0, 0]], "gap_mm": 50})).contains("at most"));
    assert!(err(&mut s, "path.fill_region", json!({"points": [[0, 0]], "layer": "nope"})).contains("nope"));
    assert!(err(&mut s, "path.fill_region", json!({"points": [[0, 0]], "boundary_ids": ["a"], "boundary_layers": ["b"]})).contains("not both"));
    assert!(err(&mut s, "path.fill_region", json!({"points": [[0, 0]], "colour": 1})).contains("unknown field"));
}

#[test]
fn boundary_choices_hidden_layers_open_paths_and_images() {
    let mut s = session();
    let (frost, eye, pupil) = ghost(&mut s);
    let eyes = s.document().object(&eye).unwrap().layer.clone();
    // Hide the eyes: the click inside the eye now fills the body.
    ok(&mut s, "layer.update", json!({"id": eyes, "visible": false}));
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[-8.5, 5]]}));
    assert!((r["faces"][0]["area_mm2"].as_f64().unwrap() - 1600.0).abs() < 1e-3, "{r}");
    // Listed ids count even on a hidden layer.
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[-8.5, 5]], "boundary_ids": [eye, pupil]}));
    assert_eq!(r["faces"][0]["contours"].as_array().unwrap().len(), 2, "{r}");
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[-8.5, 5]], "boundary_layers": [eyes]}));
    assert_eq!(r["faces"][0]["contours"].as_array().unwrap().len(), 2, "{r}");
    // An open line across the body splits it unless open paths are left out.
    ok(&mut s, "shape.line", json!({"points": [[-20, -10], [20, -10]], "layer": frost}));
    let split = ok(&mut s, "path.fill_region_preview", json!({"points": [[0, -15]]}));
    assert!((split["faces"][0]["area_mm2"].as_f64().unwrap() - 400.0).abs() < 1e-3, "{split}");
    let whole = ok(&mut s, "path.fill_region_preview", json!({"points": [[0, -15]], "open_paths": false}));
    assert!((whole["faces"][0]["area_mm2"].as_f64().unwrap() - 1600.0).abs() < 1e-3, "{whole}");
    // An image is never a boundary.
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    ok(&mut s, "art.import_image", json!({"data_base64": png, "mime": "image/png", "width_mm": 5}));
    let img = s.document().objects.last().unwrap().bounds();
    assert!(img.contains(kurbo::Point::new(1.0, 1.0)), "{img:?}");
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[1, 1]]}));
    assert!((r["faces"][0]["area_mm2"].as_f64().unwrap() - 1200.0).abs() < 1e-3, "{r}");
}

#[test]
fn preview_reads_only_and_reports_each_point() {
    let mut s = session();
    let _ = ghost(&mut s);
    let n = s.history().len();
    let rev = s.document().revision;
    let objects = s.document().objects.len();
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[-8.5, 5], [60, 60], [-8.4, 5.1]]}));
    assert_eq!(s.history().len(), n);
    assert_eq!(s.document().revision, rev);
    assert_eq!(s.document().objects.len(), objects);
    assert_eq!(r["faces"].as_array().unwrap().len(), 1);
    assert_eq!(r["points"][0]["face"], json!(0));
    assert_eq!(r["points"][1]["face"], Value::Null);
    assert_eq!(r["points"][1]["code"], json!("not_enclosed"));
    assert_eq!(r["points"][2]["face"], json!(0));
    assert_eq!(r["boundary_objects"], json!(3));
    let c = &r["faces"][0]["contours"];
    assert_eq!(c.as_array().unwrap().len(), 2, "outer and the pupil's hole");
    assert!(r["faces"][0]["bounds"]["size"][0].as_f64().unwrap() > 7.9);
}

#[test]
fn gap_tolerance_closes_a_small_opening() {
    let mut s = session();
    ok(&mut s, "shape.path", json!({"svg_path_data": "M 10 9.8 L 10 0 L 0 0 L 0 10 L 10 10"}));
    let open = ok(&mut s, "path.fill_region_preview", json!({"points": [[5, 5]]}));
    assert_eq!(open["points"][0]["code"], json!("not_enclosed"));
    let closed = ok(&mut s, "path.fill_region", json!({"points": [[5, 5]], "gap_mm": 0.3}));
    let a = closed["faces"][0]["area_mm2"].as_f64().unwrap();
    assert!((a - 100.0).abs() < 0.1, "{a}");
}

#[test]
fn transforms_and_groups_are_followed() {
    let mut s = session();
    let a = ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [0, 0]}))["created"][0].clone();
    let b = ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4, "center": [0, 0]}))["created"][0].clone();
    ok(&mut s, "object.transform", json!({"ids": [a], "scale": [2, 1]}));
    let g = ok(&mut s, "group.create", json!({"ids": [a, b]}))["group"].clone();
    let r = ok(&mut s, "path.fill_region_preview", json!({"points": [[8, 0]], "boundary_ids": [g]}));
    let area = r["faces"][0]["area_mm2"].as_f64().unwrap();
    assert!((area - (200.0 - 16.0)).abs() < 1e-3, "{r}");
}
