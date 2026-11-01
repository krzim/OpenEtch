//! Camera for galvo (LightBurn's Calibrate Camera Lens and the galvo camera alignment): a camera looking at the
//! work area gives a live overlay to place designs on. Three steps, as in LightBurn:
//!
//! 1. **Frames**: from a UVC camera on the engine's computer (feature `camera`, `nokhwa`) or uploaded by a browser
//!    (`POST /api/camera/frame`, PNG or JPEG), so a camera on the client also works.
//! 2. **Lens calibration** from checkerboard views: inner corners found by [`detect_checkerboard`] (or given by
//!    the client), then a two-term radial model (`k1`, `k2` about the image centre) fitted so every view's corners
//!    become a perspective image of a flat grid (Gauss-Newton on the homography reprojection error).
//! 3. **Field alignment**: at least four image points whose field positions are known (marks made with
//!    `generate.camera_alignment`-style crosses at known positions, picked in the image), giving a homography from
//!    the undistorted image to field mm.
//!
//! The overlay ([`overlay`]) resamples the latest frame into the field square, +Y up, ready to draw under the
//! design. ArUco/AprilTag marker detection is not implemented: the alignment points are picked by hand or by the
//! client. Unverified on hardware (no camera is fitted to the owner's B6).

use serde::{Deserialize, Serialize};

/// A 3x3 homography, row-major, mapping `(x, y, 1)`.
pub type Mat3 = [f64; 9];

/// Grayscale image, row-major, 0..=255.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub px: Vec<f32>,
}

impl Gray {
    pub fn from_luma8(img: &image::GrayImage) -> Gray {
        Gray { width: img.width() as usize, height: img.height() as usize, px: img.pixels().map(|p| f32::from(p.0[0])).collect() }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * self.width + x]
    }

    /// Bilinear sample; `None` outside.
    pub fn sample(&self, x: f64, y: f64) -> Option<f32> {
        if !(x >= 0.0 && y >= 0.0 && x <= (self.width - 1) as f64 && y <= (self.height - 1) as f64) {
            return None;
        }
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let top = self.at(x0, y0) * (1.0 - fx) + self.at(x1, y0) * fx;
        let bottom = self.at(x0, y1) * (1.0 - fx) + self.at(x1, y1) * fx;
        Some(top * (1.0 - fy) + bottom * fy)
    }
}

pub fn apply(h: &Mat3, p: [f64; 2]) -> [f64; 2] {
    let w = h[6] * p[0] + h[7] * p[1] + h[8];
    [(h[0] * p[0] + h[1] * p[1] + h[2]) / w, (h[3] * p[0] + h[4] * p[1] + h[5]) / w]
}

pub fn invert(h: &Mat3) -> Option<Mat3> {
    let [a, b, c, d, e, f, g, hh, i] = *h;
    let det = a * (e * i - f * hh) - b * (d * i - f * g) + c * (d * hh - e * g);
    if det.abs() < 1e-15 || !det.is_finite() {
        return None;
    }
    Some([
        (e * i - f * hh) / det,
        (c * hh - b * i) / det,
        (b * f - c * e) / det,
        (f * g - d * i) / det,
        (a * i - c * g) / det,
        (c * d - a * f) / det,
        (d * hh - e * g) / det,
        (b * g - a * hh) / det,
        (a * e - b * d) / det,
    ])
}

