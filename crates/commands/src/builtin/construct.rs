//! Construction commands: `node.extend` (LightBurn's Edit Nodes "E"), `path.warp` (Warp / Deform Selection),
//! `art.tangent_circle` and `art.tangent_circle_candidates` (Tools > Generate Tangent Circle), `art.rubber_band`
//! (Arrange > Create Rubber-Band Outline) and `art.toolpath_to_path` (a layer's planned marks as editable paths).

use doc::{Document, Geometry, ObjectId};
use geom::construct::{Circle, tangent_circles};
use geom::warp::{Mapper, Warp};
use geom::{join, split};
use kurbo::{Affine, BezPath, Point, Rect};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, MAX_CREATED, geom_err};
use super::shape::add_object;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

fn default_pick() -> f64 {
    1.0
}

fn default_max_extend() -> f64 {
    10_000.0
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtendInput {
    /// Exact id of the object (not a group).
    pub id: ObjectId,
    /// The clicked point [x, y] in field mm, near the open end to extend.
    pub at: [f64; 2],
    /// How far from `at` the end may be, mm (default 1).
    #[serde(default = "default_pick")]
    pub pick_mm: f64,
    /// Longest extension, mm (default 10000).
    #[serde(default = "default_max_extend")]
    pub max_mm: f64,
}

/// `node.extend`.
pub struct Extend;

impl Command for Extend {
    type Input = ExtendInput;
    const ID: &'static str = "node.extend";
    const TITLE: &'static str = "Extend to intersection";
    const DESCRIPTION: &'static str = "Extend the open end of a path nearest `at` (field mm) straight on along its \
        end direction until it meets the nearest line: another vector object, another subpath of the same object or \
        the same subpath further along (LightBurn's Edit Nodes \"E\", extend line). A line at that end is lengthened; \
        a curve gets a new line. Fails when no open end is within pick_mm or nothing is hit within max_mm. Returns \
        the affected id, `from`, `to` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ExtendInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let o = pathutil::single_object(cmd, doc, &i.id, true)?;
            let [x, y] = util::finite2(cmd, "at", i.at)?;
            let pick = util::positive(cmd, "pick_mm", i.pick_mm)?;
            let max = util::positive(cmd, "max_mm", i.max_mm)?.min(1e6);
            let path = pathutil::vector_path(cmd, &o)?;
            let subs = split(&path);
            let reach = o.bounds().inflate(max, max);
            let mut others: Vec<Vec<Point>> = Vec::new();
            for other in doc.objects.iter().filter(|x| x.id != o.id && !matches!(x.geometry, Geometry::Image { .. })) {
                let b = other.bounds();
                if b.x1 < reach.x0 || b.x0 > reach.x1 || b.y1 < reach.y0 || b.y0 > reach.y1 {
                    continue;
                }
                for l in geom::flatten(&other.field_path(), geom::TOLERANCE).map_err(geom_err(cmd))? {
                    others.push(l.drawn());
                }
            }
            let e = geom::extend::extend(&subs, &others, Point::new(x, y), pick, max).map_err(geom_err(cmd))?;
            pathutil::set_field_path(doc, &o.id, &join(&e.subs))?;
            let mut r = util::edit_result(doc, &[], std::slice::from_ref(&o.id));
            r["from"] = util::pt_json(e.from);
            r["to"] = util::pt_json(e.to);
            Ok((r, format!("extended {} by {} mm", o.id, util::r4(e.from.distance(e.to)))))
        })
    }
}

/// The kind of map `path.warp` applies.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarpMode {
    /// Projective: the selection's box corners go to `corners`; straight lines stay straight (LightBurn's Warp
    /// used for perspective).
    Perspective,
    /// Bilinear through `corners`: the box's edges stay straight, lines inside bend.
    Envelope,
    /// A 4 x 4 grid of handles (`grid`), a bicubic patch (LightBurn's Deform Selection).
    Grid,
}

