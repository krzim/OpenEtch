//! Image engraving, LightBurn's Image mode: an image object ([`doc::Geometry::Image`]) becomes scan-line runs.
//!
//! 1. **Decode** the embedded file (PNG, JPEG, GIF, BMP, WebP; `image` crate, pure Rust) into grey levels and alpha.
//!    Transparent pixels are composited on white and never fire.
//! 2. **Adjust** ([`doc::ImageAdjust`], per image): enhance (unsharp mask: a box-blurred copy three times over,
//!    with the denoise threshold), brightness, contrast, gamma ([`tone_factors`]); at the image's own resolution.
//! 3. **Grid**: square dots of the line interval in rows along the scan angle, on a lattice anchored at the field
//!    origin (so rows of neighbouring images line up), covering the image's field-frame outline. Each dot samples the
//!    image at its centre through the inverse object transform (so rotation, scaling and mirroring are honoured),
//!    bilinearly, from a box-shrunk copy when one dot spans two or more pixels. With pass-through the grid is the
//!    image's own pixel grid instead (rows along the image's rows, nothing resampled).
//! 4. **Dither** ([`doc::DitherMode`]) row by row from the first (lowest) row, left to right along the scan
//!    direction; error diffusion keeps only three rows of error, so memory does not grow with the image. `negative`
//!    inverts first.
//! 5. **Runs**: consecutive "on" dots of a row merge into one mark from the first dot's leading edge to the last
//!    dot's trailing edge (grayscale: consecutive dots of the same power level), clipped to the image outline. Dot
//!    width correction pulls each end that borders an unmarked dot in by half the width, never below a quarter of a
//!    dot. Rows alternate direction when bidirectional.
//!
//! Grayscale runs carry their power (`min + (max - min) × level / 255` for darkness levels 1..=255; pure white does
//! not fire), which the encoder sends as a `MarkCurrent` before the run (see the `encode` module).
//!
//! The dither kernels are the published Floyd-Steinberg, Jarvis-Judice-Ninke, Stucki and Atkinson weights and the
//! standard 8 x 8 Bayer matrix; the code is new.

use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{DitherMode, ImageAdjust, ImageSettings};
use kurbo::{Affine, Point as KPoint, Vec2};

use crate::geom::Point;

/// Most source pixels an image may decode to (a 8000 x 6000 photo is 48 M).
pub const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// Most dots one image may be sampled into for one pass (rows × dots per row): 10,000 x 10,000, a 100 mm image at
/// 0.01 mm. Sampling runs row by row, so this bounds time, not memory.
pub const MAX_IMAGE_DOTS: u64 = 100_000_000;

/// One image, decoded and adjusted: brightness (0 black ..= 1 white) and alpha per pixel, row-major from the top.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gray {
    pub width: usize,
    pub height: usize,
    pub value: Vec<f32>,
    pub alpha: Vec<f32>,
}

impl Gray {
    #[cfg(test)]
    pub fn from_rows<R: AsRef<[f32]>>(rows: &[R]) -> Gray {
        let width = rows[0].as_ref().len();
        Gray {
            width,
            height: rows.len(),
            value: rows.iter().flat_map(|r| r.as_ref().iter().copied()).collect(),
            alpha: vec![1.0; width * rows.len()],
        }
    }

    /// Shrink by an integer factor, averaging each `f` x `f` block (partial blocks at the edges average what they
    /// hold); brightness is averaged weighted by alpha.
    fn shrink(&self, f: usize) -> Gray {
        let (w, h) = (self.width.div_ceil(f), self.height.div_ceil(f));
        let mut value = vec![0.0; w * h];
        let mut alpha = vec![0.0; w * h];
        let mut count = vec![0u32; w * h];
        for y in 0..self.height {
            for x in 0..self.width {
                let (i, o) = (y * self.width + x, (y / f) * w + x / f);
                value[o] += self.value[i] * self.alpha[i];
                alpha[o] += self.alpha[i];
                count[o] += 1;
            }
        }
        for o in 0..w * h {
            value[o] = if alpha[o] > 0.0 { value[o] / alpha[o] } else { 1.0 };
            alpha[o] /= count[o].max(1) as f32;
        }
        Gray { width: w, height: h, value, alpha }
    }

