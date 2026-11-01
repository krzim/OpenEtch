//! Arrange commands: `object.order` (draw order), `object.lock`, `object.make_same`, `object.rotate_to`,
//! `object.scale_to` (LightBurn's two-point rotate / scale).

use std::collections::BTreeSet;

use doc::{Document, ObjectId};
use kurbo::{Affine, Point};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::apply_affine;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Where `object.order` moves objects.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderTo {
    /// The end of the document's object list: drawn last, on top.
    Front,
    /// The start of the list: drawn first, underneath.
    Back,
    /// One step toward the front (past the next object that is not moving).
    Forward,
    /// One step toward the back.
    Backward,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrderInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// front, back, forward or backward.
    pub to: OrderTo,
}

/// `object.order`.
pub struct Order;

impl Command for Order {
    type Input = OrderInput;
    const ID: &'static str = "object.order";
    const TITLE: &'static str = "Change draw order";
    const DESCRIPTION: &'static str = "Move objects in the document's object list (LightBurn's draw order; the moved \
        objects keep their order among themselves): front = end of the list (drawn last, on top), back = start, \
        forward/backward one step. Jobs burn layer by layer and order each layer's shapes by nearest-neighbour \
        travel, so this only breaks ties there; it does not set the cut order. Returns `order` (all ids) and the \
        affected ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: OrderInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let ids = util::object_ids(cmd, doc, &i.ids)?;
        let sel: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
        let mut objs = doc.objects.clone();
        match i.to {
            OrderTo::Front | OrderTo::Back => {
                let (picked, rest): (Vec<_>, Vec<_>) = objs.into_iter().partition(|o| sel.contains(o.id.as_str()));
                objs = if i.to == OrderTo::Front { rest.into_iter().chain(picked).collect() } else { picked.into_iter().chain(rest).collect() };
            }
            OrderTo::Forward => {
                for k in (0..objs.len().saturating_sub(1)).rev() {
                    if sel.contains(objs[k].id.as_str()) && !sel.contains(objs[k + 1].id.as_str()) {
                        objs.swap(k, k + 1);
                    }
                }
            }
            OrderTo::Backward => {
                for k in 1..objs.len() {
                    if sel.contains(objs[k].id.as_str()) && !sel.contains(objs[k - 1].id.as_str()) {
                        objs.swap(k, k - 1);
                    }
                }
            }
        }
        if objs.iter().map(|o| &o.id).eq(doc.objects.iter().map(|o| &o.id)) {
            return Err(CommandError::Failed(format!("{cmd}: the objects are already there; nothing changed")));
        }
        doc.objects = objs;
        let mut r = util::edit_result(doc, &[], &ids);
        r["order"] = json!(doc.objects.iter().map(|o| &o.id).collect::<Vec<_>>());
        Ok((r, format!("moved {} {:?}", ids.join(", "), i.to).to_lowercase()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LockInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// true locks, false unlocks.
    pub locked: bool,
}

/// `object.lock`.
pub struct Lock;

impl Command for Lock {
    type Input = LockInput;
    const ID: &'static str = "object.lock";
    const TITLE: &'static str = "Lock / unlock";
    const DESCRIPTION: &'static str = "Lock or unlock objects (LightBurn's Lock Shapes, e.g. a jig outline). Locked \
        objects can still be selected, copied, renamed, grouped and burned, but every command that would move, \
        transform, reshape, relayer or delete them refuses with an error naming the object. The lock list is the \
        document's `locked`. Returns the affected ids and `locked`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: LockInput) -> Result<(Value, String)> {
        let ids = util::object_ids(Self::ID, doc, &i.ids)?;
        // Nothing to change is refused, so it does not leave an empty undo step.
        if ids.iter().all(|id| doc.is_locked(id) == i.locked) {
            let state = if i.locked { "locked" } else { "unlocked" };
            return Err(CommandError::Failed(format!("{}: {} already {state}; nothing changed", Self::ID, ids.join(", "))));
        }
        for id in &ids {
            if i.locked {
                doc.locked.insert(id.clone());
            } else {
                doc.locked.remove(id);
            }
        }
        doc.prune_locked();
        let mut r = util::edit_result(doc, &[], &ids);
        r["locked"] = json!(i.locked);
        Ok((r, format!("{} {}", if i.locked { "locked" } else { "unlocked" }, ids.join(", "))))
    }
}

/// Which size `object.make_same` matches.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Width,
    Height,
    Both,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MakeSameInput {
    /// Exact object or group ids in selection order (a group is one object), at least two. The reference is the
    /// last one, or the only locked one when exactly one is locked (LightBurn).
    pub ids: Vec<ObjectId>,
    /// width, height or both.
    pub dimension: Dimension,
    /// Scale the other dimension by the same factor (LightBurn's Shift). Not with both. Default false.
    #[serde(default)]
    pub keep_aspect: bool,
}

/// `object.make_same`.
pub struct MakeSame;

impl Command for MakeSame {
    type Input = MakeSameInput;
    const ID: &'static str = "object.make_same";
    const TITLE: &'static str = "Make same size";
    const DESCRIPTION: &'static str = "Resize objects to the field-frame width and/or height of a reference \
        (LightBurn's Make Same Width/Height): the last id, or the only locked one. Each other object or group scales \
        about its own bounds centre; keep_aspect scales the other dimension proportionally. Objects with no extent \
        in that dimension are skipped. Returns affected ids, `reference`, `skipped` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MakeSameInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.keep_aspect && i.dimension == Dimension::Both {
                return Err(invalid(cmd, "keep_aspect works with width or height, not both"));
            }
            let units = util::units(cmd, doc, &i.ids)?;
            if units.len() < 2 {
                return Err(invalid(cmd, "give at least two objects: the ones to resize and the reference"));
            }
            let locked: Vec<usize> =
                (0..units.len()).filter(|&k| units[k].members.iter().all(|m| doc.is_locked(m))).collect();
            let ref_k = if locked.len() == 1 { locked[0] } else { units.len() - 1 };
            let rb = units[ref_k].bounds(doc).ok_or_else(|| CommandError::Failed(format!("{cmd}: the reference has no size")))?;
            let mut affected = Vec::new();
            let mut skipped = Vec::new();
            for (k, u) in units.iter().enumerate() {
                if k == ref_k {
                    continue;
                }
                util::check_unlocked(cmd, doc, &u.members)?;
                let Some(b) = u.bounds(doc) else { continue };
                let fx = (rb.width() > 1e-9 && b.width() > 1e-9).then(|| rb.width() / b.width());
                let fy = (rb.height() > 1e-9 && b.height() > 1e-9).then(|| rb.height() / b.height());
                let (sx, sy) = match (i.dimension, fx, fy) {
                    (Dimension::Width, Some(f), _) => (f, if i.keep_aspect { f } else { 1.0 }),
                    (Dimension::Height, _, Some(f)) => (if i.keep_aspect { f } else { 1.0 }, f),
                    (Dimension::Both, Some(a), Some(b)) => (a, b),
                    _ => {
                        skipped.push(u.id.clone());
                        continue;
                    }
                };
                let c = b.center().to_vec2();
                apply_affine(doc, &u.members, Affine::translate(c) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-c));
                affected.extend(u.members.iter().cloned());
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: nothing could be resized")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["reference"] = json!(units[ref_k].id);
            r["skipped"] = json!(skipped);
            Ok((r, format!("made {} the same {:?} as {}", affected.join(", "), i.dimension, units[ref_k].id).to_lowercase()))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TwoPointInput {
    /// Exact object or group ids (a group stands for all its members), moved together.
    pub ids: Vec<ObjectId>,
    /// The fixed point [x, y] in field mm (the first click).
    pub pivot: [f64; 2],
    /// The point grabbed [x, y] in field mm (the second click).
    pub from: [f64; 2],
    /// Where `from` should end up [x, y] in field mm (the release).
    pub to: [f64; 2],
    /// object.scale_to only: also turn so `from` lands exactly on `to` (LightBurn's Shift-drag). Default false.
    #[serde(default)]
    pub rotate: bool,
}

fn two_points(cmd: &str, i: &TwoPointInput) -> Result<(Point, Point, Point)> {
    let p = util::finite2(cmd, "pivot", i.pivot)?;
    let f = util::finite2(cmd, "from", i.from)?;
    let t = util::finite2(cmd, "to", i.to)?;
    let (p, f, t) = (Point::new(p[0], p[1]), Point::new(f[0], f[1]), Point::new(t[0], t[1]));
    if f.distance(p) < 1e-6 || t.distance(p) < 1e-6 {
        return Err(invalid(cmd, "from and to must both differ from the pivot"));
    }
    Ok((p, f, t))
}

fn two_point_apply(cmd: &str, doc: &mut Document, ids: &[ObjectId], t: Affine, what: String) -> Result<(Value, String)> {
    util::check_unlocked(cmd, doc, ids)?;
    apply_affine(doc, ids, t);
    Ok((util::edit_result(doc, &[], ids), what))
}

/// `object.rotate_to`.
pub struct RotateTo;

impl Command for RotateTo {
    type Input = TwoPointInput;
    const ID: &'static str = "object.rotate_to";
    const TITLE: &'static str = "Two-point rotate";
    const DESCRIPTION: &'static str = "Rotate objects about `pivot` so the direction from the pivot to `from` turns \
        to point at `to` (LightBurn's two-point rotate; field mm). Distances are kept. Returns the affected ids, \
        `angle_deg` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TwoPointInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.rotate {
                return Err(invalid(cmd, "rotate is only for object.scale_to"));
            }
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let (p, f, t) = two_points(cmd, &i)?;
            let a = (t - p).atan2() - (f - p).atan2();
            let (mut r, s) =
                two_point_apply(cmd, doc, &ids, Affine::rotate_about(a, p), format!("rotated {} by {:.3}°", ids.join(", "), a.to_degrees()))?;
            r["angle_deg"] = json!(util::r4(a.to_degrees()));
            Ok((r, s))
        })
    }
}

