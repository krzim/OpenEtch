//! Fill Region hover in the browser: the answer of the engine's `path.fill_region_preview` command
//! (crates/commands/src/builtin/fill_region.rs) for the default boundaries (every vector object on a visible layer,
//! open paths included), computed from the browser's copy of the document. The prepared boundaries are kept until
//! the document changes, so moving the pointer costs only the face search.
//!
//! Requests the engine command takes but this does not (`boundary_ids`, `boundary_layers`, `open_paths`) are refused
//! with [`UNSUPPORTED`]; the web app then asks the engine. Creating the fill is always the engine's command.

use doc::{Document, Geometry, Object};
use geom::fill_region::{Boundaries, FillOptions, MAX_GAP_MM, MAX_POINTS_PER_CALL, Miss, PointResult, fill_regions};
use geom::{GeomError, Polyline, Region};
use kurbo::{BezPath, Point, Rect, Shape};
use serde::Deserialize;
use serde_json::{Value, json};

const CMD: &str = "path.fill_region_preview";

/// The error text of a request only the engine answers.
pub const UNSUPPORTED: &str = "unsupported here";

/// The engine command's input (`FillRegionPreviewInput`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub points: Vec<[f64; 2]>,
    pub boundary_ids: Option<Vec<String>>,
    pub boundary_layers: Option<Vec<String>>,
    pub open_paths: Option<bool>,
    pub gap_mm: Option<f64>,
    pub holes: Option<bool>,
}

/// The default boundaries of one document, prepared once.
#[derive(Debug, Default)]
pub struct Cache {
    prepared: Option<(Boundaries, usize)>,
}

impl Cache {
    /// Forget the boundaries (the document changed).
    pub fn clear(&mut self) {
        self.prepared = None;
    }
}

fn invalid(message: impl std::fmt::Display) -> String {
    format!("invalid input for '{CMD}': {message}")
}

fn geom_err(e: GeomError) -> String {
    match e {
        GeomError::Invalid(m) => invalid(m),
        GeomError::Engine(m) => format!("{CMD}: the geometry engine failed ({m})"),
    }
}

fn r4(x: f64) -> f64 {
    let r = (x * 1e4).round() / 1e4;
    if !r.is_finite() {
        x
    } else if r == 0.0 {
        0.0
    } else {
        r
    }
}

fn pt_json(p: Point) -> Value {
    json!([r4(p.x), r4(p.y)])
}

fn rect_json(r: Rect) -> Value {
    json!({
        "min": [r4(r.x0), r4(r.y0)],
        "max": [r4(r.x1), r4(r.y1)],
        "size": [r4(r.width()), r4(r.height())],
        "center": pt_json(r.center()),
    })
}

/// The object's outline in the field frame as the path tools see it (`pathutil::vector_path`): a sharp rectangle is
/// just its four lines.
fn vector_path(o: &Object) -> BezPath {
    let local = match o.geometry {
        Geometry::Rect { width_mm, height_mm, corner_radius_mm: 0.0 } => Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)).to_path(0.01),
        ref g => g.local_path(),
    };
    o.transform * local
}

/// Every vector object on a visible layer, flattened (`boundary_lines` with no options).
fn boundary_lines(doc: &Document) -> Result<(Vec<Polyline>, usize), String> {
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut total = 0usize;
    for ob in doc.objects.iter().filter(|ob| doc.layer(&ob.layer).is_some_and(|l| l.visible)) {
        if matches!(ob.geometry, Geometry::Image { .. }) {
            continue;
        }
        let flat = geom::flatten(&vector_path(ob), geom::TOLERANCE).map_err(geom_err)?;
        let before = lines.len();
        for l in flat {
            total += l.points.len();
            if total > geom::MAX_POINTS {
                return Err(invalid(format!(
                    "the boundaries are too detailed (more than {} points); limit them with boundary_ids or boundary_layers",
                    geom::MAX_POINTS
                )));
            }
            lines.push(l);
        }
        if lines.len() > before {
            used += 1;
        }
    }
    Ok((lines, used))
}

fn contours_json(r: &Region) -> Value {
    json!(r.shapes.iter().flatten().map(|c| c.iter().map(|p| [r4(p[0]), r4(p[1])]).collect::<Vec<_>>()).collect::<Vec<_>>())
}

fn miss_code(m: Miss) -> &'static str {
    match m {
        Miss::NotEnclosed => "not_enclosed",
        Miss::OnLine => "on_line",
    }
}

/// The engine command's result for `params` on `doc`, or its error text ([`UNSUPPORTED`] for options only the engine
/// takes).
pub fn preview(doc: &Document, cache: &mut Cache, params: &Value) -> Result<Value, String> {
    let i: Input = serde_json::from_value(params.clone()).map_err(invalid)?;
    if i.boundary_ids.is_some() || i.boundary_layers.is_some() || i.open_paths.is_some() {
        return Err(UNSUPPORTED.into());
    }
    let gap_mm = i.gap_mm.unwrap_or(0.0);
    if !gap_mm.is_finite() || gap_mm < 0.0 {
        return Err(invalid(format!("gap_mm must be a non-negative number (got {gap_mm})")));
    }
    if gap_mm > MAX_GAP_MM {
        return Err(invalid(format!("gap_mm must be at most {MAX_GAP_MM} (got {gap_mm})")));
    }
    let opts = FillOptions { gap_mm, holes: i.holes.unwrap_or(true) };
    if i.points.is_empty() {
        return Err(invalid("points must list at least one [x, y] point"));
    }
    if i.points.len() > MAX_POINTS_PER_CALL {
        return Err(invalid(format!("at most {MAX_POINTS_PER_CALL} points at once")));
    }
    if i.points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(invalid("points must be finite numbers"));
    }
    let pts: Vec<Point> = i.points.iter().map(|&[x, y]| Point::new(x, y)).collect();
    if cache.prepared.is_none() {
        let (lines, used) = boundary_lines(doc)?;
        cache.prepared = Some((Boundaries::new(&lines).map_err(geom_err)?, used));
    }
    let Some((b, used)) = &cache.prepared else { unreachable!("prepared above") };
    let r = fill_regions(b, &pts, &opts).map_err(geom_err)?;
    let faces: Vec<Value> = r
        .faces
        .iter()
        .map(|f| json!({ "contours": contours_json(f), "area_mm2": r4(f.area()), "bounds": f.bounds().map_or(Value::Null, rect_json) }))
        .collect();
    let points: Vec<Value> = pts
        .iter()
        .zip(&r.points)
        .map(|(p, res)| match *res {
            PointResult::Face(k) => json!({ "point": pt_json(*p), "face": k }),
            PointResult::Miss(m) => json!({ "point": pt_json(*p), "face": null, "error": m.message(), "code": miss_code(m) }),
        })
        .collect();
    Ok(json!({ "faces": faces, "points": points, "boundary_objects": used }))
}
