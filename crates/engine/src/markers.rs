//! Camera marker detection (JOB-007): finds the square fiducials `generate.camera_markers` adds (ArUco layout,
//! OpenEtch's dictionary, `commands::camera_markers`) in a camera frame, so the camera aligns to the field without
//! points being clicked. Pure Rust, written for OpenEtch (the steps are the usual ones of ArUco-style detectors):
//!
//! 1. **Threshold**: a pixel is dark when it is more than [`THRESHOLD_OFFSET`] grey levels below the mean of its
//!    neighbourhood (integral image; window about 1/20 of the frame), so uneven light does not matter. Both
//!    polarities are tried (a mark can come out dark or light on the material).
//! 2. **Candidates**: 4-connected dark regions of a plausible size; each region's outline is reduced to its convex
//!    hull and then to four corners (the farthest pair and the farthest points on each side of it). Regions whose
//!    hull is not close to that quadrilateral are dropped.
//! 3. **Decode**: a homography maps the 6 x 6 cell grid onto the corners; each cell is read at its centre (mean of
//!    five samples), split at the midpoint of the darkest and lightest cell. The border must be all dark; the 4 x 4
//!    inner cells give the code, matched against the dictionary in all four turns with at most one cell wrong.
//!
//! The result is each marker's id, its corners (ordered as drawn: top-left, top-right, bottom-right, bottom-left)
//! and its centre in frame pixels, as seen (lens distortion not removed; [`crate::camera::align`] does that).

use commands::camera_markers::{CELLS, match_code};
use serde::Serialize;

use crate::camera::{Gray, Mat3, apply, homography};

/// How far below the local mean a dark pixel is, grey levels.
pub const THRESHOLD_OFFSET: f32 = 7.0;
/// Smallest marker side considered, pixels (6 cells of at least 2 px).
const MIN_SIDE_PX: f64 = 12.0;
/// Least contrast between a marker's dark and light cells, grey levels.
const MIN_CONTRAST: f32 = 25.0;
/// Most candidates decoded per polarity (a noisy frame stays fast).
const MAX_CANDIDATES: usize = 2000;

/// One marker found in a frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Marker {
    pub id: usize,
    /// Frame pixels, top-left, top-right, bottom-right, bottom-left of the marker as drawn.
    pub corners: [[f64; 2]; 4],
    pub center: [f64; 2],
    /// The marks were light on a dark ground.
    pub inverted: bool,
    /// Cells read wrong (0 or 1).
    pub errors: u32,
}

/// Every marker in `img` (one per id: the larger when an id is seen twice).
pub fn detect_markers(img: &Gray) -> Vec<Marker> {
    let mut found: Vec<Marker> = Vec::new();
    if img.width < 16 || img.height < 16 {
        return found;
    }
    for inverted in [false, true] {
        let mask = threshold(img, inverted);
        for region in regions(&mask, img.width, img.height).into_iter().take(MAX_CANDIDATES) {
            let Some(quad) = quad_of(&region, img.width) else { continue };
            if let Some(m) = decode(img, quad, inverted) {
                match found.iter_mut().find(|f| f.id == m.id) {
                    Some(f) if side(&m.corners) > side(&f.corners) => *f = m,
                    Some(_) => {}
                    None => found.push(m),
                }
            }
        }
    }
    found.sort_by_key(|m| m.id);
    found
}

fn side(c: &[[f64; 2]; 4]) -> f64 {
    (0..4).map(|i| dist(c[i], c[(i + 1) % 4])).sum::<f64>() / 4.0
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Adaptive threshold: `true` where the pixel is dark (light when `inverted`) against its neighbourhood.
fn threshold(img: &Gray, inverted: bool) -> Vec<bool> {
    let (w, h) = (img.width, img.height);
    let mut sum = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += f64::from(img.px[y * w + x]);
            sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
        }
    }
    let r = (w.min(h) / 40).max(7);
    let mut out = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = sum[y1 * (w + 1) + x1] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0] + sum[y0 * (w + 1) + x0];
            let mean = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
            let v = img.px[y * w + x];
            out[y * w + x] = if inverted { v > mean + THRESHOLD_OFFSET } else { v < mean - THRESHOLD_OFFSET };
        }
    }
    out
}

/// The outline pixels (`y * width + x`) of each 4-connected region of `mask` of a plausible size, largest first.
fn regions(mask: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    let max_area = w * h / 4;
    let min_area = (MIN_SIDE_PX * MIN_SIDE_PX / 4.0) as usize;
    let mut stack = Vec::new();
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut area = 0usize;
        let mut edge = Vec::new();
        while let Some(i) = stack.pop() {
            area += 1;
            let (x, y) = (i % w, i / w);
            let mut outline = x == 0 || y == 0 || x == w - 1 || y == h - 1;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx >= w || ny >= h {
                    continue;
                }
                let j = ny * w + nx;
                if mask[j] {
                    if !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                } else {
                    outline = true;
                }
            }
            if outline {
                edge.push(i);
            }
        }
        if (min_area..=max_area).contains(&area) {
            out.push(edge);
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.len()));
    out
}