fn default_warp_tolerance() -> f64 {
    0.01
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WarpInput {
    /// Exact object or group ids, warped together by their combined field-frame bounding box.
    pub ids: Vec<ObjectId>,
    /// "perspective", "envelope" or "grid".
    pub mode: WarpMode,
    /// For perspective and envelope: where the box's corners go, [bottom_left, bottom_right, top_right, top_left],
    /// each [x, y] field mm ("top" is +Y).
    pub corners: Option<[[f64; 2]; 4]>,
    /// For grid: 4 rows (bottom to top) of 4 handles (left to right), [x, y] field mm. Handles evenly spaced over the
    /// box (at thirds) change nothing.
    pub grid: Option<[[[f64; 2]; 4]; 4]>,
    /// Largest deviation of the result from the exact warp, mm (default 0.01).
    #[serde(default = "default_warp_tolerance")]
    pub tolerance_mm: f64,
}

/// `path.warp`.
pub struct PathWarp;

impl Command for PathWarp {
    type Input = WarpInput;
    const ID: &'static str = "path.warp";
    const TITLE: &'static str = "Warp / deform";
    const DESCRIPTION: &'static str = "Distort the selection by moving the handles of its combined bounding box \
        (LightBurn's Warp Selection with 4 corners and Deform Selection with a 4 x 4 grid): mode perspective (a \
        projective map; lines stay straight; corners must form a convex quadrilateral), envelope (bilinear between the \
        4 corners) or grid (bicubic patch on 16 handles). Curves are kept as Béziers, split until within tolerance_mm \
        of the exact warp. Every object becomes a path in its own frame (rectangles, ellipses, polygons and text \
        too); images are refused. Returns affected ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: WarpInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let tol = util::positive(cmd, "tolerance_mm", i.tolerance_mm)?.clamp(0.0005, 1.0);
            let pt = |field: &str, p: [f64; 2]| -> Result<Point> {
                let [x, y] = util::finite2(cmd, field, p)?;
                Ok(Point::new(x, y))
            };
            let warp = match i.mode {
                WarpMode::Perspective | WarpMode::Envelope => {
                    if i.grid.is_some() {
                        return Err(invalid(cmd, "grid is only for mode \"grid\"; give corners"));
                    }
                    let c = i.corners.ok_or_else(|| invalid(cmd, "this mode needs corners (4 points)"))?;
                    let mut q = [Point::ORIGIN; 4];
                    for (k, p) in c.iter().enumerate() {
                        q[k] = pt("corners", *p)?;
                    }
                    if i.mode == WarpMode::Perspective { Warp::Perspective(q) } else { Warp::Envelope(q) }
                }
                WarpMode::Grid => {
                    if i.corners.is_some() {
                        return Err(invalid(cmd, "corners are for perspective and envelope; give grid"));
                    }
                    let g = i.grid.ok_or_else(|| invalid(cmd, "mode \"grid\" needs grid (4 rows of 4 points)"))?;
                    let mut out = [[Point::ORIGIN; 4]; 4];
                    for (r, row) in g.iter().enumerate() {
                        for (c, p) in row.iter().enumerate() {
                            out[r][c] = pt("grid", *p)?;
                        }
                    }
                    Warp::Grid(out)
                }
            };
            for id in &ids {
                pathutil::vector_path(cmd, util::object(doc, id)?)?;
            }
            let rect: Rect =
                doc.bounds_of(&ids).ok_or_else(|| CommandError::Failed(format!("{cmd}: nothing to warp")))?;
            let m = Mapper::new(rect, &warp).map_err(geom_err(cmd))?;
            // Splitting adds at most MAX_WARP_SEGMENTS segments over the whole selection, not per object (many
            // objects could otherwise each make the most segments one warp may).
            let mut paths = Vec::with_capacity(ids.len());
            for id in &ids {
                paths.push(pathutil::vector_path(cmd, util::object(doc, id)?)?);
            }
            let allowed = geom::warp::MAX_WARP_SEGMENTS + paths.iter().map(|p| p.segments().count()).sum::<usize>();
            let mut total = 0usize;
            for (id, path) in ids.iter().zip(&paths) {
                let out = m.map_path(path, tol).map_err(geom_err(cmd))?;
                total += out.segments().count();
                if total > allowed {
                    return Err(invalid(
                        cmd,
                        format!(
                            "the warped selection would need more than {} segments; warp fewer shapes or use a larger \
                             tolerance_mm",
                            geom::warp::MAX_WARP_SEGMENTS
                        ),
                    ));
                }
                pathutil::set_field_path(doc, id, &out)?;
            }
            let mode = match i.mode {
                WarpMode::Perspective => "perspective",
                WarpMode::Envelope => "envelope",
                WarpMode::Grid => "grid",
            };
            Ok((util::edit_result(doc, &[], &ids), format!("warped {} ({mode})", ids.join(", "))))
        })
    }
}

