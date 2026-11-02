//! Fill Region (a paint bucket): `path.fill_region` creates a closed shape for the area the lines around a point
//! enclose, on a chosen layer; `path.fill_region_preview` returns the same outlines without creating anything (the
//! canvas highlight). The geometry is `geom::fill_region` (a planar arrangement of every boundary line, the face
//! around each point, islands as holes); curves are flattened at 0.01 mm, so the new shapes are polylines.
//!
//! Boundaries: by default every vector object on a visible layer, output on or off (what the canvas shows); or the
//! listed objects / groups, or the objects on the listed layers. Open paths count as lines unless `open_paths` is
//! false. Text counts by its outline (as drawn); images never bound anything (their frame is not a line).

use doc::{Document, Geometry, LayerId, ObjectId};
use geom::fill_region::{Boundaries, FillOptions, FillResult, MAX_GAP_MM, Miss, PointResult, fill_regions};
use geom::{Polyline, Region};
use kurbo::Point;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::{self, geom_err};
use super::util::{self, invalid, r4, transact};
use crate::{Command, CommandError, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FillRegionInput {
    /// Field points [x, y] mm, one per area to fill (at least one, at most 256). Points in the same area make one shape.
    pub points: Vec<[f64; 2]>,
    /// Exact layer id for the new shapes (`T1` / `T2` name the tool layers). Default: the first layer.
    pub layer: Option<LayerId>,
    /// Only these object or group ids are boundaries (any layer, shown or hidden). Not with `boundary_layers`.
    pub boundary_ids: Option<Vec<ObjectId>>,
    /// Only objects on these layer ids are boundaries (shown or hidden). Not with `boundary_ids`.
    pub boundary_layers: Option<Vec<LayerId>>,
    /// Open paths count as boundary lines (default true); false uses closed outlines only.
    pub open_paths: Option<bool>,
    /// Close openings narrower than this, mm (default 0: lines must meet; at most 10).
    pub gap_mm: Option<f64>,
    /// Islands inside an area (a pupil inside an eye) become holes (default true); false fills over them.
    pub holes: Option<bool>,
    /// Name of the new shapes. Default "Fill region".
    pub name: Option<String>,
}

/// What bounds the areas, shared by `path.fill_region` and `path.fill_region_preview`.
#[derive(Debug, Default)]
struct RegionOptions {
    boundary_ids: Option<Vec<ObjectId>>,
    boundary_layers: Option<Vec<LayerId>>,
    open_paths: Option<bool>,
    gap_mm: Option<f64>,
    holes: Option<bool>,
}

macro_rules! region_options {
    ($i:expr) => {
        RegionOptions {
            boundary_ids: $i.boundary_ids.clone(),
            boundary_layers: $i.boundary_layers.clone(),
            open_paths: $i.open_paths,
            gap_mm: $i.gap_mm,
            holes: $i.holes,
        }
    };
}

/// The boundary polylines (field frame) chosen by `o`, and how many objects they came from.
fn boundary_lines(cmd: &str, doc: &Document, o: &RegionOptions) -> Result<(Vec<Polyline>, usize)> {
    let objects: Vec<&doc::Object> = match (&o.boundary_ids, &o.boundary_layers) {
        (Some(_), Some(_)) => return Err(invalid(cmd, "give boundary_ids or boundary_layers, not both")),
        (Some(ids), None) => {
            let ids = util::object_ids(cmd, doc, ids)?;
            ids.iter().filter_map(|id| doc.object(id)).collect()
        }
        (None, Some(layers)) => {
            if layers.is_empty() {
                return Err(invalid(cmd, "boundary_layers must list at least one layer id"));
            }
            for l in layers {
                util::layer(doc, l)?;
            }
            doc.objects.iter().filter(|ob| layers.contains(&ob.layer)).collect()
        }
        (None, None) => doc.objects.iter().filter(|ob| doc.layer(&ob.layer).is_some_and(|l| l.visible)).collect(),
    };
    let open = o.open_paths.unwrap_or(true);
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut total = 0usize;
    for ob in objects {
        if matches!(ob.geometry, Geometry::Image { .. }) {
            continue;
        }
        let path = pathutil::vector_path(cmd, ob)?;
        let flat = geom::flatten(&path, geom::TOLERANCE).map_err(geom_err(cmd))?;
        let before = lines.len();
        for l in flat {
            if !open && !l.closed {
                continue;
            }
            total += l.points.len();
            if total > geom::MAX_POINTS {
                return Err(invalid(
                    cmd,
                    format!("the boundaries are too detailed (more than {} points); limit them with boundary_ids or boundary_layers", geom::MAX_POINTS),
                ));
            }
            lines.push(l);
        }
        if lines.len() > before {
            used += 1;
        }
    }
    Ok((lines, used))
}

fn options(cmd: &str, o: &RegionOptions) -> Result<FillOptions> {
    let gap_mm = util::non_negative(cmd, "gap_mm", o.gap_mm.unwrap_or(0.0))?;
    if gap_mm > MAX_GAP_MM {
        return Err(invalid(cmd, format!("gap_mm must be at most {MAX_GAP_MM} (got {gap_mm})")));
    }
    Ok(FillOptions { gap_mm, holes: o.holes.unwrap_or(true) })
}

fn points(cmd: &str, pts: &[[f64; 2]]) -> Result<Vec<Point>> {
    if pts.is_empty() {
        return Err(invalid(cmd, "points must list at least one [x, y] point"));
    }
    if pts.len() > geom::fill_region::MAX_POINTS_PER_CALL {
        return Err(invalid(cmd, format!("at most {} points at once", geom::fill_region::MAX_POINTS_PER_CALL)));
    }
    pts.iter().map(|&p| util::finite2(cmd, "points", p).map(|[x, y]| Point::new(x, y))).collect()
}

/// Run the fill for `pts`.
fn compute(cmd: &str, doc: &Document, pts: &[[f64; 2]], o: &RegionOptions) -> Result<(Vec<Point>, FillResult, usize)> {
    let opts = options(cmd, o)?;
    let pts = points(cmd, pts)?;
    let (lines, used) = boundary_lines(cmd, doc, o)?;
    let b = Boundaries::new(&lines).map_err(geom_err(cmd))?;
    let r = fill_regions(&b, &pts, &opts).map_err(geom_err(cmd))?;
    Ok((pts, r, used))
}

fn miss_code(m: Miss) -> &'static str {
    match m {
        Miss::NotEnclosed => "not_enclosed",
        Miss::OnLine => "on_line",
    }
}

