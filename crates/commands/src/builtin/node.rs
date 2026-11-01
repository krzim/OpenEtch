//! Node editing commands (`node.*`), the backend of LightBurn's Edit Nodes. Paths stay in the object's own frame
//! under its transform; every point going in or out is in field mm. Node and segment indices are those `node.list`
//! returns (see `geom::nodes` for the numbering). Rectangles and ellipses are listed as the path they draw and
//! become paths when edited; images have no nodes.

use doc::{Document, ObjectId};
use geom::nodes::{Handle, NodeKind, NodePath, SegKind};
use kurbo::Affine;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, geom_err};
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

fn load(cmd: &str, doc: &Document, id: &str, edit: bool) -> Result<(doc::Object, NodePath)> {
    let o = pathutil::single_object(cmd, doc, id, edit)?;
    let local = pathutil::vector_local(cmd, &o)?;
    let np = NodePath::from_path(&local);
    if np.subs.is_empty() {
        return Err(invalid(cmd, format!("{id} has no nodes")));
    }
    Ok((o, np))
}

fn store(doc: &mut Document, id: &str, np: &NodePath) -> Result<()> {
    pathutil::set_local_path(doc, id, np.to_path())
}

fn kind_name(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Corner => "corner",
        NodeKind::Smooth => "smooth",
        NodeKind::Symmetric => "symmetric",
    }
}

/// The node list as JSON, points in field mm.
fn listing(o: &doc::Object, np: &NodePath) -> Value {
    let t: Affine = o.transform;
    let nodes: Vec<Value> = np
        .nodes()
        .into_iter()
        .map(|n| {
            json!({
                "index": n.index,
                "subpath": n.subpath,
                "point": pathutil::to_field(t, n.point),
                "type": kind_name(n.kind),
                "handle_in": n.handle_in.map(|p| pathutil::to_field(t, p)),
                "handle_out": n.handle_out.map(|p| pathutil::to_field(t, p)),
                "segment_in": n.seg_in,
                "segment_out": n.seg_out,
                "endpoint": n.endpoint,
            })
        })
        .collect();
    let segments: Vec<Value> = np
        .segments()
        .into_iter()
        .map(|s| {
            let kind = if s.kind == SegKind::Line { "line" } else { "curve" };
            json!({ "index": s.index, "subpath": s.subpath, "type": kind, "from": s.from, "to": s.to })
        })
        .collect();
    let mut first = 0;
    let subpaths: Vec<Value> = np
        .subs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let v = json!({ "index": i, "closed": s.closed, "first_node": first, "node_count": s.node_count() });
            first += s.node_count();
            v
        })
        .collect();
    json!({
        "id": o.id,
        "kind": o.geometry.kind_name(),
        "converts_on_edit": !matches!(o.geometry, doc::Geometry::Path { .. }),
        "nodes": nodes,
        "segments": segments,
        "subpaths": subpaths,
    })
}

fn edited(doc: &Document, id: &ObjectId, extra: Value) -> Value {
    let mut r = util::edit_result(doc, &[], std::slice::from_ref(id));
    if let (Some(obj), Some(more)) = (r.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    r
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeListInput {
    /// Exact object id (not a group).
    pub id: ObjectId,
}

/// `node.list`.
pub struct NodeList;

impl Command for NodeList {
    type Input = NodeListInput;
    const ID: &'static str = "node.list";
    const TITLE: &'static str = "List nodes";
    const DESCRIPTION: &'static str = "List an object's nodes and segments in field mm: per node its index, point, \
        type (corner, smooth, symmetric; read from the handles), handle_in/handle_out (Bézier control points, null on \
        a line side), adjacent segment indices and whether it ends an open subpath; per segment its type (line or \
        curve) and end nodes; per subpath whether it is closed. Indices are what the other node.* commands take. \
        Rectangles and ellipses list the path they draw (editing converts them).";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: NodeListInput) -> Result<(Value, String)> {
        let (o, np) = load(Self::ID, doc, &i.id, false)?;
        Ok((listing(&o, &np), format!("listed {} nodes of {}", np.node_count(), o.id)))
    }
}

/// A handle side.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandleSide {
    /// The control point on the incoming curve.
    In,
    /// The control point on the outgoing curve.
    Out,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeMoveInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Node index from node.list.
    pub index: usize,
    /// Target point [x, y] in field mm.
    pub to: [f64; 2],
    /// Move this handle instead of the node: "in" or "out". The node's side must be a curve.
    pub handle: Option<HandleSide>,
}