/// An object as a true circle: an ellipse with equal radii under a transform without stretch or shear.
fn as_circle(cmd: &str, doc: &Document, id: &str) -> Result<Circle> {
    let o = pathutil::single_object(cmd, doc, id, false)?;
    let not = || {
        invalid(
            cmd,
            format!(
                "{} (\"{}\") is not a circle; select three circles (ellipses with equal width and height)",
                o.id, o.name
            ),
        )
    };
    let Geometry::Ellipse { rx_mm, ry_mm } = o.geometry else { return Err(not()) };
    let [a, b, c, d, _, _] = o.transform.as_coeffs();
    let (sx, sy) = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
    let ortho = (a * c + b * d).abs() <= 1e-9 * sx.max(sy).max(1.0);
    let (rx, ry) = (rx_mm * sx, ry_mm * sy);
    if !ortho || (rx - ry).abs() > 1e-6 * rx.max(ry).max(1.0) {
        return Err(not());
    }
    Ok(Circle { center: o.transform * Point::ORIGIN, radius: rx })
}

fn three_circles(cmd: &str, doc: &Document, ids: &[ObjectId]) -> Result<[Circle; 3]> {
    if ids.len() != 3 {
        return Err(invalid(cmd, format!("select exactly three circles (got {})", ids.len())));
    }
    Ok([as_circle(cmd, doc, &ids[0])?, as_circle(cmd, doc, &ids[1])?, as_circle(cmd, doc, &ids[2])?])
}

fn candidates_json(c: &[Circle]) -> Value {
    json!(
        c.iter()
            .enumerate()
            .map(|(k, c)| json!({ "index": k, "center": util::pt_json(c.center), "radius_mm": util::r4(c.radius) }))
            .collect::<Vec<_>>()
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TangentCandidatesInput {
    /// Exactly three circle object ids.
    pub ids: Vec<ObjectId>,
}

/// `art.tangent_circle_candidates`.
pub struct TangentCandidates;

impl Command for TangentCandidates {
    type Input = TangentCandidatesInput;
    const ID: &'static str = "art.tangent_circle_candidates";
    const TITLE: &'static str = "Tangent circle candidates";
    const DESCRIPTION: &'static str = "Every circle tangent to three circles (each touching from outside or inside; \
        up to eight), smallest first, for the UI to show before one is picked (LightBurn draws them in light red). \
        Reads only. Returns `candidates` [{index, center, radius_mm}].";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: TangentCandidatesInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let c = three_circles(cmd, doc, &i.ids)?;
        let sol = tangent_circles(c).map_err(geom_err(cmd))?;
        Ok((json!({ "candidates": candidates_json(&sol) }), format!("{} tangent circles", sol.len())))
    }
}

/// Which candidate circles `art.tangent_circle` creates.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TangentPick {
    /// "all": every candidate.
    All(AllWord),
    /// Candidate indices as art.tangent_circle_candidates lists them.
    Indices(Vec<usize>),
}

