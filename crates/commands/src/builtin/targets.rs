//! Start points and registration targets: `object.set_start_point` / `object.clear_start_point` (LightBurn's Set
//! Start Point) and `object.align_to_targets` (LightBurn's Print and Cut, here with positions found with the red
//! dot on a galvo).

use doc::{Document, Geometry, ObjectId, StartPoint};
use geom::nodes::NodePath;
use kurbo::{Affine, Point, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetStartPointInput {
    /// Exact object id (not an image).
    pub id: ObjectId,
    /// The node to start at, numbered across subpaths as `node.list` numbers them.
    pub node: usize,
    /// Run the outline the other way (LightBurn: Shift+click). Default false.
    #[serde(default)]
    pub reverse: bool,
}

/// `object.set_start_point`.
pub struct SetStartPoint;

impl Command for SetStartPoint {
    type Input = SetStartPointInput;
    const ID: &'static str = "object.set_start_point";
    const TITLE: &'static str = "Set start point";
    const DESCRIPTION: &'static str = "Set where a shape's closed outline starts (and, with `reverse`, run it the other \
        way), LightBurn's Set Start Point. `node` is a node index as node.list gives it; the outline through that \
        node starts there. As in LightBurn, the Optimization Settings' 'choose best starting point' overrides it \
        while on (job.set_optimization {best_start: false} to use it). Fill and image passes are not affected.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetStartPointInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let o = util::object(doc, &i.id)?;
        if matches!(o.geometry, Geometry::Image { .. }) {
            return Err(invalid(cmd, format!("{} is an image; images have no start point", o.id)));
        }
        let nodes = NodePath::from_path(&o.geometry.local_path()).nodes();
        let Some(n) = nodes.get(i.node) else {
            return Err(invalid(cmd, format!("{} has nodes 0..{}; got {}", o.id, nodes.len(), i.node)));
        };
        let local = [n.point.x, n.point.y];
        let field = o.transform * n.point;
        let id = o.id.clone();
        let options = doc.object_options.entry(id.clone()).or_default();
        options.start = Some(StartPoint { node: i.node, local, reverse: i.reverse });
        Ok((
            json!({ "created": [], "affected": [id], "start": [util::r4(field.x), util::r4(field.y)], "reverse": i.reverse, "bounds": Value::Null }),
            format!("set the start point of {id} to node {}", i.node),
        ))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearStartPointInput {
    /// Object or group ids.
    pub ids: Vec<String>,
}

/// `object.clear_start_point`.
pub struct ClearStartPoint;

impl Command for ClearStartPoint {
    type Input = ClearStartPointInput;
    const ID: &'static str = "object.clear_start_point";
    const TITLE: &'static str = "Clear start point";
    const DESCRIPTION: &'static str = "Restore the default start point and direction of shapes (LightBurn: Ctrl+click a \
        node in Set Start Point).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ClearStartPointInput) -> Result<(Value, String)> {
        let ids = util::object_ids(Self::ID, doc, &i.ids)?;
        let mut affected = Vec::new();
        for id in &ids {
            if let Some(o) = doc.object_options.get_mut(id)
                && o.start.take().is_some()
            {
                affected.push(id.clone());
                if o.is_default() {
                    doc.object_options.remove(id);
                }
            }
        }
        if affected.is_empty() {
            return Err(invalid(Self::ID, "none of these shapes has a start point"));
        }
        let n = affected.len();
        Ok((json!({ "created": [], "affected": affected, "bounds": Value::Null }), format!("cleared {n} start point(s)")))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlignTargetsInput {
    /// The two registration targets in the design: object or group ids (a group counts as one target, by the
    /// centre of its bounds, as LightBurn aligns to "the centre of each target object or Group").
    pub targets: [String; 2],
    /// Where those targets really are on the material, field mm (found with the red dot).
    pub positions: [[f64; 2]; 2],
    /// Also scale by the ratio of the distances (LightBurn's "Scaled"); default false ("No Scaling").
    #[serde(default)]
    pub scale: bool,
    /// Objects or groups to move. Default: every object of the document.
    pub ids: Option<Vec<String>>,
}

/// `object.align_to_targets`.
pub struct AlignTargets;

/// Least distance between the two targets (design and measured), mm: closer targets give no usable rotation.
pub const MIN_TARGET_SEPARATION_MM: f64 = 5.0;

impl Command for AlignTargets {
    type Input = AlignTargetsInput;
    const ID: &'static str = "object.align_to_targets";
    const TITLE: &'static str = "Align to registration targets";
    const DESCRIPTION: &'static str = "Print and Cut: move (rotate, and with `scale` resize) the design so its two \
        registration targets land on the positions measured on the material (field mm; find them with the red \
        dot, POST /api/machine/pointer). The first target maps exactly onto the first position and the second lies \
        on the line through the second. Refused when the targets are closer than 5 mm. Returns the transform's \
        rotation, scale and translation.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AlignTargetsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.targets[0] == i.targets[1] {
                return Err(invalid(cmd, "select a different graphic for the second target"));
            }
            let centre = |id: &String| -> Result<Point> {
                let members = util::object_ids(cmd, doc, std::slice::from_ref(id))?;
                doc.bounds_of(&members).map(|b| b.center()).ok_or_else(|| invalid(cmd, format!("target {id} has no outline")))
            };
            let (a, b) = (centre(&i.targets[0])?, centre(&i.targets[1])?);
            let pa = util::finite2(cmd, "positions[0]", i.positions[0])?;
            let pb = util::finite2(cmd, "positions[1]", i.positions[1])?;
            let (pa, pb) = (Point::new(pa[0], pa[1]), Point::new(pb[0], pb[1]));
            let (d, m): (Vec2, Vec2) = (b - a, pb - pa);
            if d.hypot() < MIN_TARGET_SEPARATION_MM || m.hypot() < MIN_TARGET_SEPARATION_MM {
                return Err(invalid(cmd, format!("the targets are closer than {MIN_TARGET_SEPARATION_MM} mm; spread them further apart")));
            }
            let angle = m.atan2() - d.atan2();
            let k = if i.scale { m.hypot() / d.hypot() } else { 1.0 };
            let t = Affine::translate(pa.to_vec2()) * Affine::rotate(angle) * Affine::scale(k) * Affine::translate(-a.to_vec2());
            let ids = match &i.ids {
                Some(ids) => util::object_ids(cmd, doc, ids)?,
                None => doc.objects.iter().map(|o| o.id.clone()).collect(),
            };
            util::check_unlocked(cmd, doc, &ids)?;
            for id in &ids {
                let o = doc.object_mut(id).expect("listed");
                o.transform = t * o.transform;
                if let Some(p) = o.placement_problem() {
                    return Err(invalid(cmd, format!("{id}: {p}")));
                }
            }
            let mut r = util::edit_result(doc, &[], &ids);
            r["rotation_deg"] = json!(util::r4(angle.to_degrees()));
            r["scale"] = json!(k);
            r["residual_mm"] = json!(util::r4(((t * b) - pb).hypot()));
            Ok((r, format!("aligned {} object(s) to the registration targets", ids.len())))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Invocation, Origin, Session};
    use doc::Document;
    use serde_json::{Value, json};

    fn run(s: &mut Session, id: &str, params: Value) -> std::result::Result<Value, String> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result).map_err(|e| e.to_string())
    }

    fn rect(s: &mut Session, x: f64, y: f64, w: f64) -> String {
        let r = run(s, "shape.rect", json!({ "width_mm": w, "height_mm": w, "center": [x, y] })).unwrap();
        r["id"].as_str().or_else(|| r["created"][0].as_str()).unwrap().to_string()
    }

    #[test]
    fn start_point_set_and_clear() {
        let mut s = Session::new(Document::default());
        let id = rect(&mut s, 10.0, 0.0, 4.0);
        let r = run(&mut s, "object.set_start_point", json!({ "id": id, "node": 2, "reverse": true })).unwrap();
        assert_eq!(r["reverse"], true);
        let o = s.document().object_options[&id].start.clone().unwrap();
        assert_eq!(o.node, 2);
        assert!(run(&mut s, "object.set_start_point", json!({ "id": id, "node": 99 })).is_err());
        run(&mut s, "object.clear_start_point", json!({ "ids": [id] })).unwrap();
        assert!(!s.document().object_options.contains_key(&id));
        assert!(run(&mut s, "object.clear_start_point", json!({ "ids": [id] })).is_err());
    }

    #[test]
    fn align_to_targets_rotates_and_scales() {
        let mut s = Session::new(Document::default());
        let a = rect(&mut s, -20.0, 0.0, 2.0);
        let b = rect(&mut s, 20.0, 0.0, 2.0);
        let art = rect(&mut s, 0.0, 10.0, 6.0);
        // The material is turned 90° and shifted: the targets were found at (5, -20) and (5, 20).
        let r = run(&mut s, "object.align_to_targets", json!({ "targets": [a, b], "positions": [[5.0, -20.0], [5.0, 20.0]] })).unwrap();
        assert!((r["rotation_deg"].as_f64().unwrap() - 90.0).abs() < 1e-6);
        assert!(r["residual_mm"].as_f64().unwrap() < 1e-6);
        let c = s.document().object(&art).unwrap().bounds().center();
        assert!((c.x - (5.0 - 10.0)).abs() < 1e-6 && c.y.abs() < 1e-6, "{c:?}");
        // Scaled: positions twice as far apart.
        let mut t = Session::new(Document::default());
        let a = rect(&mut t, -10.0, 0.0, 2.0);
        let b = rect(&mut t, 10.0, 0.0, 2.0);
        let r = run(&mut t, "object.align_to_targets", json!({ "targets": [a, b], "positions": [[-20.0, 0.0], [20.0, 0.0]], "scale": true })).unwrap();
        assert!((r["scale"].as_f64().unwrap() - 2.0).abs() < 1e-9);
        assert!(run(&mut t, "object.align_to_targets", json!({ "targets": [a, a], "positions": [[0.0, 0.0], [10.0, 0.0]] })).is_err());
        assert!(run(&mut t, "object.align_to_targets", json!({ "targets": [a, b], "positions": [[0.0, 0.0], [1.0, 0.0]] })).is_err());
    }
}