    /// Bilinear brightness and alpha at pixel-edge coordinates (pixel `i` spans `i..i+1`), clamped to the edges.
    fn sample(&self, x: f64, y: f64) -> (f32, f32) {
        let fx = (x - 0.5).clamp(0.0, (self.width - 1) as f64);
        let fy = (y - 0.5).clamp(0.0, (self.height - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let at = |v: &[f32], x: usize, y: usize| v[y * self.width + x];
        let lerp = |v: &[f32]| {
            let top = at(v, x0, y0) * (1.0 - tx) + at(v, x1, y0) * tx;
            let bottom = at(v, x0, y1) * (1.0 - tx) + at(v, x1, y1) * tx;
            top * (1.0 - ty) + bottom * ty
        };
        let a = lerp(&self.alpha);
        // Brightness weighted by alpha, so transparent neighbours do not pull an edge towards white.
        let mut wsum = 0.0;
        let mut vsum = 0.0;
        for (xx, yy, w) in [(x0, y0, (1.0 - tx) * (1.0 - ty)), (x1, y0, tx * (1.0 - ty)), (x0, y1, (1.0 - tx) * ty), (x1, y1, tx * ty)] {
            let wa = w * at(&self.alpha, xx, yy);
            wsum += wa;
            vsum += wa * at(&self.value, xx, yy);
        }
        (if wsum > 0.0 { vsum / wsum } else { 1.0 }, a)
    }
}

/// Decode base64 image data into grey levels (see the module docs), shrunk right away by an integer factor when
/// `dot_mm` (the finest dot in the image's own millimetres; `None` keeps every pixel) spans 4 or more pixels of an
/// image `width_mm` wide: by half the pixels per dot, so a dot still spans two. Returns the image and the factor.
/// Errors are short sentences for the plan error.
pub(crate) fn decode(data_base64: &str, width_mm: f64, dot_mm: Option<f64>) -> Result<(Gray, usize), String> {
    let bytes = STANDARD.decode(data_base64.as_bytes()).map_err(|_| "its data is not valid base64".to_string())?;
    let reader = || image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(|e| e.to_string());
    let (w, h) = reader()?.into_dimensions().map_err(|e| format!("it cannot be read ({e})"))?;
    if u64::from(w) * u64::from(h) > MAX_SOURCE_PIXELS || w == 0 || h == 0 {
        return Err(format!("it is {w} x {h} pixels; at most {MAX_SOURCE_PIXELS} pixels can be engraved (scale it down)"));
    }
    let mut r = reader()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 30);
    r.limits(limits);
    let la = r.decode().map_err(|e| format!("it cannot be decoded ({e})"))?.into_luma_alpha8();
    let (width, height) = (la.width() as usize, la.height() as usize);
    let dot_px = dot_mm.map_or(0.0, |d| d * width as f64 / width_mm);
    let factor = if dot_px >= 4.0 { (dot_px / 2.0).floor().min(1024.0) as usize } else { 1 };
    let (sw, sh) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut value = vec![0.0f32; sw * sh];
    let mut alpha = vec![0.0f32; sw * sh];
    let mut count = vec![0u32; sw * sh];
    for (x, y, p) in la.enumerate_pixels() {
        let [l, a] = p.0;
        let (l, a) = (f32::from(l) / 255.0, f32::from(a) / 255.0);
        let o = (y as usize / factor) * sw + x as usize / factor;
        // Composited on white, averaged over the block.
        value[o] += l * a + (1.0 - a);
        alpha[o] += a;
        count[o] += 1;
    }
    for o in 0..sw * sh {
        let n = count[o].max(1) as f32;
        value[o] = (value[o] / n).clamp(0.0, 1.0);
        alpha[o] = (alpha[o] / n).clamp(0.0, 1.0);
    }
    Ok((Gray { width: sw, height: sh, value, alpha }, factor))
}

/// Apply LightBurn's Adjust Image values (see [`ImageAdjust`]) in place.
pub(crate) fn adjust(g: &mut Gray, a: &ImageAdjust) {
    if a.enhance_radius > 0.0 && a.enhance_amount > 0.0 {
        let blurred = blur(g, a.enhance_radius);
        let amount = (a.enhance_amount / 100.0) as f32;
        let threshold = (a.denoise / 255.0) as f32;
        for (v, b) in g.value.iter_mut().zip(blurred) {
            let d = *v - b;
            if d.abs() >= threshold && d != 0.0 {
                *v = (*v + amount * d).clamp(0.0, 1.0);
            }
        }
    }
    let (brightness, contrast) = tone_factors(a);
    let gamma = a.gamma as f32;
    if a.contrast == 0.0 && a.brightness == 0.0 && a.gamma == 1.0 {
        return;
    }
    for v in &mut g.value {
        let x = ((*v + brightness - 0.5) * contrast + 0.5).clamp(0.0, 1.0);
        *v = x.powf(gamma);
    }
}

/// The brightness shift (grey 0..=1) and the contrast slope (around mid-grey) of `a`'s Brightness and Contrast,
/// fitted to LightBurn 2.2's own processed bitmap (`adjust-image.lbrn2`: Contrast 25, Brightness 10, Gamma 1.2 turn
/// grey v into ((v + 0.2 - 0.5) · 2 + 0.5)^1.2 to within 1-2 % of the dithered density): the shift is twice the
/// brightness fraction (10 % → +0.2) and the slope 2^(contrast / 25 %) (25 % → 2, -25 % → 0.5, 0 → 1). The
/// brightness is applied before the contrast and gamma after both. One sample fixes one point of each scale: the
/// forms between are OpenEtch's choice.
pub fn tone_factors(a: &ImageAdjust) -> (f32, f32) {
    let brightness = 2.0 * a.brightness / 100.0;
    let contrast = 2f64.powf(a.contrast / 25.0);
    (brightness as f32, contrast as f32)
}

/// A Gaussian-like blur of radius `r` pixels: three box blurs of that half-width, horizontal then vertical.
fn blur(g: &Gray, r: f64) -> Vec<f32> {
    let r = (r.round() as usize).max(1);
    let mut v = g.value.clone();
    let mut tmp = vec![0.0; v.len()];
    for _ in 0..3 {
        box_pass(&v, &mut tmp, g.width, g.height, r, true);
        box_pass(&tmp, &mut v, g.width, g.height, r, false);
    }
    v
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    for line in 0..lines {
        let mut sum = 0.0f64;
        let window = |i: isize| src[idx(line, i.clamp(0, len as isize - 1) as usize)] as f64;
        for i in -(r as isize)..=(r as isize) {
            sum += window(i);
        }
        let n = (2 * r + 1) as f64;
        for i in 0..len {
            dst[idx(line, i)] = (sum / n) as f32;
            sum += window(i as isize + r as isize + 1) - window(i as isize - r as isize);
        }
    }
}

/// One mark of an image: from `from` to `to` (field mm) at power level `level` (255 = the layer's power; grayscale
/// uses 1..=255).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Run {
    pub from: Point,
    pub to: Point,
    pub level: u8,
}

/// Why an image cannot be rastered.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RasterError {
    /// More than [`MAX_IMAGE_DOTS`] dots.
    TooManyDots { rows: u64, cols: u64 },
    /// More runs than the budget allows.
    OverBudget,
}

/// The image of one object placed in the field: its pixels and the object's size and transform.
pub(crate) struct Placed<'a> {
    pub gray: &'a Gray,
    pub width_mm: f64,
    pub height_mm: f64,
    pub transform: Affine,
}

impl Placed<'_> {
    /// Local mm → pixel-edge coordinates (x right, y down from the top-left corner).
    fn local_to_pixel(&self) -> Affine {
        let (w, h) = (self.gray.width as f64, self.gray.height as f64);
        Affine::new([w / self.width_mm, 0.0, 0.0, -h / self.height_mm, w / 2.0, h / 2.0])
    }
}

/// The dot grid an image is rastered on: dot (column c, row r) covers `c..c + 1` x `r..r + 1` under `to_field`
/// (grid units to field mm). Rows burn from row 0 up, and a row's runs lie on its centre line `r + 0.5`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScanGrid {
    pub to_field: Affine,
    pub cols: usize,
    pub rows: usize,
}

/// The rows of runs of one image for one pass with settings `s` at scan angle `angle_deg`, rows in burn order, at
/// most `budget` runs in all.
#[cfg(test)]
pub(crate) fn raster(p: &Placed, s: &ImageSettings, angle_deg: f64, budget: usize) -> Result<Vec<Vec<Run>>, RasterError> {
    raster_slice(p, s, angle_deg, budget, 0.0).map(|(rows, _)| rows)
}