fn cross(o: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Convex hull (monotone chain), counter-clockwise in a y-up sense.
fn hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let mut lower: Vec<[f64; 2]> = Vec::new();
    for p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<[f64; 2]> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n).map(|i| cross([0.0, 0.0], poly[i], poly[(i + 1) % n])).sum::<f64>().abs() / 2.0
}

/// Four corners of an outline (pixel corners included, so the quad covers whole pixels), in hull order; `None`
/// when the region is not close to a quadrilateral.
fn quad_of(edge: &[usize], w: usize) -> Option<[[f64; 2]; 4]> {
    let mut pts = Vec::with_capacity(edge.len() * 4);
    for &i in edge {
        let (x, y) = ((i % w) as f64, (i / w) as f64);
        pts.extend([[x, y], [x + 1.0, y], [x, y + 1.0], [x + 1.0, y + 1.0]]);
    }
    let h = hull(pts);
    if h.len() < 4 {
        return None;
    }
    // The farthest pair, then the farthest point from that diagonal on each side.
    let (mut a, mut b, mut best) = (0, 0, 0.0);
    for i in 0..h.len() {
        for j in i + 1..h.len() {
            let d = dist(h[i], h[j]);
            if d > best {
                (a, b, best) = (i, j, d);
            }
        }
    }
    if best < MIN_SIDE_PX * std::f64::consts::SQRT_2 {
        return None;
    }
    let (mut c, mut cd, mut d, mut dd) = (None, 0.0, None, 0.0);
    for (k, p) in h.iter().enumerate() {
        let s = cross(h[a], h[b], *p);
        if s > cd {
            (c, cd) = (Some(k), s);
        }
        if -s > dd {
            (d, dd) = (Some(k), -s);
        }
    }
    let (c, d) = (c?, d?);
    let mut idx = [a, b, c, d];
    idx.sort_unstable();
    let quad = idx.map(|k| h[k]);
    // The hull must be nearly the quad (a square seen in perspective), and every side long enough.
    if area(&quad) < 0.9 * area(&h) || (0..4).any(|i| dist(quad[i], quad[(i + 1) % 4]) < MIN_SIDE_PX) {
        return None;
    }
    Some(quad)
}

