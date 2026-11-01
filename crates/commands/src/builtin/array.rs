//! `art.array`: rectangular grid of copies.

use doc::{Document, ObjectId};
use kurbo::{Affine, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::{Mirror, apply_affine, copy_objects};
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// Upper bound on cells, to keep a typo from creating millions of objects.
const MAX_CELLS: u32 = 10_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayInput {
    /// Exact object or group ids (a group stands for all its members), copied together per cell.
    pub ids: Vec<ObjectId>,
    /// Number of rows (along field Y), >= 1.
    pub rows: u32,
    /// Number of columns (along field X), >= 1.
    pub cols: u32,
    /// Distance between columns in mm along field +X, centre to centre (negative builds toward -X); with spacing_x
    /// "gap", the gap between neighbouring copies' bounding boxes.
    pub pitch_x_mm: f64,
    /// Distance between rows in mm along field +Y, away from the operator (negative builds toward the operator);
    /// with spacing_y "gap", the gap between neighbouring rows' bounding boxes.
    pub pitch_y_mm: f64,
    /// How pitch_x_mm is measured: "center" (centre to centre, default) or "gap" (LightBurn's Padding Between
    /// Edges: the selection's width plus the pitch).
    #[serde(default)]
    pub spacing_x: Spacing,
    /// How pitch_y_mm is measured: "center" (default) or "gap".
    #[serde(default)]
    pub spacing_y: Spacing,
    /// Build columns toward -X instead of +X (LightBurn's Reverse Direction). Default false.
    #[serde(default)]
    pub reverse_x: bool,
    /// Build rows toward -Y instead of +Y. Default false.
    #[serde(default)]
    pub reverse_y: bool,
    /// Shift every other row (rows 1, 3, ...) along X by this many mm, or "half" for half the selection's width
    /// (LightBurn's Row Shift / Shift by half: a brick or honeycomb layout).
    pub row_shift: Option<Shift>,
    /// Shift every other column (columns 1, 3, ...) along Y by this many mm, or "half" for half its height.
    pub col_shift: Option<Shift>,
    /// Flip the copies in every other column about their own centre: "x" left-right, "y" top-bottom (LightBurn's
    /// Mirror Alternate Columns).
    pub mirror_alternate_cols: Option<Mirror>,
    /// Flip the copies in every other row about their own centre: "x" or "y" (Mirror Alternate Rows).
    pub mirror_alternate_rows: Option<Mirror>,
}

/// How a grid pitch is measured.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Spacing {
    /// Centre to centre.
    #[default]
    Center,
    /// Between neighbouring bounding boxes (the selection's size is added).
    Gap,
}

/// An alternate row or column shift.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Shift {
    /// This many mm.
    Mm(f64),
    /// "half": half the selection's width (rows) or height (columns).
    Half(HalfWord),
}

/// The word "half".
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HalfWord {
    Half,
}

/// `art.array`.
pub struct Array;

