//! `art.array`: rectangular grid of copies.

use doc::{Document, ObjectId};
use kurbo::Vec2;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::copy_objects;
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// Upper bound on cells, to keep a typo from creating millions of objects.
const MAX_CELLS: u32 = 10_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayInput {
    /// Exact object ids, copied together as one group per cell.
    pub ids: Vec<ObjectId>,
    /// Number of rows (along field Y), >= 1.
    pub rows: u32,
    /// Number of columns (along field X), >= 1.
    pub cols: u32,
    /// Distance between columns in mm along field +X (negative builds toward -X).
    pub pitch_x_mm: f64,
    /// Distance between rows in mm along field +Y, away from the operator (negative builds toward the operator).
    pub pitch_y_mm: f64,
}

/// `art.array`.
pub struct Array;

impl Command for Array {
    type Input = ArrayInput;
    const ID: &'static str = "art.array";
    const TITLE: &'static str = "Grid array";
    const DESCRIPTION: &'static str = "Copy objects into a rows x cols grid. The originals stay as row 0, column 0; \
        the copy at (row r, column c) is offset by [c * pitch_x_mm, r * pitch_y_mm] in the field frame (mm, +Y away \
        from the operator). Returns the created ids per cell and the bounds of the whole grid.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArrayInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let ids = util::object_ids(id, doc, &i.ids)?;
            if i.rows == 0 || i.cols == 0 {
                return Err(invalid(id, "rows and cols must be >= 1"));
            }
            if i.rows.saturating_mul(i.cols) > MAX_CELLS {
                return Err(invalid(id, format!("rows x cols must be at most {MAX_CELLS}")));
            }
            let px = util::finite(id, "pitch_x_mm", i.pitch_x_mm)?;
            let py = util::finite(id, "pitch_y_mm", i.pitch_y_mm)?;
            if (i.cols > 1 && px == 0.0) || (i.rows > 1 && py == 0.0) {
                return Err(invalid(id, "a zero pitch would stack copies on top of each other"));
            }
            let mut created = Vec::new();
            let mut cells = Vec::new();
            for r in 0..i.rows {
                for c in 0..i.cols {
                    if r == 0 && c == 0 {
                        continue;
                    }
                    let offset = Vec2::new(f64::from(c) * px, f64::from(r) * py);
                    let copies = copy_objects(doc, &ids, offset, &format!(" [{r},{c}]"));
                    cells.push(json!({ "row": r, "col": c, "ids": copies }));
                    created.extend(copies);
                }
            }
            let mut res = util::edit_result(doc, &created, &ids);
            res["cells"] = json!(cells);
            Ok((res, format!("arrayed {} into {}x{} at {px} x {py} mm", ids.join(", "), i.rows, i.cols)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::shape::ShapeEllipse;

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    #[test]
    fn grid_of_copies() {
        let mut d = Document::default();
        run::<ShapeEllipse>(&mut d, json!({"rx_mm": 1, "ry_mm": 1})).unwrap();
        let (r, _) =
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 2, "cols": 3, "pitch_x_mm": 5, "pitch_y_mm": -4}))
                .unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 5);
        assert_eq!(d.objects.len(), 6);
        assert_eq!(r["bounds"]["min"], json!([-1.0, -5.0]));
        assert_eq!(r["bounds"]["max"], json!([11.0, 1.0]));
        assert_eq!(r["cells"][4], json!({"row": 1, "col": 2, "ids": ["obj-7"]}));
    }

    #[test]
    fn refuses_degenerate_grids() {
        let mut d = Document::default();
        run::<ShapeEllipse>(&mut d, json!({"rx_mm": 1, "ry_mm": 1})).unwrap();
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 0, "cols": 3, "pitch_x_mm": 5, "pitch_y_mm": 5}))
                .is_err()
        );
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 1, "cols": 3, "pitch_x_mm": 0, "pitch_y_mm": 0}))
                .is_err()
        );
        assert!(
            run::<Array>(
                &mut d,
                json!({"ids": ["obj-2"], "rows": 1000, "cols": 1000, "pitch_x_mm": 1, "pitch_y_mm": 1})
            )
            .is_err()
        );
        assert_eq!(d.objects.len(), 1);
    }
}
