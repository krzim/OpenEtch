//! `art.import_svg`: SVG text → field-frame paths in mm.
//!
//! usvg resolves the whole document for us: styles, `<use>`, basic shapes, every transform, and the root
//! `viewBox` → `width`/`height` mapping. After that, every path's absolute coordinates are in CSS pixels at the
//! [`usvg::Options::dpi`] we pass (96), whatever units the root `width`/`height` were in. So one px → mm factor covers
//! both unit-less (px) files and files with mm `width`/`height` plus a `viewBox`.

use doc::{Document, Geometry, ObjectId};
use kurbo::{Affine, BezPath, Rect, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use usvg::tiny_skia_path::PathSegment;

use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// CSS reference resolution.
const DPI: f64 = 96.0;
const MM_PER_PX: f64 = 25.4 / DPI;
/// Refuse absurd inputs before parsing.
const MAX_SVG_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportSvgInput {
    /// The SVG file contents as text (not a path or URL).
    pub svg: String,
    /// Exact layer id for the imported objects. Default: the first layer.
    pub layer: Option<String>,
    /// Display name (with " 1", " 2"... appended when split). Default "Imported art".
    pub name: Option<String>,
    /// Scale uniformly so the art's total width is this many mm. Default: keep the SVG's physical size (px at 96 dpi,
    /// or the root width/height units).
    pub width_mm: Option<f64>,
    /// Create one object per SVG path instead of a single combined object. Default false.
    #[serde(default)]
    pub split: bool,
}

/// `art.import_svg`.
pub struct ImportSvg;

impl Command for ImportSvg {
    type Input = ImportSvgInput;
    const ID: &'static str = "art.import_svg";
    const TITLE: &'static str = "Import SVG";
    const DESCRIPTION: &'static str = "Import vector art from SVG text: every filled or stroked shape becomes its \
        outline path, sized in mm (px at 96 dpi, or the root's mm/in units), flipped to the y-up field frame and \
        centred on the field origin. Optional width_mm rescales uniformly; split makes one object per path. Use \
        art.place afterwards to position it on the stock. Text elements are ignored (convert text to paths first).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportSvgInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = Self::ID;
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let base = i
                .name
                .as_deref()
                .map(|n| util::name(id, "name", n))
                .transpose()?
                .unwrap_or_else(|| "Imported art".into());
            let width = i.width_mm.map(|w| util::positive(id, "width_mm", w)).transpose()?;
            let (paths, mut warnings) = svg_to_field_paths(&i.svg, width)?;

            let created: Vec<ObjectId> = if i.split {
                let n = paths.len();
                paths
                    .into_iter()
                    .enumerate()
                    .map(|(k, (path, label))| {
                        let name = match label {
                            Some(l) => format!("{base} {l}"),
                            None if n == 1 => base.clone(),
                            None => format!("{base} {}", k + 1),
                        };
                        add_object(doc, name, layer.clone(), Geometry::Path { path }, Affine::IDENTITY)
                    })
                    .collect::<Result<_>>()?
            } else {
                let mut all = BezPath::new();
                for (p, _) in paths {
                    all.extend(p);
                }
                vec![add_object(doc, base.clone(), layer.clone(), Geometry::Path { path: all }, Affine::IDENTITY)?]
            };
            for c in &created {
                if let Some(o) = doc.object(c) {
                    validate_path(id, &o.geometry.local_path())?;
                }
            }
            if i.svg.contains("<text") {
                warnings.push("text elements were ignored; convert text to paths before importing".into());
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["warnings"] = json!(warnings);
            Ok((r, format!("imported SVG as {} ({} object(s))", base, created.len())))
        })
    }
}

/// An imported path in field mm and the SVG element id it came from, if any.
type LabelledPath = (BezPath, Option<String>);

/// Parse `svg` and return its paths in field mm (y-up, combined bounds centred on the origin), each with its SVG id if
/// any, plus warnings.
fn svg_to_field_paths(svg: &str, width_mm: Option<f64>) -> Result<(Vec<LabelledPath>, Vec<String>)> {
    let id = ImportSvg::ID;
    if svg.len() > MAX_SVG_BYTES {
        return Err(invalid(id, format!("svg is larger than {} MB", MAX_SVG_BYTES / (1024 * 1024))));
    }
    let mut opt = usvg::Options { dpi: DPI as f32, ..usvg::Options::default() };
    // Never touch the file system or decode embedded images: only geometry is imported.
    opt.image_href_resolver =
        usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) };
    let tree = usvg::Tree::from_str(svg, &opt).map_err(|e| invalid(id, format!("svg could not be parsed: {e}")))?;

    let mut px_paths = Vec::new();
    collect(tree.root(), &mut px_paths);
    if px_paths.is_empty() {
        return Err(CommandError::Failed("the SVG contains no visible paths or shapes".into()));
    }

    // px (y-down) → mm (y-up).
    let to_mm = Affine::scale_non_uniform(MM_PER_PX, -MM_PER_PX);
    let mut paths: Vec<LabelledPath> = px_paths.into_iter().map(|(p, l)| (to_mm * p, l)).collect();
    let bounds = paths.iter().map(|(p, _)| p.bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);

    let scale = match width_mm {
        Some(w) if bounds.width() <= 1e-9 => {
            return Err(CommandError::Failed(format!("cannot scale to width {w} mm: the art has zero width")));
        }
        Some(w) => w / bounds.width(),
        None => 1.0,
    };
    let fit = Affine::scale(scale) * Affine::translate(-bounds.center().to_vec2());
    for (p, _) in &mut paths {
        p.apply_affine(fit);
    }
    let mut warnings = Vec::new();
    let size = bounds.size() * scale;
    if size.width > 300.0 || size.height > 300.0 {
        warnings.push(format!(
            "imported art is {:.1} x {:.1} mm, larger than a typical galvo field; check the SVG units or pass width_mm",
            size.width, size.height
        ));
    }
    Ok((paths, warnings))
}

