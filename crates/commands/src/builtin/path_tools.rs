//! Path tools: `path.break_apart`, `path.auto_join`, `path.close`, `path.delete_duplicates`, `path.convert`,
//! `path.optimize`, `path.fillet`, `path.trim`, `path.reverse`. They keep curves (see `geom::ops`) and work in the
//! field frame, writing results back into each object's own frame (its transform is kept).

use std::collections::{BTreeMap, HashMap};

use doc::{Document, Geometry, ObjectId};
use geom::ops::{self, CloseMode, Piece};
use geom::{SubPath, join, split};
use kurbo::{BezPath, Point};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, MAX_CREATED, geom_err};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// How `path.break_apart` splits a shape.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BreakMode {
    /// LightBurn's Break Apart: a shape of several subpaths splits into its subpaths; a single subpath splits into
    /// its segments (a rectangle becomes four lines). Default.
    #[default]
    Auto,
    /// Every line and curve segment becomes its own open path.
    Segments,
    /// Every subpath (closed contour or open run) becomes its own shape; holes are separated too.
    Subpaths,
    /// Every filled island becomes its own shape and keeps its holes (letters stay whole: an "o" keeps its
    /// counter); open subpaths become separate shapes.
    Islands,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BreakApartInput {
    /// Exact object or group ids (a group stands for all its members). Images are refused.
    pub ids: Vec<ObjectId>,
    /// auto (default, LightBurn), segments, subpaths or islands.
    #[serde(default)]
    pub mode: BreakMode,
}

/// `path.break_apart`.
pub struct BreakApart;

impl Command for BreakApart {
    type Input = BreakApartInput;
    const ID: &'static str = "path.break_apart";
    const TITLE: &'static str = "Break apart";
    const DESCRIPTION: &'static str = "Split shapes into separate objects. mode auto (LightBurn): several subpaths \
        split into subpaths, a single subpath into its segments; segments, subpaths, or islands (each filled island \
        with its own holes, so letters keep their counters). The first piece stays in the original object (same \
        id); the others are new objects on the same layer and group. Curves are kept. Objects that would not split \
        are reported in `unchanged`. At most 10000 new objects. Returns created and affected ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: BreakApartInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut created = Vec::new();
            let mut affected = Vec::new();
            let mut unchanged = Vec::new();
            for id in &ids {
                let o = util::object(doc, id)?.clone();
                let local = pathutil::vector_local(cmd, &o)?;
                let pieces = match i.mode {
                    BreakMode::Segments => ops::segments(&local),
                    BreakMode::Subpaths => ops::subpaths(&local),
                    BreakMode::Islands => ops::islands(&local).map_err(geom_err(cmd))?,
                    BreakMode::Auto => {
                        if split(&local).len() > 1 { ops::subpaths(&local) } else { ops::segments(&local) }
                    }
                };
                if pieces.len() < 2 {
                    unchanged.push(id.clone());
                    continue;
                }
                if created.len() + pieces.len() > MAX_CREATED {
                    return Err(CommandError::Failed(format!("{cmd}: more than {MAX_CREATED} pieces")));
                }
                let mut it = pieces.into_iter();
                pathutil::set_local_path(doc, id, it.next().expect("two pieces"))?;
                affected.push(id.clone());
                for p in it {
                    let nid = doc.alloc_id("obj")?;
                    let mut n = doc::Object::new(nid.clone(), o.name.clone(), o.layer.clone(), Geometry::Path { path: p }, o.transform);
                    n.group = o.group.clone();
                    doc.objects.push(n);
                    created.push(nid);
                }
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: nothing to break apart (each shape is one piece)")));
            }
            let mut r = util::edit_result(doc, &created, &affected);
            r["unchanged"] = json!(unchanged);
            Ok((r, format!("broke {} into {} more pieces", affected.join(", "), created.len())))
        })
    }
}

fn tolerance(cmd: &str, v: f64) -> Result<f64> {
    let t = util::non_negative(cmd, "tolerance_mm", v)?;
    if t > 10.0 {
        return Err(invalid(cmd, "tolerance_mm must be at most 10"));
    }
    Ok(t)
}

