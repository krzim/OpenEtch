//! DXF export (LightBurn's File > Export as DXF): AutoCAD R12 (`AC1009`) ASCII, the version every CAD and laser
//! program reads. Millimetres (`$INSUNITS 4`), field frame (+Y up, origin at the field centre). Each OpenEtch layer is
//! a DXF layer named after it, coloured with the nearest AutoCAD colour index. Every subpath is a `POLYLINE` (closed
//! flag set for closed subpaths); curves are flattened to lines within `tolerance_mm` ("convert arcs to lines"), and
//! with `keep_circles` a circle (an ellipse with equal radii under a uniform scale) is written as a `CIRCLE` instead.
//! Text is written as its outline. Images cannot be written to DXF and are left out (with a warning).

use std::fmt::Write as _;

use doc::{Document, Geometry, Object, ObjectId};
use kurbo::{Affine, BezPath, PathEl, Point, Rect};

use crate::{FileError, Result, export_objects, num, parse_color};

#[derive(Debug, Clone, PartialEq)]
pub struct DxfOptions {
    pub ids: Option<Vec<ObjectId>>,
    /// Flattening tolerance for curves, mm.
    pub tolerance_mm: f64,
    /// Write circles as `CIRCLE` entities.
    pub keep_circles: bool,
}

impl Default for DxfOptions {
    fn default() -> Self {
        DxfOptions { ids: None, tolerance_mm: 0.01, keep_circles: true }
    }
}

/// The AutoCAD Color Index (1..=255) RGB value. 1-9 are the named colours, 10-249 a 24-hue wheel with five shades
/// at full and half saturation, 250-255 greys.
pub fn aci_rgb(i: u8) -> [u8; 3] {
    const BASIC: [[u8; 3]; 9] = [
        [255, 0, 0],
        [255, 255, 0],
        [0, 255, 0],
        [0, 255, 255],
        [0, 0, 255],
        [255, 0, 255],
        [255, 255, 255],
        [128, 128, 128],
        [192, 192, 192],
    ];
    match i {
        0 => [0, 0, 0],
        1..=9 => BASIC[usize::from(i) - 1],
        10..=249 => {
            let k = i - 10;
            let hue = f64::from(k / 10) * 15.0;
            let sub = k % 10;
            let v = [1.0, 1.0, 0.8, 0.8, 0.6, 0.6, 0.5, 0.5, 0.3, 0.3][usize::from(sub)];
            let s = if sub.is_multiple_of(2) { 1.0 } else { 0.5 };
            hsv(hue, s, v)
        }
        _ => {
            let g = [51u8, 80, 105, 130, 190, 255][usize::from(i - 250)];
            [g, g, g]
        }
    }
}

fn hsv(h: f64, s: f64, v: f64) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|u| ((u + m) * 255.0).round() as u8)
}

/// The colour index nearest to `rgb` (white maps to 7, AutoCAD's white/black).
pub fn nearest_aci(rgb: [u8; 3]) -> u8 {
    (1..=255u8)
        .min_by_key(|&i| {
            let c = aci_rgb(i);
            (0..3).map(|k| (i32::from(c[k]) - i32::from(rgb[k])).pow(2)).sum::<i32>()
        })
        .unwrap_or(7)
}

/// A DXF layer name: printable ASCII without the characters DXF forbids, at most 255 characters; never empty.
fn layer_name(name: &str, id: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && !"<>/\\\":;?*|=,`".contains(c) || c == ' ' { c } else { '_' })
        .take(255)
        .collect();
    let n = n.trim().to_string();
    if n.is_empty() { id.to_string() } else { n }
}

fn pair(s: &mut String, code: u16, value: impl std::fmt::Display) {
    let _ = write!(s, "{code:>3}\r\n{value}\r\n");
}

/// A circle under `t`: centre and radius, when `g` is a circle and `t` scales uniformly (rotation and mirroring are
/// fine).
fn as_circle(g: &Geometry, t: Affine) -> Option<(Point, f64)> {
    let Geometry::Ellipse { rx_mm, ry_mm } = *g else { return None };
    if (rx_mm - ry_mm).abs() > 1e-9 {
        return None;
    }
    let [a, b, c, d, e, f] = t.as_coeffs();
    let (sx, sy) = (a.hypot(b), c.hypot(d));
    let ortho = (a * c + b * d).abs() < 1e-9;
    ((sx - sy).abs() < 1e-9 && ortho).then_some((Point::new(e, f), rx_mm * sx))
}

