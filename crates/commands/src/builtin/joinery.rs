//! Joinery tools: `path.dogbone` (dogbone and T-bone reliefs in inside corners, OTH-003; `geom::dogbone`) and
//! `path.resize_slots` (LightBurn's Resize Slots in Selection, DES-024; `geom::slots`). Both work in the field frame
//! and write each changed shape back into its own frame (its transform is kept); shapes become paths.

use doc::{Document, ObjectId};
use geom::dogbone::{self, Relief};
use geom::slots::{self, SlotOptions};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, geom_err};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Which relief `path.dogbone` cuts.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DogboneStyle {
    /// A circle on the corner's bisector, biting into both walls (default).
    #[default]
    Dogbone,
    /// A circle biting into the longer of the two lines at the corner only (a T-bone).
    TboneLonger,
    /// A circle biting into the shorter of the two lines only.
    TboneShorter,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DogboneInput {
    /// Exact object or group ids (a group stands for all its members). Images are refused.
    pub ids: Vec<ObjectId>,
    /// The tool (or kerf) diameter the relief must clear, mm: > 0 and at most 100.
    pub diameter_mm: f64,
    /// dogbone (default), tbone_longer or tbone_shorter.
    #[serde(default)]
    pub style: DogboneStyle,
}

/// `path.dogbone`.
pub struct Dogbone;

impl Command for Dogbone {
    type Input = DogboneInput;
    const ID: &'static str = "path.dogbone";
    const TITLE: &'static str = "Dogbone corners";
    const DESCRIPTION: &'static str = "Add dogbone or T-bone reliefs to the inside corners of closed \
        shapes so a round tool or a wide kerf clears the corner (MillMage / CNC CAM's Dogbone). diameter_mm: the tool \
        or kerf diameter. style dogbone (default): a circle through the corner on its bisector; tbone_longer / \
        tbone_shorter: bite into one line only. Inside corners: reflex corners of parts and every corner of holes, \
        between two straight lines. A relief that does not fit is skipped with the reason. Shapes become paths. Fails \
        when no corner is relieved. Returns affected, `objects` {id: {corners, skipped}}, bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: DogboneInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let d = util::finite(cmd, "diameter_mm", i.diameter_mm)?;
        if !(d > 0.0 && d <= dogbone::MAX_DIAMETER_MM) {
            return Err(invalid(cmd, format!("diameter_mm must be > 0 and at most {} (got {d})", dogbone::MAX_DIAMETER_MM)));
        }
        let relief = match i.style {
            DogboneStyle::Dogbone => Relief::Dogbone,
            DogboneStyle::TboneLonger => Relief::TboneLonger,
            DogboneStyle::TboneShorter => Relief::TboneShorter,
        };
        transact(doc, |doc| {
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut affected = Vec::new();
            let mut report = serde_json::Map::new();
            let mut total = 0;
            for id in &ids {
                let o = util::object(doc, id)?.clone();
                let path = pathutil::vector_path(cmd, &o)?;
                let r = dogbone::dogbone(&path, d, relief).map_err(geom_err(cmd))?;
                let skipped: Vec<Value> =
                    r.skipped.iter().map(|s| json!({ "at": [util::r4(s.at.x), util::r4(s.at.y)], "reason": s.reason })).collect();
                if !r.corners.is_empty() {
                    pathutil::set_field_path(doc, id, &r.path)?;
                    affected.push(id.clone());
                    total += r.corners.len();
                }
                report.insert(
                    id.clone(),
                    json!({ "corners": r.corners.iter().map(|c| [util::r4(c.x), util::r4(c.y)]).collect::<Vec<_>>(), "skipped": skipped }),
                );
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!(
                    "{cmd}: no inside corner to relieve (inside corners are reflex corners of closed shapes, or corners of holes, between straight lines){}",
                    if report.values().any(|v| v["skipped"].as_array().is_some_and(|s| !s.is_empty())) { "; some did not fit the tool diameter" } else { "" }
                )));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["objects"] = Value::Object(report);
            Ok((r, format!("added {total} relief(s) of {d} mm to {}", affected.join(", "))))
        })
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResizeSlotsInput {
    /// Exact object or group ids (a group stands for all its members). Images are refused.
    pub ids: Vec<ObjectId>,
    /// The material thickness the design was made for: the length of the slots or tabs to find, mm.
    pub old_thickness_mm: f64,
    /// The new material thickness, mm.
    pub new_thickness_mm: f64,
    /// Slots or tabs within old ± this match, mm (default 0.1; smaller than the old thickness).
    pub tolerance_mm: Option<f64>,
    /// Move the bottom (innermost) line of slots so the assembled outer size stays (default true).
    #[serde(default = "yes")]
    pub adjust_slot_depth: bool,
    /// Move the sides of U-shaped slots apart or together around their centre (default false).
    #[serde(default)]
    pub adjust_slot_width: bool,
    /// Move the top (outermost) line of tabs so the inner size stays (default false).
    #[serde(default)]
    pub adjust_tab_height: bool,
}

/// `path.resize_slots`.
pub struct ResizeSlots;

