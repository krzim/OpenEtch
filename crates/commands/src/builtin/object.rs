//! Object commands: `object.transform`, `object.set_layer`, `object.rename`, `object.delete`, `object.duplicate`.

use std::collections::BTreeMap;

use doc::{Document, ObjectId};
use kurbo::{Affine, Point, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Uniform or per-axis scale factor.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Scale {
    /// The same factor on both axes, e.g. 2 doubles the size.
    Uniform(f64),
    /// [sx, sy] factors along field X and Y.
    PerAxis([f64; 2]),
}

impl Scale {
    fn factors(self) -> [f64; 2] {
        match self {
            Scale::Uniform(s) => [s, s],
            Scale::PerAxis(s) => s,
        }
    }
}

/// Which axis to mirror.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mirror {
    /// Negate X: flip left-right across a vertical line through the pivot.
    X,
    /// Negate Y: flip top-bottom across a horizontal line through the pivot.
    Y,
}

/// A named pivot.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamedPivot {
    /// Centre of the selection's combined field-frame bounding box.
    Center,
    /// The field origin [0, 0].
    Origin,
}

/// Pivot for rotate, scale and mirror.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Pivot {
    /// "center" or "origin".
    Named(NamedPivot),
    /// An explicit field-frame point [x, y] in mm.
    Point([f64; 2]),
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformInput {
    /// Exact object or group ids (a group stands for all its members); transformed together as one selection.
    pub ids: Vec<ObjectId>,
    /// Move by [dx, dy] mm in the field frame, applied last.
    pub translate: Option<[f64; 2]>,
    /// Rotate counter-clockwise by this many degrees about the pivot.
    pub rotate_deg: Option<f64>,
    /// Scale about the pivot: a number for uniform scale or [sx, sy]. Factors must be > 0 (use `mirror` to flip).
    pub scale: Option<Scale>,
    /// Mirror about the pivot: "x" negates X (left-right flip), "y" negates Y (top-bottom flip).
    pub mirror: Option<Mirror>,
    /// Pivot for scale, shear, mirror and rotate: "center" (default; the selection's combined bounds centre),
    /// "origin" (the field origin) or [x, y] in field mm.
    pub about: Option<Pivot>,
    /// Shear (skew) about the pivot by [x, y] degrees, each within ±89: x slants vertical lines (x moves by
    /// y·tan(x)), y slants horizontal lines (y moves by x·tan(y)).
    pub shear_deg: Option<[f64; 2]>,
    /// Mirror across the line through points a and b (field mm), after rotate and before translate.
    pub mirror_line: Option<MirrorLine>,
}

/// A mirror line through two distinct field-frame points.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MirrorLine {
    /// A point [x, y] on the line, mm.
    pub a: [f64; 2],
    /// Another point [x, y] on the line, mm.
    pub b: [f64; 2],
}

/// Reflection across the line through `a` and `b` (which must differ).
pub(super) fn reflection(a: Point, b: Point) -> Affine {
    let ang = 2.0 * (b - a).atan2();
    let (s, c) = ang.sin_cos();
    Affine::translate(a.to_vec2()) * Affine::new([c, s, s, -c, 0.0, 0.0]) * Affine::translate(-a.to_vec2())
}

/// `object.transform`.
pub struct Transform;