fn default_join_tolerance() -> f64 {
    0.05
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutoJoinInput {
    /// Exact object or group ids (a group stands for all its members) whose open paths may be joined.
    pub ids: Vec<ObjectId>,
    /// Largest gap between path ends that still joins, in mm (0..=10, default 0.05 like LightBurn).
    #[serde(default = "default_join_tolerance")]
    pub tolerance_mm: f64,
}

/// `path.auto_join`.
pub struct AutoJoin;

impl Command for AutoJoin {
    type Input = AutoJoinInput;
    const ID: &'static str = "path.auto_join";
    const TITLE: &'static str = "Auto-join";
    const DESCRIPTION: &'static str = "Join open paths whose ends lie within tolerance_mm (default 0.05, LightBurn's \
        value) into longer paths, nearest ends first, reversing pieces as needed; a chain whose ends meet is closed. \
        Objects joined together merge into the first of them (in ids order; it keeps its id, layer and transform) \
        and the others are deleted. Curves are kept. Fails when nothing joins. Returns affected and deleted ids, \
        `joins` and `closed` counts, and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AutoJoinInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let tol = tolerance(cmd, i.tolerance_mm)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut pieces = Vec::new();
            for (k, id) in ids.iter().enumerate() {
                let path = pathutil::vector_path(cmd, util::object(doc, id)?)?;
                pieces.extend(split(&path).into_iter().map(|sub| Piece { tag: k, sub }));
            }
            if pieces.len() > 200_000 {
                return Err(invalid(cmd, "too many subpaths to join (more than 200000)"));
            }
            let joined = ops::auto_join(pieces, tol);
            let joins: usize = joined.iter().map(|j| j.joins).sum();
            let closed = joined.iter().filter(|j| j.closed_by_join).count();
            if joins == 0 && closed == 0 {
                return Err(CommandError::Failed(format!("{cmd}: no open path ends lie within {tol} mm of each other")));
            }
            // Union-find over objects joined together.
            let mut parent: Vec<usize> = (0..ids.len()).collect();
            fn root(p: &mut [usize], i: usize) -> usize {
                let mut r = i;
                while p[r] != r {
                    r = p[r];
                }
                p[i] = r;
                r
            }
            for j in &joined {
                for w in j.tags.windows(2) {
                    let (a, b) = (root(&mut parent, w[0]), root(&mut parent, w[1]));
                    parent[a.max(b)] = a.min(b);
                }
            }
            let mut by_root: BTreeMap<usize, Vec<SubPath>> = BTreeMap::new();
            let mut touched: BTreeMap<usize, bool> = BTreeMap::new();
            for j in joined {
                let r = root(&mut parent, j.tags[0]);
                *touched.entry(r).or_default() |= j.joins > 0 || j.closed_by_join;
                by_root.entry(r).or_default().push(j.sub);
            }
            let mut affected = Vec::new();
            let mut deleted = Vec::new();
            for (r, subs) in by_root {
                let members: Vec<usize> = (0..ids.len()).filter(|&k| root(&mut parent, k) == r).collect();
                if !touched.get(&r).copied().unwrap_or(false) && members.len() == 1 {
                    continue;
                }
                pathutil::set_field_path(doc, &ids[r], &join(&subs))?;
                affected.push(ids[r].clone());
                deleted.extend(members.iter().filter(|&&k| k != r).map(|&k| ids[k].clone()));
            }
            let groups = pathutil::delete_objects(doc, &deleted);
            let mut res = util::edit_result(doc, &[], &affected);
            res["deleted"] = json!(deleted);
            res["deleted_groups"] = json!(groups);
            res["joins"] = json!(joins);
            res["closed"] = json!(closed);
            Ok((res, format!("joined {joins} gaps ({closed} closed) in {}", affected.join(", "))))
        })
    }
}

/// How `path.close` closes a gap.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseHow {
    /// Add a straight line from the end back to the start. Default.
    #[default]
    Line,
    /// Move both end nodes onto their midpoint, adding no line.
    MoveEnds,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// Only close paths whose ends are at most this far apart, in mm (0..=1000; LightBurn's Close Selected Paths With
    /// Tolerance). Leave it out to close every open path (LightBurn's Close Path).
    pub tolerance_mm: Option<f64>,
    /// line (default: add a closing line) or move_ends (pull the ends together).
    #[serde(default)]
    pub mode: CloseHow,
}

/// `path.close`.
pub struct ClosePath;

