//! Convert to Bitmap (LightBurn's Edit > Convert to Bitmap, Ctrl+Shift+B): vector shapes → a grey bitmap.
//!
//! Render types as LightBurn: `Outlines` (only the outlines), `FillAll` (the inside of closed shapes, even-odd per
//! shape), `CutSettings` (outline or fill by each shape's layer: line layers outlined, fill layers filled). Drawn
//! pixels get the `gray` level (LightBurn starts every pixel at 50 % grey, `gray` 128; adjust it afterwards with Adjust
//! Image), the rest is white, which never fires. Edges are anti-aliased (coverage of 4 x 4 samples for fills, distance
//! for outlines). Outlines are `line_width_mm` wide (default one pixel). The bitmap covers the shapes' bounds at `dpi`.

use kurbo::{BezPath, Point, Rect, Shape};

use crate::{FileError, Result, invalid};

/// Most pixels a converted bitmap may have.
pub const MAX_PIXELS: u64 = 64_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderType {
    Outlines,
    #[default]
    FillAll,
    CutSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterOptions {
    pub dpi: f64,
    pub gray: u8,
    pub line_width_mm: Option<f64>,
}

impl Default for RasterOptions {
    fn default() -> Self {
        RasterOptions { dpi: 254.0, gray: 128, line_width_mm: None }
    }
}

/// One shape in the field frame and how to draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterShape {
    pub path: BezPath,
    pub fill: bool,
    pub outline: bool,
}

/// The bitmap and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct Raster {
    pub png: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    /// Field-frame rectangle the bitmap covers.
    pub bounds: Rect,
}

/// Rasterise `shapes`.
pub fn rasterize(shapes: &[RasterShape], o: &RasterOptions) -> Result<Raster> {
    if !(o.dpi.is_finite() && o.dpi > 0.0 && o.dpi <= 10_000.0) {
        return invalid("dpi must be between 0 and 10000");
    }
    let px_mm = 25.4 / o.dpi;
    let lw = match o.line_width_mm {
        Some(w) if w.is_finite() && w > 0.0 => w,
        Some(_) => return invalid("the line width must be a positive number of mm"),
        None => px_mm,
    };
    let bounds = shapes
        .iter()
        .filter(|s| s.fill || s.outline)
        .map(|s| {
            let b = s.path.bounding_box();
            if s.outline { b.inflate(lw / 2.0, lw / 2.0) } else { b }
        })
        .reduce(|a, b| a.union(b))
        .ok_or_else(|| FileError::Invalid("nothing to convert".into()))?;
    let (wf, hf) = ((bounds.width() / px_mm).ceil().max(1.0), (bounds.height() / px_mm).ceil().max(1.0));
    if wf * hf > MAX_PIXELS as f64 {
        return Err(FileError::TooLarge(format!("{wf} x {hf} pixels is too large; lower the dpi")));
    }
    let (w, h) = (wf as usize, hf as usize);
    // Bitmap covers exactly w x h pixels from the top-left of the bounds.
    let area = Rect::new(bounds.x0, bounds.y1 - h as f64 * px_mm, bounds.x0 + w as f64 * px_mm, bounds.y1);
    let to_px = |p: Point| Point::new((p.x - area.x0) / px_mm, (area.y1 - p.y) / px_mm);
    let mut cov = vec![0f32; w * h];
    for s in shapes {
        let polys = crate::dxf_out::polylines(&s.path, px_mm / 8.0);
        if s.fill {
            let rings: Vec<Vec<Point>> = polys.iter().filter(|(p, _)| p.len() >= 3).map(|(p, _)| p.iter().map(|q| to_px(*q)).collect()).collect();
            fill(&rings, w, h, &mut cov);
        }
        if s.outline {
            let hw = lw / px_mm / 2.0;
            for (p, closed) in &polys {
                let pts: Vec<Point> = p.iter().map(|q| to_px(*q)).collect();
                let n = pts.len();
                let segs = if *closed { n } else { n - 1 };
                for i in 0..segs {
                    stroke(pts[i], pts[(i + 1) % n], hw, w, h, &mut cov);
                }
            }
        }
    }
    let g = f32::from(o.gray);
    let pixels: Vec<u8> = cov.iter().map(|c| (255.0 - c.clamp(0.0, 1.0) * (255.0 - g)).round() as u8).collect();
    let img = image::GrayImage::from_raw(w as u32, h as u32, pixels).ok_or_else(|| FileError::Invalid("bad bitmap size".into()))?;
    let png = crate::bitmap::png(&image::DynamicImage::ImageLuma8(img), Some(o.dpi))?;
    Ok(Raster { png, width_px: w as u32, height_px: h as u32, bounds: area })
}

