//! The job ledger's thumbnails (module `ledger`): a small PNG of the burned document, framed on what was marked.
//!
//! Drawn straight with tiny-skia (resvg's renderer, no fonts or SVG needed): the stock in tan, the images of output
//! layers as their pictures (decoded and shrunk first), every other object of an output layer as its outline in the
//! layer's colour, on white. The view fits the output objects (and the stock) with a small margin, so a coin in a
//! corner of the field still fills the thumbnail.

use base64::Engine as _;
use doc::{Document, Geometry};
use kurbo::{Affine, BezPath, PathEl, Rect, Shape as _};
use resvg::tiny_skia::{self, Color, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use crate::EngineError;

/// `#rrggbb` (or `#rgb`) as RGB; grey for anything else.
fn rgb(c: &str) -> (u8, u8, u8) {
    let h = c.trim().trim_start_matches('#');
    let hex = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        6 => match (hex(&h[0..2]), hex(&h[2..4]), hex(&h[4..6])) {
            (Some(r), Some(g), Some(b)) => (r, g, b),
            _ => (128, 128, 128),
        },
        3 => match (hex(&h[0..1]), hex(&h[1..2]), hex(&h[2..3])) {
            (Some(r), Some(g), Some(b)) => (r * 17, g * 17, b * 17),
            _ => (128, 128, 128),
        },
        _ => (128, 128, 128),
    }
}

fn skia_path(p: &BezPath, view: Affine) -> Option<tiny_skia::Path> {
    let mut b = PathBuilder::new();
    let f = |pt: kurbo::Point| {
        let q = view * pt;
        (q.x as f32, q.y as f32)
    };
    for el in p.elements() {
        match *el {
            PathEl::MoveTo(a) => {
                let (x, y) = f(a);
                b.move_to(x, y);
            }
            PathEl::LineTo(a) => {
                let (x, y) = f(a);
                b.line_to(x, y);
            }
            PathEl::QuadTo(a, c) => {
                let ((x1, y1), (x, y)) = (f(a), f(c));
                b.quad_to(x1, y1, x, y);
            }
            PathEl::CurveTo(a, c, d) => {
                let ((x1, y1), (x2, y2), (x, y)) = (f(a), f(c), f(d));
                b.cubic_to(x1, y1, x2, y2, x, y);
            }
            PathEl::ClosePath => b.close(),
        }
    }
    b.finish()
}

/// The decoded image of an image object, shrunk to about `max_px` on its longer side, as premultiplied RGBA.
fn image_pixmap(data_base64: &str, max_px: u32) -> Option<Pixmap> {
    if data_base64.len() > crate::ledger::MAX_THUMB_IMAGE_B64 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64.as_bytes()).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let (w, h) = (img.width().max(1), img.height().max(1));
    let k = (f64::from(max_px.max(2)) / f64::from(w.max(h))).min(1.0);
    let (tw, th) = (((f64::from(w) * k).round() as u32).max(1), ((f64::from(h) * k).round() as u32).max(1));
    let rgba = image::imageops::thumbnail(&img.to_rgba8(), tw, th);
    let mut data = rgba.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(tw, th)?)
}

