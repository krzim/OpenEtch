//! Arranging commands: `object.move`, `object.align` and `object.distribute`.
//!
//! Align follows LightBurn (docs "Align"): by default every object moves so its chosen edge or centre lands on the
//! same edge or centre of the **last** object in `ids` (the UI passes the selection in the order objects were added
//! to it, so that is the last one added). A group aligns as one object, using its overall bounds, and an object's
//! edges are those of its field-frame bounding box. Top is +Y (away from the operator).

use doc::{Document, ObjectId};
use kurbo::{Affine, Rect};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::apply_affine;
use super::util::{self, Unit, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveInput {
    /// Exact object or group ids (a group stands for all its members), moved together.
    pub ids: Vec<ObjectId>,
    /// The grabbed point [x, y] in field mm, e.g. a corner of the selection the user started dragging from.
    pub from_point: [f64; 2],
    /// Where that point goes [x, y] in field mm, e.g. the snap target.
    pub to_point: [f64; 2],
}

/// `object.move`.
pub struct Move;

impl Command for Move {
    type Input = MoveInput;
    const ID: &'static str = "object.move";
    const TITLE: &'static str = "Move objects point to point";
    const DESCRIPTION: &'static str = "Move objects so that from_point lands on to_point (both field mm), e.g. a \
        snapped drag: the same as object.transform translate by to_point - from_point. Returns the affected ids and \
        their bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let ids = util::object_ids(id, doc, &i.ids)?;
            let [fx, fy] = util::finite2(id, "from_point", i.from_point)?;
            let [tx, ty] = util::finite2(id, "to_point", i.to_point)?;
            let (dx, dy) = (tx - fx, ty - fy);
            apply_affine(doc, &ids, Affine::translate((dx, dy)));
            let summary = format!("moved {} by [{}, {}]", ids.join(", "), util::r4(dx), util::r4(dy));
            Ok((util::edit_result(doc, &[], &ids), summary))
        })
    }
}

/// What objects are aligned to.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AlignReference {
    /// The last id in `ids` (LightBurn's default: the last object added to the selection). It does not move.
    Last,
    /// The first id in `ids`. It does not move.
    First,
    /// The bounds of the whole selection.
    Selection,
    /// Another object or group by exact id (it does not move, also when it is in `ids`).
    Object {
        /// Exact object or group id.
        id: String,
    },
    /// The stock's field-frame bounding box (fails if no stock is defined).
    Stock,
    /// The galvo field (or a gantry's bed): centre [0, 0], edges at ±field_mm / 2 (± workspace_mm / 2).
    Field,
}

/// Horizontal keypoint.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlignX {
    Left,
    Center,
    Right,
}

/// Vertical keypoint ("top" is +Y, away from the operator).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlignY {
    Top,
    Center,
    Bottom,
}

/// Whether every object aligns on its own or the selection moves as one block.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AlignMode {
    /// Each object (or group) is aligned on its own (default).
    #[default]
    Each,
    /// The objects keep their layout and move together, aligned by their combined bounds.
    Group,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlignInput {
    /// Exact object or group ids, in the order they were added to the selection (order matters for the "last" and
    /// "first" references). A group aligns as one object.
    pub ids: Vec<String>,
    /// What to align to: {"kind": "last"} (default), {"kind": "first"}, {"kind": "selection"},
    /// {"kind": "object", "id"}, {"kind": "stock"} or {"kind": "field"}.
    pub reference: Option<AlignReference>,
    /// Horizontal alignment: "left", "center" or "right". Omit to leave X alone.
    pub x: Option<AlignX>,
    /// Vertical alignment: "top" (+Y), "center" or "bottom". Omit to leave Y alone.
    pub y: Option<AlignY>,
    /// "each" (default): every object aligns on its own; "group": they move together, keeping their layout.
    pub mode: Option<AlignMode>,
}

fn key_x(r: Rect, x: AlignX) -> f64 {
    match x {
        AlignX::Left => r.x0,
        AlignX::Center => r.center().x,
        AlignX::Right => r.x1,
    }
}

fn key_y(r: Rect, y: AlignY) -> f64 {
    match y {
        AlignY::Bottom => r.y0,
        AlignY::Center => r.center().y,
        AlignY::Top => r.y1,
    }
}