/// `node.move`.
pub struct NodeMove;

impl Command for NodeMove {
    type Input = NodeMoveInput;
    const ID: &'static str = "node.move";
    const TITLE: &'static str = "Move node";
    const DESCRIPTION: &'static str = "Move a node to `to` (field mm), its handles moving along; or, with handle \
        \"in\"/\"out\", move that Bézier handle (on a smooth node the other handle turns to stay opposite; on a \
        symmetric one it also matches the length). A line side has no handle: make it a curve first with \
        node.segment_type. Returns the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeMoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let to = pathutil::to_local(cmd, "to", o.transform, i.to)?;
            match i.handle {
                None => np.move_node(i.index, to),
                Some(HandleSide::In) => np.move_handle(i.index, Handle::In, to),
                Some(HandleSide::Out) => np.move_handle(i.index, Handle::Out, to),
            }
            .map_err(geom_err(cmd))?;
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({})), format!("moved node {} of {}", i.index, o.id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeInsertInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Segment index from node.list.
    pub segment: usize,
    /// Where on the segment, 0 < t < 1 (Bézier parameter; 0.5 is the middle of a line).
    pub t: f64,
}

/// `node.insert`.
pub struct NodeInsert;

impl Command for NodeInsert {
    type Input = NodeInsertInput;
    const ID: &'static str = "node.insert";
    const TITLE: &'static str = "Insert node";
    const DESCRIPTION: &'static str = "Split a segment at parameter t (0 < t < 1) with a new node; the shape does \
        not change. Returns the new node's `index`, the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeInsertInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let n = np.insert(i.segment, i.t).map_err(geom_err(cmd))?;
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({ "index": n })), format!("inserted node {n} in {}", o.id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDeleteInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Node indices from node.list (at least one).
    pub indices: Vec<usize>,
}

/// `node.delete`.
pub struct NodeDelete;

impl Command for NodeDelete {
    type Input = NodeDeleteInput;
    const ID: &'static str = "node.delete";
    const TITLE: &'static str = "Delete nodes";
    const DESCRIPTION: &'static str = "Delete nodes: the segments on either side become one (a line when both were \
        lines, else a curve keeping the outer handles); deleting an end of an open path drops its end segment, and \
        subpaths left too small go. Refuses to delete every node (delete the object instead). Returns the affected \
        id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeDeleteInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.indices.is_empty() {
                return Err(invalid(cmd, "indices must list at least one node"));
            }
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            np.delete(&i.indices).map_err(geom_err(cmd))?;
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({})), format!("deleted {} nodes of {}", i.indices.len(), o.id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeBreakInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Node index from node.list.
    pub index: usize,
}

/// `node.break`.
pub struct NodeBreak;

impl Command for NodeBreak {
    type Input = NodeBreakInput;
    const ID: &'static str = "node.break";
    const TITLE: &'static str = "Break at node";
    const DESCRIPTION: &'static str = "Break the path at a node: a closed subpath opens there (the node becomes both \
        of its ends), an open one splits in two. The ends stay in place, so the drawing looks the same. Returns the \
        two end node indices as `ends`, the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeBreakInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let (a, b) = np.break_at(i.index).map_err(geom_err(cmd))?;
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({ "ends": [a, b] })), format!("broke {} at node {}", o.id, i.index)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeJoinInput {
    /// Exact object id.
    pub id: ObjectId,
    /// One end node (of an open subpath).
    pub a: usize,
    /// The other end node.
    pub b: usize,
}

/// `node.join`.
pub struct NodeJoin;