/// Depth-first walk collecting every visible path's absolute geometry in px. Clip paths, masks and patterns are
/// not drawn geometry and are skipped.
fn collect(group: &usvg::Group, out: &mut Vec<LabelledPath>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => collect(g, out),
            usvg::Node::Path(p) if p.is_visible() && (p.fill().is_some() || p.stroke().is_some()) => {
                let path = to_bez(p.data(), p.abs_transform());
                if path.elements().len() > 1 {
                    out.push((path, (!p.id().is_empty()).then(|| p.id().to_string())));
                }
            }
            _ => {}
        }
    }
}

fn to_bez(data: &usvg::tiny_skia_path::Path, ts: usvg::Transform) -> BezPath {
    let t = Affine::new([ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty].map(f64::from));
    let pt = |p: usvg::tiny_skia_path::Point| t * kurbo::Point::new(f64::from(p.x), f64::from(p.y));
    let mut b = BezPath::new();
    for seg in data.segments() {
        match seg {
            PathSegment::MoveTo(p) => b.move_to(pt(p)),
            PathSegment::LineTo(p) => b.line_to(pt(p)),
            PathSegment::QuadTo(a, p) => b.quad_to(pt(a), pt(p)),
            PathSegment::CubicTo(a, c, p) => b.curve_to(pt(a), pt(c), pt(p)),
            PathSegment::Close => b.close_path(),
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(d: &mut Document, v: Value) -> Result<(Value, String)> {
        ImportSvg::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    fn close(a: &Value, b: [f64; 2], tol: f64) -> bool {
        (a[0].as_f64().unwrap() - b[0]).abs() < tol && (a[1].as_f64().unwrap() - b[1]).abs() < tol
    }

    const MM_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40mm" height="20mm" viewBox="0 0 400 200">
        <rect x="0" y="0" width="200" height="100" fill="black"/>
        <g transform="translate(300 100)"><circle cx="0" cy="0" r="50" fill="none" stroke="red"/></g>
    </svg>"#;

    #[test]
    fn mm_units_with_viewbox() {
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"svg": MM_SVG})).unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 1);
        // viewBox 400 units = 40 mm, so 0.1 mm per unit. Art spans x 0..350, y 0..150 units = 35 x 15 mm.
        assert!(close(&r["bounds"]["size"], [35.0, 15.0], 1e-3), "{}", r["bounds"]);
        assert!(close(&r["bounds"]["center"], [0.0, 0.0], 1e-3));
        // The rect is at the SVG top-left, so after the y flip it is at the field's upper left.
        let rect = d.objects[0].geometry.local_path();
        let first = rect.elements()[0].end_point().unwrap();
        assert!((first.x + 17.5).abs() < 1e-3 && (first.y - 7.5).abs() < 1e-3, "{first:?}");
    }

    #[test]
    fn px_units_and_split() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="192" height="96">
            <rect id="a" x="0" y="0" width="96" height="48"/>
            <path d="M 144 48 L 192 96" stroke="black"/>
        </svg>"#;
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"svg": svg, "split": true, "name": "logo"})).unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        // 96 px = 1 in = 25.4 mm. Art spans 192 x 96 px.
        assert!(close(&r["bounds"]["size"], [50.8, 25.4], 1e-3), "{}", r["bounds"]);
        assert_eq!(d.objects[0].name, "logo a");
        assert_eq!(d.objects[1].name, "logo 2");
        let b0 = d.objects[0].bounds();
        assert!((b0.width() - 25.4).abs() < 1e-3 && (b0.y1 - 12.7).abs() < 1e-3, "{b0:?}");
    }

    #[test]
    fn width_mm_scales_uniformly() {
        let mut d = Document::default();
        let (r, _) = import(&mut d, json!({"svg": MM_SVG, "width_mm": 70})).unwrap();
        assert!(close(&r["bounds"]["size"], [70.0, 30.0], 1e-3), "{}", r["bounds"]);
    }

    #[test]
    fn rejects_garbage_and_empty() {
        let mut d = Document::default();
        assert!(import(&mut d, json!({"svg": "not xml"})).is_err());
        assert!(
            import(&mut d, json!({"svg": r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#}))
                .is_err()
        );
        assert!(d.objects.is_empty());
    }
}
