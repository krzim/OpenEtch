//! Shape generators: `shape.polygon`, `shape.line`, `shape.circle`.

use doc::{Document, Geometry};
use kurbo::{Affine, Point, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil::geom_err;
use super::shape::add_object;
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// Most points `shape.line` takes.
const MAX_LINE_POINTS: usize = 100_000;

/// Largest `shape.circle` diameter (1 km radius, as `shape.polygon`'s largest radius).
const MAX_CIRCLE_DIAMETER_MM: f64 = 2e6;

fn name_or(cmd: &str, name: Option<&str>, default: &str) -> Result<String> {
    Ok(name.map(|n| util::name(cmd, "name", n)).transpose()?.unwrap_or_else(|| default.to_string()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolygonInput {
    /// Number of sides (or star points), 3..=1000.
    pub sides: u32,
    /// Distance from the centre to the corners (or star points), in mm. Give this or width_mm.
    pub radius_mm: Option<f64>,
    /// Overall width of the shape in mm (before rotation); the radius is derived from it. Give this or radius_mm.
    pub width_mm: Option<f64>,
    /// Make a star: inner corners at this fraction of the radius (0 < r < 1, e.g. 0.5).
    pub star_inner_ratio: Option<f64>,
    /// Round every corner with this radius in mm (corners where it does not fit stay sharp). Default 0.
    #[serde(default)]
    pub corner_radius_mm: f64,
    /// Centre of the corner circle in the field frame, mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Counter-clockwise rotation in degrees; at 0 the first corner points straight up (+Y). Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Polygon" or "Star".
    pub name: Option<String>,
    /// Keep it a parametric polygon (`Geometry::Polygon`), so `shape.update` can change its sides, radius, star ratio
    /// and corner radius later (LightBurn's Shape Properties). Default false: a plain path, for clients that do not
    /// draw the polygon kind yet.
    #[serde(default)]
    pub parametric: bool,
}

/// `shape.polygon`.
pub struct ShapePolygon;

impl Command for ShapePolygon {
    type Input = PolygonInput;
    const ID: &'static str = "shape.polygon";
    const TITLE: &'static str = "Polygon / star";
    const DESCRIPTION: &'static str = "Add a regular polygon (3..=1000 sides) or, with star_inner_ratio, a star, \
        sized by radius_mm (centre to corner) or width_mm, centred at `center` (field mm), first corner up, rotated \
        counter-clockwise by rotation_deg, optionally with rounded corners. It is stored as a path, or with \
        parametric: true as a polygon whose parameters shape.update edits. Returns the new id, `corners_left_sharp` \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PolygonInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let star = i.star_inner_ratio;
            let unit = geom::shapes::polygon_points(i.sides, 1.0, star).map_err(geom_err(cmd))?;
            let radius = match (i.radius_mm, i.width_mm) {
                (Some(r), None) => util::positive(cmd, "radius_mm", r)?,
                (None, Some(w)) => {
                    let w = util::positive(cmd, "width_mm", w)?;
                    let (lo, hi) = unit.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
                    w / (hi - lo)
                }
                _ => return Err(invalid(cmd, "give exactly one of radius_mm and width_mm")),
            };
            if radius > 1e6 {
                return Err(invalid(cmd, "the polygon is too large"));
            }
            let cr = util::non_negative(cmd, "corner_radius_mm", i.corner_radius_mm)?;
            let pts: Vec<Point> = unit.iter().map(|p| Point::new(p.x * radius, p.y * radius)).collect();
            let (path, sharp) = geom::shapes::polygon_path(&pts, cr);
            let [x, y] = util::finite2(cmd, "center", i.center)?;
            let rot = util::finite(cmd, "rotation_deg", i.rotation_deg)?;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = name_or(cmd, i.name.as_deref(), if star.is_some() { "Star" } else { "Polygon" })?;
            let t = Affine::translate((x, y)) * Affine::rotate(rot.to_radians());
            let geometry = if i.parametric {
                Geometry::Polygon { sides: i.sides, radius_mm: radius, star_ratio: star, corner_radius_mm: cr }
            } else {
                Geometry::Path { path }
            };
            let id = add_object(doc, name, layer, geometry, t)?;
            let mut r = util::edit_result(doc, std::slice::from_ref(&id), &[]);
            r["corners_left_sharp"] = json!(if cr > 0.0 { sharp } else { 0 });
            Ok((r, format!("added {}-sided {} {id}", i.sides, if star.is_some() { "star" } else { "polygon" })))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineInput {
    /// The points [[x, y], ...] in field mm, at least 2 (3 when closed), at most 100000.
    pub points: Vec<[f64; 2]>,
    /// Close the line back to the first point. Default false.
    #[serde(default)]
    pub closed: bool,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Line".
    pub name: Option<String>,
}

/// `shape.line`.
pub struct ShapeLine;

impl Command for ShapeLine {
    type Input = LineInput;
    const ID: &'static str = "shape.line";
    const TITLE: &'static str = "Line / polyline";
    const DESCRIPTION: &'static str = "Add a straight line or polyline through field-frame points (mm), optionally \
        closed (LightBurn's Draw Lines without curves; for curves use shape.path with SVG path data). Repeated \
        points are merged. Stored as a path whose local origin is the bounds centre. Returns the new id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: LineInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.points.len() > MAX_LINE_POINTS {
                return Err(invalid(cmd, format!("at most {MAX_LINE_POINTS} points")));
            }
            let pts: Vec<Point> = i.points.iter().map(|p| Point::new(p[0], p[1])).collect();
            let field = geom::shapes::polyline(&pts, i.closed).map_err(geom_err(cmd))?;
            let c = field.bounding_box().center();
            let local = Affine::translate(-c.to_vec2()) * field;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = name_or(cmd, i.name.as_deref(), "Line")?;
            let id = add_object(doc, name, layer, Geometry::Path { path: local }, Affine::translate(c.to_vec2()))?;
            let r = util::edit_result(doc, std::slice::from_ref(&id), &[]);
            Ok((r, format!("added line {id} through {} points", i.points.len())))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CircleInput {
    /// Diameter in mm, > 0 and at most 2000000.
    pub diameter_mm: f64,
    /// Centre in the field frame, mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Circle".
    pub name: Option<String>,
}

/// `shape.circle`.
pub struct ShapeCircle;

impl Command for ShapeCircle {
    type Input = CircleInput;
    const ID: &'static str = "shape.circle";
    const TITLE: &'static str = "Circle";
    const DESCRIPTION: &'static str = "Add a circle of diameter_mm centred at `center` (field mm), stored as an \
        ellipse with equal radii (as LightBurn stores circles). Returns the new id and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CircleInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let d = util::positive(cmd, "diameter_mm", i.diameter_mm)?;
            // The outline is drawn with arcs at 0.01 mm tolerance, so its size in memory grows with the radius: a
            // diameter of 1e308 asked for a 60 GB allocation and aborted the process.
            if d > MAX_CIRCLE_DIAMETER_MM {
                return Err(invalid(cmd, format!("diameter_mm must be at most {MAX_CIRCLE_DIAMETER_MM}")));
            }
            let [x, y] = util::finite2(cmd, "center", i.center)?;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let name = name_or(cmd, i.name.as_deref(), "Circle")?;
            let g = Geometry::Ellipse { rx_mm: d / 2.0, ry_mm: d / 2.0 };
            let id = add_object(doc, name, layer, g, Affine::translate((x, y)))?;
            Ok((util::edit_result(doc, std::slice::from_ref(&id), &[]), format!("added circle {id} ⌀{d} mm")))
        })
    }
}