/// The workspace (the galvo field, or a gantry's bed) as a rectangle centred on the origin.
fn field_rect(cmd: &str, doc: &Document) -> Result<Rect> {
    let [w, h] = doc.workspace().ok_or_else(|| {
        CommandError::Failed(format!("{cmd}: the document has no field size; set one by opening or importing a file"))
    })?;
    Ok(Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0))
}

/// `object.align`.
pub struct Align;

impl Command for Align {
    type Input = AlignInput;
    const ID: &'static str = "object.align";
    const TITLE: &'static str = "Align objects";
    const DESCRIPTION: &'static str = "Align objects (groups as one object, by bounding box) so their left/centre/\
        right (x) and/or top/centre/bottom (y; top is +Y) lands on the reference's same edge or centre. Reference \
        default {\"kind\":\"last\"}: the last id in ids, which stays put (LightBurn's rule: the last object added to \
        the selection); also first, selection, object {id}, stock or field. mode \"each\" (default) aligns every \
        object on its own, \"group\" moves them together. E.g. centre on stock: reference stock, x and y center, mode \
        group.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AlignInput) -> Result<(Value, String)> {
        transact(doc, |doc| align(doc, i))
    }
}

fn align(doc: &mut Document, i: AlignInput) -> Result<(Value, String)> {
    let id = Align::ID;
    if i.x.is_none() && i.y.is_none() {
        return Err(invalid(id, "nothing to do: give x and/or y"));
    }
    let units = util::units(id, doc, &i.ids)?;
    let reference = i.reference.unwrap_or(AlignReference::Last);
    // The reference rectangle, and which unit (if any) is the reference and stays put.
    let (target, fixed, label): (Rect, Option<String>, String) = match &reference {
        AlignReference::Last | AlignReference::First => {
            if units.len() < 2 {
                return Err(invalid(
                    id,
                    "aligning to another object needs at least two objects; use reference stock, field or object",
                ));
            }
            let u = if matches!(reference, AlignReference::Last) { units.last() } else { units.first() };
            let u = u.expect("at least two units");
            (bounds(doc, u)?, Some(u.id.clone()), u.id.clone())
        }
        AlignReference::Selection => {
            let all: Vec<ObjectId> = units.iter().flat_map(|u| u.members.iter().cloned()).collect();
            (doc.bounds_of(&all).ok_or_else(|| CommandError::Failed("the selection has no bounds".into()))?, None, "the selection".into())
        }
        AlignReference::Object { id: ref_id } => {
            let r = util::units(id, doc, std::slice::from_ref(ref_id))?;
            let u = r.first().ok_or_else(|| CommandError::Failed(format!("{ref_id} has no members")))?;
            (bounds(doc, u)?, Some(ref_id.clone()), ref_id.clone())
        }
        AlignReference::Stock => {
            let stock = doc
                .stock
                .as_ref()
                .ok_or_else(|| CommandError::Failed("no stock is defined; run stock.define first".into()))?;
            (stock.bounds(), None, "the stock".into())
        }
        AlignReference::Field => (field_rect(id, doc)?, None, "the field".into()),
    };
    let moving: Vec<&Unit> = units.iter().filter(|u| fixed.as_deref() != Some(u.id.as_str())).collect();
    let delta = |r: Rect| -> (f64, f64) {
        let dx = i.x.map_or(0.0, |x| key_x(target, x) - key_x(r, x));
        let dy = i.y.map_or(0.0, |y| key_y(target, y) - key_y(r, y));
        (dx, dy)
    };
    let mut affected: Vec<ObjectId> = Vec::new();
    match i.mode.unwrap_or_default() {
        AlignMode::Each => {
            for u in &moving {
                let d = delta(bounds(doc, u)?);
                apply_affine(doc, &u.members, Affine::translate(d));
                affected.extend(u.members.iter().cloned());
            }
        }
        AlignMode::Group => {
            let members: Vec<ObjectId> = moving.iter().flat_map(|u| u.members.iter().cloned()).collect();
            if let Some(r) = doc.bounds_of(&members) {
                apply_affine(doc, &members, Affine::translate(delta(r)));
            }
            affected = members;
        }
    }
    let how = [i.x.map(|x| format!("{x:?}").to_lowercase()), i.y.map(|y| format!("{y:?}").to_lowercase())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("/");
    let names: Vec<&str> = moving.iter().map(|u| u.id.as_str()).collect();
    let summary = format!("aligned {} {how} to {label}", if names.is_empty() { "nothing".into() } else { names.join(", ") });
    let mut r = util::edit_result(doc, &[], &affected);
    r["reference"] = json!({ "id": fixed, "bounds": util::rect_json(target) });
    Ok((r, summary))
}

fn bounds(doc: &Document, u: &Unit) -> Result<Rect> {
    u.bounds(doc).ok_or_else(|| CommandError::Failed(format!("{} has no bounds", u.id)))
}

/// Distribution axis.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Along field X (left to right).
    X,
    /// Along field Y (operator to column).
    Y,
}