/// `object.scale_to`.
pub struct ScaleTo;

impl Command for ScaleTo {
    type Input = TwoPointInput;
    const ID: &'static str = "object.scale_to";
    const TITLE: &'static str = "Two-point scale";
    const DESCRIPTION: &'static str = "Scale objects uniformly about `pivot` by |to - pivot| / |from - pivot| (field \
        mm), so a known length can be matched; with rotate true they also turn so `from` lands on `to` (LightBurn's \
        Shift two-point drag). Returns the affected ids, `factor`, `angle_deg` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TwoPointInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let (p, f, t) = two_points(cmd, &i)?;
            let k = t.distance(p) / f.distance(p);
            if !(k.is_finite() && k > 1e-6 && k < 1e6) {
                return Err(invalid(cmd, "the scale factor must be between 1e-6 and 1e6"));
            }
            let a = if i.rotate { (t - p).atan2() - (f - p).atan2() } else { 0.0 };
            let c = p.to_vec2();
            let m = Affine::translate(c) * Affine::rotate(a) * Affine::scale(k) * Affine::translate(-c);
            let (mut r, s) = two_point_apply(cmd, doc, &ids, m, format!("scaled {} by {:.4}", ids.join(", "), k))?;
            r["factor"] = json!(util::r4(k));
            r["angle_deg"] = json!(util::r4(a.to_degrees()));
            Ok((r, s))
        })
    }
}
