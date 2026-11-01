//! `art.import_dxf`: DXF text → paths in mm, read with the `dxf` crate (MIT, ixmilia/dxf-rs).
//!
//! Units come from the header's `$INSUNITS`; without them the file is taken to be in mm (with a warning). DXF is
//! y-up like the field frame, so nothing is flipped. Read: LINE, LWPOLYLINE and POLYLINE (with bulge arcs), CIRCLE,
//! ARC, ELLIPSE, SPLINE (flattened) and INSERT (block references, with their scale, rotation and arrays, nested up to
//! [`MAX_DEPTH`]). Entities on paper space are skipped; TEXT, MTEXT, HATCH, DIMENSION and others are skipped with a
//! warning. Arcs and circles drawn in a flipped plane (extrusion 0,0,-1, common in CAM exports) are mirrored back.

use std::collections::BTreeMap;
use std::f64::consts::{PI, TAU};

use doc::{Document, Geometry, Group, ObjectId};
use dxf::entities::{Entity, EntityType};
use dxf::enums::Units;
use kurbo::{Affine, Arc, BezPath, Circle, ParamCurve, Point, Rect, Shape, SvgArc, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Refuse absurd inputs before parsing.
const MAX_DXF_BYTES: usize = 32 * 1024 * 1024;
/// Most nested block references followed.
const MAX_DEPTH: usize = 16;
/// Most path elements created in total.
const MAX_ELEMENTS: usize = 2_000_000;
/// Flattening tolerance for splines, mm (before unit scaling the tolerance is applied in file units).
const SPLINE_STEPS_PER_SPAN: usize = 16;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportDxfInput {
    /// The DXF file contents as text (ASCII DXF, not a path or URL).
    pub content: String,
    /// Exact layer id for the imported objects. Default: the first layer.
    pub layer: Option<String>,
    /// Display name (with " 1", " 2"... appended when split). Default "Imported DXF".
    pub name: Option<String>,
    /// Scale uniformly so the art's total width is this many mm. Default: the file's own size.
    pub width_mm: Option<f64>,
    /// One object per DXF entity, grouped, instead of a single combined object. Default false.
    #[serde(default)]
    pub split: bool,
    /// Keep the drawing's coordinates (its origin on the field centre) instead of centring the art on the field
    /// origin. Default false.
    #[serde(default)]
    pub keep_position: bool,
}

/// `art.import_dxf`.
pub struct ImportDxf;

/// mm per drawing unit, and whether the unit was assumed.
fn unit_scale(u: Units) -> (f64, bool) {
    let mm = match u {
        Units::Unitless => return (1.0, true),
        Units::Inches | Units::USSurveyInch => 25.4,
        Units::Feet | Units::USSurveyFeet => 304.8,
        Units::Miles | Units::USSurveyMile => 1_609_344.0,
        Units::Millimeters => 1.0,
        Units::Centimeters => 10.0,
        Units::Meters => 1000.0,
        Units::Kilometers => 1_000_000.0,
        Units::Microinches => 25.4e-6,
        Units::Mils => 0.0254,
        Units::Yards | Units::USSurveyYard => 914.4,
        Units::Angstroms => 1e-7,
        Units::Nanometers => 1e-6,
        Units::Microns => 1e-3,
        Units::Decimeters => 100.0,
        Units::Decameters => 10_000.0,
        Units::Hectometers => 100_000.0,
        _ => return (1.0, true),
    };
    (mm, false)
}

fn pt(p: &dxf::Point) -> Point {
    Point::new(p.x, p.y)
}

/// The mirror for entities drawn in a plane whose normal points down (their object coordinates have x reversed).
fn ocs(normal_z: f64) -> Affine {
    if normal_z < 0.0 { Affine::FLIP_X } else { Affine::IDENTITY }
}

/// Collects paths from entities.
struct Reader<'a> {
    drawing: &'a dxf::Drawing,
    paths: Vec<(BezPath, String)>,
    skipped: BTreeMap<String, usize>,
    elements: usize,
    warnings: Vec<String>,
}

