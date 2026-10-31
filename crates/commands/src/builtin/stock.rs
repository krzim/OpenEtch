//! Stock commands: `stock.define` and `stock.clear`.

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
    fn clear_requires_stock() {
        let mut d = Document::default();
        assert!(StockClear::apply(&mut d, StockClearInput {}).is_err());
        define(&mut d, json!({"shape": {"kind": "circle", "diameter_mm": 30}, "thickness_mm": 1})).unwrap();
        StockClear::apply(&mut d, StockClearInput {}).unwrap();
        assert!(d.stock.is_none());
    }
}
