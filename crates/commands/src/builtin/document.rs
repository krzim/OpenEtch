//! Whole-document commands: `doc.inspect` and `doc.rename`.

use doc::{Document, Geometry, StockShape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::region::{self, Containment};
use super::util::{self, r4};
use crate::{Command, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectInput {}

/// `doc.inspect`.
pub struct Inspect;

impl Command for Inspect {
    type Input = InspectInput;
    const ID: &'static str = "doc.inspect";
    const TITLE: &'static str = "Inspect document";
    const DESCRIPTION: &'static str = "Structured summary of the document: name, revision, stock (shape, size, \
        material, field-frame centre and bounds), layers with full cut settings, objects with layer, kind and \
        field-frame bounds in mm, and warnings (objects outside the stock, empty layers, objects that will not run). \
        Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _input: InspectInput) -> Result<(Value, String)> {
        Ok((inspect(doc), String::new()))
    }
}

fn inspect(doc: &Document) -> Value {
    let stock = doc.stock.as_ref().map(|s| {
        let (shape, size) = match s.shape {
            StockShape::Rect { width_mm, height_mm, corner_radius_mm } => (
                json!({"kind": "rect", "width_mm": width_mm, "height_mm": height_mm, "corner_radius_mm": corner_radius_mm}),
                json!([width_mm, height_mm]),
            ),
            StockShape::Circle { diameter_mm } => (json!({"kind": "circle", "diameter_mm": diameter_mm}), json!([diameter_mm, diameter_mm])),
        };
        json!({
            "name": s.name,
            "shape": shape,
            "size_mm": size,
            "thickness_mm": s.thickness_mm,
            "material": s.material,
            "center": [r4(s.center[0]), r4(s.center[1])],
            "rotation_deg": s.rotation_deg,
            "bounds": util::rect_json(s.bounds()),
        })
    });

    let layers: Vec<Value> = doc
        .layers
        .iter()
        .map(|l| {
            json!({
                "id": l.id,
                "name": l.name,
                "color": l.color,
                "visible": l.visible,
                "output": l.output,
                "objects": doc.objects.iter().filter(|o| o.layer == l.id).count(),
                "settings": serde_json::to_value(&l.settings).unwrap_or(Value::Null),
                "sub_layers": serde_json::to_value(&l.sub_layers).unwrap_or(Value::Null),
            })
        })
        .collect();

    let objects: Vec<Value> = doc
        .objects
        .iter()
        .map(|o| {
            let b = o.bounds();
            let kind = match o.geometry {
                Geometry::Rect { .. } => "rect",
                Geometry::Ellipse { .. } => "ellipse",
                Geometry::Path { .. } => "path",
            };
            json!({
                "id": o.id,
                "name": o.name,
                "layer": o.layer,
                "kind": kind,
                "bounds": util::rect_json(b),
                "size_mm": [r4(b.width()), r4(b.height())],
            })
        })
        .collect();

    json!({
        "name": doc.name,
        "revision": doc.revision,
        "frame": "field frame, mm: origin at the galvo field centre, +X right, +Y away from the operator",
        "stock": stock,
        "layers": layers,
        "objects": objects,
        "bounds": doc.bounds_of(&doc.objects.iter().map(|o| o.id.clone()).collect::<Vec<_>>()).map(util::rect_json),
        "warnings": warnings(doc),
    })
}

/// Problems a human or agent should look at before running a job.
fn warnings(doc: &Document) -> Vec<String> {
    let mut w = Vec::new();
    if doc.objects.is_empty() {
        w.push("the document has no objects".to_string());
    }
    for o in &doc.objects {
        match doc.layer(&o.layer) {
            None => w.push(format!("object {} (\"{}\") refers to missing layer {}", o.id, o.name, o.layer)),
            Some(l) if !l.output => w.push(format!(
                "object {} (\"{}\") is on layer {} which has output off and will not be marked",
                o.id, o.name, l.id
            )),
            Some(l) if !l.visible => w.push(format!(
                "object {} (\"{}\") is on hidden layer {} (it will still be marked)",
                o.id, o.name, l.id
            )),
            Some(_) => {}
        }
        if let Some(s) = &doc.stock {
            match region::containment(s, &o.field_path()) {
                Containment::Inside => {}
                Containment::Partly => w.push(format!("object {} (\"{}\") is partly outside the stock", o.id, o.name)),
                Containment::Outside => {
                    w.push(format!("object {} (\"{}\") is entirely outside the stock", o.id, o.name))
                }
            }
        }
    }
    for l in &doc.layers {
        if !doc.objects.iter().any(|o| o.layer == l.id) {
            w.push(format!("layer {} (\"{}\") has no objects", l.id, l.name));
        }
    }
    w
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameInput {
    /// New document name.
    pub name: String,
}

/// `doc.rename`.
pub struct Rename;

impl Command for Rename {
    type Input = RenameInput;
    const ID: &'static str = "doc.rename";
    const TITLE: &'static str = "Rename document";
    const DESCRIPTION: &'static str = "Set the document's display name.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: RenameInput) -> Result<(Value, String)> {
        let name = util::name(Self::ID, "name", &input.name)?;
        let summary = format!("renamed document to \"{name}\"");
        doc.name = name;
        Ok((json!({ "created": [], "affected": [], "name": doc.name, "bounds": Value::Null }), summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Object, Stock};
    use kurbo::Affine;

    #[test]
    fn inspect_reports_warnings() {
        let mut d = Document {
            stock: Some(Stock {
                name: "plate".into(),
                shape: StockShape::Rect { width_mm: 20.0, height_mm: 20.0, corner_radius_mm: 0.0 },
                thickness_mm: 1.0,
                material: None,
                center: [0.0, 0.0],
                rotation_deg: 0.0,
            }),
            ..Document::default()
        };
        let mut l2 = doc::Layer::new("layer-9".into(), "Off", "#000");
        l2.output = false;
        d.layers.push(l2);
        d.layers.push(doc::Layer::new("layer-10".into(), "Empty", "#000"));
        let mk = |id: &str, layer: &str, x: f64| Object {
            id: id.into(),
            name: id.into(),
            layer: layer.into(),
            geometry: Geometry::Rect { width_mm: 4.0, height_mm: 4.0, corner_radius_mm: 0.0 },
            transform: Affine::translate((x, 0.0)),
        };
        d.objects = vec![mk("obj-a", "layer-1", 0.0), mk("obj-b", "layer-1", 10.0), mk("obj-c", "layer-9", 50.0)];
        let v = inspect(&d);
        let w: Vec<String> = serde_json::from_value(v["warnings"].clone()).unwrap();
        assert!(w.iter().any(|s| s.contains("obj-b") && s.contains("partly")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("obj-c") && s.contains("entirely")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("obj-c") && s.contains("output off")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("layer-10") && s.contains("no objects")), "{w:?}");
        assert!(!w.iter().any(|s| s.contains("obj-a")), "{w:?}");
        assert_eq!(v["objects"][1]["bounds"]["min"], json!([8.0, -2.0]));
        assert_eq!(v["layers"][0]["settings"]["power_pct"], json!(20.0));
        assert_eq!(v["stock"]["size_mm"], json!([20.0, 20.0]));
    }
}