impl Reader<'_> {
    fn push(&mut self, path: BezPath, label: &str, tf: Affine) -> Result<()> {
        if path.elements().len() < 2 {
            return Ok(());
        }
        self.elements += path.elements().len();
        if self.elements > MAX_ELEMENTS {
            return Err(CommandError::Failed(format!(
                "the DXF makes more than {MAX_ELEMENTS} path elements (block references are expanded for every use)"
            )));
        }
        self.paths.push((tf * path, label.to_string()));
        Ok(())
    }

    fn entities<'e>(&mut self, entities: impl Iterator<Item = &'e Entity>, tf: Affine, depth: usize) -> Result<()> {
        for e in entities {
            if e.common.is_in_paper_space || !e.common.is_visible {
                continue;
            }
            self.entity(e, tf, depth)?;
        }
        Ok(())
    }

    fn entity(&mut self, e: &Entity, tf: Affine, depth: usize) -> Result<()> {
        match &e.specific {
            EntityType::Line(l) => {
                let mut p = BezPath::new();
                p.move_to(pt(&l.p1));
                p.line_to(pt(&l.p2));
                self.push(p, "line", tf)
            }
            EntityType::LwPolyline(l) => {
                let v: Vec<(Point, f64)> = l.vertices.iter().map(|v| (Point::new(v.x, v.y), v.bulge)).collect();
                self.push(polyline(&v, l.is_closed()), "polyline", tf * ocs(l.extrusion_direction.z))
            }
            EntityType::Polyline(l) => {
                let v: Vec<(Point, f64)> = l.vertices().map(|v| (pt(&v.location), v.bulge)).collect();
                self.push(polyline(&v, l.is_closed()), "polyline", tf * ocs(l.normal.z))
            }
            EntityType::Circle(c) => {
                let p = Circle::new(pt(&c.center), c.radius.abs()).to_path(0.001);
                self.push(p, "circle", tf * ocs(c.normal.z))
            }
            EntityType::Arc(a) => {
                let start = a.start_angle.to_radians();
                let mut sweep = (a.end_angle - a.start_angle).to_radians().rem_euclid(TAU);
                if sweep == 0.0 {
                    sweep = TAU;
                }
                let arc = Arc::new(pt(&a.center), (a.radius.abs(), a.radius.abs()), start, sweep, 0.0);
                self.push(arc_path(&arc), "arc", tf * ocs(a.normal.z))
            }
            EntityType::Ellipse(el) => {
                let major = Vec2::new(el.major_axis.x, el.major_axis.y);
                let rx = major.hypot();
                let flipped = el.normal.z < 0.0;
                let (mut t0, t1) = (el.start_parameter, el.end_parameter);
                let mut sweep = (t1 - t0).rem_euclid(TAU);
                if sweep == 0.0 {
                    sweep = TAU;
                }
                if flipped {
                    // The minor axis runs the other way: walk the parameter backwards.
                    t0 = -t0;
                    sweep = -sweep;
                }
                let arc = Arc::new(pt(&el.center), (rx, rx * el.minor_axis_ratio.abs()), t0, sweep, major.atan2());
                self.push(arc_path(&arc), "ellipse", tf)
            }
            EntityType::Spline(s) => {
                let p = spline(s);
                if p.is_none() {
                    *self.skipped.entry("SPLINE (unreadable)".into()).or_default() += 1;
                }
                match p {
                    Some(p) => self.push(p, "spline", tf * ocs(s.normal.z)),
                    None => Ok(()),
                }
            }
            EntityType::Insert(ins) => {
                if depth >= MAX_DEPTH {
                    self.warnings.push(format!("block references nested deeper than {MAX_DEPTH} were skipped"));
                    return Ok(());
                }
                let drawing = self.drawing;
                let Some(block) = drawing.blocks().find(|b| b.name == ins.name) else {
                    self.warnings.push(format!("block \"{}\" is referenced but not defined; skipped", ins.name));
                    return Ok(());
                };
                let base = Affine::scale_non_uniform(ins.x_scale_factor, ins.y_scale_factor)
                    * Affine::translate(-pt(&block.base_point).to_vec2());
                let (cols, rows) = (i32::from(ins.column_count).max(1), i32::from(ins.row_count).max(1));
                if i64::from(cols) * i64::from(rows) > 10_000 {
                    return Err(invalid(ImportDxf::ID, "a block reference array has more than 10000 copies"));
                }
                for r in 0..rows {
                    for c in 0..cols {
                        let offset = Vec2::new(f64::from(c) * ins.column_spacing, f64::from(r) * ins.row_spacing);
                        let place = Affine::translate(pt(&ins.location).to_vec2())
                            * Affine::rotate(ins.rotation.to_radians())
                            * Affine::translate(offset)
                            * base;
                        self.entities(block.entities.iter(), tf * ocs(ins.extrusion_direction.z) * place, depth + 1)?;
                    }
                }
                Ok(())
            }
            other => {
                let kind = format!("{other:?}");
                let kind = kind.split(['(', ' ', '{']).next().unwrap_or("entity").to_uppercase();
                *self.skipped.entry(kind).or_default() += 1;
                Ok(())
            }
        }
    }
}

