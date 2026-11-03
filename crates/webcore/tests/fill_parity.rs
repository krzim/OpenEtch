//! Fill Region hover in the browser (`webcore::fill`) answers exactly as the engine's `path.fill_region_preview`
//! command, on a ghost-like drawing (an outline with eyes and pupils, a hidden layer, an open line, text and an
//! image), for a grid of points with and without a gap tolerance and holes; and it refuses what only the engine takes.

use commands::{Invocation, Origin, Session};
use doc::Document;
use serde_json::{Value, json};
use webcore::fill;

fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, String> {
    s.execute(Invocation { id: id.into(), params }, Origin::Agent("test".into())).map(|o| o.result).map_err(|e| e.to_string())
}

fn ok(s: &mut Session, id: &str, params: Value) -> Value {
    run(s, id, params).unwrap_or_else(|e| panic!("{id} failed: {e}"))
}

fn layer(s: &mut Session, name: &str) -> String {
    ok(s, "layer.add", json!({ "name": name }))["created"][0].as_str().unwrap().to_string()
}

fn drawing() -> Session {
    let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
    let body = layer(&mut s, "Body");
    let eyes = layer(&mut s, "Eyes");
    let hidden = layer(&mut s, "Hidden");
    ok(&mut s, "shape.rect", json!({ "width_mm": 40, "height_mm": 40, "center": [0, 0], "layer": body }));
    ok(&mut s, "shape.ellipse", json!({ "rx_mm": 4, "ry_mm": 3, "center": [-6, 5], "layer": eyes }));
    ok(&mut s, "shape.ellipse", json!({ "rx_mm": 1.2, "ry_mm": 1.2, "center": [-5, 5], "layer": eyes }));
    ok(&mut s, "shape.ellipse", json!({ "rx_mm": 4, "ry_mm": 3, "center": [7, 5], "layer": eyes }));
    // A mouth with a 0.4 mm opening, and a line across the bottom.
    ok(&mut s, "shape.path", json!({ "svg_path_data": "M -8 -8 L 8 -8 L 8 -4 L -8 -4 L -8 -7.6", "layer": eyes }));
    ok(&mut s, "shape.line", json!({ "points": [[-20, -14], [20, -14]] }));
    // Hidden layers bound nothing.
    ok(&mut s, "shape.rect", json!({ "width_mm": 6, "height_mm": 6, "center": [10, -9], "layer": hidden }));
    ok(&mut s, "layer.update", json!({ "id": hidden, "visible": false }));
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    ok(&mut s, "shape.text", json!({ "text": "OB", "font": font, "height_mm": 8, "center": [0, 14] }));
    s
}

fn webcore_answer(doc: &Document, cache: &mut fill::Cache, params: &Value) -> Result<Value, String> {
    let s = serde_json::to_string(doc).unwrap();
    let d: Document = serde_json::from_str(&s).unwrap();
    fill::preview(&d, cache, params)
}

#[test]
fn hover_answers_equal_the_engine_command() {
    let mut s = drawing();
    let doc = s.document().clone();
    let mut cache = fill::Cache::default();
    let mut faces = 0;
    for opts in [json!({}), json!({ "gap_mm": 0.6 }), json!({ "gap_mm": 2, "holes": false })] {
        for iy in -5..=5 {
            for ix in -6..=6 {
                let p = [f64::from(ix) * 3.7 + 0.13, f64::from(iy) * 3.9 + 0.07];
                let mut params = opts.clone();
                params["points"] = json!([p]);
                let engine = run(&mut s, "path.fill_region_preview", params.clone());
                let browser = webcore_answer(&doc, &mut cache, &params);
                assert_eq!(browser, engine, "point {p:?} with {opts}");
                if let Ok(v) = &engine {
                    faces += v["faces"].as_array().map_or(0, Vec::len);
                }
            }
        }
        // Several points in one call, some in the same face.
        let mut params = opts.clone();
        params["points"] = json!([[-6.2, 5.1], [-5.0, 5.0], [7.0, 5.0], [7.5, 5.5], [0.5, 0.5], [30, 30]]);
        assert_eq!(webcore_answer(&doc, &mut cache, &params), run(&mut s, "path.fill_region_preview", params.clone()));
    }
    assert!(faces > 40, "the grid finds areas: {faces}");
}

#[test]
fn bad_input_and_engine_only_options() {
    let mut s = drawing();
    let doc = s.document().clone();
    let mut cache = fill::Cache::default();
    for bad in [json!({ "points": [] }), json!({ "points": [[0, 0]], "gap_mm": 50 }), json!({ "points": [[0, 0]], "colour": 1 })] {
        let e = run(&mut s, "path.fill_region_preview", bad.clone()).unwrap_err();
        let b = webcore_answer(&doc, &mut cache, &bad).unwrap_err();
        assert!(b.starts_with("invalid input for 'path.fill_region_preview'"), "{b}");
        assert!(e.starts_with("invalid input for 'path.fill_region_preview'"), "{e}");
    }
    for engine_only in [json!({ "points": [[0, 0]], "boundary_layers": ["layer-1"] }), json!({ "points": [[0, 0]], "open_paths": false })] {
        assert_eq!(webcore_answer(&doc, &mut cache, &engine_only).unwrap_err(), fill::UNSUPPORTED);
    }
}