impl Command for ResizeSlots {
    type Input = ResizeSlotsInput;
    const ID: &'static str = "path.resize_slots";
    const TITLE: &'static str = "Resize slots";
    const DESCRIPTION: &'static str = "LightBurn's Resize Slots in Selection: fit a slotted or \
        finger-jointed design to another material thickness. A slot or tab side is a straight line of \
        old_thickness_mm ± tolerance_mm (default 0.1) between a convex and a reflex corner. adjust_slot_depth \
        (default) moves the slot bottoms, keeping the outer size; adjust_tab_height moves the tab tops (both: half \
        each); adjust_slot_width widens U-shaped slots of that width around their centre. Closed outlines of straight \
        lines only; holes, curves and folds are skipped with the reason. Fails when nothing matched. Returns \
        affected, steps, widths, `objects` {id: {steps, widths, skipped}}, bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ResizeSlotsInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let o = SlotOptions {
            old_mm: util::positive(cmd, "old_thickness_mm", i.old_thickness_mm)?,
            new_mm: util::positive(cmd, "new_thickness_mm", i.new_thickness_mm)?,
            tolerance_mm: util::non_negative(cmd, "tolerance_mm", i.tolerance_mm.unwrap_or(0.1))?,
            slot_depth: i.adjust_slot_depth,
            slot_width: i.adjust_slot_width,
            tab_height: i.adjust_tab_height,
        };
        transact(doc, |doc| {
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut affected = Vec::new();
            let mut report = serde_json::Map::new();
            let (mut steps, mut widths) = (0, 0);
            for id in &ids {
                let obj = util::object(doc, id)?.clone();
                let path = pathutil::vector_path(cmd, &obj)?;
                let r = slots::resize_slots(&path, &o).map_err(geom_err(cmd))?;
                if r.steps + r.widths > 0 {
                    pathutil::set_field_path(doc, id, &r.path)?;
                    affected.push(id.clone());
                    steps += r.steps;
                    widths += r.widths;
                }
                report.insert(id.clone(), json!({ "steps": r.steps, "widths": r.widths, "skipped": r.skipped }));
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!(
                    "{cmd}: no slot or tab of {} ± {} mm found in the selection",
                    o.old_mm, o.tolerance_mm
                )));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["objects"] = Value::Object(report);
            r["steps"] = json!(steps);
            r["widths"] = json!(widths);
            Ok((r, format!("resized {steps} slot/tab side(s) and {widths} slot width(s) from {} to {} mm", o.old_mm, o.new_mm)))
        })
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::json;

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: serde_json::Value) -> crate::Result<serde_json::Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn slotted(s: &mut Session) -> String {
        // A 20 x 10 part with a 4 mm wide, 5 mm deep slot, drawn around (40, 0).
        let pts = [[30, -5], [50, -5], [50, 5], [42, 5], [42, 0], [38, 0], [38, 5], [30, 5]];
        run(s, "shape.line", json!({"points": pts, "closed": true})).unwrap()["created"][0].as_str().unwrap().to_owned()
    }

    #[test]
    fn dogbones_relieve_inside_corners_and_undo() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let id = slotted(&mut s);
        let r = run(&mut s, "path.dogbone", json!({"ids": [id], "diameter_mm": 1})).unwrap();
        assert_eq!(r["objects"][&id]["corners"], json!([[42.0, 0.0], [38.0, 0.0]]), "{r}");
        assert_eq!(r["affected"], json!([id]));
        assert!(run(&mut s, "path.dogbone", json!({"ids": [id], "diameter_mm": 0})).is_err());
        // The relieved corners are arcs now: a second pass finds no straight inside corner.
        assert!(run(&mut s, "path.dogbone", json!({"ids": [id], "diameter_mm": 1})).is_err());
        s.undo().unwrap();
        let r = run(&mut s, "path.dogbone", json!({"ids": [id], "diameter_mm": 1, "style": "tbone_shorter"})).unwrap();
        assert_eq!(r["objects"][&id]["corners"].as_array().unwrap().len(), 2);
        // A plain rectangle has no inside corner.
        let rect = run(&mut s, "shape.rect", json!({"width_mm": 5, "height_mm": 5, "center": [0, 30]})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let e = run(&mut s, "path.dogbone", json!({"ids": [rect], "diameter_mm": 1})).unwrap_err().to_string();
        assert!(e.contains("no inside corner"), "{e}");
    }

    #[test]
    fn resize_slots_moves_slot_bottoms() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let id = slotted(&mut s);
        // The slot is 5 deep: steps of 5 become 6 (the bottom drops from y = 0 to y = -1).
        let r = run(&mut s, "path.resize_slots", json!({"ids": [id], "old_thickness_mm": 5, "new_thickness_mm": 6})).unwrap();
        assert_eq!(r["steps"], 2, "{r}");
        let nodes = run(&mut s, "node.list", json!({"id": id})).unwrap();
        let ys: Vec<f64> = nodes["nodes"].as_array().unwrap().iter().map(|n| n["point"][1].as_f64().unwrap()).collect();
        assert!(ys.iter().any(|y| (y + 1.0).abs() < 1e-9), "{ys:?}");
        let e = run(&mut s, "path.resize_slots", json!({"ids": [id], "old_thickness_mm": 3, "new_thickness_mm": 4})).unwrap_err().to_string();
        assert!(e.contains("no slot or tab"), "{e}");
        // The width (4 mm) as a slot width.
        let r = run(&mut s, "path.resize_slots", json!({"ids": [id], "old_thickness_mm": 4, "new_thickness_mm": 3, "adjust_slot_depth": false, "adjust_slot_width": true})).unwrap();
        assert_eq!(r["widths"], 1, "{r}");
    }
}