/// [`raster`] for one slice of a 3D Sliced pass: dots fire where the darkness is above `slice` (0..1); other dither
/// modes ignore it. Also gives the grid the runs lie on.
pub(crate) fn raster_slice(
    p: &Placed,
    s: &ImageSettings,
    angle_deg: f64,
    budget: usize,
    slice: f32,
) -> Result<(Vec<Vec<Run>>, ScanGrid), RasterError> {
    let to_pixel = p.local_to_pixel() * p.transform.inverse();
    // Grid (column, row) → field mm, with the dot pitch, and the grid size.
    let (grid, cols, rows, pitch) = if s.pass_through {
        let (w, h) = (p.gray.width, p.gray.height);
        // Row 0 is the bottom pixel row: grid (c, r) is pixel edge (c, h - r).
        let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, h as f64]);
        let g = p.transform * p.local_to_pixel().inverse() * flip;
        (g, w, h, (g * KPoint::new(1.0, 0.0) - g * KPoint::ORIGIN).length())
    } else {
        let d = s.interval_mm;
        let (sin, cos) = angle_deg.to_radians().sin_cos();
        let (u, v) = (Vec2::new(cos, sin), Vec2::new(-sin, cos));
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .map(|(x, y)| p.transform * KPoint::new(x * p.width_mm / 2.0, y * p.height_mm / 2.0));
        let along = |a: Vec2| {
            let proj = corners.map(|c| c.to_vec2().dot(a));
            (proj.iter().copied().fold(f64::INFINITY, f64::min), proj.iter().copied().fold(f64::NEG_INFINITY, f64::max))
        };
        let ((s0, s1), (t0, t1)) = (along(u), along(v));
        let (c0, r0) = ((s0 / d).floor(), (t0 / d).floor());
        let (cols, rows) = (((s1 / d).ceil() - c0).max(1.0), ((t1 / d).ceil() - r0).max(1.0));
        if cols * rows > MAX_IMAGE_DOTS as f64 || !(cols * rows).is_finite() {
            return Err(RasterError::TooManyDots { rows: rows as u64, cols: cols as u64 });
        }
        let origin = u * (c0 * d) + v * (r0 * d);
        let g = Affine::new([u.x * d, u.y * d, v.x * d, v.y * d, origin.x, origin.y]);
        (g, cols as usize, rows as usize, d)
    };
    if (cols as u64) * (rows as u64) > MAX_IMAGE_DOTS {
        return Err(RasterError::TooManyDots { rows: rows as u64, cols: cols as u64 });
    }
    // Sample from a shrunk copy when one dot spans two or more pixels.
    let m = to_pixel * grid;
    let [a, b, c, d, _, _] = m.as_coeffs();
    let span = (a.hypot(b)).min(c.hypot(d));
    let factor = if s.pass_through { 1 } else { span.floor().clamp(1.0, 1024.0) as usize };
    let shrunk;
    let (src, m) = if factor >= 2 {
        shrunk = p.gray.shrink(factor);
        (&shrunk, Affine::scale(1.0 / factor as f64) * m)
    } else {
        (p.gray, m)
    };
    // Source pixels per dot as sampled (after any shrink), so sketch measures edges per pixel, not per dot.
    let span_px = if s.pass_through { 1.0 } else { span / factor as f64 };
    let sampler = Sampler { src, m, pass_through: s.pass_through, negative: s.negative, cols, span_px };
    let mut rows_out: Vec<Vec<Run>> = Vec::new();
    let mut runs = 0usize;
    let mut emit = |r: usize, levels: &[u8]| -> Result<(), RasterError> {
        let row = row_runs(r, levels, &grid, cols, pitch, s.dot_width_mm, p);
        runs += row.len();
        if runs > budget {
            return Err(RasterError::OverBudget);
        }
        if !row.is_empty() {
            rows_out.push(row);
        }
        Ok(())
    };
    dither(s, &sampler, &grid, rows, slice, &mut emit)?;
    // Rows burn in alternating directions when bidirectional (counting only rows that mark).
    if s.bidirectional {
        for row in rows_out.iter_mut().skip(1).step_by(2) {
            row.reverse();
            for run in row.iter_mut() {
                std::mem::swap(&mut run.from, &mut run.to);
            }
        }
    }
    Ok((rows_out, ScanGrid { to_field: grid, cols, rows }))
}

/// Samples one grid row at a time: darkness (0 white ..= 1 black, after `negative`) per dot, or `None` outside the
/// image (or where it is transparent).
struct Sampler<'a> {
    src: &'a Gray,
    /// Grid (column, row) → pixel-edge coordinates of `src`.
    m: Affine,
    pass_through: bool,
    negative: bool,
    cols: usize,
    /// Source pixels per dot.
    span_px: f64,
}

impl Sampler<'_> {
    fn row(&self, r: usize, out: &mut Vec<Option<f32>>) {
        out.clear();
        let (w, h) = (self.src.width as f64, self.src.height as f64);
        for c in 0..self.cols {
            let q = self.m * KPoint::new(c as f64 + 0.5, r as f64 + 0.5);
            let inside = q.x >= 0.0 && q.x < w && q.y >= 0.0 && q.y < h;
            let v = inside.then(|| {
                if self.pass_through {
                    let i = (q.y.floor() as usize) * self.src.width + q.x.floor() as usize;
                    (self.src.value[i], self.src.alpha[i])
                } else {
                    self.src.sample(q.x, q.y)
                }
            });
            out.push(v.filter(|(_, a)| *a >= 0.5).map(|(v, _)| if self.negative { v } else { 1.0 - v }));
        }
    }
}

/// Error diffusion kernels: (dx, dy, weight) and the divisor.
const FLOYD_STEINBERG: (&[(i32, usize, f32)], f32) = (&[(1, 0, 7.0), (-1, 1, 3.0), (0, 1, 5.0), (1, 1, 1.0)], 16.0);
const JARVIS: (&[(i32, usize, f32)], f32) = (
    &[
        (1, 0, 7.0),
        (2, 0, 5.0),
        (-2, 1, 3.0),
        (-1, 1, 5.0),
        (0, 1, 7.0),
        (1, 1, 5.0),
        (2, 1, 3.0),
        (-2, 2, 1.0),
        (-1, 2, 3.0),
        (0, 2, 5.0),
        (1, 2, 3.0),
        (2, 2, 1.0),
    ],
    48.0,
);
const STUCKI: (&[(i32, usize, f32)], f32) = (
    &[
        (1, 0, 8.0),
        (2, 0, 4.0),
        (-2, 1, 2.0),
        (-1, 1, 4.0),
        (0, 1, 8.0),
        (1, 1, 4.0),
        (2, 1, 2.0),
        (-2, 2, 1.0),
        (-1, 2, 2.0),
        (0, 2, 4.0),
        (1, 2, 2.0),
        (2, 2, 1.0),
    ],
    42.0,
);
/// Atkinson passes on 6/8 of the error.
const ATKINSON: (&[(i32, usize, f32)], f32) =
    (&[(1, 0, 1.0), (2, 0, 1.0), (-1, 1, 1.0), (0, 1, 1.0), (1, 1, 1.0), (0, 2, 1.0)], 8.0);

/// The 8 x 8 Bayer index matrix.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// Takes each dithered row: its index and levels.
type RowSink<'a> = dyn FnMut(usize, &[u8]) -> Result<(), RasterError> + 'a;

