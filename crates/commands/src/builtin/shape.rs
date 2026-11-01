//! Primitive shape commands: `shape.rect`, `shape.ellipse`, `shape.path`.

use doc::{Document, Geometry, Object, ObjectId};
use kurbo::{Affine, BezPath};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// Append a new object and return its id. `layer` must already be resolved. Control characters in `name` (e.g.
/// from an SVG id) become spaces.
pub(super) fn add_object(
    doc: &mut Document,
    name: String,
    layer: String,
    geometry: Geometry,
    transform: Affine,
) -> Result<ObjectId> {
    let id = doc.alloc_id("obj")?;
    let name = if name.contains(char::is_control) {
        name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
    } else {
        name
    };
    doc.objects.push(Object::new(id.clone(), name, layer, geometry, transform));
    Ok(id)
}

/// Translation to `center` then rotation about it, both in the field frame.
fn placement(cmd: &str, center: [f64; 2], rotation_deg: f64) -> Result<Affine> {
    let [x, y] = util::finite2(cmd, "center", center)?;
    let rot = util::finite(cmd, "rotation_deg", rotation_deg)?;
    Ok(Affine::translate((x, y)) * Affine::rotate(rot.to_radians()))
}

fn object_name(cmd: &str, name: Option<&str>, default: &str) -> Result<String> {
    Ok(name.map(|n| util::name(cmd, "name", n)).transpose()?.unwrap_or_else(|| default.to_string()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectInput {
    /// Width in mm (along the rectangle's own X axis, before rotation).
    pub width_mm: f64,
    /// Height in mm (along the rectangle's own Y axis, before rotation).
    pub height_mm: f64,
    /// Corner radius in mm, at most half the shorter side. Default 0.
    #[serde(default)]
    pub corner_radius_mm: f64,
    /// Centre in the field frame, in mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Counter-clockwise rotation about the centre, in degrees. Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Rectangle".
    pub name: Option<String>,
}

/// `shape.rect`.
pub struct ShapeRect;

impl Command for ShapeRect {
    type Input = RectInput;
    const ID: &'static str = "shape.rect";
    const TITLE: &'static str = "Rectangle";
    const DESCRIPTION: &'static str = "Add a rectangle (optionally rounded) of the given size in mm, centred at \
        `center` in the field frame (default [0,0]) and rotated counter-clockwise by `rotation_deg`. Returns the new \
        object id and its field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RectInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            util::positive(id, "width_mm", i.width_mm)?;
            util::positive(id, "height_mm", i.height_mm)?;
            util::non_negative(id, "corner_radius_mm", i.corner_radius_mm)?;
            if i.corner_radius_mm > i.width_mm.min(i.height_mm) / 2.0 {
                return Err(invalid(id, "corner_radius_mm must be at most half the shorter side"));
            }
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = object_name(id, i.name.as_deref(), "Rectangle")?;
            let geometry =
                Geometry::Rect { width_mm: i.width_mm, height_mm: i.height_mm, corner_radius_mm: i.corner_radius_mm };
            let obj = add_object(doc, name, layer, geometry, placement(id, i.center, i.rotation_deg)?)?;
            let summary = format!("added rectangle {obj} {}x{} mm", i.width_mm, i.height_mm);
            Ok((util::edit_result(doc, &[obj], &[]), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EllipseInput {
    /// Radius along the ellipse's own X axis, in mm. Equal radii make a circle.
    pub rx_mm: f64,
    /// Radius along the ellipse's own Y axis, in mm.
    pub ry_mm: f64,
    /// Centre in the field frame, in mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Counter-clockwise rotation about the centre, in degrees. Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Ellipse".
    pub name: Option<String>,
}

/// `shape.ellipse`.
pub struct ShapeEllipse;

impl Command for ShapeEllipse {
    type Input = EllipseInput;
    const ID: &'static str = "shape.ellipse";
    const TITLE: &'static str = "Ellipse";
    const DESCRIPTION: &'static str = "Add an ellipse (or circle, with rx_mm = ry_mm) with radii in mm, centred at \
        `center` in the field frame (default [0,0]) and rotated counter-clockwise by `rotation_deg`. Returns the new \
        object id and its field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: EllipseInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            util::positive(id, "rx_mm", i.rx_mm)?;
            util::positive(id, "ry_mm", i.ry_mm)?;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = object_name(id, i.name.as_deref(), "Ellipse")?;
            let geometry = Geometry::Ellipse { rx_mm: i.rx_mm, ry_mm: i.ry_mm };
            let obj = add_object(doc, name, layer, geometry, placement(id, i.center, i.rotation_deg)?)?;
            let summary = format!("added ellipse {obj} rx {} ry {} mm", i.rx_mm, i.ry_mm);
            Ok((util::edit_result(doc, &[obj], &[]), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathInput {
    /// SVG path data ("M 0 0 L 10 0 L 10 5 Z", curves and arcs allowed). Coordinates are field-frame mm with +Y away
    /// from the operator (y-up), not SVG's y-down pixels.
    pub svg_path_data: String,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Path".
    pub name: Option<String>,
}

/// `shape.path`.
pub struct ShapePath;

impl Command for ShapePath {
    type Input = PathInput;
    const ID: &'static str = "shape.path";
    const TITLE: &'static str = "Path";
    const DESCRIPTION: &'static str = "Add a free-form path from SVG path data whose coordinates are field-frame mm \
        (y-up, origin at the field centre); the path is used exactly where it is drawn. Returns the new object id and \
        its field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PathInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let path = BezPath::from_svg(&i.svg_path_data).map_err(|e| invalid(id, format!("svg_path_data: {e}")))?;
            validate_path(id, &path)?;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = object_name(id, i.name.as_deref(), "Path")?;
            let obj = add_object(doc, name, layer, Geometry::Path { path }, Affine::IDENTITY)?;
            let summary = format!("added path {obj}");
            Ok((util::edit_result(doc, &[obj], &[]), summary))
        })
    }
}

/// A path must draw something and contain only finite coordinates.
pub(super) fn validate_path(cmd: &str, path: &BezPath) -> Result<()> {
    if path.elements().len() < 2 {
        return Err(invalid(cmd, "the path is empty; it needs a move-to and at least one segment"));
    }
    use kurbo::PathEl::*;
    let finite = path.elements().iter().all(|el| match *el {
        MoveTo(p) | LineTo(p) => p.is_finite(),
        QuadTo(a, b) => a.is_finite() && b.is_finite(),
        CurveTo(a, b, c) => a.is_finite() && b.is_finite() && c.is_finite(),
        ClosePath => true,
    });
    if !finite {
        return Err(invalid(cmd, "the path contains non-finite coordinates"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    #[test]
    fn rect_at_center() {
        let mut d = Document::default();
        let (r, _) = run::<ShapeRect>(&mut d, json!({"width_mm": 10, "height_mm": 4, "center": [5, 5]})).unwrap();
        assert_eq!(r["created"], json!(["obj-2"]));
        assert_eq!(r["bounds"]["min"], json!([0.0, 3.0]));
        assert_eq!(r["bounds"]["max"], json!([10.0, 7.0]));
        assert_eq!(d.objects[0].layer, "layer-1");
    }

    #[test]
    fn rotated_ellipse_bounds() {
        let mut d = Document::default();
        let (r, _) = run::<ShapeEllipse>(&mut d, json!({"rx_mm": 10, "ry_mm": 2, "rotation_deg": 90})).unwrap();
        assert_eq!(r["bounds"]["size"], json!([4.0, 20.0]));
    }

    #[test]
    fn path_is_used_as_drawn() {
        let mut d = Document::default();
        let (r, _) =
            run::<ShapePath>(&mut d, json!({"svg_path_data": "M 1 2 L 11 2 L 11 7 Z", "name": "tri"})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([1.0, 2.0]));
        assert_eq!(r["bounds"]["max"], json!([11.0, 7.0]));
        assert!(run::<ShapePath>(&mut d, json!({"svg_path_data": "M 1 2 Q"})).is_err());
        assert!(run::<ShapePath>(&mut d, json!({"svg_path_data": ""})).is_err());
    }

    #[test]
    fn unknown_layer_is_not_found() {
        let mut d = Document::default();
        let e = run::<ShapeRect>(&mut d, json!({"width_mm": 1, "height_mm": 1, "layer": "layer-2"})).unwrap_err();
        assert!(
            matches!(e, crate::CommandError::NotFound { ref candidates, .. } if candidates == &["layer-1"]),
            "{e:?}"
        );
        assert!(d.objects.is_empty());
    }

    #[test]
    fn bad_sizes_are_refused() {
        let mut d = Document::default();
        assert!(run::<ShapeRect>(&mut d, json!({"width_mm": 0, "height_mm": 1})).is_err());
        assert!(run::<ShapeRect>(&mut d, json!({"width_mm": 2, "height_mm": 2, "corner_radius_mm": 1.5})).is_err());
        assert!(run::<ShapeEllipse>(&mut d, json!({"rx_mm": -1, "ry_mm": 1})).is_err());
        assert!(d.objects.is_empty());
    }
}