fn arc_path(arc: &Arc) -> BezPath {
    let mut p = BezPath::new();
    p.move_to(arc.start());
    arc.to_cubic_beziers(0.001, |a, b, c| p.curve_to(a, b, c));
    p
}

/// A polyline with DXF bulges (`tan(angle / 4)` of the arc to the next vertex; positive = counter-clockwise).
fn polyline(v: &[(Point, f64)], closed: bool) -> BezPath {
    let mut p = BezPath::new();
    let Some(&(first, _)) = v.first() else { return p };
    p.move_to(first);
    let n = v.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    for k in 0..segments {
        let (a, bulge) = v[k];
        let b = v[(k + 1) % n].0;
        if bulge.abs() > 1e-12 && a != b {
            let theta = 4.0 * bulge.atan();
            let r = (b - a).hypot() / (2.0 * (theta / 2.0).sin().abs());
            let svg = SvgArc {
                from: a,
                to: b,
                radii: Vec2::new(r, r),
                x_rotation: 0.0,
                large_arc: theta.abs() > PI,
                sweep: bulge > 0.0,
            };
            match Arc::from_svg_arc(&svg) {
                Some(arc) => arc.to_cubic_beziers(0.001, |c1, c2, end| p.curve_to(c1, c2, end)),
                None => p.line_to(b),
            }
        } else if !(closed && k + 1 == n) {
            p.line_to(b);
        }
    }
    if closed {
        p.close_path();
    }
    p
}

/// A NURBS curve (degree, knots, weights, control points) as a polyline, or a polyline through the fit points
/// when there are no control points.
fn spline(s: &dxf::entities::Spline) -> Option<BezPath> {
    let ctrl: Vec<Point> = s.control_points.iter().map(pt).collect();
    let mut p = BezPath::new();
    if ctrl.is_empty() {
        let fit: Vec<Point> = s.fit_points.iter().map(pt).collect();
        let (&first, rest) = fit.split_first()?;
        p.move_to(first);
        rest.iter().for_each(|q| p.line_to(*q));
        if s.is_closed() {
            p.close_path();
        }
        return Some(p);
    }
    let degree = usize::try_from(s.degree_of_curve).ok().filter(|d| (1..=11).contains(d))?;
    let knots = &s.knot_values;
    if knots.len() != ctrl.len() + degree + 1 || knots.windows(2).any(|w| w[1] < w[0]) {
        return None;
    }
    let weights: Vec<f64> = if s.weight_values.len() == ctrl.len() { s.weight_values.clone() } else { vec![1.0; ctrl.len()] };
    let (lo, hi) = (knots[degree], knots[ctrl.len()]);
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let steps = (ctrl.len() - degree) * SPLINE_STEPS_PER_SPAN;
    for k in 0..=steps {
        let t = lo + (hi - lo) * k as f64 / steps as f64;
        let q = de_boor(degree, knots, &ctrl, &weights, t)?;
        if k == 0 {
            p.move_to(q);
        } else {
            p.line_to(q);
        }
    }
    if s.is_closed() {
        p.close_path();
    }
    Some(p)
}