/// Solve `a x = b` (n x n, Gaussian elimination with partial pivoting).
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let p = (col..n).max_by(|i, j| a[*i][col].abs().total_cmp(&a[*j][col].abs()))?;
        if a[p][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, p);
        b.swap(col, p);
        let pivot = a[col].clone();
        for r in col + 1..n {
            let f = a[r][col] / pivot[col];
            for (x, p) in a[r].iter_mut().zip(&pivot).skip(col) {
                *x -= f * p;
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|c| a[r][c] * x[c]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// Normalising similarity of a point set (centroid to 0, mean distance √2).
fn normaliser(pts: &[[f64; 2]]) -> Mat3 {
    let n = pts.len() as f64;
    let (cx, cy) = (pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n);
    let d = pts.iter().map(|p| (p[0] - cx).hypot(p[1] - cy)).sum::<f64>() / n;
    let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
    [s, 0.0, -s * cx, 0.0, s, -s * cy, 0.0, 0.0, 1.0]
}

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [0.0; 9];
    for r in 0..3 {
        for c in 0..3 {
            m[r * 3 + c] = (0..3).map(|k| a[r * 3 + k] * b[k * 3 + c]).sum();
        }
    }
    m
}

/// The homography mapping `from[i]` to `to[i]` (at least 4 pairs; least squares with h33 = 1 on normalised
/// points). `None` for degenerate input (e.g. three points on a line).
pub fn homography(from: &[[f64; 2]], to: &[[f64; 2]]) -> Option<Mat3> {
    if from.len() != to.len() || from.len() < 4 || from.iter().chain(to).any(|p| !(p[0].is_finite() && p[1].is_finite())) {
        return None;
    }
    let (tf, tt) = (normaliser(from), normaliser(to));
    let mut ata = vec![vec![0.0; 8]; 8];
    let mut atb = vec![0.0; 8];
    for (p, q) in from.iter().zip(to) {
        let (x, y) = { let v = apply(&tf, *p); (v[0], v[1]) };
        let (u, v) = { let w = apply(&tt, *q); (w[0], w[1]) };
        let rows = [
            ([x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y], u),
            ([0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y], v),
        ];
        for (r, rhs) in rows {
            for i in 0..8 {
                atb[i] += r[i] * rhs;
                for j in 0..8 {
                    ata[i][j] += r[i] * r[j];
                }
            }
        }
    }
    let h = solve(ata, atb)?;
    let hn = [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0];
    let out = mul(&mul(&invert(&tt)?, &hn), &tf);
    let s = out[8];
    (s.abs() > 1e-15).then(|| out.map(|v| v / s))
}

/// Root-mean-square distance between `h(from)` and `to`.
pub fn rms(h: &Mat3, from: &[[f64; 2]], to: &[[f64; 2]]) -> f64 {
    let s: f64 = from.iter().zip(to).map(|(p, q)| { let r = apply(h, *p); (r[0] - q[0]).powi(2) + (r[1] - q[1]).powi(2) }).sum();
    (s / from.len().max(1) as f64).sqrt()
}

/// Radial lens model: an image point `p` (pixels) is undistorted to `c + (p - c)(1 + k1 r² + k2 r⁴)`, with `r` the
/// distance from the centre `c` over the half diagonal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Lens {
    pub width: u32,
    pub height: u32,
    pub k1: f64,
    pub k2: f64,
    /// RMS reprojection error of the calibration, px.
    pub rms_px: f64,
}

impl Lens {
    pub fn identity(width: u32, height: u32) -> Lens {
        Lens { width, height, k1: 0.0, k2: 0.0, rms_px: 0.0 }
    }

    fn centre_scale(&self) -> ([f64; 2], f64) {
        let (w, h) = (f64::from(self.width), f64::from(self.height));
        ([w / 2.0, h / 2.0], (w.hypot(h) / 2.0).max(1.0))
    }

    pub fn undistort(&self, p: [f64; 2]) -> [f64; 2] {
        undistort_with(p, self.k1, self.k2, self.centre_scale())
    }

    /// The distorted image point that undistorts to `u` (fixed-point iteration).
    pub fn distort(&self, u: [f64; 2]) -> [f64; 2] {
        let (c, s) = self.centre_scale();
        let mut p = u;
        for _ in 0..20 {
            let r2 = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)) / (s * s);
            let f = 1.0 + self.k1 * r2 + self.k2 * r2 * r2;
            if f.abs() < 1e-6 {
                break;
            }
            p = [c[0] + (u[0] - c[0]) / f, c[1] + (u[1] - c[1]) / f];
        }
        p
    }
}

