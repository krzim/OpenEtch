//! SVG export (LightBurn's File > Export as SVG) and print-ready SVG (File > Print, black only or keeping colours).
//!
//! Millimetre document (`width`/`height` in mm, `viewBox` in mm), +Y down as SVG wants: field y is negated. One
//! `<g>` per layer (an Inkscape layer: `inkscape:groupmode="layer"` and the layer name as label) in layer order,
//! stroked in the layer colour (or black); fill layers are also filled (even-odd, as jobs fill them). Layers with
//! output off are written but hidden (`display="none"`), as LightBurn does; print leaves them and invisible layers out.
//! Text is written as its outline (paths), so no fonts are needed. Images are embedded (`data:` URLs) with their
//! transform; images and the art keep their place relative to each other.

use std::fmt::Write as _;

use doc::{Document, FillMode, Geometry, Object, ObjectId};
use kurbo::{Affine, BezPath, PathEl, Rect, Shape};

use crate::{FileError, Result, export_objects, num, xml_escape};

/// Colours of the strokes and fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Each layer's colour.
    #[default]
    Layers,
    /// Everything black (LightBurn's "Print (black only)").
    Black,
}

/// The area a document covers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Area {
    /// The bounds of what is written.
    #[default]
    Bounds,
    /// The device workspace (galvo field or gantry bed), centred on the field origin; the bounds when unknown.
    Workspace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SvgOptions {
    /// Objects or groups to write (empty or `None`: all).
    pub ids: Option<Vec<ObjectId>>,
    pub colors: ColorMode,
    /// Stroke width in mm.
    pub stroke_mm: f64,
    /// Embed images.
    pub images: bool,
    /// Write layers that are hidden or have output off (hidden in the file); print leaves them out.
    pub hidden_layers: bool,
    pub area: Area,
}

impl Default for SvgOptions {
    fn default() -> Self {
        SvgOptions { ids: None, colors: ColorMode::Layers, stroke_mm: 0.1, images: true, hidden_layers: true, area: Area::Bounds }
    }
}

/// An exported file and what it left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Exported<T> {
    pub content: T,
    pub warnings: Vec<String>,
    /// The area written, field frame mm.
    pub bounds: Rect,
}

/// The area of `objects` (or the workspace).
pub(crate) fn area_of(doc: &Document, objects: &[&Object], area: Area) -> Rect {
    let bounds = objects.iter().map(|o| o.bounds()).reduce(|a, b| a.union(b));
    match (area, doc.workspace()) {
        (Area::Workspace, Some([w, h])) => Rect::from_center_size((0.0, 0.0), (w, h)),
        _ => bounds.unwrap_or(Rect::ZERO),
    }
}

/// SVG path data of a field-frame path, y negated.
pub fn path_data(p: &BezPath) -> String {
    let mut d = String::new();
    let pt = |d: &mut String, p: kurbo::Point| {
        let _ = write!(d, "{} {}", num(p.x), num(-p.y));
    };
    for el in p.elements() {
        if !d.is_empty() {
            d.push(' ');
        }
        match *el {
            PathEl::MoveTo(p) => {
                d.push('M');
                pt(&mut d, p);
            }
            PathEl::LineTo(p) => {
                d.push('L');
                pt(&mut d, p);
            }
            PathEl::QuadTo(a, p) => {
                d.push('Q');
                pt(&mut d, a);
                d.push(' ');
                pt(&mut d, p);
            }
            PathEl::CurveTo(a, b, p) => {
                d.push('C');
                pt(&mut d, a);
                d.push(' ');
                pt(&mut d, b);
                d.push(' ');
                pt(&mut d, p);
            }
            PathEl::ClosePath => d.push('Z'),
        }
    }
    d
}

/// The `matrix(...)` that places an image element (its own frame: x right, y down, centred) in SVG coordinates.
fn image_matrix(t: Affine) -> String {
    let flip = Affine::FLIP_Y;
    let m = (flip * t * flip).as_coeffs();
    format!("matrix({} {} {} {} {} {})", num(m[0]), num(m[1]), num(m[2]), num(m[3]), num(m[4]), num(m[5]))
}