/// Dither row by row, handing each row's levels (0 off, else the power level) to `emit`.
fn dither(
    s: &ImageSettings,
    sampler: &Sampler,
    grid: &Affine,
    rows: usize,
    slice: f32,
    emit: &mut RowSink,
) -> Result<(), RasterError> {
    let cols = sampler.cols;
    let mut row: Vec<Option<f32>> = Vec::with_capacity(cols);
    let mut levels = vec![0u8; cols];
    let on = |b: bool| if b { 255 } else { 0 };
    match s.dither {
        DitherMode::FloydSteinberg | DitherMode::Jarvis | DitherMode::Stucki | DitherMode::Atkinson => {
            let (kernel, divisor) = match s.dither {
                DitherMode::FloydSteinberg => FLOYD_STEINBERG,
                DitherMode::Jarvis => JARVIS,
                DitherMode::Stucki => STUCKI,
                _ => ATKINSON,
            };
            // Error carried into this row and the next two.
            let mut err = [vec![0.0f32; cols], vec![0.0f32; cols], vec![0.0f32; cols]];
            for r in 0..rows {
                sampler.row(r, &mut row);
                for c in 0..cols {
                    levels[c] = 0;
                    let Some(v) = row[c] else { continue };
                    let x = v + err[0][c];
                    let fire = x >= 0.5;
                    levels[c] = on(fire);
                    let e = x - if fire { 1.0 } else { 0.0 };
                    for &(dx, dy, w) in kernel {
                        let cc = c as i64 + i64::from(dx);
                        if (0..cols as i64).contains(&cc) {
                            err[dy][cc as usize] += e * w / divisor;
                        }
                    }
                }
                err.rotate_left(1);
                err[2].iter_mut().for_each(|e| *e = 0.0);
                emit(r, &levels)?;
            }
        }
        DitherMode::Sketch => {
            // Sobel edge strength on darkness, per source pixel (a dot finer than a pixel sees a bilinear ramp);
            // outside dots repeat the centre (no edge at the image border).
            let mut prev: Vec<Option<f32>> = Vec::new();
            let mut cur: Vec<Option<f32>> = Vec::new();
            let mut next: Vec<Option<f32>> = Vec::new();
            sampler.row(0, &mut cur);
            for r in 0..rows {
                if r + 1 < rows {
                    sampler.row(r + 1, &mut next);
                } else {
                    next.clear();
                }
                for c in 0..cols {
                    levels[c] = 0;
                    let Some(v) = cur[c] else { continue };
                    let at = |line: &[Option<f32>], dc: i64| -> f32 {
                        let cc = c as i64 + dc;
                        if (0..cols as i64).contains(&cc) { line.get(cc as usize).copied().flatten().unwrap_or(v) } else { v }
                    };
                    let (p, n) = (prev.as_slice(), next.as_slice());
                    let gx = (at(p, 1) + 2.0 * at(&cur, 1) + at(n, 1)) - (at(p, -1) + 2.0 * at(&cur, -1) + at(n, -1));
                    let gy = (at(n, -1) + 2.0 * at(n, 0) + at(n, 1)) - (at(p, -1) + 2.0 * at(p, 0) + at(p, 1));
                    let per_px = f64::from(gx.hypot(gy) / 4.0) / sampler.span_px.clamp(1e-3, 1.0);
                    levels[c] = on(per_px >= 0.25);
                }
                emit(r, &levels)?;
                std::mem::swap(&mut prev, &mut cur);
                std::mem::swap(&mut cur, &mut next);
            }
        }
        mode => {
            let cell = match mode {
                DitherMode::Halftone => Some((25.4 / s.halftone_cells_per_inch, s.halftone_angle_deg)),
                DitherMode::Newsprint => Some((4.0 * s.interval_mm.max(1e-6), 45.0)),
                _ => None,
            };
            let screen = cell.map(|(size, angle)| Affine::scale(1.0 / size) * Affine::rotate(-angle.to_radians()) * *grid);
            for r in 0..rows {
                sampler.row(r, &mut row);
                for c in 0..cols {
                    levels[c] = match row[c] {
                        None => 0,
                        Some(v) => match mode {
                            DitherMode::Threshold => on(v >= 0.5),
                            DitherMode::Ordered => on(v > (f32::from(BAYER[r % 8][c % 8]) + 0.5) / 64.0),
                            DitherMode::Grayscale => (v.clamp(0.0, 1.0) * 255.0).round() as u8,
                            // Above the slice level; a dot of pure white never fires.
                            DitherMode::Slice3d => on(v > slice.max(1.0 / 512.0)),
                            _ => {
                                // Round spot: the area of a disc through this dot, as a share of the cell.
                                let q = screen.expect("a screen mode") * KPoint::new(c as f64 + 0.5, r as f64 + 0.5);
                                let (fx, fy) = (q.x - q.x.round(), q.y - q.y.round());
                                let t = (std::f64::consts::PI * (fx * fx + fy * fy)).min(0.999);
                                on(f64::from(v) > t)
                            }
                        },
                    };
                }
                emit(r, &levels)?;
            }
        }
    }
    Ok(())
}

/// The runs of row `r`: consecutive dots of the same non-zero level, clipped to the image outline, with dot width
/// correction at ends next to an unmarked dot.
fn row_runs(r: usize, levels: &[u8], grid: &Affine, cols: usize, pitch: f64, dot_width_mm: f64, p: &Placed) -> Vec<Run> {
    let mut out = Vec::new();
    let mut c = 0;
    // Shortening per end, in dots, and the shortest a run may become.
    let trim = if dot_width_mm > 0.0 && pitch > 0.0 { dot_width_mm / 2.0 / pitch } else { 0.0 };
    let to_local = p.transform.inverse();
    let (hw, hh) = (p.width_mm / 2.0, p.height_mm / 2.0);
    let y = r as f64 + 0.5;
    while c < cols {
        let level = levels[c];
        if level == 0 {
            c += 1;
            continue;
        }
        let start = c;
        while c < cols && levels[c] == level {
            c += 1;
        }
        let (mut a, mut b) = (start as f64, c as f64);
        let len = b - a;
        let cut_a = if start == 0 || levels[start - 1] == 0 { trim } else { 0.0 };
        let cut_b = if c == cols || levels[c] == 0 { trim } else { 0.0 };
        let keep = len * 0.25;
        let scale = if cut_a + cut_b > len - keep { (len - keep) / (cut_a + cut_b) } else { 1.0 };
        a += cut_a * scale;
        b -= cut_b * scale;
        let (from, to) = (*grid * KPoint::new(a, y), *grid * KPoint::new(b, y));
        // Clip to the image rectangle in local coordinates (affine maps keep the segment's parameter).
        let (la, lb) = (to_local * from, to_local * to);
        let Some((t0, t1)) = clip(la, lb, hw, hh) else { continue };
        let at = |t: f64| {
            let q = from.lerp(to, t);
            [q.x, q.y]
        };
        if t1 > t0 {
            out.push(Run { from: at(t0), to: at(t1), level });
        }
    }
    out
}

/// The part `t0..t1` of segment `a`→`b` inside `|x| <= hw, |y| <= hh` (Liang-Barsky), if any.
fn clip(a: KPoint, b: KPoint, hw: f64, hh: f64) -> Option<(f64, f64)> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-dx, a.x + hw), (dx, hw - a.x), (-dy, a.y + hh), (dy, hh - a.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// Clip runs to a mask (LightBurn's Apply Mask to Image): closed polygons in the field frame, even-odd. Each run keeps
/// the pieces inside; rows that end up empty are dropped.
pub(crate) fn clip_runs(rows: &[Vec<Run>], mask: &[Vec<Point>]) -> Vec<Vec<Run>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let mut kept = Vec::new();
        for run in row {
            for (t0, t1) in inside_spans(run.from, run.to, mask) {
                let at = |t: f64| [run.from[0] + (run.to[0] - run.from[0]) * t, run.from[1] + (run.to[1] - run.from[1]) * t];
                kept.push(Run { from: at(t0), to: at(t1), level: run.level });
            }
        }
        if !kept.is_empty() {
            out.push(kept);
        }
    }
    out
}