impl Command for Transform {
    type Input = TransformInput;
    const ID: &'static str = "object.transform";
    const TITLE: &'static str = "Transform objects";
    const DESCRIPTION: &'static str = "Move, rotate, scale, shear and/or mirror objects as one selection in the field \
        frame (mm, degrees counter-clockwise). Order: scale, shear, mirror, rotate (all about the pivot, default the \
        selection's bounds centre), then mirror_line (across a line through two points), then translate. Locked \
        objects are refused. Returns the affected ids and their resulting field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TransformInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let ids = util::object_ids(id, doc, &i.ids)?;
            util::check_unlocked(id, doc, &ids)?;
            let pivot = match i.about.unwrap_or(Pivot::Named(NamedPivot::Center)) {
                Pivot::Named(NamedPivot::Center) => doc.bounds_of(&ids).map(|b| b.center()).unwrap_or(Point::ORIGIN),
                Pivot::Named(NamedPivot::Origin) => Point::ORIGIN,
                Pivot::Point(p) => {
                    let [x, y] = util::finite2(id, "about", p)?;
                    Point::new(x, y)
                }
            };
            let mut local = Affine::IDENTITY;
            let mut parts = Vec::new();
            if let Some(s) = i.scale {
                let [sx, sy] = s.factors();
                util::positive(id, "scale", sx)?;
                util::positive(id, "scale", sy)?;
                local = Affine::scale_non_uniform(sx, sy) * local;
                parts.push(format!("scale {sx}x{sy}"));
            }
            if let Some(h) = i.shear_deg {
                let [hx, hy] = util::finite2(id, "shear_deg", h)?;
                if hx.abs() > 89.0 || hy.abs() > 89.0 {
                    return Err(invalid(id, "shear_deg values must be within ±89"));
                }
                local = Affine::skew(hx.to_radians().tan(), hy.to_radians().tan()) * local;
                parts.push(format!("shear [{hx}°, {hy}°]"));
            }
            if let Some(m) = i.mirror {
                local = match m {
                    Mirror::X => Affine::FLIP_X,
                    Mirror::Y => Affine::FLIP_Y,
                } * local;
                parts.push(format!("mirror {}", if matches!(m, Mirror::X) { "x" } else { "y" }));
            }
            if let Some(r) = i.rotate_deg {
                util::finite(id, "rotate_deg", r)?;
                local = Affine::rotate(r.to_radians()) * local;
                parts.push(format!("rotate {r}°"));
            }
            let mut t = Affine::translate(pivot.to_vec2()) * local * Affine::translate(-pivot.to_vec2());
            if let Some(m) = i.mirror_line {
                let [ax, ay] = util::finite2(id, "mirror_line.a", m.a)?;
                let [bx, by] = util::finite2(id, "mirror_line.b", m.b)?;
                let (a, b) = (Point::new(ax, ay), Point::new(bx, by));
                if a.distance(b) < 1e-9 {
                    return Err(invalid(id, "mirror_line needs two different points"));
                }
                t = reflection(a, b) * t;
                parts.push(format!("mirror across [{ax}, {ay}]-[{bx}, {by}]"));
            }
            if let Some(d) = i.translate {
                let [dx, dy] = util::finite2(id, "translate", d)?;
                t = Affine::translate((dx, dy)) * t;
                parts.push(format!("move [{dx}, {dy}]"));
            }
            if parts.is_empty() {
                return Err(invalid(
                    id,
                    "nothing to do: give at least one of translate, rotate_deg, scale, shear_deg, mirror, mirror_line",
                ));
            }
            apply_affine(doc, &ids, t);
            Ok((util::edit_result(doc, &[], &ids), format!("{} on {}", parts.join(", "), ids.join(", "))))
        })
    }
}

/// Pre-multiply each object's transform by the field-frame affine `t`.
pub(super) fn apply_affine(doc: &mut Document, ids: &[ObjectId], t: Affine) {
    for o in doc.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
        o.transform = t * o.transform;
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLayerInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// Exact id of the destination layer.
    pub layer: String,
}

/// `object.set_layer`.
pub struct SetLayer;