fn undistort_with(p: [f64; 2], k1: f64, k2: f64, (c, s): ([f64; 2], f64)) -> [f64; 2] {
    let r2 = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)) / (s * s);
    let f = 1.0 + k1 * r2 + k2 * r2 * r2;
    [c[0] + (p[0] - c[0]) * f, c[1] + (p[1] - c[1]) * f]
}

/// One checkerboard view: its inner corners in image pixels, row by row (`cols` per row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub cols: usize,
    pub rows: usize,
    pub corners: Vec<[f64; 2]>,
}

fn grid(cols: usize, rows: usize) -> Vec<[f64; 2]> {
    (0..rows).flat_map(|r| (0..cols).map(move |c| [c as f64, r as f64])).collect()
}

/// Sum of squared homography residuals of every view under `k`.
fn cost(views: &[View], k: [f64; 2], cs: ([f64; 2], f64)) -> Option<f64> {
    let mut total = 0.0;
    for v in views {
        let u: Vec<[f64; 2]> = v.corners.iter().map(|p| undistort_with(*p, k[0], k[1], cs)).collect();
        let g = grid(v.cols, v.rows);
        let h = homography(&g, &u)?;
        total += g.iter().zip(&u).map(|(a, b)| { let r = apply(&h, *a); (r[0] - b[0]).powi(2) + (r[1] - b[1]).powi(2) }).sum::<f64>();
    }
    Some(total)
}

/// Most inner corners along one side of a checkerboard (LightBurn's boards have fewer than 20).
pub const MAX_BOARD_SIDE: usize = 100;

/// Whether a `cols` x `rows` board of inner corners is one this module handles (3..=[`MAX_BOARD_SIDE`] each).
pub fn board_size_ok(cols: usize, rows: usize) -> bool {
    (3..=MAX_BOARD_SIDE).contains(&cols) && (3..=MAX_BOARD_SIDE).contains(&rows)
}

/// Largest camera frame side, px (a 40 MP sensor is under 8000 px wide).
pub const MAX_FRAME_SIDE: u32 = 8192;

/// Decode an uploaded or captured frame (PNG, JPEG, BMP, GIF, WebP) to RGB, refusing a frame wider or taller than
/// [`MAX_FRAME_SIDE`] from its header, before anything is allocated (a few KB of PNG can claim gigapixels).
pub fn decode_frame(bytes: &[u8]) -> Result<image::RgbImage, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_FRAME_SIDE);
    limits.max_image_height = Some(MAX_FRAME_SIDE);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode().map_err(|e| e.to_string())?.to_rgb8())
}