fn contours_json(r: &Region) -> Value {
    json!(r.shapes.iter().flatten().map(|c| c.iter().map(|p| [r4(p[0]), r4(p[1])]).collect::<Vec<_>>()).collect::<Vec<_>>())
}

/// Per point: `{point, face}` or `{point, face: null, error, code}`.
fn points_json(pts: &[Point], r: &FillResult) -> Value {
    json!(pts
        .iter()
        .zip(&r.points)
        .map(|(p, res)| match *res {
            PointResult::Face(i) => json!({ "point": util::pt_json(*p), "face": i }),
            PointResult::Miss(m) => json!({ "point": util::pt_json(*p), "face": null, "error": m.message(), "code": miss_code(m) }),
        })
        .collect::<Vec<_>>())
}

/// `path.fill_region`.
pub struct FillRegion;

impl Command for FillRegion {
    type Input = FillRegionInput;
    const ID: &'static str = "path.fill_region";
    const TITLE: &'static str = "Fill region";
    const DESCRIPTION: &'static str = "Paint bucket: create a closed shape for the area that the lines around each \
        point enclose (field mm), on `layer`. Lines are every vector object on a visible layer (or boundary_ids / \
        boundary_layers); crossings split them, islands inside become holes unless holes is false, gap_mm closes \
        small openings. Points in one area make one shape; all shapes are one undo step. Fails, changing nothing, \
        when a point is not inside a closed area. Curves become polylines within 0.01 mm. Returns created ids, \
        faces [{id, points, area_mm2, holes}] and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FillRegionInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = match &i.name {
                Some(n) => util::name(cmd, "name", n)?,
                None => "Fill region".to_string(),
            };
            let (pts, r, _) = compute(cmd, doc, &i.points, &region_options!(i))?;
            let misses: Vec<String> = pts
                .iter()
                .zip(&r.points)
                .enumerate()
                .filter_map(|(k, (p, res))| match res {
                    PointResult::Miss(m) => Some(format!("point {} ({}, {}): {}", k + 1, r4(p.x), r4(p.y), m.message())),
                    PointResult::Face(_) => None,
                })
                .collect();
            if !misses.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: {}; nothing was created", misses.join("; "))));
            }
            let mut created: Vec<ObjectId> = Vec::new();
            let mut faces = Vec::new();
            for (k, face) in r.faces.iter().enumerate() {
                let id = pathutil::add_field_object(doc, &name, &layer, None, &face.to_path())?;
                let members: Vec<usize> = r.points.iter().enumerate().filter(|(_, x)| **x == PointResult::Face(k)).map(|(j, _)| j).collect();
                let holes = face.shapes.first().map_or(0, |s| s.len().saturating_sub(1));
                faces.push(json!({ "id": id, "points": members, "area_mm2": r4(face.area()), "holes": holes }));
                created.push(id);
            }
            let mut res = util::edit_result(doc, &created, &[]);
            res["faces"] = json!(faces);
            let lname = doc.layer(&layer).map_or(layer.clone(), |l| l.name.clone());
            let what = if created.len() == 1 { "1 region".to_string() } else { format!("{} regions", created.len()) };
            Ok((res, format!("filled {what} on {lname}")))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FillRegionPreviewInput {
    /// Field points [x, y] mm (at least one, at most 256).
    pub points: Vec<[f64; 2]>,
    /// Only these object or group ids are boundaries (any layer, shown or hidden). Not with `boundary_layers`.
    pub boundary_ids: Option<Vec<ObjectId>>,
    /// Only objects on these layer ids are boundaries (shown or hidden). Not with `boundary_ids`.
    pub boundary_layers: Option<Vec<LayerId>>,
    /// Open paths count as boundary lines (default true); false uses closed outlines only.
    pub open_paths: Option<bool>,
    /// Close openings narrower than this, mm (default 0: lines must meet; at most 10).
    pub gap_mm: Option<f64>,
    /// Islands inside an area (a pupil inside an eye) become holes (default true); false fills over them.
    pub holes: Option<bool>,
}