/// How objects are spread.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DistributeBy {
    /// Equal distances between centres; the two outermost objects stay put (at least 3 objects).
    Centers,
    /// Equal gaps between bounding boxes; the two outermost objects stay put (at least 3 objects).
    Gaps,
    /// Exactly spacing_mm between bounding boxes, packed from the first object along the axis, which stays put (at
    /// least 2 objects; negative spacing overlaps them).
    Spacing,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DistributeInput {
    /// Exact object or group ids; a group moves as one object.
    pub ids: Vec<String>,
    /// "x" or "y".
    pub axis: Axis,
    /// "centers", "gaps" or "spacing".
    pub by: DistributeBy,
    /// Gap between neighbours in mm, for by "spacing" only.
    pub spacing_mm: Option<f64>,
}

/// `object.distribute`.
pub struct Distribute;

impl Command for Distribute {
    type Input = DistributeInput;
    const ID: &'static str = "object.distribute";
    const TITLE: &'static str = "Distribute objects";
    const DESCRIPTION: &'static str = "Spread objects (groups as one object) along x or y, ordered by their centres: \
        by \"centers\" (equal centre distances) or \"gaps\" (equal gaps between bounding boxes) keep the two \
        outermost in place and need 3 or more; by \"spacing\" packs them spacing_mm apart starting from the first \
        along the axis (2 or more). Only the chosen axis changes.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: DistributeInput) -> Result<(Value, String)> {
        transact(doc, |doc| distribute(doc, i))
    }
}