/// Flatten a path into polylines (closed flag), within `tol`.
pub(crate) fn polylines(p: &BezPath, tol: f64) -> Vec<(Vec<Point>, bool)> {
    let mut out: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut closed = false;
    let flush = |cur: &mut Vec<Point>, closed: &mut bool, out: &mut Vec<(Vec<Point>, bool)>| {
        if *closed && cur.len() > 2 && cur.first() == cur.last() {
            cur.pop();
        }
        if cur.len() >= 2 {
            out.push((std::mem::take(cur), *closed));
        }
        cur.clear();
        *closed = false;
    };
    kurbo::flatten(p.iter(), tol.max(1e-4), |el| match el {
        PathEl::MoveTo(q) => {
            flush(&mut cur, &mut closed, &mut out);
            cur.push(q);
        }
        PathEl::LineTo(q) => {
            if cur.last() != Some(&q) {
                cur.push(q);
            }
        }
        PathEl::ClosePath => {
            closed = true;
            flush(&mut cur, &mut closed, &mut out);
        }
        _ => {}
    });
    flush(&mut cur, &mut closed, &mut out);
    out
}

/// Write the document (or `o.ids`) as DXF R12 text.
pub fn export_dxf(doc: &Document, o: &DxfOptions) -> Result<crate::svg_out::Exported<String>> {
    if !(o.tolerance_mm.is_finite() && o.tolerance_mm > 0.0) {
        return Err(FileError::Invalid("the tolerance must be more than 0".into()));
    }
    let objects: Vec<&Object> = export_objects(doc, o.ids.as_deref())?;
    let vectors: Vec<&Object> = objects.iter().copied().filter(|ob| !matches!(ob.geometry, Geometry::Image { .. })).collect();
    let mut warnings = Vec::new();
    let images = objects.len() - vectors.len();
    if images > 0 {
        warnings.push(format!("{images} image(s) left out: DXF holds vectors only"));
    }
    let bounds = vectors.iter().map(|ob| ob.bounds()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    let used: Vec<&doc::Layer> = doc.layers.iter().filter(|l| vectors.iter().any(|ob| ob.layer == l.id)).collect();
    let names: Vec<(String, String)> = {
        let mut seen: Vec<String> = Vec::new();
        used.iter()
            .map(|l| {
                let mut n = layer_name(&l.name, &l.id);
                if seen.iter().any(|s| s.eq_ignore_ascii_case(&n)) {
                    n = format!("{n}_{}", l.id);
                }
                seen.push(n.clone());
                (l.id.clone(), n)
            })
            .collect()
    };
    let name_of = |id: &str| names.iter().find(|(l, _)| l == id).map_or("0", |(_, n)| n.as_str()).to_string();
    let mut s = String::new();
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "HEADER");
    pair(&mut s, 9, "$ACADVER");
    pair(&mut s, 1, "AC1009");
    pair(&mut s, 9, "$INSUNITS");
    pair(&mut s, 70, 4);
    pair(&mut s, 9, "$EXTMIN");
    pair(&mut s, 10, num(bounds.x0));
    pair(&mut s, 20, num(bounds.y0));
    pair(&mut s, 9, "$EXTMAX");
    pair(&mut s, 10, num(bounds.x1));
    pair(&mut s, 20, num(bounds.y1));
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "TABLES");
    pair(&mut s, 0, "TABLE");
    pair(&mut s, 2, "LTYPE");
    pair(&mut s, 70, 1);
    pair(&mut s, 0, "LTYPE");
    pair(&mut s, 2, "CONTINUOUS");
    pair(&mut s, 70, 0);
    pair(&mut s, 3, "Solid line");
    pair(&mut s, 72, 65);
    pair(&mut s, 73, 0);
    pair(&mut s, 40, 0);
    pair(&mut s, 0, "ENDTAB");
    pair(&mut s, 0, "TABLE");
    pair(&mut s, 2, "LAYER");
    pair(&mut s, 70, used.len());
    for l in &used {
        pair(&mut s, 0, "LAYER");
        pair(&mut s, 2, name_of(&l.id));
        pair(&mut s, 70, 0);
        let aci = i32::from(nearest_aci(parse_color(&l.color)));
        // A negative colour turns a layer off; hidden layers are written off.
        pair(&mut s, 62, if l.visible { aci } else { -aci });
        pair(&mut s, 6, "CONTINUOUS");
    }
    pair(&mut s, 0, "ENDTAB");
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "SECTION");
    pair(&mut s, 2, "ENTITIES");
    for ob in &vectors {
        let layer = name_of(&ob.layer);
        if o.keep_circles
            && let Some((c, r)) = as_circle(&ob.geometry, ob.transform)
        {
            pair(&mut s, 0, "CIRCLE");
            pair(&mut s, 8, &layer);
            pair(&mut s, 10, num(c.x));
            pair(&mut s, 20, num(c.y));
            pair(&mut s, 30, 0);
            pair(&mut s, 40, num(r));
            continue;
        }
        for (pts, closed) in polylines(&crate::clean_field_path(ob), o.tolerance_mm) {
            pair(&mut s, 0, "POLYLINE");
            pair(&mut s, 8, &layer);
            pair(&mut s, 66, 1);
            pair(&mut s, 10, 0);
            pair(&mut s, 20, 0);
            pair(&mut s, 30, 0);
            pair(&mut s, 70, u8::from(closed));
            for p in pts {
                pair(&mut s, 0, "VERTEX");
                pair(&mut s, 8, &layer);
                pair(&mut s, 10, num(p.x));
                pair(&mut s, 20, num(p.y));
                pair(&mut s, 30, 0);
            }
            pair(&mut s, 0, "SEQEND");
            pair(&mut s, 8, &layer);
        }
    }
    pair(&mut s, 0, "ENDSEC");
    pair(&mut s, 0, "EOF");
    if objects.iter().any(|ob| matches!(ob.geometry, Geometry::Text(_))) {
        warnings.push("text is written as its outline (paths)".into());
    }
    Ok(crate::svg_out::Exported { content: s, warnings, bounds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sample;

    #[test]
    fn aci_palette() {
        assert_eq!(aci_rgb(1), [255, 0, 0]);
        assert_eq!(aci_rgb(10), [255, 0, 0]);
        assert_eq!(nearest_aci([255, 0, 0]), 1);
        assert_eq!(nearest_aci([0, 0, 255]), 5);
        assert_eq!(nearest_aci([255, 255, 255]), 7);
        assert_eq!(aci_rgb(250), [51, 51, 51]);
    }

    #[test]
    fn writes_r12_that_a_dxf_reader_loads_with_layers_and_circles() {
        let d = sample();
        let out = export_dxf(&d, &DxfOptions::default()).unwrap();
        let drawing = dxf::Drawing::load(&mut std::io::Cursor::new(out.content.as_bytes())).unwrap();
        let layers: Vec<(String, i16)> = drawing.layers().map(|l| (l.name.clone(), l.color.index().unwrap_or(0) as i16)).collect();
        assert_eq!(layers.iter().filter(|(n, _)| n == "Layer 1" || n == "Fill").count(), 2, "{layers:?}");
        let mut circles = 0;
        let mut polys = 0;
        for e in drawing.entities() {
            match &e.specific {
                dxf::entities::EntityType::Circle(c) => {
                    circles += 1;
                    assert_eq!((c.center.x, c.center.y, c.radius), (-20.0, 0.0, 5.0));
                    assert_eq!(e.common.layer, "Fill");
                }
                dxf::entities::EntityType::Polyline(p) => {
                    polys += 1;
                    assert!(p.is_closed());
                    assert_eq!(e.common.layer, "Layer 1");
                }
                _ => {}
            }
        }
        assert_eq!((circles, polys), (1, 2));
    }

    #[test]
    fn circles_flatten_when_asked_and_images_are_reported() {
        let mut d = sample();
        d.objects.push(doc::Object::new(
            "obj-4".into(),
            "img",
            "layer-1".into(),
            Geometry::Image { width_mm: 4.0, height_mm: 2.0, mime: "image/png".into(), data_base64: "AAAA".into(), adjust: Default::default() },
            Affine::IDENTITY,
        ));
        let out = export_dxf(&d, &DxfOptions { keep_circles: false, ids: Some(vec!["obj-2".into(), "obj-4".into()]), ..DxfOptions::default() }).unwrap();
        assert!(!out.content.contains("CIRCLE") && out.content.contains("POLYLINE"));
        assert!(out.warnings[0].contains("1 image"));
    }
}