/// The parameter spans (0..=1) of segment `a`-`b` inside the even-odd region of `polys`.
fn inside_spans(a: Point, b: Point, polys: &[Vec<Point>]) -> Vec<(f64, f64)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let mut ts = vec![0.0, 1.0];
    for poly in polys {
        for (i, p) in poly.iter().enumerate() {
            let q = poly[(i + 1) % poly.len()];
            let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
            let den = dx * ey - dy * ex;
            if den.abs() < 1e-15 {
                continue;
            }
            let t = ((p[0] - a[0]) * ey - (p[1] - a[1]) * ex) / den;
            let u = ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                ts.push(t);
            }
        }
    }
    ts.sort_by(f64::total_cmp);
    ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for w in ts.windows(2) {
        let m = (w[0] + w[1]) / 2.0;
        if w[1] - w[0] > 1e-12 && point_inside([a[0] + dx * m, a[1] + dy * m], polys) {
            match spans.last_mut() {
                Some(last) if (last.1 - w[0]).abs() < 1e-12 => last.1 = w[1],
                _ => spans.push((w[0], w[1])),
            }
        }
    }
    spans
}

/// Even-odd point in polygons (each implicitly closed).
pub(crate) fn point_inside(pt: Point, polys: &[Vec<Point>]) -> bool {
    let mut inside = false;
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (p, q) = (poly[i], poly[(i + 1) % n]);
            if (p[1] > pt[1]) != (q[1] > pt[1]) {
                let x = p[0] + (pt[1] - p[1]) / (q[1] - p[1]) * (q[0] - p[0]);
                if pt[0] < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// An image as it will be engraved (LightBurn's Save Processed Bitmap): `levels` row-major from the top, one value per
/// dot of the line interval (pass-through: per pixel), 0 = not fired (white), 255 = fired (grayscale and 3D Sliced:
/// the grey level, 255 darkest).
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedBitmap {
    pub width: usize,
    pub height: usize,
    pub levels: Vec<u8>,
    /// Dots per inch of the grid (25.4 / interval; pass-through: the image's own).
    pub dpi: f64,
}

/// Most dots [`processed_bitmap`] produces.
pub const MAX_PROCESSED_DOTS: u64 = 64_000_000;

/// Process image object `o` with `s` (decode, Adjust Image, negative, dither) on the image's own axis-aligned grid at
/// the line interval (as many whole dots as fit, stretched to the image's size, as LightBurn's Save Processed Bitmap
/// does), as [`ProcessedBitmap`], mirrored as the object's transform mirrors it (rotation is left out). 3D Sliced shows the grey levels quantised to `passes` slices (a dot of
/// darkness d is fired by ceil(d · N) of the N passes: level 255 · that / N).
pub fn processed_bitmap(o: &doc::Object, s: &ImageSettings, passes: u32) -> Result<ProcessedBitmap, String> {
    let doc::Geometry::Image { width_mm, height_mm, data_base64, adjust: a, .. } = &o.geometry else {
        return Err(format!("{} is not an image", o.id));
    };
    if let Some(p) = a.problem() {
        return Err(p);
    }
    let dot = (!s.pass_through).then_some(s.interval_mm).filter(|d| d.is_finite() && *d > 0.0);
    if !s.pass_through && dot.is_none() {
        return Err("the line interval must be positive".into());
    }
    let (mut gray, factor) = decode(data_base64, *width_mm, dot)?;
    let scaled = ImageAdjust { enhance_radius: a.enhance_radius / factor as f64, ..a.clone() };
    adjust(&mut gray, &scaled);
    let placed = Placed { gray: &gray, width_mm: *width_mm, height_mm: *height_mm, transform: Affine::IDENTITY };
    let (grid, cols, rows, dpi) = if s.pass_through {
        let (w, h) = (gray.width, gray.height);
        let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, h as f64]);
        (placed.local_to_pixel().inverse() * flip, w, h, w as f64 * factor as f64 / width_mm * 25.4)
    } else {
        // As LightBurn: whole dots of the interval that fit (its 105.848 x 92.882 mm image at 0.1 mm is 1058 x 928),
        // stretched to cover the image exactly.
        let d = s.interval_mm;
        let (cols, rows) = ((width_mm / d + 1e-9).floor().max(1.0), (height_mm / d + 1e-9).floor().max(1.0));
        if cols * rows > MAX_PROCESSED_DOTS as f64 {
            return Err(format!("{cols} x {rows} dots is more than {MAX_PROCESSED_DOTS}; use a larger interval"));
        }
        let (px, py) = (width_mm / cols, height_mm / rows);
        let g = Affine::new([px, 0.0, 0.0, py, -width_mm / 2.0, -height_mm / 2.0]);
        (g, cols as usize, rows as usize, 25.4 / px)
    };
    let m = placed.local_to_pixel() * grid;
    let [ca, cb, cc, cd, _, _] = m.as_coeffs();
    let span = (ca.hypot(cb)).min(cc.hypot(cd));
    let shrink = if s.pass_through { 1 } else { span.floor().clamp(1.0, 1024.0) as usize };
    let shrunk;
    let (src, m) = if shrink >= 2 {
        shrunk = gray.shrink(shrink);
        (&shrunk, Affine::scale(1.0 / shrink as f64) * m)
    } else {
        (&gray, m)
    };
    let span_px = if s.pass_through { 1.0 } else { span / shrink as f64 };
    let sampler = Sampler { src, m, pass_through: s.pass_through, negative: s.negative, cols, span_px };
    let mut levels = vec![0u8; cols * rows];
    if s.dither == DitherMode::Slice3d {
        let n = passes.max(1);
        let mut row = Vec::with_capacity(cols);
        for r in 0..rows {
            sampler.row(r, &mut row);
            let top = rows - 1 - r;
            for (c, v) in row.iter().enumerate() {
                let fired = v.map_or(0, |v| (f64::from(v.clamp(0.0, 1.0)) * f64::from(n) - 1e-6).ceil().max(0.0) as u32);
                levels[top * cols + c] = (255 * fired.min(n) / n) as u8;
            }
        }
    } else {
        let mut emit = |r: usize, row: &[u8]| -> Result<(), RasterError> {
            let top = rows - 1 - r;
            levels[top * cols..(top + 1) * cols].copy_from_slice(row);
            Ok(())
        };
        dither(s, &sampler, &grid, rows, 0.0, &mut emit).map_err(|_| "the image could not be dithered".to_string())?;
    }
    // As shown: an object mirrored on an axis (a LightBurn bitmap's rows are stored bottom-up, so its import adds a
    // vertical flip) is saved mirrored the same way, as LightBurn's own processed bitmap is.
    let [a, _, _, d, _, _] = o.transform.as_coeffs();
    if d < 0.0 {
        let flipped: Vec<u8> = levels.chunks(cols).rev().flatten().copied().collect();
        levels = flipped;
    }
    if a < 0.0 {
        levels.chunks_mut(cols).for_each(<[u8]>::reverse);
    }
    Ok(ProcessedBitmap { width: cols, height: rows, levels, dpi })
}

