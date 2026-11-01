//! Booleans: `path.boolean`, `path.weld`, `path.cut_shapes` (LightBurn's Boolean tools, Weld and Cut Shapes). Shapes
//! are combined as filled areas in the field frame (each object filled even-odd, as jobs fill it; weld non-zero) by
//! `geom::region`, so results are polygons whose vertices lie within 0.01 mm of the original curves.

use doc::{Document, FillMode, Group, ObjectId};
use geom::region::{Fill, Region, clip_lines};
use geom::{BoolOp, Polyline};
use kurbo::BezPath;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, MAX_CREATED, geom_err};
use super::util::{self, Unit, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// A boolean operation.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    /// The area covered by any of the shapes.
    Union,
    /// The last id (object or group) is cut out of every other object, then deleted (LightBurn: "the second shape
    /// you add to your selection is subtracted from the first, and deleted").
    Subtract,
    /// The area covered by all of the shapes.
    Intersect,
    /// The area covered by an odd number of the shapes (exclusive or).
    Xor,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BooleanInput {
    /// Exact object or group ids in selection order, at least two (a group counts as one shape: the union of its
    /// members). Every shape must be closed; images are refused.
    pub ids: Vec<ObjectId>,
    /// union, subtract (last minus the others), intersect or xor.
    pub op: BooleanOp,
}

/// The filled area of one object (closed only; images and open paths refused).
fn closed_region(cmd: &str, doc: &Document, id: &str, fill: Fill) -> Result<Region> {
    let o = util::object(doc, id)?;
    let path = pathutil::vector_path(cmd, o)?;
    if geom::region::has_open_subpaths(&path).map_err(geom_err(cmd))? {
        return Err(invalid(
            cmd,
            format!("{id} (\"{}\") is not closed; booleans need closed shapes (try path.close or path.auto_join)", o.name),
        ));
    }
    let r = Region::from_path(&path, fill).map_err(geom_err(cmd))?;
    if r.is_empty() {
        return Err(invalid(cmd, format!("{id} (\"{}\") encloses no area", o.name)));
    }
    Ok(r)
}

fn unit_region(cmd: &str, doc: &Document, u: &Unit, fill: Fill) -> Result<Region> {
    let rs = u.members.iter().map(|m| closed_region(cmd, doc, m, fill)).collect::<Result<Vec<_>>>()?;
    Region::union_all(rs).map_err(geom_err(cmd))
}

/// `path.boolean`.
pub struct PathBoolean;

impl Command for PathBoolean {
    type Input = BooleanInput;
    const ID: &'static str = "path.boolean";
    const TITLE: &'static str = "Boolean";
    const DESCRIPTION: &'static str = "Combine closed shapes as filled areas (field frame; each object filled \
        even-odd, so its holes stay holes). union/intersect/xor replace the first object's geometry with the result \
        (it keeps its id, layer, name and transform) and delete the others; subtract cuts the last id out of each \
        other object and deletes it (an object cut away entirely is deleted). Results are paths: curves become \
        polylines within 0.01 mm. Returns created/affected/deleted ids and bounds; refuses open shapes, images, \
        locked objects and an empty result.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: BooleanInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let units = util::units(cmd, doc, &i.ids)?;
            if units.len() < 2 {
                return Err(invalid(cmd, "a boolean needs at least two shapes (objects or groups)"));
            }
            let all: Vec<ObjectId> = units.iter().flat_map(|u| u.members.iter().cloned()).collect();
            util::check_unlocked(cmd, doc, &all)?;
            if i.op == BooleanOp::Subtract {
                return subtract(doc, &units);
            }
            let rule = match i.op {
                BooleanOp::Union => BoolOp::Union,
                BooleanOp::Intersect => BoolOp::Intersect,
                _ => BoolOp::Xor,
            };
            let mut acc = unit_region(cmd, doc, &units[0], Fill::EvenOdd)?;
            for u in &units[1..] {
                let r = unit_region(cmd, doc, u, Fill::EvenOdd)?;
                acc = acc.boolean(&r, rule).map_err(geom_err(cmd))?;
            }
            if acc.is_empty() {
                return Err(CommandError::Failed(format!(
                    "{cmd}: the result is empty (the shapes do not overlap); nothing was changed"
                )));
            }
            let keep = all[0].clone();
            pathutil::set_field_path(doc, &keep, &acc.to_path())?;
            let gone: Vec<ObjectId> = all[1..].to_vec();
            let groups = pathutil::delete_objects(doc, &gone);
            let mut r = util::edit_result(doc, &[], std::slice::from_ref(&keep));
            r["deleted"] = json!(gone);
            r["deleted_groups"] = json!(groups);
            r["area_mm2"] = json!(util::r4(acc.area()));
            let op = format!("{:?}", i.op).to_lowercase();
            Ok((r, format!("{op} of {} into {keep}", all.join(", "))))
        })
    }
}