/// `path.fill_region_preview`.
pub struct FillRegionPreview;

impl Command for FillRegionPreview {
    type Input = FillRegionPreviewInput;
    const ID: &'static str = "path.fill_region_preview";
    const TITLE: &'static str = "Fill region preview";
    const DESCRIPTION: &'static str = "The areas path.fill_region would create for these points, with the same \
        options, without creating anything (the canvas highlight). Reads only. Returns faces [{contours (field mm; \
        the first is the outer one, the rest holes), area_mm2, bounds}] and points [{point, face}] or, for a point \
        that is not inside a closed area, [{point, face: null, error, code: not_enclosed | on_line}], plus \
        boundary_objects (how many objects were lines).";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: FillRegionPreviewInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let (pts, r, used) = compute(cmd, doc, &i.points, &region_options!(i))?;
        let faces: Vec<Value> = r
            .faces
            .iter()
            .map(|f| {
                json!({
                    "contours": contours_json(f),
                    "area_mm2": r4(f.area()),
                    "bounds": f.bounds().map(util::rect_json).unwrap_or(Value::Null),
                })
            })
            .collect();
        let n = r.faces.len();
        Ok((json!({ "faces": faces, "points": points_json(&pts, &r), "boundary_objects": used }), format!("{n} regions")))
    }
}