impl Command for Array {
    type Input = ArrayInput;
    const ID: &'static str = "art.array";
    const TITLE: &'static str = "Grid array";
    const DESCRIPTION: &'static str = "Copy objects into a rows x cols grid (LightBurn's Grid Array). The originals \
        stay as row 0, column 0; the copy at (row r, column c) is offset by [c * step_x, r * step_y] in the field \
        frame (mm, +Y away from the operator), where a step is the pitch (spacing \"center\") or the selection's \
        size plus the pitch (spacing \"gap\"), negated by reverse_x / reverse_y. Odd rows can shift along X and odd \
        columns along Y (a number of mm or \"half\" the selection's size), and copies in odd columns or rows can be \
        mirrored about their own centre. Returns the created ids per cell (with `mirrored`) and the bounds of the \
        whole grid.";
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
            let b = doc.bounds_of(&ids).ok_or_else(|| invalid(id, "nothing to array"))?;
            let step = |pitch: f64, spacing: Spacing, size: f64, reverse: bool| {
                let s = if spacing == Spacing::Gap { size + pitch } else { pitch };
                if reverse { -s } else { s }
            };
            let sx = step(px, i.spacing_x, b.width(), i.reverse_x);
            let sy = step(py, i.spacing_y, b.height(), i.reverse_y);
            if (i.cols > 1 && sx.abs() < 1e-9) || (i.rows > 1 && sy.abs() < 1e-9) {
                return Err(invalid(id, "a zero step would stack copies on top of each other"));
            }
            let shift = |s: Option<Shift>, field: &str, half: f64| -> Result<f64> {
                match s {
                    None => Ok(0.0),
                    Some(Shift::Half(_)) => Ok(half / 2.0),
                    Some(Shift::Mm(v)) => util::finite(id, field, v),
                }
            };
            let row_shift = shift(i.row_shift, "row_shift", b.width())?;
            let col_shift = shift(i.col_shift, "col_shift", b.height())?;
            let flip = |m: Mirror| match m {
                Mirror::X => Affine::FLIP_X,
                Mirror::Y => Affine::FLIP_Y,
            };
            let mut created = Vec::new();
            let mut cells = Vec::new();
            for r in 0..i.rows {
                for c in 0..i.cols {
                    if r == 0 && c == 0 {
                        continue;
                    }
                    let dx = f64::from(c) * sx + if r % 2 == 1 { row_shift } else { 0.0 };
                    let dy = f64::from(r) * sy + if c % 2 == 1 { col_shift } else { 0.0 };
                    let copies = copy_objects(doc, &ids, Vec2::new(dx, dy), &format!(" [{r},{c}]"))?;
                    let mut m = Affine::IDENTITY;
                    if c % 2 == 1
                        && let Some(f) = i.mirror_alternate_cols
                    {
                        m = flip(f) * m;
                    }
                    if r % 2 == 1
                        && let Some(f) = i.mirror_alternate_rows
                    {
                        m = flip(f) * m;
                    }
                    let mirrored = m != Affine::IDENTITY;
                    if mirrored && let Some(cb) = doc.bounds_of(&copies) {
                        let ctr = cb.center().to_vec2();
                        apply_affine(doc, &copies, Affine::translate(ctr) * m * Affine::translate(-ctr));
                    }
                    cells.push(json!({ "row": r, "col": c, "ids": copies, "mirrored": mirrored }));
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
        assert_eq!(r["cells"][4], json!({"row": 1, "col": 2, "ids": ["obj-7"], "mirrored": false}));
    }

    #[test]
    fn gap_spacing_shifts_reverse_and_alternate_mirrors() {
        use crate::builtin::shape::ShapeRect;
        let mut d = Document::default();
        // A 4 x 2 rectangle centred on the origin.
        run::<ShapeRect>(&mut d, json!({"width_mm": 4, "height_mm": 2})).unwrap();
        let (r, _) = run::<Array>(
            &mut d,
            json!({"ids": ["obj-2"], "rows": 2, "cols": 3, "pitch_x_mm": 1, "pitch_y_mm": 0.5,
                   "spacing_x": "gap", "spacing_y": "gap", "row_shift": "half", "reverse_y": true}),
        )
        .unwrap();
        let centre = |id: &str| d.object(id).unwrap().bounds().center();
        // Columns 5 mm apart (4 + 1 gap), rows 2.5 mm apart toward -Y, row 1 shifted by half the width (2 mm).
        let cell = |k: usize| r["cells"][k]["ids"][0].as_str().unwrap().to_string();
        assert_eq!(centre(&cell(0)), kurbo::Point::new(5.0, 0.0));
        assert_eq!(centre(&cell(1)), kurbo::Point::new(10.0, 0.0));
        assert_eq!(centre(&cell(2)), kurbo::Point::new(2.0, -2.5));
        assert_eq!(centre(&cell(4)), kurbo::Point::new(12.0, -2.5));
        // Alternate columns mirrored left-right about their own centre; odd columns shifted up by 1 mm.
        let mut d = Document::default();
        let tri = "M 0 0 L 4 0 L 0 2 Z";
        run::<crate::builtin::shape::ShapePath>(&mut d, json!({"svg_path_data": tri})).unwrap();
        let (r, _) = run::<Array>(
            &mut d,
            json!({"ids": ["obj-2"], "rows": 1, "cols": 2, "pitch_x_mm": 10, "pitch_y_mm": 0,
                   "col_shift": 1, "mirror_alternate_cols": "x"}),
        )
        .unwrap();
        let copy = r["cells"][0]["ids"][0].as_str().unwrap();
        assert_eq!(r["cells"][0]["mirrored"], json!(true));
        let o = d.object("obj-2").unwrap().bounds();
        let c = d.object(copy).unwrap().bounds();
        assert!((c.x0 - o.x0 - 10.0).abs() < 1e-9 && (c.y0 - o.y0 - 1.0).abs() < 1e-9, "{o:?} {c:?}");
        // The right angle moved from the left side to the right side.
        let pts = d.object(copy).unwrap().field_path();
        let corner = pts.elements().iter().filter_map(|e| e.end_point()).find(|p| (p.y - c.y1).abs() < 1e-9).unwrap();
        assert!((corner.x - c.x1).abs() < 1e-9, "{corner:?} {c:?}");
        // A gap of minus the width would stack copies.
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 1, "cols": 2, "pitch_x_mm": -4, "pitch_y_mm": 0, "spacing_x": "gap"}))
                .is_err()
        );
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