fn subtract(doc: &mut Document, units: &[Unit]) -> Result<(Value, String)> {
    let cmd = PathBoolean::ID;
    let (cutter_unit, subjects) = units.split_last().expect("at least two units");
    let cutter = unit_region(cmd, doc, cutter_unit, Fill::EvenOdd)?;
    let mut affected = Vec::new();
    let mut deleted: Vec<ObjectId> = cutter_unit.members.clone();
    for id in subjects.iter().flat_map(|u| u.members.iter()) {
        let r = closed_region(cmd, doc, id, Fill::EvenOdd)?;
        if r.boolean(&cutter, BoolOp::Intersect).map_err(geom_err(cmd))?.is_empty() {
            continue;
        }
        let rest = r.boolean(&cutter, BoolOp::Difference).map_err(geom_err(cmd))?;
        if rest.is_empty() {
            deleted.push(id.clone());
        } else {
            pathutil::set_field_path(doc, id, &rest.to_path())?;
            affected.push(id.clone());
        }
    }
    if affected.is_empty() && deleted.len() == cutter_unit.members.len() {
        return Err(CommandError::Failed(format!(
            "{cmd}: {} does not overlap the other shapes; nothing was changed",
            cutter_unit.id
        )));
    }
    let groups = pathutil::delete_objects(doc, &deleted);
    let mut r = util::edit_result(doc, &[], &affected);
    r["deleted"] = json!(deleted);
    r["deleted_groups"] = json!(groups);
    Ok((r, format!("subtracted {} from {}", cutter_unit.id, affected.join(", "))))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WeldInput {
    /// Exact object or group ids (a group stands for all its members); at least one. Every shape must be closed.
    pub ids: Vec<ObjectId>,
}

/// `path.weld`.
pub struct PathWeld;

impl Command for PathWeld {
    type Input = WeldInput;
    const ID: &'static str = "path.weld";
    const TITLE: &'static str = "Weld";
    const DESCRIPTION: &'static str = "Merge all given closed shapes, including each shape's own overlaps, into one \
        outline per island (LightBurn's Weld, e.g. for overlapping script letters). Each object is filled non-zero, \
        so a contour drawn inside another the same way round is swallowed (LightBurn: weld can remove fully \
        contained content) while letter counters stay holes. The first object takes the result (keeps id, layer, \
        name, transform); the others are deleted. Results are polylines within 0.01 mm. Returns affected/deleted ids \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: WeldInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let regions = ids.iter().map(|id| closed_region(cmd, doc, id, Fill::NonZero)).collect::<Result<Vec<_>>>()?;
            let r = Region::union_all(regions).map_err(geom_err(cmd))?;
            if r.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: nothing to weld")));
            }
            pathutil::set_field_path(doc, &ids[0], &r.to_path())?;
            let gone = ids[1..].to_vec();
            let groups = pathutil::delete_objects(doc, &gone);
            let mut res = util::edit_result(doc, &[], &ids[..1]);
            res["deleted"] = json!(gone);
            res["deleted_groups"] = json!(groups);
            res["islands"] = json!(r.shapes.len());
            Ok((res, format!("welded {} into {} ({} islands)", ids.join(", "), ids[0], r.shapes.len())))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CutShapesInput {
    /// Exact ids in selection order. The LAST id is the cutting tool: one closed, ungrouped object, deleted by the
    /// cut. Every other id (object or group) is cut where it crosses the tool.
    pub ids: Vec<ObjectId>,
}

/// `path.cut_shapes`.
pub struct PathCutShapes;

impl Command for PathCutShapes {
    type Input = CutShapesInput;
    const ID: &'static str = "path.cut_shapes";
    const TITLE: &'static str = "Cut shapes";
    const DESCRIPTION: &'static str = "Cut shapes into pieces with the last id (a closed, ungrouped object; it is \
        deleted), as LightBurn does. Objects on fill layers become closed pieces (the cut gap bridged); objects on \
        line layers and open paths become open pieces. Pieces go into two new groups, \"Cut inside\" and \"Cut \
        outside\", one object per piece, on the original's layer; shapes the tool does not cross are left alone. \
        Pieces are polylines within 0.01 mm. Images are refused. Returns created ids (inside/outside), deleted ids \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CutShapesInput) -> Result<(Value, String)> {
        transact(doc, |doc| cut_shapes(doc, i))
    }
}

/// Every subpath of a field path as plain points (closed ones repeat their first point).
fn lines_of(cmd: &str, path: &BezPath) -> Result<Vec<Vec<kurbo::Point>>> {
    Ok(geom::flatten(path, geom::TOLERANCE).map_err(geom_err(cmd))?.iter().map(Polyline::drawn).collect())
}

/// Open pieces, re-joined where clipping split a ring at its start point.
fn join_pieces(pieces: Vec<Vec<kurbo::Point>>) -> Vec<BezPath> {
    let subs: Vec<geom::ops::Piece> = pieces
        .into_iter()
        .filter_map(|pts| geom::shapes::polyline(&pts, false).ok())
        .flat_map(|p| geom::split(&p))
        .map(|sub| geom::ops::Piece { tag: 0, sub })
        .collect();
    geom::ops::auto_join(subs, 1e-6).into_iter().map(|j| geom::join(&[j.sub])).collect()
}

fn cut_shapes(doc: &mut Document, i: CutShapesInput) -> Result<(Value, String)> {
    let cmd = PathCutShapes::ID;
    let Some(tool_id) = i.ids.last().cloned() else {
        return Err(invalid(cmd, "ids must list the shapes to cut and, last, the cutting tool"));
    };
    if doc.object(&tool_id).is_none() && doc.group(&tool_id).is_some() {
        return Err(invalid(cmd, format!("the cutting tool {tool_id} is a group; ungroup it first")));
    }
    let tool = util::object(doc, &tool_id)?.clone();
    let subjects: Vec<ObjectId> =
        util::object_ids(cmd, doc, &i.ids[..i.ids.len() - 1]).map_err(|e| match e {
            CommandError::InvalidInput { .. } => invalid(cmd, "give at least one shape to cut before the cutting tool"),
            e => e,
        })?;
    let subjects: Vec<ObjectId> = subjects.into_iter().filter(|s| *s != tool_id).collect();
    if subjects.is_empty() {
        return Err(invalid(cmd, "give at least one shape to cut before the cutting tool"));
    }
    let mut touched = subjects.clone();
    touched.push(tool_id.clone());
    util::check_unlocked(cmd, doc, &touched)?;
    let cutter = closed_region(cmd, doc, &tool.id, Fill::EvenOdd)?;
    let mut inside_ids: Vec<ObjectId> = Vec::new();
    let mut outside_ids: Vec<ObjectId> = Vec::new();
    let mut cut: Vec<ObjectId> = Vec::new();
    let mut pieces_total = 0usize;
    let mut new_pieces: Vec<(String, String, Vec<BezPath>, Vec<BezPath>)> = Vec::new();
    for id in &subjects {
        let o = util::object(doc, id)?.clone();
        let path = pathutil::vector_path(cmd, &o)?;
        let fill = doc.layer(&o.layer).is_some_and(|l| matches!(l.settings.mode, FillMode::Fill | FillMode::FillThenLine));
        let open = geom::region::has_open_subpaths(&path).map_err(geom_err(cmd))?;
        let (inside, outside): (Vec<BezPath>, Vec<BezPath>) = if fill && !open {
            let r = Region::from_path(&path, Fill::EvenOdd).map_err(geom_err(cmd))?;
            let a = r.boolean(&cutter, BoolOp::Intersect).map_err(geom_err(cmd))?;
            let b = r.boolean(&cutter, BoolOp::Difference).map_err(geom_err(cmd))?;
            (a.islands(), b.islands())
        } else {
            let lines = lines_of(cmd, &path)?;
            let a = clip_lines(&lines, &cutter, true).map_err(geom_err(cmd))?;
            let b = clip_lines(&lines, &cutter, false).map_err(geom_err(cmd))?;
            (join_pieces(a), join_pieces(b))
        };
        if inside.is_empty() || outside.is_empty() {
            continue; // not crossed by the tool
        }
        pieces_total += inside.len() + outside.len();
        if pieces_total > MAX_CREATED {
            return Err(CommandError::Failed(format!("{cmd}: the cut would make more than {MAX_CREATED} pieces")));
        }
        cut.push(id.clone());
        new_pieces.push((o.name.clone(), o.layer.clone(), inside, outside));
    }
    if cut.is_empty() {
        return Err(CommandError::Failed(format!("{cmd}: the cutting tool {tool_id} does not cross any of the shapes")));
    }
    let gin = doc.alloc_id("group")?;
    let gout = doc.alloc_id("group")?;
    for (name, layer, inside, outside) in &new_pieces {
        for p in inside {
            inside_ids.push(pathutil::add_field_object(doc, &format!("{name} inside"), layer, Some(gin.clone()), p)?);
        }
        for p in outside {
            outside_ids.push(pathutil::add_field_object(doc, &format!("{name} outside"), layer, Some(gout.clone()), p)?);
        }
    }
    doc.groups.push(Group::new(gin.clone(), "Cut inside", None));
    doc.groups.push(Group::new(gout.clone(), "Cut outside", None));
    let mut deleted = cut.clone();
    deleted.push(tool_id.clone());
    let groups = pathutil::delete_objects(doc, &deleted);
    let created: Vec<ObjectId> = inside_ids.iter().chain(&outside_ids).cloned().collect();
    let mut r = util::edit_result(doc, &created, &[]);
    r["inside"] = json!({ "group": gin, "ids": inside_ids });
    r["outside"] = json!({ "group": gout, "ids": outside_ids });
    r["deleted"] = json!(deleted);
    r["deleted_groups"] = json!(groups);
    Ok((r, format!("cut {} with {tool_id} into {} pieces", cut.join(", "), created.len())))
}
