//! PDF export: print-ready PDF (LightBurn's File > Print, black only or keeping colours) and Adobe Illustrator export.
//! An Illustrator 9-and-later `.ai` file is a PDF file (Illustrator's "PDF compatible" format), so the AI export is
//! this PDF; Illustrator, Inkscape, CorelDRAW and LightBurn all open it as AI.
//!
//! One page. Its size is the exported area (the selection's bounds or the workspace) at 1:1, or `paper_mm` with the
//! area centred and scaled down to fit inside the margins when it is larger (as LightBurn's print does). Layers draw in
//! layer order, stroked in their colour (or black) and filled (even-odd) when the layer fills; text as its outline;
//! images embedded (8-bit RGB with an alpha soft mask, Flate-compressed).

use std::fmt::Write as _;
use std::io::Cursor;

use base64::Engine;
use doc::{Document, FillMode, Geometry, Object, ObjectId};
use kurbo::{Affine, PathEl};

use crate::svg_out::{Area, ColorMode, Exported, area_of};
use crate::{FileError, Result, export_objects, num, parse_color};

const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Debug, Clone, PartialEq)]
pub struct PdfOptions {
    pub ids: Option<Vec<ObjectId>>,
    pub colors: ColorMode,
    /// Line width in mm (LightBurn 2.1's Print Settings > Line Width).
    pub stroke_mm: f64,
    pub images: bool,
    /// Leave out hidden layers and layers with output off.
    pub skip_hidden: bool,
    pub area: Area,
    /// Paper `[width, height]` in mm; `None`: the page is the area.
    pub paper_mm: Option<[f64; 2]>,
    /// Margin on the paper, mm.
    pub margin_mm: f64,
    /// Document title.
    pub title: String,
}

impl Default for PdfOptions {
    fn default() -> Self {
        PdfOptions {
            ids: None,
            colors: ColorMode::Layers,
            stroke_mm: 0.1,
            images: true,
            skip_hidden: false,
            area: Area::Bounds,
            paper_mm: None,
            margin_mm: 10.0,
            title: String::new(),
        }
    }
}

/// A PDF string literal.
fn pdf_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            _ => out.push('?'),
        }
    }
    out.push(')');
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// An image decoded to RGB and alpha planes.
fn decode_rgba(data_base64: &str) -> std::result::Result<(u32, u32, Vec<u8>, Vec<u8>), String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64.as_bytes()).map_err(|e| e.to_string())?;
    let img = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?
        .into_rgba8();
    let (w, h) = img.dimensions();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    for p in img.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
        alpha.push(p.0[3]);
    }
    Ok((w, h, rgb, alpha))
}