/// Write the document (or `o.ids`) as SVG text.
pub fn export_svg(doc: &Document, o: &SvgOptions) -> Result<Exported<String>> {
    let objects = export_objects(doc, o.ids.as_deref())?;
    if !(o.stroke_mm.is_finite() && o.stroke_mm >= 0.0) {
        return Err(FileError::Invalid("the stroke width must be 0 or more".into()));
    }
    let mut warnings = Vec::new();
    let r = area_of(doc, &objects, o.area);
    let (w, h) = (r.width().max(0.001), r.height().max(0.001));
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>"#);
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" version="1.1" width="{}mm" height="{}mm" viewBox="{} {} {} {}">"#,
        num(w),
        num(h),
        num(r.x0),
        num(-r.y1),
        num(w),
        num(h)
    );
    let _ = writeln!(s, "  <title>{}</title>", xml_escape(&doc.name));
    let mut skipped_images = 0;
    for layer in &doc.layers {
        let on_layer: Vec<&&Object> = objects.iter().filter(|ob| ob.layer == layer.id).collect();
        if on_layer.is_empty() {
            continue;
        }
        let shown = layer.visible && layer.output;
        if !shown && !o.hidden_layers {
            continue;
        }
        let color = match o.colors {
            ColorMode::Layers => xml_escape(&layer.color),
            ColorMode::Black => "#000000".into(),
        };
        let filled = matches!(layer.settings.mode, FillMode::Fill | FillMode::FillThenLine | FillMode::OffsetFill);
        let stroked = matches!(layer.settings.mode, FillMode::Line | FillMode::FillThenLine);
        let fill = if filled { format!(r#"fill="{color}" fill-rule="evenodd""#) } else { r#"fill="none""#.into() };
        let stroke = if stroked || !filled {
            format!(r#"stroke="{color}" stroke-width="{}""#, num(o.stroke_mm))
        } else {
            r#"stroke="none""#.into()
        };
        let _ = writeln!(
            s,
            r#"  <g id="{}" inkscape:groupmode="layer" inkscape:label="{}" {fill} {stroke} stroke-linejoin="round" stroke-linecap="round"{}>"#,
            xml_escape(&layer.id),
            xml_escape(&layer.name),
            if shown { "" } else { r#" display="none""# }
        );
        for ob in on_layer {
            match &ob.geometry {
                Geometry::Image { width_mm, height_mm, mime, data_base64, .. } => {
                    if !o.images {
                        skipped_images += 1;
                        continue;
                    }
                    let _ = writeln!(
                        s,
                        r#"    <image id="{}" x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" transform="{}" xlink:href="data:{};base64,{}"/>"#,
                        xml_escape(&ob.id),
                        num(-width_mm / 2.0),
                        num(-height_mm / 2.0),
                        num(*width_mm),
                        num(*height_mm),
                        image_matrix(ob.transform),
                        xml_escape(mime),
                        data_base64
                    );
                }
                _ => {
                    let d = path_data(&crate::clean_field_path(ob));
                    if d.is_empty() {
                        continue;
                    }
                    let _ = writeln!(s, r#"    <path id="{}" d="{d}"><title>{}</title></path>"#, xml_escape(&ob.id), xml_escape(&ob.name));
                }
            }
        }
        s.push_str("  </g>\n");
    }
    s.push_str("</svg>\n");
    if skipped_images > 0 {
        warnings.push(format!("{skipped_images} image(s) left out"));
    }
    if objects.iter().any(|ob| matches!(ob.geometry, Geometry::Text(_))) {
        warnings.push("text is written as its outline (paths)".into());
    }
    Ok(Exported { content: s, warnings, bounds: r })
}

/// The bounding box of an SVG export of `doc`, for callers that need the page size first.
pub fn bounds(doc: &Document, ids: Option<&[ObjectId]>) -> Result<Rect> {
    let objects = export_objects(doc, ids)?;
    Ok(objects.iter().map(|o| o.field_path().bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sample;

    #[test]
    fn layers_become_groups_with_their_colours_and_y_flips() {
        let d = sample();
        let out = export_svg(&d, &SvgOptions::default()).unwrap();
        let svg = &out.content;
        assert!(svg.contains(r#"inkscape:label="Layer 1""#) && svg.contains(r##"stroke="#ff0000""##), "{svg}");
        assert!(svg.contains(r##"fill="#0000ff" fill-rule="evenodd""##), "fill layer filled");
        // The rectangle spans x 0..20, y 0..10 in the field: SVG y 0..-10.
        assert!(svg.contains("M0 -10") || svg.contains("M20 0") || svg.contains("M0 0"), "{svg}");
        assert_eq!(out.bounds, Rect::new(-25.0, -30.0, 20.0, 10.0));
        assert!(svg.contains(r#"viewBox="-25 -10 45 40""#), "{svg}");
        // Parses as SVG.
        roxmltree::Document::parse(svg).unwrap();
    }

    #[test]
    fn selection_black_and_hidden_layers() {
        let mut d = sample();
        d.layers[1].output = false;
        let out = export_svg(&d, &SvgOptions { colors: ColorMode::Black, ..SvgOptions::default() }).unwrap();
        assert!(!out.content.contains("#ff0000") && out.content.contains(r#"display="none""#));
        let print = export_svg(&d, &SvgOptions { hidden_layers: false, ..SvgOptions::default() }).unwrap();
        assert!(!print.content.contains("obj-2"), "output-off layer left out of print");
        let one = export_svg(&d, &SvgOptions { ids: Some(vec!["obj-3".into()]), ..SvgOptions::default() }).unwrap();
        assert!(one.content.contains("obj-3") && !one.content.contains("obj-1"));
        assert_eq!(one.bounds, Rect::new(0.0, -30.0, 10.0, -20.0));
    }

    #[test]
    fn images_are_embedded_with_their_placement() {
        let mut d = sample();
        d.objects.push(doc::Object::new(
            "obj-4".into(),
            "img",
            "layer-1".into(),
            Geometry::Image { width_mm: 4.0, height_mm: 2.0, mime: "image/png".into(), data_base64: "AAAA".into(), adjust: Default::default() },
            Affine::translate((1.0, 2.0)),
        ));
        let out = export_svg(&d, &SvgOptions::default()).unwrap();
        assert!(out.content.contains(r#"x="-2" y="-1" width="4" height="2""#), "{}", out.content);
        assert!(out.content.contains("matrix(1 0 0 1 1 -2)"), "{}", out.content);
        assert!(out.content.contains("data:image/png;base64,AAAA"));
    }

    #[test]
    fn workspace_area() {
        let d = sample();
        let out = export_svg(&d, &SvgOptions { area: Area::Workspace, ..SvgOptions::default() }).unwrap();
        assert!(out.content.contains(r#"width="100mm" height="100mm" viewBox="-50 -50 100 100""#));
    }
}