impl Command for NodeJoin {
    type Input = NodeJoinInput;
    const ID: &'static str = "node.join";
    const TITLE: &'static str = "Join nodes";
    const DESCRIPTION: &'static str = "Join two end nodes of open subpaths of one object into one node at their \
        midpoint: the two ends of one subpath close it; ends of two subpaths connect them into one. To join separate \
        objects use path.auto_join. Returns the joined node's `index`, the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeJoinInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let n = np.join_nodes(i.a, i.b).map_err(geom_err(cmd))?;
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({ "index": n })), format!("joined nodes {} and {} of {}", i.a, i.b, o.id)))
        })
    }
}

/// A node type to set.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeTypeArg {
    /// A sharp point: both handles are pulled onto the node.
    Corner,
    /// The two sides line up (straight sides next to the node become curves); handle lengths kept.
    Smooth,
    /// Smooth with both handles the same length.
    Symmetric,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSetTypeInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Node indices from node.list (at least one).
    pub indices: Vec<usize>,
    /// corner, smooth or symmetric.
    #[serde(rename = "type")]
    pub kind: NodeTypeArg,
}

/// `node.set_type`.
pub struct NodeSetType;

impl Command for NodeSetType {
    type Input = NodeSetTypeInput;
    const ID: &'static str = "node.set_type";
    const TITLE: &'static str = "Set node type";
    const DESCRIPTION: &'static str = "Set node types: corner pulls both handles onto the node (a sharp point); \
        smooth lines up the two sides through the node (adjacent lines become curves), keeping handle lengths; \
        symmetric also equalises them. Ends of open subpaths only take corner and are skipped otherwise. Returns \
        `changed` node indices, the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeSetTypeInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.indices.is_empty() {
                return Err(invalid(cmd, "indices must list at least one node"));
            }
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let kind = match i.kind {
                NodeTypeArg::Corner => NodeKind::Corner,
                NodeTypeArg::Smooth => NodeKind::Smooth,
                NodeTypeArg::Symmetric => NodeKind::Symmetric,
            };
            let changed = np.set_kind(&i.indices, kind).map_err(geom_err(cmd))?;
            if changed.is_empty() {
                return Err(invalid(
                    cmd,
                    "nothing to change: these nodes already have that type, or cannot take it (ends of open paths are \
                     always corners)",
                ));
            }
            store(doc, &o.id, &np)?;
            let summary = format!("made nodes {changed:?} of {} {}", o.id, kind_name(kind));
            Ok((edited(doc, &o.id, json!({ "changed": changed })), summary))
        })
    }
}

/// A segment type.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SegTypeArg {
    /// A straight line between the two nodes.
    Line,
    /// A cubic Bézier (a line becomes a straight curve with handles at 1/3 and 2/3, ready to drag).
    Curve,
}

impl SegTypeArg {
    fn name(self) -> &'static str {
        match self {
            SegTypeArg::Line => "line",
            SegTypeArg::Curve => "curve",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSegmentTypeInput {
    /// Exact object id.
    pub id: ObjectId,
    /// Segment index from node.list.
    pub segment: usize,
    /// line or curve.
    #[serde(rename = "type")]
    pub kind: SegTypeArg,
}

/// `node.segment_type`.
pub struct NodeSegmentType;

impl Command for NodeSegmentType {
    type Input = NodeSegmentTypeInput;
    const ID: &'static str = "node.segment_type";
    const TITLE: &'static str = "Set segment type";
    const DESCRIPTION: &'static str = "Make a segment a straight line (dropping its handles) or a curve (a line \
        becomes a straight cubic whose handles sit at a third and two thirds, ready for node.move with a handle). \
        Returns the affected id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NodeSegmentTypeInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let (o, mut np) = load(cmd, doc, &i.id, true)?;
            let kind = match i.kind {
                SegTypeArg::Line => SegKind::Line,
                SegTypeArg::Curve => SegKind::Curve,
            };
            let before = np.clone();
            np.set_segment(i.segment, kind).map_err(geom_err(cmd))?;
            if np == before {
                return Err(invalid(cmd, format!("segment {} is already a {}; nothing changed", i.segment, i.kind.name())));
            }
            store(doc, &o.id, &np)?;
            Ok((edited(doc, &o.id, json!({})), format!("set segment {} of {} to {:?}", i.segment, o.id, kind)))
        })
    }
}