impl Command for SetLayer {
    type Input = SetLayerInput;
    const ID: &'static str = "object.set_layer";
    const TITLE: &'static str = "Set layer";
    const DESCRIPTION: &'static str = "Move objects to another layer, so they are marked with that layer's settings. \
        Geometry is unchanged.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetLayerInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let ids = util::object_ids(Self::ID, doc, &i.ids)?;
            util::check_unlocked(Self::ID, doc, &ids)?;
            let layer = util::layer(doc, &i.layer)?.id.clone();
            for o in doc.objects.iter_mut().filter(|o| ids.contains(&o.id)) {
                o.layer = layer.clone();
            }
            let mut r = util::edit_result(doc, &[], &ids);
            r["layer"] = json!(layer);
            Ok((r, format!("moved {} to {layer}", ids.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameInput {
    /// Exact object id.
    pub id: ObjectId,
    /// New display name.
    pub name: String,
}

/// `object.rename`.
pub struct Rename;

impl Command for Rename {
    type Input = RenameInput;
    const ID: &'static str = "object.rename";
    const TITLE: &'static str = "Rename object";
    const DESCRIPTION: &'static str = "Set an object's display name. Ids never change.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RenameInput) -> Result<(Value, String)> {
        let id = util::object(doc, &i.id)?.id.clone();
        let name = util::name(Self::ID, "name", &i.name)?;
        let summary = format!("renamed {id} to \"{name}\"");
        doc.object_mut(&id).ok_or_else(|| CommandError::Failed(format!("object {id} vanished")))?.name = name;
        Ok((util::edit_result(doc, &[], &[id]), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteInput {
    /// Exact object or group ids to delete (a group deletes all its members).
    pub ids: Vec<ObjectId>,
}

/// `object.delete`.
pub struct Delete;

impl Command for Delete {
    type Input = DeleteInput;
    const ID: &'static str = "object.delete";
    const TITLE: &'static str = "Delete objects";
    const DESCRIPTION: &'static str = "Delete objects by exact id (undoable). Fails without deleting anything if any \
        id does not exist or is locked.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: DeleteInput) -> Result<(Value, String)> {
        let ids = util::object_ids(Self::ID, doc, &i.ids)?;
        util::check_unlocked(Self::ID, doc, &ids)?;
        let bounds = util::bounds_json(doc, &ids);
        doc.objects.retain(|o| !ids.contains(&o.id));
        doc.prune_locked();
        // Deleting the last member of a group removes the group.
        let groups = doc.prune_groups();
        let summary = format!("deleted {}", ids.join(", "));
        Ok((json!({ "created": [], "affected": [], "deleted": ids, "deleted_groups": groups, "bounds": bounds }), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DuplicateInput {
    /// Exact object or group ids to copy (a fully copied group is copied as a group).
    pub ids: Vec<ObjectId>,
    /// Offset of the copies from the originals, [dx, dy] mm in the field frame. Default [0, 0] (in place).
    #[serde(default)]
    pub offset: [f64; 2],
}

/// `object.duplicate`.
pub struct Duplicate;

impl Command for Duplicate {
    type Input = DuplicateInput;
    const ID: &'static str = "object.duplicate";
    const TITLE: &'static str = "Duplicate objects";
    const DESCRIPTION: &'static str = "Copy objects (same layer and name plus \" copy\"), offset by [dx, dy] mm in the \
        field frame (default in place). Returns the new ids in the same order as `ids` and their bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: DuplicateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let ids = util::object_ids(Self::ID, doc, &i.ids)?;
            let [dx, dy] = util::finite2(Self::ID, "offset", i.offset)?;
            let created = copy_objects(doc, &ids, Vec2::new(dx, dy), " copy")?;
            let mut r = util::edit_result(doc, &created, &[]);
            r["copies"] =
                ids.iter().zip(&created).map(|(a, b)| (a.clone(), json!(b))).collect::<serde_json::Map<_, _>>().into();
            Ok((r, format!("duplicated {} as {}", ids.join(", "), created.join(", "))))
        })
    }
}

/// Append translated copies of `ids` (which must exist) and return the new ids in the same order.
///
/// Groups: a group whose members are all copied is copied too (name plus `suffix`, same nesting, the copy nested in
/// the copy of its parent when that is copied as well), and the copies belong to it. A copy of an object whose group
/// is only partly copied joins the original group.
pub(super) fn copy_objects(
    doc: &mut Document,
    ids: &[ObjectId],
    offset: Vec2,
    suffix: &str,
) -> Result<Vec<ObjectId>> {
    let originals: Vec<_> = ids.iter().filter_map(|id| doc.object(id).cloned()).collect();
    let mut groups: Vec<(usize, doc::Group)> = Vec::new();
    for o in &originals {
        let Some(g) = o.group.as_deref() else { continue };
        for c in doc.group_chain(g) {
            if groups.iter().all(|(_, have)| have.id != c.id) && doc.group_members(&c.id).iter().all(|m| ids.contains(m))
            {
                groups.push((doc.group_chain(&c.id).len(), c.clone()));
            }
        }
    }
    // Parents first, so a nested copy can name its parent's copy.
    groups.sort_by_key(|(depth, _)| *depth);
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for (_, mut g) in groups {
        let old = std::mem::replace(&mut g.id, doc.alloc_id("group")?);
        g.name.push_str(suffix);
        g.parent = g.parent.map(|p| renamed.get(&p).cloned().unwrap_or(p));
        renamed.insert(old, g.id.clone());
        doc.groups.push(g);
    }
    originals
        .into_iter()
        .map(|mut o| {
            o.id = doc.alloc_id("obj")?;
            o.name.push_str(suffix);
            o.transform = Affine::translate(offset) * o.transform;
            o.group = o.group.map(|g| renamed.get(&g).cloned().unwrap_or(g));
            let id = o.id.clone();
            doc.objects.push(o);
            Ok(id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::shape::ShapeRect;

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    /// A 10x4 rect at [5, 0] (bounds x 0..10, y -2..2) as obj-2.
    fn doc() -> Document {
        let mut d = Document::default();
        run::<ShapeRect>(&mut d, json!({"width_mm": 10, "height_mm": 4, "center": [5, 0]})).unwrap();
        d
    }

    #[test]
    fn translate_and_rotate_about_center() {
        let mut d = doc();
        let (r, _) =
            run::<Transform>(&mut d, json!({"ids": ["obj-2"], "rotate_deg": 90, "translate": [1, 1]})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([4.0, -4.0]));
        assert_eq!(r["bounds"]["max"], json!([8.0, 6.0]));
        assert_eq!(r["affected"], json!(["obj-2"]));
    }

    #[test]
    fn scale_about_origin_and_point() {
        let mut d = doc();
        let (r, _) = run::<Transform>(&mut d, json!({"ids": ["obj-2"], "scale": 2, "about": "origin"})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([0.0, -4.0]));
        assert_eq!(r["bounds"]["max"], json!([20.0, 4.0]));
        let (r, _) = run::<Transform>(&mut d, json!({"ids": ["obj-2"], "scale": [0.5, 1], "about": [20, 0]})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([10.0, -4.0]));
    }

    #[test]
    fn mirror_x_about_origin() {
        let mut d = doc();
        let (r, _) = run::<Transform>(&mut d, json!({"ids": ["obj-2"], "mirror": "x", "about": "origin"})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([-10.0, -2.0]));
        assert_eq!(r["bounds"]["max"], json!([0.0, 2.0]));
    }

    #[test]
    fn transform_refuses_empty_and_bad_input() {
        let mut d = doc();
        assert!(run::<Transform>(&mut d, json!({"ids": ["obj-2"]})).is_err());
        assert!(run::<Transform>(&mut d, json!({"ids": ["obj-2"], "scale": 0})).is_err());
        assert!(run::<Transform>(&mut d, json!({"ids": ["obj-2"], "mirror": "z"})).is_err());
        assert!(matches!(
            run::<Transform>(&mut d, json!({"ids": ["obj-3"], "scale": 2})),
            Err(CommandError::NotFound { .. })
        ));
    }

    #[test]
    fn set_layer_rename_delete() {
        let mut d = doc();
        d.layers.push(doc::Layer::new("layer-7".into(), "Cut", "#f00"));
        run::<SetLayer>(&mut d, json!({"ids": ["obj-2"], "layer": "layer-7"})).unwrap();
        assert_eq!(d.objects[0].layer, "layer-7");
        assert!(run::<SetLayer>(&mut d, json!({"ids": ["obj-2"], "layer": "cut"})).is_err());
        run::<Rename>(&mut d, json!({"id": "obj-2", "name": "Border"})).unwrap();
        assert_eq!(d.objects[0].name, "Border");
        assert!(run::<Delete>(&mut d, json!({"ids": ["obj-2", "obj-99"]})).is_err());
        assert_eq!(d.objects.len(), 1);
        let (r, _) = run::<Delete>(&mut d, json!({"ids": ["obj-2"]})).unwrap();
        assert_eq!(r["deleted"], json!(["obj-2"]));
        assert!(d.objects.is_empty());
    }

    #[test]
    fn group_ids_duplicate_and_delete_as_groups() {
        let mut d = doc();
        run::<ShapeRect>(&mut d, json!({"width_mm": 2, "height_mm": 2})).unwrap(); // obj-3
        run::<ShapeRect>(&mut d, json!({"width_mm": 2, "height_mm": 2})).unwrap(); // obj-4
        d.groups.push(doc::Group::new("group-8".into(), "Outer", None));
        d.groups.push(doc::Group::new("group-9".into(), "Inner", Some("group-8".into())));
        d.object_mut("obj-2").unwrap().group = Some("group-8".into());
        d.object_mut("obj-3").unwrap().group = Some("group-9".into());
        // Copying the whole outer group copies both groups with their nesting.
        let (r, _) = run::<Duplicate>(&mut d, json!({"ids": ["group-8"]})).unwrap();
        let created: Vec<String> = serde_json::from_value(r["created"].clone()).unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(d.groups.len(), 4);
        let inner_copy = d.object(&created[1]).unwrap().group.clone().unwrap();
        let outer_copy = d.object(&created[0]).unwrap().group.clone().unwrap();
        assert_eq!(d.group(&inner_copy).unwrap().parent.as_deref(), Some(outer_copy.as_str()));
        assert_eq!(d.group(&outer_copy).unwrap().name, "Outer copy");
        // Copying one member of a group puts the copy in the same group.
        let (r, _) = run::<Duplicate>(&mut d, json!({"ids": ["obj-2"]})).unwrap();
        let one = r["created"][0].as_str().unwrap();
        assert_eq!(d.object(one).unwrap().group.as_deref(), Some("group-8"));
        // Deleting the inner group's only member removes the inner group.
        let (r, _) = run::<Delete>(&mut d, json!({"ids": ["obj-3"]})).unwrap();
        assert_eq!(r["deleted_groups"], json!(["group-9"]));
        assert!(d.group("group-9").is_none() && d.group("group-8").is_some());
        // A group id deletes every member.
        let (r, _) = run::<Delete>(&mut d, json!({"ids": ["group-8"]})).unwrap();
        assert_eq!(r["deleted"], json!(["obj-2", one]));
        assert!(d.group("group-8").is_none());
        assert!(matches!(run::<Delete>(&mut d, json!({"ids": ["group-77"]})), Err(CommandError::NotFound { .. })));
    }

    #[test]
    fn duplicate_with_offset() {
        let mut d = doc();
        let (r, _) = run::<Duplicate>(&mut d, json!({"ids": ["obj-2"], "offset": [0, 10]})).unwrap();
        assert_eq!(r["created"], json!(["obj-3"]));
        assert_eq!(r["copies"]["obj-2"], json!("obj-3"));
        assert_eq!(r["bounds"]["min"], json!([0.0, 8.0]));
        assert_eq!(d.objects[1].name, "Rectangle copy");
    }
}