/// Even-odd fill with 4 x 4 samples per pixel, combined into `cov` by maximum.
fn fill(rings: &[Vec<Point>], w: usize, h: usize, cov: &mut [f32]) {
    if rings.is_empty() {
        return;
    }
    let (mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in rings.iter().flatten() {
        y0 = y0.min(p.y);
        y1 = y1.max(p.y);
    }
    let (r0, r1) = (y0.floor().max(0.0) as usize, (y1.ceil().max(0.0) as usize).min(h));
    const S: usize = 4;
    let mut row = vec![0f32; w];
    for r in r0..r1 {
        row.iter_mut().for_each(|v| *v = 0.0);
        for sub in 0..S {
            let y = r as f64 + (sub as f64 + 0.5) / S as f64;
            let mut xs: Vec<f64> = Vec::new();
            for ring in rings {
                let n = ring.len();
                for i in 0..n {
                    let (a, b) = (ring[i], ring[(i + 1) % n]);
                    if (a.y > y) != (b.y > y) {
                        xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                    }
                }
            }
            xs.sort_by(f64::total_cmp);
            for [a, b] in xs.as_chunks::<2>().0 {
                let (xa, xb) = (a.max(0.0), b.min(w as f64));
                if xb <= xa {
                    continue;
                }
                let (ca, cb) = (xa.floor() as usize, (xb.ceil() as usize).min(w));
                for (c, v) in row.iter_mut().enumerate().take(cb).skip(ca) {
                    let overlap = (xb.min(c as f64 + 1.0) - xa.max(c as f64)).max(0.0);
                    *v += (overlap / S as f64) as f32;
                }
            }
        }
        for (c, v) in row.iter().enumerate() {
            let i = r * w + c;
            cov[i] = cov[i].max(*v);
        }
    }
}

/// A segment `hw` pixels either side, anti-aliased by distance.
fn stroke(a: Point, b: Point, hw: f64, w: usize, h: usize, cov: &mut [f32]) {
    let x0 = (a.x.min(b.x) - hw - 1.0).floor().max(0.0) as usize;
    let x1 = ((a.x.max(b.x) + hw + 1.0).ceil().max(0.0) as usize).min(w);
    let y0 = (a.y.min(b.y) - hw - 1.0).floor().max(0.0) as usize;
    let y1 = ((a.y.max(b.y) + hw + 1.0).ceil().max(0.0) as usize).min(h);
    let ab = b - a;
    let l2 = ab.hypot2();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = Point::new(x as f64 + 0.5, y as f64 + 0.5);
            let t = if l2 > 0.0 { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = (p - (a + ab * t)).hypot();
            let c = (hw + 0.5 - d).clamp(0.0, 1.0) as f32;
            let i = y * w + x;
            if c > cov[i] {
                cov[i] = c;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> BezPath {
        Rect::new(x0, y0, x0 + s, y0 + s).to_path(0.01)
    }

    #[test]
    fn fills_a_square_at_the_given_dpi_with_grey() {
        let o = RasterOptions { dpi: 25.4, gray: 0, line_width_mm: None };
        let r = rasterize(&[RasterShape { path: square(0.0, 0.0, 10.0), fill: true, outline: false }], &o).unwrap();
        assert_eq!((r.width_px, r.height_px), (10, 10));
        assert_eq!(r.bounds, Rect::new(0.0, 0.0, 10.0, 10.0));
        let img = image::load_from_memory(&r.png).unwrap().into_luma8();
        assert!(img.pixels().all(|p| p.0 == [0]));
        // 50 % grey by default.
        let r = rasterize(&[RasterShape { path: square(0.0, 0.0, 10.0), fill: true, outline: false }], &RasterOptions { dpi: 25.4, ..RasterOptions::default() }).unwrap();
        assert_eq!(image::load_from_memory(&r.png).unwrap().into_luma8().get_pixel(5, 5).0, [128]);
    }

    #[test]
    fn outlines_leave_the_inside_white_and_holes_are_even_odd() {
        let o = RasterOptions { dpi: 25.4, gray: 0, line_width_mm: Some(1.0) };
        let r = rasterize(&[RasterShape { path: square(0.0, 0.0, 10.0), fill: false, outline: true }], &o).unwrap();
        assert_eq!((r.width_px, r.height_px), (11, 11));
        let img = image::load_from_memory(&r.png).unwrap().into_luma8();
        assert_eq!(img.get_pixel(5, 5).0, [255]);
        assert_eq!(img.get_pixel(0, 5).0, [0]);
        let mut ring = square(0.0, 0.0, 10.0);
        ring.extend(square(3.0, 3.0, 4.0));
        let r = rasterize(&[RasterShape { path: ring, fill: true, outline: false }], &o).unwrap();
        let img = image::load_from_memory(&r.png).unwrap().into_luma8();
        assert_eq!(img.get_pixel(5, 5).0, [255], "the hole");
        assert_eq!(img.get_pixel(1, 1).0, [0]);
        assert!(rasterize(&[], &o).is_err());
    }
}