/// [`ProcessedBitmap`] as a PNG: fired dots black, the rest white (grey levels as greys), with its dpi in `pHYs`.
pub fn processed_png(b: &ProcessedBitmap) -> Result<Vec<u8>, String> {
    let pixels: Vec<u8> = b.levels.iter().map(|l| 255 - l).collect();
    let (w, h) = (u32::try_from(b.width).map_err(|e| e.to_string())?, u32::try_from(b.height).map_err(|e| e.to_string())?);
    let mut out = Vec::new();
    let enc = image::codecs::png::PngEncoder::new(&mut out);
    image::ImageEncoder::write_image(enc, &pixels, w, h, image::ExtendedColorType::L8).map_err(|e| e.to_string())?;
    let ppm = (b.dpi / 0.0254).round();
    if ppm.is_finite() && ppm >= 1.0 && ppm < f64::from(u32::MAX) {
        insert_phys(&mut out, ppm as u32);
    }
    Ok(out)
}

/// Insert a `pHYs` chunk (pixels per metre on both axes) right after a PNG's IHDR.
fn insert_phys(png: &mut Vec<u8>, ppm: u32) {
    // Signature (8) + IHDR chunk (length 4 + type 4 + data 13 + CRC 4).
    let at = 8 + 25;
    if png.len() < at {
        return;
    }
    let mut body = b"pHYs".to_vec();
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1);
    let mut chunk = 9u32.to_be_bytes().to_vec();
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc32(&body).to_be_bytes());
    png.splice(at..at, chunk);
}