/// Write the document (or `o.ids`) as one PDF page.
pub fn export_pdf(doc: &Document, o: &PdfOptions) -> Result<Exported<Vec<u8>>> {
    if !(o.stroke_mm.is_finite() && o.stroke_mm >= 0.0 && o.margin_mm.is_finite() && o.margin_mm >= 0.0) {
        return Err(FileError::Invalid("the line width and margin must be 0 or more".into()));
    }
    let objects: Vec<&Object> = export_objects(doc, o.ids.as_deref())?;
    let mut warnings = Vec::new();
    let area = area_of(doc, &objects, o.area);
    let (aw, ah) = (area.width().max(0.001), area.height().max(0.001));
    // Page size and the mm → page-mm placement (scale, offset).
    let (pw, ph, scale, ox, oy) = match o.paper_mm {
        Some([w, h]) => {
            if !(w.is_finite() && h.is_finite() && w > 2.0 * o.margin_mm && h > 2.0 * o.margin_mm) {
                return Err(FileError::Invalid("the paper must be larger than twice the margin".into()));
            }
            let fit = ((w - 2.0 * o.margin_mm) / aw).min((h - 2.0 * o.margin_mm) / ah).min(1.0);
            if fit < 1.0 {
                warnings.push(format!("scaled to {:.1} % to fit the paper", fit * 100.0));
            }
            (w, h, fit, (w - aw * fit) / 2.0, (h - ah * fit) / 2.0)
        }
        None => (aw, ah, 1.0, 0.0, 0.0),
    };
    let mut content = String::new();
    // Field mm → page points.
    let k = PT_PER_MM * scale;
    let _ = writeln!(
        content,
        "{} 0 0 {} {} {} cm",
        num(k),
        num(k),
        num((ox - area.x0 * scale) * PT_PER_MM),
        num((oy - area.y0 * scale) * PT_PER_MM)
    );
    let _ = writeln!(content, "{} w 1 J 1 j", num(o.stroke_mm / scale.max(1e-9)));
    let mut images: Vec<(u32, u32, Vec<u8>, Vec<u8>)> = Vec::new();
    for layer in &doc.layers {
        let shown = layer.visible && layer.output;
        if !shown && o.skip_hidden {
            continue;
        }
        let on_layer: Vec<&&Object> = objects.iter().filter(|ob| ob.layer == layer.id).collect();
        if on_layer.is_empty() {
            continue;
        }
        let [r, g, b] = match o.colors {
            ColorMode::Layers => parse_color(&layer.color).map(|c| f64::from(c) / 255.0),
            ColorMode::Black => [0.0; 3],
        };
        let _ = writeln!(content, "{} {} {} RG {} {} {} rg", num(r), num(g), num(b), num(r), num(g), num(b));
        let paint = match layer.settings.mode {
            FillMode::Line => "S",
            FillMode::Fill => "f*",
            FillMode::FillThenLine => "B*",
            FillMode::OffsetFill => "f*",
        };
        for ob in on_layer {
            if let Geometry::Image { width_mm, height_mm, data_base64, .. } = &ob.geometry {
                if !o.images {
                    continue;
                }
                match decode_rgba(data_base64) {
                    Ok(img) => {
                        images.push(img);
                        let m = (ob.transform * Affine::new([*width_mm, 0.0, 0.0, *height_mm, -width_mm / 2.0, -height_mm / 2.0])).as_coeffs();
                        let _ = writeln!(
                            content,
                            "q {} {} {} {} {} {} cm /Im{} Do Q",
                            num(m[0]),
                            num(m[1]),
                            num(m[2]),
                            num(m[3]),
                            num(m[4]),
                            num(m[5]),
                            images.len()
                        );
                    }
                    Err(e) => warnings.push(format!("image {} left out: {e}", ob.id)),
                }
                continue;
            }
            let path = crate::clean_field_path(ob);
            let mut last = kurbo::Point::ZERO;
            let mut any = false;
            for el in path.elements() {
                match *el {
                    PathEl::MoveTo(p) => {
                        let _ = writeln!(content, "{} {} m", num(p.x), num(p.y));
                        last = p;
                    }
                    PathEl::LineTo(p) => {
                        let _ = writeln!(content, "{} {} l", num(p.x), num(p.y));
                        last = p;
                        any = true;
                    }
                    PathEl::QuadTo(a, p) => {
                        let c1 = last + (a - last) * (2.0 / 3.0);
                        let c2 = p + (a - p) * (2.0 / 3.0);
                        let _ = writeln!(content, "{} {} {} {} {} {} c", num(c1.x), num(c1.y), num(c2.x), num(c2.y), num(p.x), num(p.y));
                        last = p;
                        any = true;
                    }
                    PathEl::CurveTo(a, b, p) => {
                        let _ = writeln!(content, "{} {} {} {} {} {} c", num(a.x), num(a.y), num(b.x), num(b.y), num(p.x), num(p.y));
                        last = p;
                        any = true;
                    }
                    PathEl::ClosePath => content.push_str("h\n"),
                }
            }
            if any {
                let _ = writeln!(content, "{paint}");
            } else {
                content.push_str("n\n");
            }
        }
    }
    // Objects: 1 catalog, 2 pages, 3 page, 4 content, 5 info, then two per image (RGB, alpha).
    let mut objs: Vec<Vec<u8>> = Vec::new();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    let xobjects: String = (1..=images.len()).map(|i| format!("/Im{i} {} 0 R ", 6 + 2 * (i - 1))).collect();
    objs.push(
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /XObject << {xobjects}>> >> /Contents 4 0 R >>",
            num(pw * PT_PER_MM),
            num(ph * PT_PER_MM)
        )
        .into_bytes(),
    );
    let packed = zlib(content.as_bytes());
    let mut c = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", packed.len()).into_bytes();
    c.extend_from_slice(&packed);
    c.extend_from_slice(b"\nendstream");
    objs.push(c);
    let title = if o.title.is_empty() { doc.name.clone() } else { o.title.clone() };
    objs.push(format!("<< /Title {} /Creator (OpenEtch) /Producer (OpenEtch) >>", pdf_string(&title)).into_bytes());
    for (i, (w, h, rgb, alpha)) in images.iter().enumerate() {
        let smask = 7 + 2 * i;
        let data = zlib(rgb);
        let mut im = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {smask} 0 R /Length {} /Filter /FlateDecode >>\nstream\n",
            data.len()
        )
        .into_bytes();
        im.extend_from_slice(&data);
        im.extend_from_slice(b"\nendstream");
        objs.push(im);
        let data = zlib(alpha);
        let mut m = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} /Filter /FlateDecode >>\nstream\n",
            data.len()
        )
        .into_bytes();
        m.extend_from_slice(&data);
        m.extend_from_slice(b"\nendstream");
        objs.push(m);
    }
    let mut out = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for off in offsets {
        let _ = writeln!(table, "{off:010} 00000 n ");
    }
    let _ = write!(table, "trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
    out.extend_from_slice(table.as_bytes());
    if objects.iter().any(|ob| matches!(ob.geometry, Geometry::Text(_))) {
        warnings.push("text is written as its outline (paths)".into());
    }
    Ok(Exported { content: out, warnings, bounds: area })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sample;

    fn content_of(pdf: &[u8]) -> String {
        let s = pdf.windows(7).position(|w| w == b"stream\n").unwrap() + 7;
        let len_at = pdf.windows(8).position(|w| w == b"/Length ").unwrap() + 8;
        let len: usize = String::from_utf8_lossy(&pdf[len_at..len_at + 12]).split_whitespace().next().unwrap().parse().unwrap();
        String::from_utf8(miniz_oxide::inflate::decompress_to_vec_zlib(&pdf[s..s + len]).unwrap()).unwrap()
    }

    #[test]
    fn page_is_the_area_and_paths_are_painted_by_layer_mode() {
        let d = sample();
        let out = export_pdf(&d, &PdfOptions::default()).unwrap();
        let pdf = &out.content;
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8_lossy(pdf);
        // 45 x 40 mm.
        assert!(text.contains("/MediaBox [0 0 127.5591 113.3858]"), "{text}");
        let c = content_of(pdf);
        assert!(c.contains("1 0 0 RG"), "{c}");
        assert!(c.contains("f*") && c.contains("\nS\n"), "{c}");
        // xref offsets point at objects.
        let xref_at: usize = text.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(pdf[xref_at..].starts_with(b"xref"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let parsed = lopdf::Document::load_mem(pdf).unwrap();
            assert_eq!(parsed.get_pages().len(), 1);
        }
    }

    #[test]
    fn paper_fits_and_black_only() {
        let d = sample();
        let out = export_pdf(&d, &PdfOptions { paper_mm: Some([40.0, 40.0]), margin_mm: 5.0, colors: ColorMode::Black, ..PdfOptions::default() }).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("scaled to 66.7 %")), "{:?}", out.warnings);
        let c = content_of(&out.content);
        assert!(!c.contains("1 0 0 RG") && c.contains("0 0 0 RG"));
    }

    #[test]
    fn images_are_embedded() {
        let mut d = sample();
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        d.objects.push(doc::Object::new(
            "obj-4".into(),
            "img",
            "layer-1".into(),
            Geometry::Image { width_mm: 4.0, height_mm: 2.0, mime: "image/png".into(), data_base64: b64.into(), adjust: Default::default() },
            Affine::IDENTITY,
        ));
        let out = export_pdf(&d, &PdfOptions::default()).unwrap();
        let text = String::from_utf8_lossy(&out.content);
        assert!(text.contains("/Subtype /Image /Width 2 /Height 2") && text.contains("/SMask"));
        assert!(content_of(&out.content).contains("4 0 0 2 -2 -1 cm /Im1 Do"));
    }
}