/// Fit the radial model to checkerboard views of a `width` x `height` camera (LightBurn's Calibrate Camera Lens).
pub fn calibrate_lens(width: u32, height: u32, views: &[View]) -> Result<Lens, String> {
    if views.is_empty() {
        return Err("at least one checkerboard view is needed (LightBurn asks for several)".into());
    }
    for v in views {
        // Bounded first: `cols * rows` of a hostile view could wrap to the number of corners given (0) and the grid
        // of `cols x rows` points would then never finish.
        if !board_size_ok(v.cols, v.rows) || v.corners.len() != v.cols * v.rows {
            return Err(format!("a view needs cols x rows (at least 3 x 3) corners; got {} for {} x {}", v.corners.len(), v.cols, v.rows));
        }
    }
    let lens = Lens::identity(width, height);
    let cs = lens.centre_scale();
    let mut k = [0.0f64, 0.0];
    let mut c = cost(views, k, cs).ok_or("the corners are degenerate")?;
    let mut lambda = 1e-3;
    for _ in 0..100 {
        // Numeric gradient and Gauss-Newton-like step (Levenberg-Marquardt on two parameters).
        let eps = 1e-6;
        let g: Vec<f64> = (0..2)
            .map(|i| {
                let mut kp = k;
                kp[i] += eps;
                let mut km = k;
                km[i] -= eps;
                (cost(views, kp, cs).unwrap_or(f64::INFINITY) - cost(views, km, cs).unwrap_or(f64::INFINITY)) / (2.0 * eps)
            })
            .collect();
        let hess: Vec<Vec<f64>> = (0..2)
            .map(|i| {
                (0..2)
                    .map(|j| {
                        let f = |di: f64, dj: f64| {
                            let mut kk = k;
                            kk[i] += di;
                            kk[j] += dj;
                            cost(views, kk, cs).unwrap_or(f64::INFINITY)
                        };
                        let h = 1e-4;
                        (f(h, h) - f(h, -h) - f(-h, h) + f(-h, -h)) / (4.0 * h * h)
                    })
                    .collect()
            })
            .collect();
        let mut improved = false;
        for _ in 0..10 {
            let a = vec![vec![hess[0][0] * (1.0 + lambda) + lambda, hess[0][1]], vec![hess[1][0], hess[1][1] * (1.0 + lambda) + lambda]];
            let Some(step) = solve(a, vec![-g[0], -g[1]]) else { lambda *= 10.0; continue };
            let next = [k[0] + step[0], k[1] + step[1]];
            match cost(views, next, cs) {
                Some(nc) if nc < c => {
                    let done = (c - nc) < 1e-12 * c.max(1.0);
                    k = next;
                    c = nc;
                    lambda = (lambda / 10.0).max(1e-9);
                    improved = !done;
                    break;
                }
                _ => lambda *= 10.0,
            }
        }
        if !improved {
            break;
        }
    }
    let n: usize = views.iter().map(|v| v.corners.len()).sum();
    Ok(Lens { width, height, k1: k[0], k2: k[1], rms_px: (c / n as f64).sqrt() })
}

/// Field alignment: the homography from the undistorted image to field mm, from point pairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alignment {
    pub image_to_field: Mat3,
    pub rms_mm: f64,
    pub points: usize,
}

/// Align with at least four `(image px, field mm)` pairs (image points as seen, before undistortion).
pub fn align(lens: &Lens, image: &[[f64; 2]], field: &[[f64; 2]]) -> Result<Alignment, String> {
    if image.len() != field.len() || image.len() < 4 {
        return Err("at least four image points with their field positions are needed".into());
    }
    let u: Vec<[f64; 2]> = image.iter().map(|p| lens.undistort(*p)).collect();
    let h = homography(&u, field).ok_or("the points are degenerate (three or more on one line?)")?;
    Ok(Alignment { rms_mm: rms(&h, &u, field), image_to_field: h, points: image.len() })
}

/// The latest frame resampled into the field square (`field_mm` wide, `px_per_mm`), +Y up (row 0 is the top,
/// +field/2). Pixels the camera does not see are transparent.
pub fn overlay(frame: &image::RgbImage, lens: &Lens, a: &Alignment, field_mm: f64, px_per_mm: f64) -> Result<image::RgbaImage, String> {
    let n = (field_mm * px_per_mm).round();
    if !(16.0..=8192.0).contains(&n) {
        return Err(format!("the overlay would be {n} px wide; choose px_per_mm so it is 16..=8192"));
    }
    let n = n as u32;
    let to_image = invert(&a.image_to_field).ok_or("the alignment cannot be inverted")?;
    let (w, h) = (frame.width() as f64, frame.height() as f64);
    let mut out = image::RgbaImage::new(n, n);
    for (x, y, px) in out.enumerate_pixels_mut() {
        let f = [(f64::from(x) + 0.5) / px_per_mm - field_mm / 2.0, field_mm / 2.0 - (f64::from(y) + 0.5) / px_per_mm];
        let p = lens.distort(apply(&to_image, f));
        if p[0] >= 0.0 && p[1] >= 0.0 && p[0] < w - 1.0 && p[1] < h - 1.0 {
            let (x0, y0) = (p[0] as u32, p[1] as u32);
            let (fx, fy) = (p[0].fract(), p[1].fract());
            let mut c = [0.0f64; 3];
            for (dx, dy, wgt) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
                let s = frame.get_pixel(x0 + dx, y0 + dy).0;
                for k in 0..3 {
                    c[k] += f64::from(s[k]) * wgt;
                }
            }
            *px = image::Rgba([c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255]);
        }
    }
    Ok(out)
}