/// A PNG thumbnail of `doc`, `px` on its longer side (see the module docs).
pub fn thumbnail_png(doc: &Document, px: u32) -> Result<Vec<u8>, EngineError> {
    let px = px.clamp(16, 1024);
    let emits = |layer: &str| doc.layer(layer).is_some_and(|l| l.emits() && l.visible);
    let objects: Vec<&doc::Object> = doc.objects.iter().filter(|o| emits(&o.layer)).collect();
    let mut bounds: Option<Rect> = None;
    let mut grow = |r: Rect| bounds = Some(bounds.map_or(r, |b| b.union(r)));
    for o in &objects {
        let r = o.field_path().bounding_box();
        if r.width().is_finite() && r.height().is_finite() {
            grow(r);
        }
    }
    let stock = doc.stock.as_ref().map(|s| s.outline());
    if let Some(s) = &stock {
        grow(s.bounding_box());
    }
    let [fw, fh] = doc.workspace().unwrap_or([150.0, 150.0]).map(|v| v.max(1.0) / 2.0);
    let b = bounds.filter(|b| b.width() > 0.0 || b.height() > 0.0).unwrap_or(Rect::new(-fw, -fh, fw, fh));
    let margin = b.width().max(b.height()).max(1.0) * 0.06;
    let b = b.inflate(margin, margin);
    let scale = f64::from(px) / b.width().max(b.height());
    let (w, h) = (((b.width() * scale).ceil() as u32).clamp(1, px), ((b.height() * scale).ceil() as u32).clamp(1, px));
    // Field mm (+Y up) to pixels (+Y down), centred.
    let ox = (f64::from(w) - b.width() * scale) / 2.0;
    let oy = (f64::from(h) - b.height() * scale) / 2.0;
    let view = Affine::new([scale, 0.0, 0.0, -scale, ox - b.x0 * scale, oy + b.y1 * scale]);
    let mut pm = Pixmap::new(w, h).ok_or_else(|| EngineError::Internal("could not allocate the thumbnail".into()))?;
    pm.fill(Color::WHITE);
    let line = (f64::from(px) / 200.0).max(0.6) as f32;
    let stroke = Stroke { width: line, ..Stroke::default() };
    if let Some(path) = stock.as_ref().and_then(|s| skia_path(s, view)) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0xe6, 0xdc, 0xc8, 0xff);
        paint.anti_alias = true;
        pm.fill_path(&path, &paint, tiny_skia::FillRule::EvenOdd, Transform::identity(), None);
        paint.set_color_rgba8(0x8a, 0x73, 0x50, 0xff);
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    for o in &objects {
        let (r, g, bl) = doc.layer(&o.layer).map_or((128, 128, 128), |l| rgb(&l.color));
        if let Geometry::Image { width_mm, height_mm, data_base64, .. } = &o.geometry {
            // About two image pixels per thumbnail pixel of the object's size on screen.
            let on_screen = (width_mm.max(*height_mm) * scale * o.transform.determinant().abs().sqrt() * 2.0).ceil();
            if let Some(img) = image_pixmap(data_base64, (on_screen as u32).clamp(8, 1024)) {
                let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
                // Image pixels (+Y down, from the top-left corner) to the object's own frame, centred.
                let local = Affine::new([width_mm / iw, 0.0, 0.0, -height_mm / ih, -width_mm / 2.0, height_mm / 2.0]);
                let [a, bb, c, d, e, f] = (view * o.transform * local).as_coeffs();
                let t = Transform::from_row(a as f32, bb as f32, c as f32, d as f32, e as f32, f as f32);
                let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
                pm.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
                continue;
            }
        }
        if let Some(path) = skia_path(&o.field_path(), view) {
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, bl, 0xff);
            paint.anti_alias = true;
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    pm.encode_png().map_err(|e| EngineError::Internal(format!("thumbnail PNG: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::Object;

    #[test]
    fn thumbnails_fit_the_marked_objects_and_draw_images() {
        let mut d = Document::default();
        d.layers[0].color = "#0000ff".into();
        let g = Geometry::Rect { width_mm: 10.0, height_mm: 5.0, corner_radius_mm: 0.0 };
        d.objects.push(Object::new("obj-2".into(), "r", "layer-1".into(), g, Affine::translate((40.0, 40.0))));
        // A 2x2 black PNG as an image object.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let data: std::sync::Arc<str> = base64::engine::general_purpose::STANDARD.encode(&png).into();
        let img = Geometry::Image { width_mm: 6.0, height_mm: 6.0, mime: "image/png".into(), data_base64: data, adjust: Default::default() };
        d.objects.push(Object::new("obj-3".into(), "i", "layer-1".into(), img, Affine::translate((30.0, 40.0))));
        let bytes = thumbnail_png(&d, 120).unwrap();
        let t = image::load_from_memory(&bytes).unwrap().to_rgba8();
        // Framed on the objects (16 x 6 mm plus margins): wide, 120 px across.
        assert_eq!(t.width(), 120);
        assert!(t.height() < 80, "{}", t.height());
        let dark = t.pixels().filter(|p| p[0] < 60 && p[1] < 60 && p[2] < 60).count();
        let blue = t.pixels().filter(|p| p[2] > 150 && p[0] < 120).count();
        assert!(dark > 100, "the image is drawn ({dark} dark pixels)");
        assert!(blue > 20, "the rectangle's outline is drawn in its layer colour ({blue})");
        // An empty document still gives a (white) thumbnail of the field.
        let empty = thumbnail_png(&Document::default(), 64).unwrap();
        assert_eq!(&empty[1..4], b"PNG");
    }
}