/// CRC-32 (IEEE), as PNG chunks use.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod slice_tests {
    use super::*;
    use base64::Engine;

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> String {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([f(x, y)]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    fn object(data: String, w: f64, h: f64) -> doc::Object {
        let g = doc::Geometry::Image { width_mm: w, height_mm: h, mime: "image/png".into(), data_base64: data.into(), adjust: Default::default() };
        doc::Object::new("obj-1".into(), "i", "layer-1".into(), g, Affine::IDENTITY)
    }

    #[test]
    fn processed_bitmap_thresholds_and_flips_rows_to_the_top() {
        // Left half black, right half white, top row black only on the right.
        let data = png(4, 2, |x, y| if (x < 2) != (y == 0) { 0 } else { 255 });
        let s = ImageSettings { dither: DitherMode::Threshold, interval_mm: 1.0, ..ImageSettings::default() };
        let b = processed_bitmap(&object(data, 4.0, 2.0), &s, 1).unwrap();
        assert_eq!((b.width, b.height), (4, 2));
        assert_eq!(b.levels, [0, 0, 255, 255, 255, 255, 0, 0]);
        assert!((b.dpi - 25.4).abs() < 1e-9);
        let png = processed_png(&b).unwrap();
        let back = image::load_from_memory(&png).unwrap().into_luma8();
        assert_eq!(back.get_pixel(0, 0).0, [255]);
        assert_eq!(back.get_pixel(3, 0).0, [0]);
        assert!(png.windows(4).any(|w| w == b"pHYs"));
    }

    #[test]
    fn sliced_levels_count_passes() {
        // Darkness 0, 1/4, 1/2, 1 across.
        let data = png(4, 1, |x, _| [255u8, 192, 128, 0][x as usize]);
        let s = ImageSettings { dither: DitherMode::Slice3d, interval_mm: 1.0, ..ImageSettings::default() };
        let b = processed_bitmap(&object(data, 4.0, 1.0), &s, 4).unwrap();
        assert_eq!(b.levels, [0, 63, 127, 255]);
    }

    #[test]
    fn runs_are_clipped_to_the_mask() {
        let rows = vec![vec![Run { from: [-10.0, 0.0], to: [10.0, 0.0], level: 255 }], vec![Run { from: [-10.0, 9.0], to: [10.0, 9.0], level: 255 }]];
        // A square ring: outer ±5, hole ±2.
        let outer = vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]];
        let hole = vec![[-2.0, -2.0], [2.0, -2.0], [2.0, 2.0], [-2.0, 2.0]];
        let out = clip_runs(&rows, &[outer, hole]);
        assert_eq!(out.len(), 1, "the row at y 9 is outside");
        let spans: Vec<(f64, f64)> = out[0].iter().map(|r| (r.from[0], r.to[0])).collect();
        assert_eq!(spans, [(-5.0, -2.0), (2.0, 5.0)]);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Settings for a 1 mm grid scanning along +X, one-way, so dots and runs are easy to read.
    pub fn one_mm(dither: DitherMode) -> ImageSettings {
        ImageSettings { dither, interval_mm: 1.0, bidirectional: false, ..ImageSettings::default() }
    }

    /// The image shown at 1 mm per pixel with its edges on whole millimetres (centred on the origin, or half a
    /// millimetre off it for an odd size), so grid dots fall exactly on pixels.
    fn placed(g: &Gray) -> Placed<'_> {
        let (w, h) = (g.width as f64, g.height as f64);
        Placed { gray: g, width_mm: w, height_mm: h, transform: Affine::translate(((w / 2.0).fract(), (h / 2.0).fract())) }
    }

    /// The dots of each row, bottom row first, as strings of `#` (on) and `.` (off).
    fn bits(g: &Gray, s: &ImageSettings) -> Vec<String> {
        let p = placed(g);
        let rows = raster(&p, s, 0.0, usize::MAX).unwrap();
        let mut out = vec![vec!['.'; g.width]; g.height];
        let x0 = -(g.width as f64 / 2.0).floor();
        let y0 = -(g.height as f64 / 2.0).floor();
        for row in rows {
            for run in row {
                let r = (run.from[1] - y0).floor() as usize;
                let (a, b) = (run.from[0].min(run.to[0]), run.from[0].max(run.to[0]));
                for cell in &mut out[r][((a - x0).round() as usize)..((b - x0).round() as usize)] {
                    *cell = '#';
                }
            }
        }
        out.into_iter().map(|r| r.into_iter().collect()).collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn threshold_and_ordered_on_a_ramp() {
        // Brightness; darkness is 1 - brightness and fires from 0.5 on.
        let g = Gray::from_rows(&[&[1.0, 0.8, 0.6, 0.5, 0.4, 0.2, 0.0, 0.49]]);
        assert_eq!(bits(&g, &one_mm(DitherMode::Threshold)), ["...#####"]);
        let neg = ImageSettings { negative: true, ..one_mm(DitherMode::Threshold) };
        assert_eq!(bits(&g, &neg), ["####...."], "negative inverts first");
        // Ordered: darkness 0.5 fires where the Bayer index is below 32 (rows 0 32 8 40 2 34 10 42 and
        // 48 16 56 24 50 18 58 26).
        let half = Gray::from_rows(&[&[0.5; 8], &[0.5; 8]]);
        assert_eq!(bits(&half, &one_mm(DitherMode::Ordered)), strings(&["#.#.#.#.", ".#.#.#.#"]));
    }

    #[test]
    fn error_diffusion_kernels_give_their_known_patterns() {
        // Expected bits from an independent re-implementation of the four kernels (weights as published).
        // One row of 40 % darkness: only the same-row weights act (FS 7/16; Jarvis 7/48, 5/48; Stucki 8/42, 4/42;
        // Atkinson 1/8, 1/8).
        let row = Gray::from_rows(&[&[0.6; 4]]);
        assert_eq!(bits(&row, &one_mm(DitherMode::FloydSteinberg)), [".#.."]);
        for m in [DitherMode::Jarvis, DitherMode::Stucki, DitherMode::Atkinson] {
            assert_eq!(bits(&row, &one_mm(m)), ["..#."], "{m:?}");
        }
        // Three rows of 30 % darkness, first row processed (and listed) first.
        let g = Gray::from_rows(&[&[0.7; 5], &[0.7; 5], &[0.7; 5]]);
        let expect = [
            (DitherMode::FloydSteinberg, ["...#.", ".#...", "..#.#"]),
            (DitherMode::Jarvis, [".....", ".#..#", "..#.."]),
            (DitherMode::Stucki, [".....", ".#.#.", "..#.."]),
            (DitherMode::Atkinson, [".....", "..#..", "..#.."]),
        ];
        for (m, rows) in expect {
            assert_eq!(bits(&g, &one_mm(m)), strings(&rows), "{m:?}");
        }
        // Mid grey dithers to about half the dots with the full kernels.
        let half = Gray::from_rows(&[&[0.5; 8]; 8]);
        for m in [DitherMode::FloydSteinberg, DitherMode::Jarvis, DitherMode::Stucki] {
            let on: usize = bits(&half, &one_mm(m)).iter().map(|r| r.matches('#').count()).sum();
            assert!((28..=36).contains(&on), "{m:?}: {on} of 64");
        }
    }

    #[test]
    fn runs_merge_consecutive_dots_and_alternate_when_bidirectional() {
        // Top row first: the bottom row (burned first) is all dark but its last dot.
        let g = Gray::from_rows(&[&[0.0, 0.0, 1.0, 0.0], &[0.0, 0.0, 0.0, 1.0]]);
        let p = placed(&g);
        let ends = |rows: &[Vec<Run>]| -> Vec<Vec<(Point, Point)>> {
            rows.iter().map(|r| r.iter().map(|run| (run.from, run.to)).collect()).collect()
        };
        // The image spans x -2..2, y -1..1.
        let one_way = raster(&p, &one_mm(DitherMode::Threshold), 0.0, usize::MAX).unwrap();
        assert_eq!(
            ends(&one_way),
            [vec![([-2.0, -0.5], [1.0, -0.5])], vec![([-2.0, 0.5], [0.0, 0.5]), ([1.0, 0.5], [2.0, 0.5])]]
        );
        let both = raster(&p, &ImageSettings { bidirectional: true, ..one_mm(DitherMode::Threshold) }, 0.0, usize::MAX).unwrap();
        assert_eq!(ends(&both)[0], ends(&one_way)[0]);
        assert_eq!(ends(&both)[1], [([2.0, 0.5], [1.0, 0.5]), ([0.0, 0.5], [-2.0, 0.5])], "the second row runs back");
        assert_eq!(raster(&p, &one_mm(DitherMode::Threshold), 0.0, 2), Err(RasterError::OverBudget));
    }

    #[test]
    fn dot_width_shortens_free_ends_only() {
        let g = Gray::from_rows(&[&[0.0, 0.0, 0.0, 1.0, 0.0]]);
        let s = ImageSettings { dot_width_mm: 0.4, ..one_mm(DitherMode::Threshold) };
        let rows = raster(&placed(&g), &s, 0.0, usize::MAX).unwrap();
        let r: Vec<(Point, Point)> = rows[0].iter().map(|r| (r.from, r.to)).collect();
        // The image spans x -2..3: runs -2..1 and 2..3, each end pulled in by 0.2.
        assert_eq!(r.len(), 2);
        assert!((r[0].0[0] + 1.8).abs() < 1e-9 && (r[0].1[0] - 0.8).abs() < 1e-9, "{r:?}");
        assert!((r[1].0[0] - 2.2).abs() < 1e-9 && (r[1].1[0] - 2.8).abs() < 1e-9, "{r:?}");
        // A dot width of a whole dot would leave nothing: a single dot keeps a quarter.
        let wide = ImageSettings { dot_width_mm: 1.0, ..s };
        let rows = raster(&placed(&g), &wide, 0.0, usize::MAX).unwrap();
        let single = rows[0][1];
        assert!(((single.to[0] - single.from[0]) - 0.25).abs() < 1e-9, "{single:?}");
        // Grayscale runs touching each other keep their shared end.
        let g = Gray::from_rows(&[&[0.0, 0.5, 1.0]]);
        let gs = ImageSettings { dot_width_mm: 0.4, ..one_mm(DitherMode::Grayscale) };
        let rows = raster(&placed(&g), &gs, 0.0, usize::MAX).unwrap();
        let r: Vec<(f64, f64)> = rows[0].iter().map(|r| (r.from[0], r.to[0])).collect();
        // The image spans x -1..2: dark (-1..0) then mid grey (0..1); only the outer ends move.
        assert_eq!(r.len(), 2);
        assert!((r[0].0 + 0.8).abs() < 1e-9 && r[0].1.abs() < 1e-9 && r[1].0.abs() < 1e-9, "{r:?}");
        assert!((r[1].1 - 0.8).abs() < 1e-9, "{r:?}");
    }

    #[test]
    fn grayscale_levels_merge_equal_runs() {
        let g = Gray::from_rows(&[&[1.0, 0.0, 0.0, 0.5, 1.0]]);
        let rows = raster(&placed(&g), &one_mm(DitherMode::Grayscale), 0.0, usize::MAX).unwrap();
        let levels: Vec<(u8, f64)> = rows[0].iter().map(|r| (r.level, r.to[0] - r.from[0])).collect();
        assert_eq!(levels, [(255, 2.0), (128, 1.0)], "white does not fire; equal levels merge");
    }

    #[test]
    fn rotation_mirroring_and_scan_angle_follow_the_transform() {
        // A 2 x 1 image, left pixel black. Turned 90° counter-clockwise it stands upright with black at the bottom:
        // the dot at field (-0.5, -0.5), clipped to the image's x -0.5..0.5.
        let g = Gray::from_rows(&[&[0.0, 1.0]]);
        let p = Placed { gray: &g, width_mm: 2.0, height_mm: 1.0, transform: Affine::rotate(std::f64::consts::FRAC_PI_2) };
        let all: Vec<Run> = raster(&p, &one_mm(DitherMode::Threshold), 0.0, usize::MAX).unwrap().concat();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].from, all[0].to), ([-0.5, -0.5], [0.0, -0.5]));
        // Mirrored in X: the black pixel lands on the right.
        let p = Placed { gray: &g, width_mm: 2.0, height_mm: 1.0, transform: Affine::translate((0.0, 0.5)) * Affine::FLIP_X };
        let all: Vec<Run> = raster(&p, &one_mm(DitherMode::Threshold), 0.0, usize::MAX).unwrap().concat();
        assert_eq!((all[0].from, all[0].to), ([0.0, 0.5], [1.0, 0.5]));
        // Scanning at 90°: the run goes along +Y, over the black pixel (x -1..0).
        let p = Placed { gray: &g, width_mm: 2.0, height_mm: 1.0, transform: Affine::translate((0.0, 0.5)) };
        let all: Vec<Run> = raster(&p, &one_mm(DitherMode::Threshold), 90.0, usize::MAX).unwrap().concat();
        assert_eq!(all.len(), 1);
        let near = |a: Point, b: Point| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        assert!(near(all[0].from, [-0.5, 0.0]) && near(all[0].to, [-0.5, 1.0]), "{all:?}");
    }

    #[test]
    fn pass_through_uses_the_pixels_and_transparency_never_fires() {
        let mut g = Gray::from_rows(&[&[0.0, 1.0, 0.0], &[0.0, 0.0, 0.0]]);
        g.alpha[5] = 0.0;
        // Shown at 0.3 mm per pixel; the interval and the scan angle are not used.
        let p = Placed { gray: &g, width_mm: 0.9, height_mm: 0.6, transform: Affine::IDENTITY };
        let s = ImageSettings { pass_through: true, ..one_mm(DitherMode::Threshold) };
        let rows = raster(&p, &s, 33.0, usize::MAX).unwrap();
        let r = |v: f64| (v * 1e6).round() / 1e6;
        let spans: Vec<Vec<(f64, f64, f64)>> =
            rows.iter().map(|row| row.iter().map(|run| (r(run.from[0]), r(run.to[0]), r(run.from[1]))).collect()).collect();
        assert_eq!(spans, [vec![(-0.45, 0.15, -0.15)], vec![(-0.45, -0.15, 0.15), (0.15, 0.45, 0.15)]]);
    }

    #[test]
    fn adjust_changes_grey_levels_as_documented() {
        let mut g = Gray::from_rows(&[&[0.25, 0.5, 0.75]]);
        adjust(&mut g, &ImageAdjust { gamma: 2.0, ..ImageAdjust::default() });
        assert!((g.value[1] - 0.25).abs() < 1e-6, "gamma above 1 darkens mid-tones: {:?}", g.value);
        let mut g = Gray::from_rows(&[&[0.25, 0.5, 0.75]]);
        // LightBurn's scales (fitted to its processed bitmap): brightness 10 % shifts by 0.2 first, contrast 25 %
        // doubles the slope around mid-grey, -25 % halves it.
        adjust(&mut g, &ImageAdjust { contrast: 25.0, brightness: 10.0, ..ImageAdjust::default() });
        let near = |a: &[f32], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert!(near(&g.value, [0.4, 0.9, 1.0]), "{:?}", g.value);
        let mut g = Gray::from_rows(&[&[0.25, 0.5, 0.75]]);
        adjust(&mut g, &ImageAdjust { contrast: -25.0, ..ImageAdjust::default() });
        assert!(near(&g.value, [0.375, 0.5, 0.625]), "{:?}", g.value);
        assert_eq!(tone_factors(&ImageAdjust::default()), (0.0, 1.0));
        let mut flat = Gray::from_rows(&[&[0.5; 5]]);
        adjust(&mut flat, &ImageAdjust { enhance_radius: 1.0, enhance_amount: 400.0, ..ImageAdjust::default() });
        assert!(flat.value.iter().all(|v| (v - 0.5).abs() < 1e-6), "a flat image has no edges to enhance");
        let mut edge = Gray::from_rows(&[&[0.4, 0.4, 0.6, 0.6]]);
        adjust(&mut edge, &ImageAdjust { enhance_radius: 1.0, enhance_amount: 100.0, ..ImageAdjust::default() });
        assert!(edge.value[1] < 0.4 && edge.value[2] > 0.6, "edges gain contrast: {:?}", edge.value);
        let mut quiet = Gray::from_rows(&[&[0.4, 0.4, 0.6, 0.6]]);
        adjust(&mut quiet, &ImageAdjust { enhance_radius: 1.0, enhance_amount: 100.0, denoise: 200.0, ..ImageAdjust::default() });
        assert_eq!(quiet.value, [0.4, 0.4, 0.6, 0.6], "differences below the denoise level are left alone");
    }

    #[test]
    fn halftone_newsprint_and_sketch_mark_what_they_should() {
        let white = Gray::from_rows(&[&[1.0; 8]; 8]);
        let black = Gray::from_rows(&[&[0.0; 8]; 8]);
        for m in [DitherMode::Halftone, DitherMode::Newsprint, DitherMode::Sketch] {
            let count = |g: &Gray| bits(g, &one_mm(m)).iter().map(|r| r.matches('#').count()).sum::<usize>();
            assert_eq!(count(&white), 0, "{m:?} on white");
            if m != DitherMode::Sketch {
                assert_eq!(count(&black), 64, "{m:?} on black");
            } else {
                assert_eq!(count(&black), 0, "a flat image has no edges");
            }
        }
        // Sketch marks both sides of a vertical edge.
        let edge = Gray::from_rows(&[&[1.0, 1.0, 1.0, 0.0, 0.0, 0.0]; 3]);
        assert_eq!(bits(&edge, &one_mm(DitherMode::Sketch)), ["..##..", "..##..", "..##.."].map(String::from));
    }

    #[test]
    fn decode_reads_png_with_alpha_and_refuses_junk() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_fn(2, 1, |x, _| if x == 0 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let (g, factor) = decode(&STANDARD.encode(&png), 2.0, None).unwrap();
        assert_eq!(factor, 1);
        assert_eq!((g.width, g.height), (2, 1));
        assert_eq!((g.value[0], g.alpha[0]), (0.0, 1.0));
        assert_eq!((g.value[1], g.alpha[1]), (1.0, 0.0), "transparent is white");
        assert!(decode("not base64!", 1.0, None).is_err());
        assert!(decode("iVBORw0KGgo=", 1.0, None).is_err());
    }

    #[test]
    fn huge_grids_are_refused_before_sampling() {
        let g = Gray::from_rows(&[&[0.0]]);
        let p = Placed { gray: &g, width_mm: 100.0, height_mm: 100.0, transform: Affine::IDENTITY };
        let s = ImageSettings { interval_mm: 0.001, ..ImageSettings::default() };
        assert!(matches!(raster(&p, &s, 0.0, usize::MAX), Err(RasterError::TooManyDots { .. })));
    }
}