fn distribute(doc: &mut Document, i: DistributeInput) -> Result<(Value, String)> {
    let id = Distribute::ID;
    let spacing = match (i.by, i.spacing_mm) {
        (DistributeBy::Spacing, Some(s)) => Some(util::finite(id, "spacing_mm", s)?),
        (DistributeBy::Spacing, None) => return Err(invalid(id, "by \"spacing\" needs spacing_mm")),
        (_, Some(_)) => return Err(invalid(id, "spacing_mm is only for by \"spacing\"")),
        (_, None) => None,
    };
    let units = util::units(id, doc, &i.ids)?;
    let min = if spacing.is_some() { 2 } else { 3 };
    if units.len() < min {
        return Err(invalid(id, format!("distributing this way needs at least {min} objects (got {})", units.len())));
    }
    // (lo, hi) along the axis.
    let span = |r: Rect| if i.axis == Axis::X { (r.x0, r.x1) } else { (r.y0, r.y1) };
    let mut items: Vec<(&Unit, f64, f64)> = Vec::with_capacity(units.len());
    for u in &units {
        let (lo, hi) = span(bounds(doc, u)?);
        items.push((u, lo, hi));
    }
    // Stable: equal centres keep the given order.
    items.sort_by(|a, b| (a.1 + a.2).total_cmp(&(b.1 + b.2)));
    let n = items.len();
    let (first, last) = (items[0], items[n - 1]);
    // Target low edge per item.
    let targets: Vec<f64> = match i.by {
        DistributeBy::Centers => {
            let (c0, c1) = ((first.1 + first.2) / 2.0, (last.1 + last.2) / 2.0);
            #[allow(clippy::cast_precision_loss)]
            let step = (c1 - c0) / (n - 1) as f64;
            #[allow(clippy::cast_precision_loss)]
            items.iter().enumerate().map(|(k, (_, lo, hi))| c0 + step * k as f64 - (hi - lo) / 2.0).collect()
        }
        DistributeBy::Gaps | DistributeBy::Spacing => {
            let gap = spacing.unwrap_or_else(|| {
                let sizes: f64 = items.iter().map(|(_, lo, hi)| hi - lo).sum();
                #[allow(clippy::cast_precision_loss)]
                let g = (last.2 - first.1 - sizes) / (n - 1) as f64;
                g
            });
            let mut cursor = first.1;
            items
                .iter()
                .map(|(_, lo, hi)| {
                    let t = cursor;
                    cursor += (hi - lo) + gap;
                    t
                })
                .collect()
        }
    };
    let mut affected: Vec<ObjectId> = Vec::new();
    for ((u, lo, _), t) in items.iter().zip(&targets) {
        let d = t - lo;
        let v = if i.axis == Axis::X { (d, 0.0) } else { (0.0, d) };
        apply_affine(doc, &u.members, Affine::translate(v));
        affected.extend(u.members.iter().cloned());
    }
    let how = match i.by {
        DistributeBy::Centers => "centres".to_string(),
        DistributeBy::Gaps => "gaps".to_string(),
        DistributeBy::Spacing => format!("{} mm apart", spacing.unwrap_or_default()),
    };
    let axis = if i.axis == Axis::X { "x" } else { "y" };
    let names: Vec<&str> = items.iter().map(|(u, _, _)| u.id.as_str()).collect();
    Ok((util::edit_result(doc, &[], &affected), format!("distributed {} along {axis} by {how}", names.join(", "))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::shape::{ShapeEllipse, ShapeRect};

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    /// Bounds rounded to 0.1 µm.
    fn b(d: &Document, id: &str) -> Rect {
        let r = d.object(id).unwrap().bounds();
        Rect::new(util::r4(r.x0), util::r4(r.y0), util::r4(r.x1), util::r4(r.y1))
    }

    /// obj-2: 10x4 rect centred at [0, 0]; obj-3: 4x4 at [20, 10]; obj-4: 2x2 at [-30, -5]. Field 150 mm.
    fn doc() -> Document {
        let mut d = Document { field_mm: Some(150.0), ..Document::default() };
        run::<ShapeRect>(&mut d, json!({"width_mm": 10, "height_mm": 4})).unwrap();
        run::<ShapeRect>(&mut d, json!({"width_mm": 4, "height_mm": 4, "center": [20, 10]})).unwrap();
        run::<ShapeEllipse>(&mut d, json!({"rx_mm": 1, "ry_mm": 1, "center": [-30, -5]})).unwrap();
        d
    }

    #[test]
    fn move_point_to_point() {
        let mut d = doc();
        let (r, _) = run::<Move>(&mut d, json!({"ids": ["obj-2"], "from_point": [-5, -2], "to_point": [0, 0]})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([0.0, 0.0]));
        assert!(run::<Move>(&mut d, json!({"ids": ["obj-9"], "from_point": [0, 0], "to_point": [1, 1]})).is_err());
    }

    #[test]
    fn align_defaults_to_the_last_object_which_stays_put() {
        let mut d = doc();
        let (r, s) = run::<Align>(&mut d, json!({"ids": ["obj-2", "obj-4", "obj-3"], "x": "left"})).unwrap();
        assert_eq!(b(&d, "obj-3").x0, 18.0, "reference unchanged");
        assert_eq!(b(&d, "obj-2").x0, 18.0);
        assert_eq!(b(&d, "obj-4").x0, 18.0);
        assert_eq!(b(&d, "obj-4").y0, -6.0, "y untouched");
        assert_eq!(r["reference"]["id"], "obj-3");
        assert!(s.contains("left"), "{s}");
        // First, top (+Y).
        run::<Align>(&mut d, json!({"ids": ["obj-4", "obj-2"], "reference": {"kind": "first"}, "y": "top"})).unwrap();
        assert_eq!(b(&d, "obj-2").y1, b(&d, "obj-4").y1);
        assert!(run::<Align>(&mut d, json!({"ids": ["obj-2"], "x": "left"})).is_err(), "one object, no reference");
        assert!(run::<Align>(&mut d, json!({"ids": ["obj-2", "obj-3"]})).is_err(), "no axis");
    }

    #[test]
    fn centre_on_stock_moves_the_selection_as_a_block() {
        let mut d = doc();
        assert!(
            run::<Align>(&mut d, json!({"ids": ["obj-2"], "reference": {"kind": "stock"}, "x": "center"})).is_err(),
            "no stock"
        );
        d.stock = Some(doc::Stock {
            name: "tag".into(),
            shape: doc::StockShape::Circle { diameter_mm: 40.0 },
            thickness_mm: 1.0,
            material: None,
            center: [10.0, 10.0],
            rotation_deg: 0.0,
        });
        let before = d.bounds_of(&["obj-2".into(), "obj-3".into()]).unwrap();
        let (r, _) = run::<Align>(
            &mut d,
            json!({"ids": ["obj-2", "obj-3"], "reference": {"kind": "stock"}, "x": "center", "y": "center", "mode": "group"}),
        )
        .unwrap();
        assert_eq!(r["bounds"]["center"], json!([10.0, 10.0]));
        let after = d.bounds_of(&["obj-2".into(), "obj-3".into()]).unwrap();
        assert_eq!(after.size(), before.size(), "layout kept");
    }

    #[test]
    fn align_each_to_field_selection_and_object() {
        let mut d = doc();
        run::<Align>(&mut d, json!({"ids": ["obj-2", "obj-3"], "reference": {"kind": "field"}, "x": "right"})).unwrap();
        assert_eq!(b(&d, "obj-2").x1, 75.0);
        assert_eq!(b(&d, "obj-3").x1, 75.0);
        run::<Align>(&mut d, json!({"ids": ["obj-2", "obj-4"], "reference": {"kind": "selection"}, "y": "bottom"}))
            .unwrap();
        assert_eq!(b(&d, "obj-2").y0, -6.0);
        assert_eq!(b(&d, "obj-4").y0, -6.0);
        run::<Align>(&mut d, json!({"ids": ["obj-2", "obj-3"], "reference": {"kind": "object", "id": "obj-4"}, "x": "center"}))
            .unwrap();
        assert_eq!(b(&d, "obj-2").center().x, -30.0);
        assert_eq!(b(&d, "obj-4").center().x, -30.0, "the reference never moves");
        assert!(matches!(
            run::<Align>(&mut d, json!({"ids": ["obj-2"], "reference": {"kind": "object", "id": "obj-99"}, "x": "left"})),
            Err(CommandError::NotFound { .. })
        ));
    }

    #[test]
    fn groups_align_as_one_object() {
        let mut d = doc();
        d.groups.push(doc::Group::new("group-9".into(), "Pair", None));
        d.object_mut("obj-2").unwrap().group = Some("group-9".into());
        d.object_mut("obj-3").unwrap().group = Some("group-9".into());
        // The group's bounds: x -5..22. Align its left edge to obj-4's left (-31).
        run::<Align>(&mut d, json!({"ids": ["group-9", "obj-4"], "x": "left"})).unwrap();
        assert_eq!(b(&d, "obj-2").x0, -31.0);
        assert_eq!(b(&d, "obj-3").x0, -31.0 + 23.0, "members keep their layout");
        // A group id works for every id-taking command.
        let (r, _) = run::<crate::builtin::object::Transform>(&mut d, json!({"ids": ["group-9"], "translate": [1, 0]}))
            .unwrap();
        assert_eq!(r["affected"], json!(["obj-2", "obj-3"]));
    }

    #[test]
    fn distribute_centres_gaps_and_spacing() {
        let mut d = doc();
        // Centres along x: obj-4 at -30, obj-2 at 0, obj-3 at 20 -> obj-2 moves to -5.
        run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3", "obj-4"], "axis": "x", "by": "centers"})).unwrap();
        assert_eq!(b(&d, "obj-2").center().x, -5.0);
        assert_eq!(b(&d, "obj-4").center().x, -30.0);
        assert_eq!(b(&d, "obj-3").center().x, 20.0);
        // Gaps: span -31..22 = 53, sizes 2 + 10 + 4 = 16, gap 18.5.
        run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3", "obj-4"], "axis": "x", "by": "gaps"})).unwrap();
        assert_eq!(b(&d, "obj-2").x0, -31.0 + 2.0 + 18.5);
        assert_eq!(b(&d, "obj-3").x1, 22.0);
        // Spacing along y from the lowest (obj-4, y -6..-4): obj-2 (height 4) then obj-3.
        run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3", "obj-4"], "axis": "y", "by": "spacing", "spacing_mm": 1}))
            .unwrap();
        assert_eq!(b(&d, "obj-4").y0, -6.0);
        assert_eq!(b(&d, "obj-2").y0, -3.0);
        assert_eq!(b(&d, "obj-3").y0, 2.0);
        assert!(run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3"], "axis": "x", "by": "centers"})).is_err());
        assert!(run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3"], "axis": "x", "by": "spacing"})).is_err());
        assert!(
            run::<Distribute>(&mut d, json!({"ids": ["obj-2", "obj-3", "obj-4"], "axis": "x", "by": "gaps", "spacing_mm": 1}))
                .is_err()
        );
    }
}