/// Inner corners of a `cols` x `rows` checkerboard in `img`, row by row, or `None` when not found. A saddle-point
/// detector (the Hessian determinant of the smoothed image is strongly negative at a checkerboard corner) with
/// non-maximum suppression, sub-pixel refinement, and ordering along the board's two principal directions. It needs
/// the whole board in view, roughly facing the camera; otherwise give the corners explicitly.
pub fn detect_checkerboard(img: &Gray, cols: usize, rows: usize) -> Option<Vec<[f64; 2]>> {
    let (w, h) = (img.width, img.height);
    if w < 16 || h < 16 || !board_size_ok(cols, rows) {
        return None;
    }
    let s = blur(img, 2);
    let mut resp = vec![0.0f32; w * h];
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let v = |dx: isize, dy: isize| s.at((x as isize + dx) as usize, (y as isize + dy) as usize);
            let dxx = v(2, 0) - 2.0 * v(0, 0) + v(-2, 0);
            let dyy = v(0, 2) - 2.0 * v(0, 0) + v(0, -2);
            let dxy = (v(2, 2) - v(2, -2) - v(-2, 2) + v(-2, -2)) / 4.0;
            resp[y * w + x] = (dxy * dxy - dxx * dyy).max(0.0);
        }
    }
    let max = resp.iter().copied().fold(0.0f32, f32::max);
    if max <= 0.0 {
        return None;
    }
    let radius = (w.min(h) / (2 * cols.max(rows) + 2)).clamp(3, 40);
    let mut cand: Vec<(f32, [f64; 2])> = Vec::new();
    for y in radius..h - radius {
        for x in radius..w - radius {
            let r = resp[y * w + x];
            if r < 0.15 * max {
                continue;
            }
            let mut is_max = true;
            'n: for yy in y - radius..=y + radius {
                for xx in x - radius..=x + radius {
                    if resp[yy * w + xx] > r || (resp[yy * w + xx] == r && (yy, xx) < (y, x)) {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if is_max {
                // Sub-pixel: centroid of the response around the peak.
                let (mut sx, mut sy, mut sw) = (0.0f64, 0.0f64, 0.0f64);
                for yy in y - 2..=y + 2 {
                    for xx in x - 2..=x + 2 {
                        let q = f64::from(resp[yy * w + xx]);
                        sx += q * xx as f64;
                        sy += q * yy as f64;
                        sw += q;
                    }
                }
                cand.push((r, [sx / sw, sy / sw]));
            }
        }
    }
    let n = cols * rows;
    if cand.len() < n {
        return None;
    }
    cand.sort_by(|a, b| b.0.total_cmp(&a.0));
    let pts: Vec<[f64; 2]> = cand.into_iter().take(n).map(|c| c.1).collect();
    order_grid(&pts, cols, rows)
}

fn blur(img: &Gray, r: usize) -> Gray {
    let (w, h) = (img.width, img.height);
    let mut tmp = vec![0.0f32; w * h];
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = (a..=b).map(|xx| img.at(xx, y)).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            out[y * w + x] = (a..=b).map(|yy| tmp[yy * w + x]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    Gray { width: w, height: h, px: out }
}

/// Order `pts` into rows of `cols` along the set's principal directions (the longer side of the board along
/// `cols` when cols >= rows).
fn order_grid(pts: &[[f64; 2]], cols: usize, rows: usize) -> Option<Vec<[f64; 2]>> {
    let n = pts.len() as f64;
    let c = [pts.iter().map(|p| p[0]).sum::<f64>() / n, pts.iter().map(|p| p[1]).sum::<f64>() / n];
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (mut u, mut v) = ([theta.cos(), theta.sin()], [-theta.sin(), theta.cos()]);
    if cols < rows {
        std::mem::swap(&mut u, &mut v);
    }
    // Rows run along u, from the image top (smallest v·y) down; make u point right.
    if u[0] < 0.0 {
        u = [-u[0], -u[1]];
    }
    if v[1] < 0.0 {
        v = [-v[0], -v[1]];
    }
    let mut sorted: Vec<[f64; 2]> = pts.to_vec();
    let along = |p: &[f64; 2], d: [f64; 2]| (p[0] - c[0]) * d[0] + (p[1] - c[1]) * d[1];
    sorted.sort_by(|a, b| along(a, v).total_cmp(&along(b, v)));
    let mut out = Vec::with_capacity(pts.len());
    for r in 0..rows {
        let mut row: Vec<[f64; 2]> = sorted[r * cols..(r + 1) * cols].to_vec();
        row.sort_by(|a, b| along(a, u).total_cmp(&along(b, u)));
        out.extend(row);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_recovers_a_known_transform() {
        let h: Mat3 = [1.2, 0.1, 5.0, -0.05, 0.9, -3.0, 0.0005, -0.0002, 1.0];
        let from: Vec<[f64; 2]> = (0..12).map(|i| [f64::from(i % 4) * 30.0, f64::from(i / 4) * 25.0]).collect();
        let to: Vec<[f64; 2]> = from.iter().map(|p| apply(&h, *p)).collect();
        let got = homography(&from, &to).unwrap();
        assert!(rms(&got, &from, &to) < 1e-9);
        let back = invert(&got).unwrap();
        let p = apply(&back, apply(&got, [7.0, 11.0]));
        assert!((p[0] - 7.0).abs() < 1e-9 && (p[1] - 11.0).abs() < 1e-9);
        assert!(homography(&from[..3], &to[..3]).is_none());
        let line: Vec<[f64; 2]> = (0..5).map(|i| [f64::from(i), f64::from(i)]).collect();
        assert!(homography(&line, &line).is_none(), "collinear points");
    }

    fn synthetic_view(lens: &Lens, h: &Mat3, cols: usize, rows: usize) -> View {
        let corners = grid(cols, rows).into_iter().map(|g| lens.distort(apply(h, g))).collect();
        View { cols, rows, corners }
    }

    #[test]
    fn lens_calibration_recovers_radial_distortion() {
        let truth = Lens { width: 640, height: 480, k1: -0.08, k2: 0.01, rms_px: 0.0 };
        let views = vec![
            synthetic_view(&truth, &[40.0, 2.0, 120.0, -1.0, 40.0, 80.0, 0.0, 0.0, 1.0], 9, 6),
            synthetic_view(&truth, &[35.0, 6.0, 140.0, -4.0, 37.0, 60.0, 0.0002, 0.0001, 1.0], 9, 6),
            synthetic_view(&truth, &[50.0, -3.0, 60.0, 3.0, 52.0, 40.0, -0.0001, 0.0003, 1.0], 9, 6),
        ];
        let got = calibrate_lens(640, 480, &views).unwrap();
        assert!((got.k1 - truth.k1).abs() < 0.01, "{got:?}");
        assert!(got.rms_px < 0.05, "{got:?}");
        for p in [[10.0, 10.0], [320.0, 240.0], [600.0, 50.0]] {
            let q = got.distort(got.undistort(p));
            assert!((q[0] - p[0]).abs() < 1e-3 && (q[1] - p[1]).abs() < 1e-3);
        }
        assert!(calibrate_lens(640, 480, &[]).is_err());
    }

    #[test]
    fn alignment_and_overlay() {
        let lens = Lens::identity(200, 200);
        // The camera sees the 100 mm field square as pixels 0..200, +Y up (image y down).
        let image = [[0.0, 0.0], [200.0, 0.0], [200.0, 200.0], [0.0, 200.0]];
        let field = [[-50.0, 50.0], [50.0, 50.0], [50.0, -50.0], [-50.0, -50.0]];
        let a = align(&lens, &image, &field).unwrap();
        assert!(a.rms_mm < 1e-9);
        let mut frame = image::RgbImage::new(200, 200);
        for (x, y, p) in frame.enumerate_pixels_mut() {
            *p = image::Rgb([x as u8, y as u8, 0]);
        }
        let o = overlay(&frame, &lens, &a, 100.0, 1.0).unwrap();
        assert_eq!(o.dimensions(), (100, 100));
        let p = o.get_pixel(25, 75).0;
        assert!((i32::from(p[0]) - 51).abs() <= 1 && (i32::from(p[1]) - 151).abs() <= 1 && p[3] == 255, "{p:?}");
        assert!(align(&lens, &image[..3], &field[..3]).is_err());
    }

    #[test]
    fn detects_a_rendered_checkerboard() {
        // A 6 x 4 inner-corner board (7 x 5 squares of 24 px), slightly rotated.
        let (w, h) = (320usize, 240usize);
        let (cols, rows, sq) = (6usize, 4usize, 24.0f64);
        let a = 0.08f64;
        let (ox, oy) = (160.0, 120.0);
        let mut px = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f64 - ox, y as f64 - oy);
                let (u, v) = (dx * a.cos() + dy * a.sin(), -dx * a.sin() + dy * a.cos());
                let (i, j) = ((u / sq + 3.5).floor(), (v / sq + 2.5).floor());
                let inside = (0.0..7.0).contains(&i) && (0.0..5.0).contains(&j);
                px[y * w + x] = if inside && (i + j) as i64 % 2 == 0 { 20.0 } else { 235.0 };
            }
        }
        let img = Gray { width: w, height: h, px };
        let corners = detect_checkerboard(&img, cols, rows).expect("found");
        assert_eq!(corners.len(), 24);
        // First corner: board coordinates (-2.5, -1.5) squares from the centre.
        let expect = |c: usize, r: usize| {
            let (u, v) = ((c as f64 - 2.5) * sq, (r as f64 - 1.5) * sq);
            [ox + u * a.cos() - v * a.sin(), oy + u * a.sin() + v * a.cos()]
        };
        for r in 0..rows {
            for c in 0..cols {
                let (p, e) = (corners[r * cols + c], expect(c, r));
                assert!((p[0] - e[0]).abs() < 1.5 && (p[1] - e[1]).abs() < 1.5, "corner {c},{r}: {p:?} vs {e:?}");
            }
        }
    }

    #[test]
    fn hostile_boards_and_frames_are_refused() {
        // cols * rows wraps to 0 in a release build: zero corners "matched" and the grid never ended.
        let half = 1usize << (usize::BITS / 2);
        let v = View { cols: half, rows: half, corners: Vec::new() };
        assert!(calibrate_lens(640, 480, &[v]).is_err());
        let v = View { cols: 101, rows: 3, corners: vec![[0.0, 0.0]; 303] };
        assert!(calibrate_lens(640, 480, &[v]).is_err());
        let g = Gray::from_luma8(&image::GrayImage::new(64, 64));
        assert!(detect_checkerboard(&g, usize::MAX, 3).is_none(), "no overflow panic");
        // A frame wider than MAX_FRAME_SIDE is refused from its header.
        let encode = |w: u32, h: u32| {
            let mut out = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageLuma8(image::GrayImage::new(w, h)).write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        let wide = encode(MAX_FRAME_SIDE + 1, 1);
        assert!(wide.len() < 4096, "a few KB claim a huge frame");
        assert!(decode_frame(&wide).is_err());
        assert_eq!(decode_frame(&encode(32, 8)).unwrap().dimensions(), (32, 8));
        assert!(decode_frame(b"not an image").is_err());
    }
}
