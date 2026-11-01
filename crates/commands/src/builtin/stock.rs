//! Stock commands: `stock.define`, `stock.move` and `stock.clear`.

use doc::{Document, Stock, StockShape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Outline of the stock in its own frame (centred on `center`, rotated by `rotation_deg`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StockShapeInput {
    /// A rectangle, optionally with rounded corners.
    Rect {
        /// Width along the stock's own X axis, in mm.
        width_mm: f64,
        /// Height along the stock's own Y axis, in mm.
        height_mm: f64,
        /// Corner radius in mm, at most half the shorter side. Default 0.
        #[serde(default)]
        corner_radius_mm: f64,
    },
    /// A disc, e.g. a round tag or coaster.
    Circle {
        /// Diameter in mm.
        diameter_mm: f64,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StockDefineInput {
    /// Display name, e.g. "dog tag". Default "Stock".
    pub name: Option<String>,
    /// The outline: {"kind": "rect", "width_mm", "height_mm", "corner_radius_mm"?} or {"kind": "circle", "diameter_mm"}.
    pub shape: StockShapeInput,
    /// Material thickness in mm (user-measured; the machine cannot read it).
    pub thickness_mm: f64,
    /// Free text or a material-library key, e.g. "stainless 304".
    pub material: Option<String>,
    /// Centre of the stock in the field frame, in mm. Default [0, 0] (field centre).
    #[serde(default)]
    pub center: [f64; 2],
    /// Rotation of the stock counter-clockwise in degrees. Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
}

/// `stock.define`.
pub struct StockDefine;

impl Command for StockDefine {
    type Input = StockDefineInput;
    const ID: &'static str = "stock.define";
    const TITLE: &'static str = "Define stock";
    const DESCRIPTION: &'static str = "Set (or replace) the physical piece being marked: a rect or circle in mm, its \
        thickness, material, centre in the field frame (default [0,0], the field centre) and rotation in degrees. \
        Returns the stock's field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: StockDefineInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let shape = match input.shape {
                StockShapeInput::Rect { width_mm, height_mm, corner_radius_mm } => {
                    util::positive(id, "shape.width_mm", width_mm)?;
                    util::positive(id, "shape.height_mm", height_mm)?;
                    util::non_negative(id, "shape.corner_radius_mm", corner_radius_mm)?;
                    if corner_radius_mm > width_mm.min(height_mm) / 2.0 {
                        return Err(invalid(id, "shape.corner_radius_mm must be at most half the shorter side"));
                    }
                    StockShape::Rect { width_mm, height_mm, corner_radius_mm }
                }
                StockShapeInput::Circle { diameter_mm } => {
                    util::positive(id, "shape.diameter_mm", diameter_mm)?;
                    StockShape::Circle { diameter_mm }
                }
            };
            let stock = Stock {
                name: input
                    .name
                    .as_deref()
                    .map(|n| util::name(id, "name", n))
                    .transpose()?
                    .unwrap_or_else(|| "Stock".into()),
                shape,
                thickness_mm: util::non_negative(id, "thickness_mm", input.thickness_mm)?,
                material: input.material.map(|m| m.trim().to_string()).filter(|m| !m.is_empty()),
                center: util::finite2(id, "center", input.center)?,
                rotation_deg: util::finite(id, "rotation_deg", input.rotation_deg)?,
            };
            let bounds = util::rect_json(stock.bounds());
            let summary = format!("defined stock \"{}\"", stock.name);
            doc.stock = Some(stock);
            Ok((json!({ "created": [], "affected": [], "stock_bounds": bounds.clone(), "bounds": bounds }), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StockMoveInput {
    /// New centre [x, y] in field mm. Not together with translate.
    pub center: Option<[f64; 2]>,
    /// Move by [dx, dy] mm in the field frame. Not together with center.
    pub translate: Option<[f64; 2]>,
    /// New rotation in degrees counter-clockwise (absolute), about the stock's centre. Not together with
    /// rotate_by_deg.
    pub rotation_deg: Option<f64>,
    /// Rotate by this many degrees counter-clockwise (relative), about the stock's centre. Not together with
    /// rotation_deg.
    pub rotate_by_deg: Option<f64>,
}

/// `stock.move`.
pub struct StockMove;

impl Command for StockMove {
    type Input = StockMoveInput;
    const ID: &'static str = "stock.move";
    const TITLE: &'static str = "Move stock";
    const DESCRIPTION: &'static str = "Move and/or rotate the stock (e.g. to match where the piece sits in a jig):         center [x, y] or translate [dx, dy] in field mm, rotation_deg (absolute) or rotate_by_deg (relative),         counter-clockwise about the stock's centre. Objects are not moved. Fails if no stock is defined. Returns         the stock's field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: StockMoveInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let mut stock =
            doc.stock.clone().ok_or_else(|| CommandError::Failed("no stock is defined; run stock.define first".into()))?;
        if i.center.is_some() && i.translate.is_some() {
            return Err(invalid(id, "give center or translate, not both"));
        }
        if i.rotation_deg.is_some() && i.rotate_by_deg.is_some() {
            return Err(invalid(id, "give rotation_deg or rotate_by_deg, not both"));
        }
        let mut parts = Vec::new();
        if let Some(c) = i.center {
            stock.center = util::finite2(id, "center", c)?;
            parts.push(format!("centre [{}, {}]", util::r4(c[0]), util::r4(c[1])));
        }
        if let Some(t) = i.translate {
            let [dx, dy] = util::finite2(id, "translate", t)?;
            stock.center = [stock.center[0] + dx, stock.center[1] + dy];
            parts.push(format!("move [{}, {}]", util::r4(dx), util::r4(dy)));
        }
        if let Some(r) = i.rotation_deg {
            stock.rotation_deg = util::finite(id, "rotation_deg", r)?;
            parts.push(format!("rotation {}°", util::r4(r)));
        }
        if let Some(r) = i.rotate_by_deg {
            util::finite(id, "rotate_by_deg", r)?;
            // Kept in (-360, 360) so repeated nudges do not grow without bound.
            stock.rotation_deg = (stock.rotation_deg + r) % 360.0;
            parts.push(format!("rotate {}°", util::r4(r)));
        }
        if parts.is_empty() {
            return Err(invalid(id, "nothing to do: give center, translate, rotation_deg or rotate_by_deg"));
        }
        if !stock.center.iter().all(|v| v.is_finite()) {
            return Err(invalid(id, "the stock would move to a position that is not a finite number"));
        }
        let bounds = util::rect_json(stock.bounds());
        let summary = format!("stock \"{}\": {}", stock.name, parts.join(", "));
        doc.stock = Some(stock);
        Ok((json!({ "created": [], "affected": [], "stock_bounds": bounds.clone(), "bounds": bounds }), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StockClearInput {}

/// `stock.clear`.
pub struct StockClear;

impl Command for StockClear {
    type Input = StockClearInput;
    const ID: &'static str = "stock.clear";
    const TITLE: &'static str = "Clear stock";
    const DESCRIPTION: &'static str = "Remove the stock definition. Objects are not moved or deleted.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, _input: StockClearInput) -> Result<(Value, String)> {
        if doc.stock.take().is_none() {
            return Err(CommandError::Failed("there is no stock to clear".into()));
        }
        Ok((json!({ "created": [], "affected": [], "bounds": Value::Null }), "cleared stock".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn define(doc: &mut Document, v: Value) -> Result<(Value, String)> {
        StockDefine::apply(doc, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    #[test]
    fn defines_rect_with_defaults() {
        let mut d = Document::default();
        let (r, _) =
            define(&mut d, json!({"shape": {"kind": "rect", "width_mm": 40, "height_mm": 20}, "thickness_mm": 2}))
                .unwrap();
        let s = d.stock.as_ref().unwrap();
        assert_eq!(s.center, [0.0, 0.0]);
        assert_eq!(s.name, "Stock");
        assert_eq!(r["bounds"]["size"], json!([40.0, 20.0]));
    }

    #[test]
    fn rotated_stock_bounds() {
        let mut d = Document::default();
        let (r, _) = define(
            &mut d,
            json!({"shape": {"kind": "rect", "width_mm": 40, "height_mm": 20}, "thickness_mm": 2, "rotation_deg": 90, "center": [5, 0]}),
        )
        .unwrap();
        assert_eq!(r["bounds"]["min"], json!([-5.0, -20.0]));
        assert_eq!(r["bounds"]["max"], json!([15.0, 20.0]));
    }

    #[test]
    fn refuses_bad_shapes() {
        let mut d = Document::default();
        assert!(define(&mut d, json!({"shape": {"kind": "circle", "diameter_mm": 0}, "thickness_mm": 1})).is_err());
        assert!(define(&mut d, json!({"shape": {"kind": "rect", "width_mm": 10, "height_mm": 10, "corner_radius_mm": 6}, "thickness_mm": 1})).is_err());
        assert!(define(&mut d, json!({"shape": {"kind": "hexagon"}, "thickness_mm": 1})).is_err());
        assert!(d.stock.is_none());
    }

    #[test]
    fn move_stock_by_centre_translate_and_rotation() {
        let mut d = Document::default();
        let mv = |d: &mut Document, v: Value| {
            StockMove::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
        };
        assert!(mv(&mut d, json!({"translate": [1, 0]})).is_err(), "no stock");
        define(&mut d, json!({"shape": {"kind": "rect", "width_mm": 40, "height_mm": 20}, "thickness_mm": 2})).unwrap();
        mv(&mut d, json!({"center": [10, 5]})).unwrap();
        let (r, _) = mv(&mut d, json!({"translate": [1, -1], "rotation_deg": 90})).unwrap();
        let s = d.stock.as_ref().unwrap();
        assert_eq!(s.center, [11.0, 4.0]);
        assert_eq!(s.rotation_deg, 90.0);
        assert_eq!(r["bounds"]["min"], json!([1.0, -16.0]));
        mv(&mut d, json!({"rotate_by_deg": 300})).unwrap();
        assert_eq!(d.stock.as_ref().unwrap().rotation_deg, 30.0);
        assert!(mv(&mut d, json!({})).is_err());
        assert!(mv(&mut d, json!({"center": [0, 0], "translate": [1, 1]})).is_err());
        assert!(mv(&mut d, json!({"rotation_deg": 1, "rotate_by_deg": 1})).is_err());
        assert!(mv(&mut d, json!({"translate": [1e308, 0]})).is_ok());
        assert!(mv(&mut d, json!({"translate": [1e308, 0]})).is_err(), "overflow to infinity");
        assert_eq!(d.stock.as_ref().unwrap().rotation_deg, 30.0, "a failed move changes nothing");
    }

    #[test]
    fn clear_requires_stock() {
        let mut d = Document::default();
        assert!(StockClear::apply(&mut d, StockClearInput {}).is_err());
        define(&mut d, json!({"shape": {"kind": "circle", "diameter_mm": 30}, "thickness_mm": 1})).unwrap();
        StockClear::apply(&mut d, StockClearInput {}).unwrap();
        assert!(d.stock.is_none());
    }
}