impl Command for ClosePath {
    type Input = CloseInput;
    const ID: &'static str = "path.close";
    const TITLE: &'static str = "Close paths";
    const DESCRIPTION: &'static str = "Close open paths: each open subpath (whose ends are within tolerance_mm, when \
        given) gets a closing line (mode line) or has its end nodes moved together (move_ends). A single straight \
        line is never closed. Each subpath closes on its own; to join separate paths use path.auto_join. Fails when \
        nothing closes. Returns affected ids, `closed` and `still_open` counts and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CloseInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let tol = i
                .tolerance_mm
                .map(|t| {
                    let t = util::non_negative(cmd, "tolerance_mm", t)?;
                    if t > 1000.0 { Err(invalid(cmd, "tolerance_mm must be at most 1000")) } else { Ok(t) }
                })
                .transpose()?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mode = match i.mode {
                CloseHow::Line => CloseMode::JoinWithLine,
                CloseHow::MoveEnds => CloseMode::MoveEndsTogether,
            };
            let (mut closed, mut still_open) = (0, 0);
            let mut affected = Vec::new();
            for id in &ids {
                let path = pathutil::vector_path(cmd, util::object(doc, id)?)?;
                let mut subs = split(&path);
                let mut n = 0;
                for s in &mut subs {
                    if s.closed {
                        continue;
                    }
                    if ops::close(s, tol, mode) {
                        n += 1;
                    } else {
                        still_open += 1;
                    }
                }
                if n > 0 {
                    closed += n;
                    pathutil::set_field_path(doc, id, &join(&subs))?;
                    affected.push(id.clone());
                }
            }
            if closed == 0 {
                return Err(CommandError::Failed(format!("{cmd}: no open path could be closed ({still_open} open)")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["closed"] = json!(closed);
            r["still_open"] = json!(still_open);
            Ok((r, format!("closed {closed} paths in {}", affected.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteDuplicatesInput {
    /// Exact object or group ids to search; leave out to search the whole document (LightBurn needs no selection).
    pub ids: Option<Vec<ObjectId>>,
    /// Also remove line and curve segments that repeat an earlier segment of the searched shapes (e.g. a shared edge
    /// between imported parts), splitting paths there. Default false.
    #[serde(default)]
    pub segments: bool,
}

/// `path.delete_duplicates`.
pub struct DeleteDuplicates;

/// A shape's identity for duplicate detection: its layer and its field-frame outline, rounded to 1 µm, starting
/// point and direction ignored.
fn shape_key(cmd: &str, o: &doc::Object) -> Result<String> {
    if let Geometry::Image { data_base64, width_mm, height_mm, .. } = &o.geometry {
        let c = o.transform.as_coeffs().map(|v| (v * 1e6).round() as i64);
        return Ok(format!("{}|img|{width_mm}|{height_mm}|{c:?}|{}", o.layer, data_base64.len()) + data_base64);
    }
    let q = |p: Point| ((p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64);
    let mut rings: Vec<Vec<(i64, i64)>> = geom::flatten(&o.field_path(), geom::TOLERANCE)
        .map_err(geom_err(cmd))?
        .into_iter()
        .map(|l| {
            let mut pts: Vec<(i64, i64)> = l.points.iter().copied().map(q).collect();
            if l.closed {
                let fwd = canonical_ring(&pts);
                pts.reverse();
                fwd.min(canonical_ring(&pts))
            } else {
                let rev: Vec<(i64, i64)> = pts.iter().rev().copied().collect();
                pts.min(rev)
            }
        })
        .collect();
    rings.sort();
    Ok(format!("{}|{rings:?}", o.layer))
}

fn canonical_ring(pts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let start = (0..pts.len()).min_by_key(|&i| pts[i]).unwrap_or(0);
    pts[start..].iter().chain(&pts[..start]).copied().collect()
}

impl Command for DeleteDuplicates {
    type Input = DeleteDuplicatesInput;
    const ID: &'static str = "path.delete_duplicates";
    const TITLE: &'static str = "Delete duplicates";
    const DESCRIPTION: &'static str = "Delete objects that repeat an earlier object exactly (same layer, same outline \
        in the field frame to 1 µm, whatever the start point or direction; images: same file and placement), \
        keeping the first in document order (LightBurn's Delete Duplicates; duplicates burn twice). Locked objects \
        are never deleted. With segments true, also remove repeated segments across the searched shapes. Searches \
        the whole document when ids is left out. Fails when nothing is found. Returns deleted ids, affected ids and \
        `segments_removed`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: DeleteDuplicatesInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = match &i.ids {
                Some(ids) => util::object_ids(cmd, doc, ids)?,
                None => doc.objects.iter().map(|o| o.id.clone()).collect(),
            };
            let mut seen: HashMap<String, ObjectId> = HashMap::new();
            let mut deleted: Vec<ObjectId> = Vec::new();
            for id in &ids {
                let key = shape_key(cmd, util::object(doc, id)?)?;
                match seen.entry(key) {
                    std::collections::hash_map::Entry::Occupied(_) => {
                        if !doc.is_locked(id) {
                            deleted.push(id.clone());
                        }
                    }
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(id.clone());
                    }
                }
            }
            let groups = pathutil::delete_objects(doc, &deleted);
            let mut affected = Vec::new();
            let mut removed = 0;
            let mut emptied = Vec::new();
            if i.segments {
                let rest: Vec<ObjectId> = ids
                    .iter()
                    .filter(|id| !deleted.contains(id) && !doc.is_locked(id))
                    .filter(|id| doc.object(id).is_some_and(|o| !matches!(o.geometry, Geometry::Image { .. })))
                    .cloned()
                    .collect();
                let paths: Vec<Vec<SubPath>> =
                    rest.iter().map(|id| split(&doc.object(id).expect("checked").field_path())).collect();
                let (out, n) = ops::remove_duplicate_segments(&paths, 1e-3);
                removed = n;
                for ((id, before), after) in rest.iter().zip(&paths).zip(out) {
                    if *before == after {
                        continue;
                    }
                    if after.is_empty() {
                        emptied.push(id.clone());
                    } else {
                        pathutil::set_field_path(doc, id, &join(&after))?;
                        affected.push(id.clone());
                    }
                }
                pathutil::delete_objects(doc, &emptied);
            }
            deleted.extend(emptied);
            if deleted.is_empty() && affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: no duplicates found")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["deleted"] = json!(deleted);
            r["deleted_groups"] = json!(groups);
            r["segments_removed"] = json!(removed);
            Ok((r, format!("deleted {} duplicate objects and {removed} duplicate segments", deleted.len())))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdsInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
}

/// `path.convert`.
pub struct ConvertToPath;

impl Command for ConvertToPath {
    type Input = IdsInput;
    const ID: &'static str = "path.convert";
    const TITLE: &'static str = "Convert to path";
    const DESCRIPTION: &'static str = "Turn rectangles and ellipses into plain paths (lines and Bézier curves, same \
        shape and placement), so their nodes can be edited; their size parameters are gone afterwards. Paths are \
        left as they are; images cannot be converted. Fails when nothing converts. Returns affected ids, `skipped` \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: IdsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut affected = Vec::new();
            let mut skipped = Vec::new();
            for id in &ids {
                let o = util::object(doc, id)?;
                match o.geometry {
                    Geometry::Rect { .. } | Geometry::Ellipse { .. } => {
                        let local = pathutil::clean_local(&o.geometry);
                        pathutil::set_local_path(doc, id, local)?;
                        affected.push(id.clone());
                    }
                    Geometry::Path { .. } => skipped.push(json!({ "id": id, "reason": "already a path" })),
                    Geometry::Image { .. } => skipped.push(json!({ "id": id, "reason": "an image has no outline to convert" })),
                }
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: nothing to convert (only rectangles and ellipses convert)")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["skipped"] = json!(skipped);
            Ok((r, format!("converted {} to paths", affected.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptimizeInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// How far (mm, 0..=10) the result may stray from the original. 0.01 is a good start.
    pub tolerance_mm: f64,
}

/// `path.optimize`.
pub struct Optimize;

impl Command for Optimize {
    type Input = OptimizeInput;
    const ID: &'static str = "path.optimize";
    const TITLE: &'static str = "Optimize shapes";
    const DESCRIPTION: &'static str = "Remove redundant nodes within tolerance_mm (field frame): zero-length segments \
        go, curves straight within the tolerance become lines, and runs of lines lose collinear and near-collinear \
        nodes (Douglas-Peucker; LightBurn's Optimize with \"fit to lines\"). Other curves are kept; arcs are not \
        fitted and nothing is smoothed. Rectangles and ellipses become paths. Fails when no node can go. Returns \
        affected ids, `nodes_before`, `nodes_after` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: OptimizeInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let tol = tolerance(cmd, i.tolerance_mm)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let (mut before, mut after) = (0, 0);
            let mut affected = Vec::new();
            for id in &ids {
                let path = pathutil::vector_path(cmd, util::object(doc, id)?)?;
                let subs = split(&path);
                let mut out = Vec::with_capacity(subs.len());
                for s in &subs {
                    let (o, st) = ops::optimize(s, tol);
                    before += st.nodes_before;
                    after += st.nodes_after;
                    out.push(o);
                }
                if out != subs {
                    pathutil::set_field_path(doc, id, &join(&out))?;
                    affected.push(id.clone());
                }
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: nothing to optimize within {tol} mm")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["nodes_before"] = json!(before);
            r["nodes_after"] = json!(after);
            Ok((r, format!("optimized {} ({before} → {after} nodes)", affected.join(", "))))
        })
    }
}

/// Which corners `path.fillet` rounds.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Corners {
    /// "all": every corner between two straight lines.
    All(AllCorners),
    /// Node indices as `node.list` numbers them (only with a single object).
    Nodes(Vec<usize>),
}

/// The word "all".
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AllCorners {
    All,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilletInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// Corner radius in mm (field frame), |r| <= 1000, not 0. Negative cuts a concave bite of that radius centred on
    /// the corner (LightBurn).
    pub radius_mm: f64,
    /// "all" (default) or a list of node indices from node.list (one object only).
    pub corners: Option<Corners>,
}

/// `path.fillet`.
pub struct Fillet;

impl Command for Fillet {
    type Input = FilletInput;
    const ID: &'static str = "path.fillet";
    const TITLE: &'static str = "Fillet corners";
    const DESCRIPTION: &'static str = "Round corners with circular arcs of radius_mm (LightBurn's Radius tool; \
        negative: a concave bite). Only corners between two straight lines are rounded; a radius must fit on both \
        lines (half each when the neighbouring corner is rounded too), else the corner is skipped with the reason. \
        A rectangle with corners all keeps being a rectangle with that corner radius when it fits; other shapes \
        become paths. Fails when no corner is rounded. Returns affected ids, `filleted` and `skipped` per object, \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FilletInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let r = util::finite(cmd, "radius_mm", i.radius_mm)?;
            if r == 0.0 || r.abs() > 1000.0 {
                return Err(invalid(cmd, "radius_mm must be non-zero and within ±1000"));
            }
            let nodes = match i.corners {
                None | Some(Corners::All(_)) => None,
                Some(Corners::Nodes(n)) => {
                    if ids.len() != 1 {
                        return Err(invalid(cmd, "corner indices need exactly one object"));
                    }
                    if n.is_empty() {
                        return Err(invalid(cmd, "corners must list at least one node index (or be \"all\")"));
                    }
                    Some(n)
                }
            };
            util::check_unlocked(cmd, doc, &ids)?;
            let mut affected = Vec::new();
            let mut report = serde_json::Map::new();
            for id in &ids {
                let o = util::object(doc, id)?.clone();
                // A whole rectangle stays a rectangle.
                if let (Geometry::Rect { width_mm, height_mm, .. }, None, true) = (&o.geometry, &nodes, r > 0.0)
                    && o.transform.as_coeffs()[1].abs() < 1e-12
                    && o.transform.as_coeffs()[2].abs() < 1e-12
                    && (o.transform.as_coeffs()[0].abs() - 1.0).abs() < 1e-12
                    && (o.transform.as_coeffs()[3].abs() - 1.0).abs() < 1e-12
                    && r <= width_mm.min(*height_mm) / 2.0
                {
                    let (w, h) = (*width_mm, *height_mm);
                    doc.object_mut(id).expect("exists").geometry = Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: r };
                    affected.push(id.clone());
                    report.insert(id.clone(), json!({ "filleted": [0, 1, 2, 3], "skipped": [] }));
                    continue;
                }
                let path = pathutil::vector_path(cmd, &o)?;
                let subs = split(&path);
                let mut out = Vec::with_capacity(subs.len());
                let (mut done, mut skipped) = (Vec::new(), Vec::new());
                let mut base = 0;
                for s in &subs {
                    let n = s.node_count();
                    let want: Vec<usize> = match &nodes {
                        None => ops::line_corners(s),
                        Some(list) => list.iter().filter(|&&k| k >= base && k < base + n).map(|k| k - base).collect(),
                    };
                    let (f, applied, sk) = ops::fillet(s, &want, r);
                    done.extend(applied.into_iter().map(|k| k + base));
                    skipped.extend(sk.into_iter().map(|x| json!({ "node": x.node + base, "reason": x.reason })));
                    out.push(f);
                    base += n;
                }
                if let Some(list) = &nodes {
                    for &k in list.iter().filter(|&&k| k >= base) {
                        skipped.push(json!({ "node": k, "reason": "no such node" }));
                    }
                }
                if !done.is_empty() {
                    pathutil::set_field_path(doc, id, &join(&out))?;
                    affected.push(id.clone());
                }
                report.insert(id.clone(), json!({ "filleted": done, "skipped": skipped }));
            }
            if affected.is_empty() {
                let why = report
                    .values()
                    .find_map(|v| v["skipped"].get(0).and_then(|s| s["reason"].as_str()).map(str::to_string))
                    .unwrap_or_else(|| "no corners between straight lines".into());
                return Err(CommandError::Failed(format!("{cmd}: no corner was rounded ({why})")));
            }
            let mut res = util::edit_result(doc, &[], &affected);
            res["corners"] = Value::Object(report);
            Ok((res, format!("filleted {} with radius {r} mm", affected.join(", "))))
        })
    }
}

fn default_pick() -> f64 {
    1.0
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrimInput {
    /// Exact id of the object to trim (not a group).
    pub id: ObjectId,
    /// The clicked point [x, y] in field mm, on or near the piece to remove.
    pub at: [f64; 2],
    /// How far from `at` the piece may be, in mm (default 1).
    #[serde(default = "default_pick")]
    pub pick_mm: f64,
}

/// `path.trim`.
pub struct Trim;

impl Command for Trim {
    type Input = TrimInput;
    const ID: &'static str = "path.trim";
    const TITLE: &'static str = "Trim";
    const DESCRIPTION: &'static str = "Remove the piece of a shape nearest `at` (field mm) between the nearest \
        intersections on either side (LightBurn's Trim Shapes): crossings with every other vector object, the \
        shape's other subpaths and itself. Without a crossing on one side the piece runs to that end; a path with no \
        crossings at all is removed (the object is deleted when nothing is left). A trimmed closed path becomes \
        open. Curves are kept. Returns affected or deleted ids, `cut_at` points and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TrimInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let o = pathutil::single_object(cmd, doc, &i.id, true)?;
            let [x, y] = util::finite2(cmd, "at", i.at)?;
            let pick = util::positive(cmd, "pick_mm", i.pick_mm)?;
            let path = pathutil::vector_path(cmd, &o)?;
            let subs = split(&path);
            let bounds = o.bounds().inflate(1.0, 1.0);
            let mut others: Vec<Vec<Point>> = Vec::new();
            for other in doc.objects.iter().filter(|x| x.id != o.id && !matches!(x.geometry, Geometry::Image { .. })) {
                let b = other.bounds();
                if b.x1 < bounds.x0 || b.x0 > bounds.x1 || b.y1 < bounds.y0 || b.y0 > bounds.y1 {
                    continue;
                }
                for l in geom::flatten(&other.field_path(), geom::TOLERANCE).map_err(geom_err(cmd))? {
                    others.push(l.drawn());
                }
            }
            let t = ops::trim(&subs, &others, Point::new(x, y), pick).map_err(geom_err(cmd))?;
            let cut: Vec<Value> = t.cut_at.iter().map(|p| util::pt_json(*p)).collect();
            if t.subs.is_empty() {
                let groups = pathutil::delete_objects(doc, std::slice::from_ref(&o.id));
                let r = json!({ "created": [], "affected": [], "deleted": [o.id], "deleted_groups": groups, "cut_at": cut, "bounds": Value::Null });
                return Ok((r, format!("trimmed {} away entirely", o.id)));
            }
            pathutil::set_field_path(doc, &o.id, &join(&t.subs))?;
            let mut r = util::edit_result(doc, &[], std::slice::from_ref(&o.id));
            r["cut_at"] = json!(cut);
            r["deleted"] = json!([]);
            Ok((r, format!("trimmed {} at [{x}, {y}]", o.id)))
        })
    }
}

/// `path.reverse`.
pub struct Reverse;

impl Command for Reverse {
    type Input = IdsInput;
    const ID: &'static str = "path.reverse";
    const TITLE: &'static str = "Reverse direction";
    const DESCRIPTION: &'static str = "Reverse the direction of every subpath of the given shapes (the burn runs the \
        other way; the outline is unchanged). Rectangles and ellipses become paths. Images are refused. Returns \
        affected ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: IdsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            for id in &ids {
                let local: BezPath = pathutil::vector_local(cmd, util::object(doc, id)?)?;
                pathutil::set_local_path(doc, id, join(&ops::reverse(&split(&local))))?;
            }
            Ok((util::edit_result(doc, &[], &ids), format!("reversed {}", ids.join(", "))))
        })
    }
}