/// Point of a rational B-spline at `t` (de Boor's algorithm in homogeneous coordinates).
fn de_boor(p: usize, u: &[f64], ctrl: &[Point], w: &[f64], t: f64) -> Option<Point> {
    let n = ctrl.len();
    // The span k with u[k] <= t < u[k + 1], clamped to the valid range.
    let mut k = p;
    while k + 1 < n && u[k + 1] <= t {
        k += 1;
    }
    let mut d: Vec<(f64, f64, f64)> =
        (0..=p).map(|j| ctrl[j + k - p]).zip((0..=p).map(|j| w[j + k - p])).map(|(c, w)| (c.x * w, c.y * w, w)).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = j + k - p;
            let den = u[i + p + 1 - r] - u[i];
            let a = if den.abs() < 1e-300 { 0.0 } else { (t - u[i]) / den };
            d[j] = (
                (1.0 - a) * d[j - 1].0 + a * d[j].0,
                (1.0 - a) * d[j - 1].1 + a * d[j].1,
                (1.0 - a) * d[j - 1].2 + a * d[j].2,
            );
        }
    }
    let (x, y, w) = d[p];
    (w.abs() > 1e-300).then(|| Point::new(x / w, y / w))
}

impl Command for ImportDxf {
    type Input = ImportDxfInput;
    const ID: &'static str = "art.import_dxf";
    const TITLE: &'static str = "Import DXF";
    const DESCRIPTION: &'static str = "Import vector art from DXF text (lines, polylines with arcs, circles, arcs, \
        ellipses, splines, block references) in mm from the file's $INSUNITS (no units: mm, with a warning), \
        centred on the field origin unless keep_position. Optional width_mm rescales uniformly; split makes one \
        object per entity, grouped. Text, hatches and dimensions are skipped. Returns {created, group, bounds, \
        units, warnings}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportDxfInput) -> Result<(Value, String)> {
        let id = Self::ID;
        if i.content.len() > MAX_DXF_BYTES {
            return Err(invalid(id, format!("content is larger than {} MB", MAX_DXF_BYTES / (1024 * 1024))));
        }
        let base = i.name.as_deref().map(|n| util::name(id, "name", n)).transpose()?.unwrap_or_else(|| "Imported DXF".into());
        let width = i.width_mm.map(|w| util::positive(id, "width_mm", w)).transpose()?;
        let text = i.content.trim_start_matches('\u{feff}');
        let drawing = dxf::Drawing::load(&mut text.as_bytes())
            .map_err(|e| invalid(id, format!("content could not be read as DXF: {e}")))?;
        let (scale, assumed) = unit_scale(drawing.header.default_drawing_units);
        let mut reader = Reader { drawing: &drawing, paths: vec![], skipped: BTreeMap::new(), elements: 0, warnings: vec![] };
        reader.entities(drawing.entities(), Affine::scale(scale), 0)?;
        let Reader { paths, skipped, mut warnings, .. } = reader;
        if assumed {
            warnings.insert(0, "the DXF does not say its units ($INSUNITS); millimetres were assumed".into());
        }
        if !skipped.is_empty() {
            let list: Vec<String> = skipped.iter().map(|(k, n)| format!("{n} {k}")).collect();
            warnings.push(format!("skipped entities OpenEtch does not import: {}", list.join(", ")));
        }
        if paths.is_empty() {
            return Err(CommandError::Failed("the DXF contains no lines, arcs, circles, polylines or splines".into()));
        }
        let bounds = paths.iter().map(|(p, _)| p.bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
        let k = match width {
            Some(w) if bounds.width() <= 1e-9 => {
                return Err(CommandError::Failed(format!("cannot scale to width {w} mm: the art has zero width")));
            }
            Some(w) => w / bounds.width(),
            None => 1.0,
        };
        let fit = if i.keep_position {
            Affine::scale(k)
        } else {
            Affine::scale(k) * Affine::translate(-bounds.center().to_vec2())
        };
        let size = bounds.size() * k;
        if size.width > 300.0 || size.height > 300.0 {
            warnings.push(format!(
                "imported art is {:.1} x {:.1} mm, larger than a typical galvo field; check the DXF units or pass \
                 width_mm",
                size.width, size.height
            ));
        }
        let units = format!("{:?}", drawing.header.default_drawing_units);
        transact(doc, |doc| {
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let mut created: Vec<ObjectId> = Vec::new();
            let mut group = None;
            if i.split && paths.len() > 1 {
                let gid = doc.alloc_id("group")?;
                doc.groups.push(Group::new(gid.clone(), base.clone(), None));
                group = Some(gid);
                let mut counts: BTreeMap<String, usize> = BTreeMap::new();
                for (p, label) in paths {
                    let n = counts.entry(label.clone()).or_default();
                    *n += 1;
                    let path = fit * p;
                    validate_path(id, &path)?;
                    let obj = add_object(doc, format!("{base} {label} {n}"), layer.clone(), Geometry::Path { path }, Affine::IDENTITY)?;
                    if let Some(o) = doc.object_mut(&obj) {
                        o.group = group.clone();
                    }
                    created.push(obj);
                }
            } else {
                let mut all = BezPath::new();
                for (p, _) in paths {
                    all.extend(fit * p);
                }
                validate_path(id, &all)?;
                created.push(add_object(doc, base.clone(), layer, Geometry::Path { path: all }, Affine::IDENTITY)?);
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["group"] = json!(group);
            r["units"] = json!(units);
            r["warnings"] = json!(warnings);
            Ok((r, format!("imported DXF as {base} ({} object(s))", created.len())))
        })
    }
}

/// Files with one of `exts` under the directories in `OPENETCH_LBRN_SAMPLES` (OS path-list form) and their
/// subdirectories, skipping `target`, `node_modules`, dot directories and source trees. `None` when the variable is
/// not set. The owner's sample files are private: tests read them in place and never copy them.
#[cfg(test)]
pub(crate) fn sample_files(exts: &[&str]) -> Option<Vec<std::path::PathBuf>> {
    fn walk(dir: &std::path::Path, exts: &[&str], out: &mut std::collections::BTreeSet<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let source = p.join("Cargo.toml").exists() || p.join(".git").exists();
                if !(name.starts_with('.') || name == "target" || name == "node_modules" || source) {
                    walk(&p, exts, out);
                }
            } else if p.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e))) {
                out.insert(p);
            }
        }
    }
    let dirs = std::env::var_os("OPENETCH_LBRN_SAMPLES")?;
    let mut out = std::collections::BTreeSet::new();
    for d in std::env::split_paths(&dirs) {
        walk(&d, exts, &mut out);
    }
    Some(out.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_dxf_files_import() {
        let Some(files) = sample_files(&["dxf"]) else {
            println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
            return;
        };
        for f in files {
            let content = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let mut d = Document { field_mm: Some(150.0), ..Document::default() };
            let (r, _) = import(&mut d, json!({"content": content, "split": true}))
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            println!(
                "== {}: {} object(s), size {} mm, units {}, warnings {}",
                f.file_name().unwrap_or_default().to_string_lossy(),
                r["created"].as_array().map_or(0, Vec::len),
                r["bounds"]["size"],
                r["units"],
                r["warnings"]
            );
            assert!(r["created"].as_array().is_some_and(|c| !c.is_empty()));
        }
    }

    fn import(d: &mut Document, v: Value) -> Result<(Value, String)> {
        ImportDxf::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    /// A minimal ASCII DXF: header units, then the given entity code pairs.
    fn dxf(units: Option<i32>, entities: &str) -> String {
        let header = match units {
            Some(u) => format!("0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n9\n$INSUNITS\n70\n{u}\n0\nENDSEC\n"),
            None => "0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1015\n0\nENDSEC\n".to_string(),
        };
        format!("{header}0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n")
    }

    const LINE: &str = "0\nLINE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n10\n21\n0\n31\n0\n";
    const CIRCLE: &str = "0\nCIRCLE\n8\n0\n10\n20\n20\n0\n30\n0\n40\n5\n";

    fn close(a: &Value, b: [f64; 2]) -> bool {
        (a[0].as_f64().unwrap() - b[0]).abs() < 1e-3 && (a[1].as_f64().unwrap() - b[1]).abs() < 1e-3
    }

    #[test]
    fn lines_and_circles_in_file_units() {
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), &format!("{LINE}{CIRCLE}"))})).unwrap();
        assert_eq!(r["created"], json!(["obj-2"]));
        assert_eq!(r["units"], json!("Millimeters"));
        // Line 0..10 on y = 0, circle centre (20, 0) r 5: x 0..25, y -5..5, centred on the origin.
        assert!(close(&r["bounds"]["size"], [25.0, 10.0]), "{}", r["bounds"]);
        assert!(close(&r["bounds"]["center"], [0.0, 0.0]));
        assert_eq!(r["warnings"], json!([]));
        // Inches, kept in place.
        let (r, _) = import(&mut d, json!({"content": dxf(Some(1), LINE), "keep_position": true})).unwrap();
        assert!(close(&r["bounds"]["min"], [0.0, 0.0]) && close(&r["bounds"]["max"], [254.0, 0.0]), "{}", r["bounds"]);
        assert_eq!(r["warnings"], json!([]));
    }

    #[test]
    fn no_units_assume_mm_and_split_makes_a_group() {
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"content": dxf(None, &format!("{LINE}{CIRCLE}")), "split": true, "name": "jig"})).unwrap();
        assert!(r["warnings"][0].as_str().unwrap().contains("millimetres were assumed"), "{}", r["warnings"]);
        assert_eq!(r["created"], json!(["obj-3", "obj-4"]));
        assert_eq!(r["group"], json!("group-2"));
        let g = d.group("group-2").unwrap();
        assert_eq!(g.name, "jig");
        assert_eq!(d.group_members("group-2"), ["obj-3", "obj-4"]);
        assert_eq!(d.objects[0].name, "jig line 1");
        assert_eq!(d.objects[1].name, "jig circle 1");
    }

    #[test]
    fn polylines_arcs_ellipses_splines_and_blocks() {
        // A closed LWPOLYLINE square 10 x 10 whose top edge bulges up by a half circle (bulge 1).
        let lw = "0\nLWPOLYLINE\n8\n0\n90\n4\n70\n1\n10\n0\n20\n0\n10\n10\n20\n0\n10\n10\n20\n10\n42\n1\n10\n0\n20\n10\n";
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), lw), "keep_position": true})).unwrap();
        assert!(close(&r["bounds"]["min"], [0.0, 0.0]) && close(&r["bounds"]["max"], [10.0, 15.0]), "{}", r["bounds"]);
        // A quarter ARC r 10 from 0 to 90 degrees, and the same flipped by its extrusion.
        let arc = "0\nARC\n8\n0\n10\n0\n20\n0\n30\n0\n40\n10\n50\n0\n51\n90\n";
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), arc), "keep_position": true})).unwrap();
        assert!(close(&r["bounds"]["min"], [0.0, 0.0]) && close(&r["bounds"]["max"], [10.0, 10.0]), "{}", r["bounds"]);
        let flipped = "0\nARC\n8\n0\n10\n0\n20\n0\n30\n0\n40\n10\n210\n0\n220\n0\n230\n-1\n50\n0\n51\n90\n";
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), flipped), "keep_position": true})).unwrap();
        assert!(close(&r["bounds"]["min"], [-10.0, 0.0]) && close(&r["bounds"]["max"], [0.0, 10.0]), "{}", r["bounds"]);
        // A full ELLIPSE with major axis 8 along y and ratio 0.5.
        let el = "0\nELLIPSE\n8\n0\n10\n0\n20\n0\n30\n0\n11\n0\n21\n8\n31\n0\n40\n0.5\n41\n0\n42\n6.283185307179586\n";
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), el)})).unwrap();
        assert!(close(&r["bounds"]["size"], [8.0, 16.0]), "{}", r["bounds"]);
        // A clamped quadratic SPLINE (0,0) (5,10) (10,0): its top is at y = 5.
        let sp = "0\nSPLINE\n8\n0\n70\n8\n71\n2\n72\n6\n73\n3\n40\n0\n40\n0\n40\n0\n40\n1\n40\n1\n40\n1\n\
            10\n0\n20\n0\n30\n0\n10\n5\n20\n10\n30\n0\n10\n10\n20\n0\n30\n0\n";
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), sp), "keep_position": true})).unwrap();
        assert!(close(&r["bounds"]["max"], [10.0, 5.0]) && close(&r["bounds"]["min"], [0.0, 0.0]), "{}", r["bounds"]);
        // A block holding the LINE, inserted twice: at (100, 0) and rotated 90 degrees at (0, 50), scaled 2.
        let blocks = format!("0\nSECTION\n2\nBLOCKS\n0\nBLOCK\n8\n0\n2\nB1\n70\n0\n10\n0\n20\n0\n30\n0\n{LINE}0\nENDBLK\n0\nENDSEC\n");
        let inserts = "0\nINSERT\n8\n0\n2\nB1\n10\n100\n20\n0\n30\n0\n\
            0\nINSERT\n8\n0\n2\nB1\n10\n0\n20\n50\n30\n0\n41\n2\n42\n2\n50\n90\n";
        let text = dxf(Some(4), inserts).replacen("0\nSECTION\n2\nENTITIES", &format!("{blocks}0\nSECTION\n2\nENTITIES"), 1);
        let (r, _) = import(&mut d, json!({"content": text, "keep_position": true, "split": true})).unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        assert!(close(&r["bounds"]["min"], [0.0, 0.0]) && close(&r["bounds"]["max"], [110.0, 70.0]), "{}", r["bounds"]);
    }

    #[test]
    fn width_text_and_refusals() {
        let mut d = Document::default();
        let text = "0\nTEXT\n8\n0\n10\n0\n20\n0\n30\n0\n40\n2\n1\nhello\n";
        let (r, _) = import(&mut d, json!({"content": dxf(Some(4), &format!("{LINE}{text}")), "width_mm": 50})).unwrap();
        assert!(close(&r["bounds"]["size"], [50.0, 0.0]));
        assert!(r["warnings"].to_string().contains("1 TEXT"), "{}", r["warnings"]);
        let before = d.clone();
        assert!(import(&mut d, json!({"content": "not a dxf"})).is_err());
        assert!(import(&mut d, json!({"content": dxf(Some(4), text)})).is_err(), "nothing to import");
        assert!(import(&mut d, json!({"content": dxf(Some(4), LINE), "width_mm": 0})).is_err());
        assert!(import(&mut d, json!({"content": dxf(Some(4), LINE), "layer": "layer-9"})).is_err());
        assert_eq!(d, before);
    }
}