/// Read the cells of the marker inside `quad`.
fn decode(img: &Gray, quad: [[f64; 2]; 4], inverted: bool) -> Option<Marker> {
    let n = CELLS as f64;
    // Grid corners in the hull's order; which marker corner each is follows from the code's turn.
    let grid = [[0.0, 0.0], [n, 0.0], [n, n], [0.0, n]];
    let h: Mat3 = homography(&grid, &quad)?;
    let mut cells = [[0f32; CELLS]; CELLS];
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for (r, row) in cells.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let (u, v) = (c as f64 + 0.5, r as f64 + 0.5);
            let mut s = 0.0;
            for (du, dv) in [(0.0, 0.0), (-0.2, -0.2), (0.2, -0.2), (-0.2, 0.2), (0.2, 0.2)] {
                let p = apply(&h, [u + du, v + dv]);
                s += img.sample(p[0], p[1])?;
            }
            *cell = if inverted { 255.0 - s / 5.0 } else { s / 5.0 };
            lo = lo.min(*cell);
            hi = hi.max(*cell);
        }
    }
    if hi - lo < MIN_CONTRAST {
        return None;
    }
    let mid = (lo + hi) / 2.0;
    let dark = |r: usize, c: usize| cells[r][c] < mid;
    for i in 0..CELLS {
        if !(dark(0, i) && dark(CELLS - 1, i) && dark(i, 0) && dark(i, CELLS - 1)) {
            return None;
        }
    }
    // The hull may run either way round; read the grid as is and mirrored (rows and columns swapped).
    for mirror in [false, true] {
        let mut code = 0u16;
        for r in 0..4 {
            for c in 0..4 {
                let (rr, cc) = if mirror { (c + 1, r + 1) } else { (r + 1, c + 1) };
                code = (code << 1) | u16::from(dark(rr, cc));
            }
        }
        let Some((id, turns, errors)) = match_code(code, 1) else { continue };
        // Grid corners in reading order: (0,0), (n,0), (n,n), (0,n), or with rows and columns swapped.
        let order: [usize; 4] = if mirror { [0, 3, 2, 1] } else { [0, 1, 2, 3] };
        let read = order.map(|k| quad[k]);
        // `turns` quarter turns clockwise bring the read code to the marker's: the marker's top-left is the read
        // grid's corner `turns` steps on (counter-clockwise in reading order).
        let t = turns as usize;
        let corners = [read[(4 - t) % 4], read[(5 - t) % 4], read[(6 - t) % 4], read[(7 - t) % 4]];
        let center = apply(&h, [n / 2.0, n / 2.0]);
        return Some(Marker { id, corners, center, inverted, errors });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use commands::camera_markers::marker_cells;

    /// A frame with markers drawn through `to_frame` (marker cell coordinates, 0..6, to frame pixels).
    /// Marker cell coordinates to frame pixels.
    type ToFrame = Box<dyn Fn([f64; 2]) -> [f64; 2]>;

    fn draw(w: usize, h: usize, markers: &[(usize, ToFrame)], light_marks: bool) -> Gray {
        let ground = if light_marks { 40.0 } else { 210.0 };
        let mut px = vec![ground; w * h];
        // A gentle light gradient across the frame.
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] += (x as f32 / w as f32) * 30.0 - 15.0;
            }
        }
        for (id, f) in markers {
            let cells = marker_cells(*id).unwrap();
            // Inverse mapping by sampling: every frame pixel whose centre falls in a cell.
            let inv = {
                let from = [[0.0, 0.0], [6.0, 0.0], [6.0, 6.0], [0.0, 6.0]];
                let to = from.map(f);
                crate::camera::homography(&to, &from).unwrap()
            };
            for y in 0..h {
                for x in 0..w {
                    let p = apply(&inv, [x as f64 + 0.5, y as f64 + 0.5]);
                    if (0.0..6.0).contains(&p[0]) && (0.0..6.0).contains(&p[1]) && cells[p[1] as usize][p[0] as usize] {
                        px[y * w + x] = if light_marks { 220.0 } else { 25.0 };
                    }
                }
            }
        }
        Gray { width: w, height: h, px }
    }

    fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
        dist(a, b) < tol
    }

    #[test]
    fn markers_are_found_turned_tilted_and_inverted() {
        let s = 10.0; // px per cell
        let straight: ToFrame = Box::new(move |p| [40.0 + p[0] * s, 30.0 + p[1] * s]);
        // Turned 30 degrees about (200, 80).
        let (sn, cs) = 30f64.to_radians().sin_cos();
        let turned: ToFrame = Box::new(move |p| {
            let (u, v) = ((p[0] - 3.0) * s, (p[1] - 3.0) * s);
            [200.0 + u * cs - v * sn, 80.0 + u * sn + v * cs]
        });
        // Seen in perspective.
        let persp: ToFrame = Box::new(move |p| {
            let w = 1.0 + 0.03 * p[1];
            [(60.0 + p[0] * 11.0) / w + 40.0, (150.0 + p[1] * 14.0) / w]
        });
        let img = draw(320, 260, &[(3, straight), (7, turned), (12, persp)], false);
        let found = detect_markers(&img);
        let ids: Vec<usize> = found.iter().map(|m| m.id).collect();
        assert_eq!(ids, [3, 7, 12], "{found:?}");
        let m3 = &found[0];
        assert!(near(m3.corners[0], [40.0, 30.0], 1.5) && near(m3.corners[2], [100.0, 90.0], 1.5), "{m3:?}");
        assert!(near(m3.center, [70.0, 60.0], 1.0));
        // The turned marker's top-left corner is where cell (0, 0) went.
        let tl = [200.0 + (-30.0) * cs - (-30.0) * sn, 80.0 + (-30.0) * sn + (-30.0) * cs];
        assert!(near(found[1].corners[0], tl, 2.0), "{:?} vs {tl:?}", found[1]);
        assert!(near(found[1].center, [200.0, 80.0], 1.0));
        assert!(near(found[2].corners[0], [100.0, 150.0], 2.0) && near(found[2].corners[1], [166.0, 150.0], 2.0), "{:?}", found[2]);

        // Light marks on a dark ground.
        let f: ToFrame = Box::new(move |p| [100.0 + p[0] * 8.0, 50.0 + p[1] * 8.0]);
        let found = detect_markers(&draw(240, 160, &[(0, f)], true));
        assert_eq!(found.len(), 1);
        assert!(found[0].inverted && found[0].id == 0);
        // Nothing in a blank or noisy frame.
        assert!(detect_markers(&draw(200, 200, &[], false)).is_empty());
        let mut noise = draw(200, 200, &[], false);
        let mut seed = 12345u32;
        for p in &mut noise.px {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *p = (seed >> 24) as f32;
        }
        assert!(detect_markers(&noise).is_empty());
    }
}