/// The word "all".
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AllWord {
    All,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TangentCircleInput {
    /// Exactly three circle object ids.
    pub ids: Vec<ObjectId>,
    /// "all" or candidate indices. Default: the candidate whose outline passes nearest `near`, else all.
    pub pick: Option<TangentPick>,
    /// Field point [x, y] mm: create the candidate whose outline passes nearest it (the one clicked).
    pub near: Option<[f64; 2]>,
    /// Exact layer id for the new circles. Default: the first circle's layer.
    pub layer: Option<String>,
}

/// `art.tangent_circle`.
pub struct TangentCircle;

impl Command for TangentCircle {
    type Input = TangentCircleInput;
    const ID: &'static str = "art.tangent_circle";
    const TITLE: &'static str = "Generate tangent circle";
    const DESCRIPTION: &'static str = "Create circles tangent to three selected circles (LightBurn's Tools > \
        Generate Tangent Circle; the problem of Apollonius, up to eight solutions, smallest first): the one whose \
        outline passes nearest `near`, the listed `pick` indices, or all of them. Each is a circle (equal-radius \
        ellipse) on `layer`. Returns created ids, `candidates` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TangentCircleInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let c = three_circles(cmd, doc, &i.ids)?;
            let sol = tangent_circles(c).map_err(geom_err(cmd))?;
            if sol.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: no circle is tangent to all three")));
            }
            let chosen: Vec<usize> = match (&i.pick, i.near) {
                (Some(_), Some(_)) => return Err(invalid(cmd, "give pick or near, not both")),
                (Some(TangentPick::All(_)), None) | (None, None) => (0..sol.len()).collect(),
                (Some(TangentPick::Indices(k)), None) => {
                    if k.is_empty() {
                        return Err(invalid(cmd, "pick must list at least one index (or be \"all\")"));
                    }
                    if let Some(bad) = k.iter().find(|&&k| k >= sol.len()) {
                        return Err(invalid(cmd, format!("no candidate {bad} (there are {})", sol.len())));
                    }
                    let mut k = k.clone();
                    k.sort_unstable();
                    k.dedup();
                    k
                }
                (None, Some(p)) => {
                    let [x, y] = util::finite2(cmd, "near", p)?;
                    let q = Point::new(x, y);
                    let d = |c: &Circle| (c.center.distance(q) - c.radius).abs();
                    let best = (0..sol.len()).min_by(|&a, &b| d(&sol[a]).total_cmp(&d(&sol[b]))).unwrap_or(0);
                    vec![best]
                }
            };
            let layer = match &i.layer {
                Some(l) => util::layer(doc, l)?.id.clone(),
                None => util::object(doc, &i.ids[0])?.layer.clone(),
            };
            let mut created = Vec::new();
            for k in &chosen {
                let c = sol[*k];
                if c.radius > 1e6 {
                    return Err(invalid(cmd, format!("candidate {k} is too large to draw ({} mm)", c.radius)));
                }
                let g = Geometry::Ellipse { rx_mm: c.radius, ry_mm: c.radius };
                created.push(add_object(
                    doc,
                    "Tangent circle".into(),
                    layer.clone(),
                    g,
                    Affine::translate(c.center.to_vec2()),
                )?);
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["candidates"] = candidates_json(&sol);
            Ok((r, format!("added {} tangent circle{}", created.len(), if created.len() == 1 { "" } else { "s" })))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RubberBandInput {
    /// Exact object or group ids to wrap.
    pub ids: Vec<ObjectId>,
    /// Exact layer id for the outline. Default: the first object's layer.
    pub layer: Option<String>,
}

/// `art.rubber_band`.
pub struct RubberBand;

impl Command for RubberBand {
    type Input = RubberBandInput;
    const ID: &'static str = "art.rubber_band";
    const TITLE: &'static str = "Rubber-band outline";
    const DESCRIPTION: &'static str = "Add a closed outline that wraps the selection as tightly as a rubber band \
        (its convex hull; LightBurn's Arrange > Create Rubber-Band Outline), e.g. a cut line around a design. Curves \
        are followed to 0.01 mm; images use their frame. Returns the new id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RubberBandInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let paths: Vec<BezPath> = ids.iter().filter_map(|id| doc.object(id)).map(doc::Object::field_path).collect();
            let band = geom::construct::rubber_band(&paths).map_err(geom_err(cmd))?;
            let layer = match &i.layer {
                Some(l) => util::layer(doc, l)?.id.clone(),
                None => util::object(doc, &ids[0])?.layer.clone(),
            };
            let id = pathutil::add_field_object(doc, "Rubber band", &layer, None, &band)?;
            Ok((util::edit_result(doc, std::slice::from_ref(&id), &[]), format!("added rubber-band outline {id}")))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolpathInput {
    /// Only these object or group ids. Default: every vector object on an output layer.
    pub ids: Option<Vec<ObjectId>>,
    /// Exact layer id for the new paths. Default: each pass's own layer.
    pub layer: Option<String>,
}

/// `art.toolpath_to_path`.
pub struct ToolpathToPath;

impl Command for ToolpathToPath {
    type Input = ToolpathInput;
    const ID: &'static str = "art.toolpath_to_path";
    const TITLE: &'static str = "Toolpath to path";
    const DESCRIPTION: &'static str = "Turn what a job would mark for vector shapes into editable paths: per layer \
        and sub-layer, one path object holding its planned lines (hatch lines, offset fills, outlines with tabs, \
        wobble and the rest, in marking order; repeated passes once), named \"Toolpath <layer>\". The originals \
        stay. Planning ignores the machine's field and power limits here; images are left out. Returns created ids \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ToolpathInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids: Vec<ObjectId> = match &i.ids {
                Some(ids) => util::object_ids(cmd, doc, ids)?,
                None => doc.objects.iter().map(|o| o.id.clone()).collect(),
            };
            let ids: Vec<ObjectId> = ids
                .into_iter()
                .filter(|id| doc.object(id).is_some_and(|o| !matches!(o.geometry, Geometry::Image { .. })))
                .collect();
            if ids.is_empty() {
                return Err(invalid(cmd, "no vector shapes to plan"));
            }
            let dest = i.layer.as_deref().map(|l| util::layer(doc, l).map(|l| l.id.clone())).transpose()?;
            let extent = doc
                .bounds_of(&ids)
                .map_or(1.0, |b| [b.x0, b.y0, b.x1, b.y1].iter().fold(1.0f64, |m, v| m.max(v.abs())));
            let target = permissive_target(extent * 2.0 + 10.0);
            let opts = plan::JobOptions { selection: Some(ids), preview: true, ..Default::default() };
            let job =
                plan::plan_job_with(doc, &target, &opts).map_err(|e| CommandError::Failed(format!("{cmd}: {e}")))?;
            let mut created = Vec::new();
            for pass in job.passes.iter().filter(|p| p.repeat == 0) {
                let mut path = BezPath::new();
                for line in &pass.polylines {
                    let Some(first) = line.first() else { continue };
                    path.move_to((first[0], first[1]));
                    let closed = line.len() > 2 && line.first() == line.last();
                    let end = if closed { line.len() - 1 } else { line.len() };
                    if line.len() == 1 {
                        // A dot: a zero-length mark stays a point to see.
                        path.line_to((first[0], first[1]));
                    }
                    for p in &line[1..end] {
                        path.line_to((p[0], p[1]));
                    }
                    if closed {
                        path.close_path();
                    }
                }
                if path.elements().is_empty() {
                    continue;
                }
                if created.len() >= MAX_CREATED {
                    return Err(invalid(cmd, format!("more than {MAX_CREATED} passes")));
                }
                let layer = dest.clone().unwrap_or_else(|| pass.layer_id.clone());
                let lname = doc.layer(&pass.layer_id).map_or(pass.layer_id.clone(), |l| l.name.clone());
                let name = if pass.sub_layer == 0 {
                    format!("Toolpath {lname}")
                } else {
                    format!("Toolpath {lname} [{}]", pass.sub_layer)
                };
                created.push(pathutil::add_field_object(doc, &name, &layer, None, &path)?);
            }
            if created.is_empty() {
                return Err(CommandError::Failed(format!(
                    "{cmd}: the selection plans no marks (are its layers set to output?)"
                )));
            }
            let r = util::edit_result(doc, &created, &[]);
            Ok((r, format!("made {} toolpath path{}", created.len(), if created.len() == 1 { "" } else { "s" })))
        })
    }
}

/// A target that accepts any layer settings in a square field of `field_mm`: for turning plans into geometry only.
fn permissive_target(field_mm: f64) -> plan::Target {
    plan::Target {
        field_mm,
        field_height_mm: None,
        curve_tolerance_mm: None,
        swap_xy: false,
        negate_x: false,
        negate_y: false,
        max_power_pct: 100.0,
        pulse_widths_ns: (1..=2000).collect(),
        timing: doc::GalvoTiming {
            laser_on_delay_us: 100.0,
            laser_off_delay_us: 100.0,
            polygon_delay_us: 10.0,
            end_delay_us: 0.0,
            jump_speed_mm_s: 4000.0,
            min_jump_delay_us: 100.0,
            max_jump_delay_us: 400.0,
            jump_distance_limit_mm: 10.0,
        },
        open_mo_delay_us: 0.0,
        calibration: Default::default(),
        pulse_width_enabled: true,
        z_axis: false,
        rotary_axis: false,
        port_outputs: 0,
    }
}
