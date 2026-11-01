//! Trace Image (LightBurn's Trace Image, Alt+T, and 2.2's Center Line Trace): a bitmap → vector paths.
//!
//! LightBurn's options map onto the steps of this tracer (written for OpenEtch; the steps follow the published
//! outline of bitmap tracing: threshold, boundary decomposition, polygon fitting, corner/curve smoothing):
//!
//! 1. **Mask**: pixels whose brightness (composited on white) lies in `cutoff..=threshold` (0-255; LightBurn's
//!    defaults 0 and 128) are traced. `sketch` instead marks pixels darker than their surroundings (a local mean),
//!    for photos of drawings with uneven light; `transparency` traces the opaque area. A `boundary` (pixel rectangle)
//!    limits the trace to part of the image.
//! 2. **Ignore less than**: traced areas and holes of fewer pixels than this are dropped (speckles).
//! 3. **Boundaries**: the edges between traced and untraced pixels, joined into closed loops (diagonal pixels
//!    connect).
//! 4. **Optimize**: each loop is reduced to a polygon within 0.5 + `optimize` pixels of it (LightBurn's default 0.2;
//!    0 only straightens the pixel staircase).
//! 5. **Smoothness** (0 ..= 1.33, LightBurn's range): each polygon corner is measured (how far it sticks out of the
//!    chord between its neighbouring edge midpoints); corners sharper than the smoothness stay corners, the others
//!    become Bézier curves. 0 gives only lines, 1.33 only curves.
//!
//! **Center line** (`centerline`): the mask is thinned to a one-pixel skeleton (Zhang-Suen), the skeleton is walked
//! into open polylines between ends and junctions, branches shorter than `ignore_less_than` pixels are pruned, and
//! the polylines are simplified and smoothed like the outlines.
//!
//! Results are in pixel-edge coordinates (x right, y down from the image's top-left corner).

use std::collections::HashMap;

use kurbo::{BezPath, Point};

use crate::bitmap::Gray;
use crate::{Result, invalid};

/// Most pixels traced (larger images are refused; scale them down).
pub const MAX_PIXELS: usize = 40_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct TraceOptions {
    pub cutoff: u8,
    pub threshold: u8,
    /// Areas (and holes) of fewer pixels are dropped; branches of fewer pixels are pruned from centre lines.
    pub ignore_less_than: u32,
    /// 0 ..= 1.333.
    pub smoothness: f64,
    pub optimize: f64,
    pub centerline: bool,
    pub sketch: bool,
    pub transparency: bool,
    /// `[x0, y0, x1, y1]` in pixels: trace only inside.
    pub boundary_px: Option<[f64; 4]>,
}

impl Default for TraceOptions {
    fn default() -> Self {
        TraceOptions {
            cutoff: 0,
            threshold: 128,
            ignore_less_than: 2,
            smoothness: 1.0,
            optimize: 0.2,
            centerline: false,
            sketch: false,
            transparency: false,
            boundary_px: None,
        }
    }
}

/// The traced paths and their size.
#[derive(Debug, Clone, PartialEq)]
pub struct Traced {
    /// All traced subpaths (pixel-edge coordinates, y down).
    pub path: BezPath,
    /// Number of subpaths.
    pub contours: usize,
    /// Nodes (path points) in all.
    pub nodes: usize,
}

/// The pixels to trace.
pub fn mask(g: &Gray, o: &TraceOptions) -> Vec<bool> {
    let (w, h) = (g.width, g.height);
    let mut m: Vec<bool> = if o.transparency {
        g.alpha.iter().map(|a| *a >= 128).collect()
    } else if o.sketch {
        // Darker than the local mean by more than a margin set by the threshold (128 → 10 grey levels).
        let r = (w.min(h) / 40).clamp(4, 64);
        let mut sum = vec![0u64; (w + 1) * (h + 1)];
        for y in 0..h {
            let mut row = 0u64;
            for x in 0..w {
                row += u64::from(g.value[y * w + x]);
                sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
            }
        }
        let margin = (256.0 - f64::from(o.threshold)) / 12.8;
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (x0, y0, x1, y1) = (x.saturating_sub(r), y.saturating_sub(r), (x + r + 1).min(w), (y + r + 1).min(h));
                let s = sum[y1 * (w + 1) + x1] + sum[y0 * (w + 1) + x0] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0];
                let mean = s as f64 / ((x1 - x0) * (y1 - y0)) as f64;
                let v = g.value[i];
                g.alpha[i] >= 128 && f64::from(v) + margin < mean && v >= o.cutoff
            })
            .collect()
    } else {
        g.value.iter().zip(&g.alpha).map(|(v, a)| *a >= 128 && (o.cutoff..=o.threshold).contains(v)).collect()
    };
    if let Some([x0, y0, x1, y1]) = o.boundary_px {
        for (i, px) in m.iter_mut().enumerate() {
            let (cx, cy) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            if !(cx >= x0.min(x1) && cx <= x0.max(x1) && cy >= y0.min(y1) && cy <= y0.max(y1)) {
                *px = false;
            }
        }
    }
    m
}

/// Drop traced areas (8-connected) and fill holes (4-connected) of fewer than `min` pixels.
pub fn despeckle(m: &mut [bool], w: usize, h: usize, min: u32) {
    if min <= 1 {
        return;
    }
    for value in [true, false] {
        let mut seen = vec![false; m.len()];
        let mut stack = Vec::new();
        let mut comp = Vec::new();
        for start in 0..m.len() {
            if seen[start] || m[start] != value {
                continue;
            }
            comp.clear();
            stack.push(start);
            seen[start] = true;
            let mut touches_edge = false;
            while let Some(i) = stack.pop() {
                comp.push(i);
                let (x, y) = ((i % w) as i64, (i / w) as i64);
                if x == 0 || y == 0 || x == w as i64 - 1 || y == h as i64 - 1 {
                    touches_edge = true;
                }
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    if !value && dx != 0 && dy != 0 {
                        continue;
                    }
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !seen[j] && m[j] == value {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            // The background around the image is one big area: never fill it.
            if comp.len() < min as usize && (value || !touches_edge) {
                for &i in &comp {
                    m[i] = !value;
                }
            }
        }
    }
}

/// The boundaries of the traced pixels as closed loops of pixel corners (traced area on the right in y-down
/// coordinates; diagonal neighbours join), without collinear points.
pub fn contours(m: &[bool], w: usize, h: usize) -> Vec<Vec<Point>> {
    let at = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize];
    // Directed edges keyed by start corner.
    let key = |x: i64, y: i64| (y as u64) * (w as u64 + 1) + x as u64;
    let mut out: HashMap<u64, Vec<(i64, i64, i64, i64)>> = HashMap::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !at(x, y) {
                continue;
            }
            let mut e = |a: (i64, i64), b: (i64, i64)| out.entry(key(a.0, a.1)).or_default().push((a.0, a.1, b.0, b.1));
            if !at(x, y - 1) {
                e((x, y), (x + 1, y));
            }
            if !at(x + 1, y) {
                e((x + 1, y), (x + 1, y + 1));
            }
            if !at(x, y + 1) {
                e((x + 1, y + 1), (x, y + 1));
            }
            if !at(x - 1, y) {
                e((x, y + 1), (x, y));
            }
        }
    }
    let mut starts: Vec<u64> = out.iter().filter(|(_, v)| v.len() == 1).map(|(k, _)| *k).collect();
    starts.sort_unstable();
    let mut loops = Vec::new();
    for s in starts {
        let Some(first) = out.get_mut(&s).and_then(|v| v.pop()) else { continue };
        let mut pts = vec![Point::new(first.0 as f64, first.1 as f64)];
        let mut cur = first;
        loop {
            let (dx, dy) = (cur.2 - cur.0, cur.3 - cur.1);
            let Some(next) = out.get_mut(&key(cur.2, cur.3)) else { break };
            if next.is_empty() {
                break;
            }
            // At a saddle take the left turn, which joins diagonal pixels.
            let pick = if next.len() > 1 {
                next.iter().position(|e| (e.2 - e.0, e.3 - e.1) == (dy, -dx)).unwrap_or(0)
            } else {
                0
            };
            let e = next.swap_remove(pick);
            pts.push(Point::new(e.0 as f64, e.1 as f64));
            cur = e;
        }
        // Drop collinear points.
        let n = pts.len();
        let mut simple: Vec<Point> = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            if (b - a).cross(c - b).abs() > 1e-9 {
                simple.push(b);
            }
        }
        if simple.len() >= 3 {
            loops.push(simple);
        }
    }
    loops
}

fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l2 = ab.hypot2();
    if l2 == 0.0 {
        return (p - a).hypot();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).hypot()
}

/// Longest run Douglas-Peucker sees at once: longer polylines are simplified in pieces (a vertex is kept every
/// `DP_CHUNK` points at most), bounding the quadratic worst case on huge contours.
const DP_CHUNK: usize = 4096;

/// Douglas-Peucker on an open polyline (every point but the last; the caller adds it). Iterative: a pixel staircase
/// (any long diagonal edge) splits next to an end every time, so recursion went as deep as the contour is long, past
/// the thread's stack. Among equally far points the one nearest the middle splits, which keeps staircases balanced.
fn dp(pts: &[Point], tol: f64, out: &mut Vec<Point>) {
    // Ranges still to simplify, the leftmost on top, so points come out in order.
    let mut todo: Vec<(usize, usize)> = Vec::new();
    let last = pts.len().saturating_sub(1);
    let mut starts: Vec<usize> = (0..last.max(1)).step_by(DP_CHUNK).collect();
    starts.reverse();
    for s in starts {
        todo.push((s, (s + DP_CHUNK + 1).min(pts.len())));
    }
    while let Some((s, e)) = todo.pop() {
        let part = &pts[s..e];
        if part.len() < 3 {
            out.extend_from_slice(&part[..part.len().saturating_sub(1)]);
            continue;
        }
        let (a, b) = (part[0], part[part.len() - 1]);
        let mid = part.len() / 2;
        let (mut best, mut at) = (0.0, 0usize);
        for (i, p) in part.iter().enumerate().take(part.len() - 1).skip(1) {
            let d = seg_dist(*p, a, b);
            if d > best + 1e-12 || ((d - best).abs() <= 1e-12 && i.abs_diff(mid) < at.abs_diff(mid)) {
                best = d;
                at = i;
            }
        }
        if best > tol {
            todo.push((s + at, e));
            todo.push((s, s + at + 1));
        } else {
            out.push(a);
        }
    }
}

/// Simplify a closed polygon within `tol`.
pub fn simplify_closed(pts: &[Point], tol: f64) -> Vec<Point> {
    if pts.len() <= 4 {
        return pts.to_vec();
    }
    // Split at the point farthest from the first.
    let far = (1..pts.len()).max_by(|&i, &j| (pts[i] - pts[0]).hypot2().total_cmp(&(pts[j] - pts[0]).hypot2())).unwrap_or(1);
    let a: Vec<Point> = pts[..=far].to_vec();
    let mut b: Vec<Point> = pts[far..].to_vec();
    b.push(pts[0]);
    let mut out = Vec::new();
    dp(&a, tol, &mut out);
    dp(&b, tol, &mut out);
    if out.len() < 3 { pts.to_vec() } else { out }
}

/// Simplify an open polyline within `tol` (ends kept).
pub fn simplify_open(pts: &[Point], tol: f64) -> Vec<Point> {
    let mut out = Vec::new();
    dp(pts, tol, &mut out);
    if let Some(&l) = pts.last() {
        out.push(l);
    }
    out
}

/// How much corner `v` (between edge midpoints `a` and `b`) sticks out: 0 (flat) ..= 4/3 (sharp).
fn corner_alpha(a: Point, v: Point, b: Point) -> f64 {
    let chord = b - a;
    let len = chord.hypot();
    let d = if len < 1e-12 { (v - a).hypot() } else { (v - a).cross(chord).abs() / len };
    let dd = d / 0.5;
    let alpha = if dd > 1.0 { 1.0 - 1.0 / dd } else { 0.0 };
    alpha / 0.75
}

/// Append the smoothed outline of polygon `pts` (closed) or polyline (open) to `path`.
pub fn smooth_into(path: &mut BezPath, pts: &[Point], closed: bool, smoothness: f64) -> usize {
    let n = pts.len();
    if n < 2 {
        return 0;
    }
    let mid = |i: usize| pts[i % n].midpoint(pts[(i + 1) % n]);
    let mut nodes = 0;
    let corner_or_curve = |path: &mut BezPath, a: Point, v: Point, b: Point, nodes: &mut usize| {
        let alpha = corner_alpha(a, v, b);
        if alpha > smoothness {
            path.line_to(v);
            path.line_to(b);
            *nodes += 2;
        } else {
            let t = 0.5 + 0.5 * alpha.clamp(0.55, 1.0);
            let c1 = a + (v - a) * t;
            let c2 = b + (v - b) * t;
            path.curve_to(c1, c2, b);
            *nodes += 1;
        }
    };
    if closed {
        if n < 3 {
            return 0;
        }
        path.move_to(mid(n - 1));
        for (i, &v) in pts.iter().enumerate() {
            corner_or_curve(path, mid(i + n - 1), v, mid(i), &mut nodes);
        }
        path.close_path();
    } else {
        path.move_to(pts[0]);
        if n == 2 {
            path.line_to(pts[1]);
            return 2;
        }
        let m = |i: usize| pts[i].midpoint(pts[i + 1]);
        path.line_to(m(0));
        for (i, &v) in pts.iter().enumerate().take(n - 1).skip(1) {
            corner_or_curve(path, m(i - 1), v, m(i), &mut nodes);
        }
        path.line_to(pts[n - 1]);
        nodes += 3;
    }
    nodes
}

/// The Zhang-Suen neighbours P2..P9 (clockwise from north) of pixel `(x, y)`; outside the image is off.
fn zs_neighbours(m: &[bool], w: usize, h: usize, x: i64, y: i64) -> [bool; 8] {
    let get = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize];
    [get(x, y - 1), get(x + 1, y - 1), get(x + 1, y), get(x + 1, y + 1), get(x, y + 1), get(x - 1, y + 1), get(x - 1, y), get(x - 1, y - 1)]
}

/// Whether Zhang-Suen deletes an on pixel with neighbours `p` in sub-step `step`.
fn zs_deletes(p: &[bool; 8], step: usize) -> bool {
    let b = p.iter().filter(|v| **v).count();
    let a = (0..8).filter(|&i| !p[i] && p[(i + 1) % 8]).count();
    let ok = if step == 0 { !(p[0] && p[2] && p[4]) && !(p[2] && p[4] && p[6]) } else { !(p[0] && p[2] && p[6]) && !(p[0] && p[4] && p[6]) };
    (2..=6).contains(&b) && a == 1 && ok
}

/// Zhang-Suen thinning in place. Only candidates are tested: on pixels with an off neighbour (an on pixel whose 8
/// neighbours are all on is never deleted), kept up to date as pixels go. The result is the classic full-scan one,
/// but the work follows the shrinking boundary instead of scanning the whole image on every one of up to thousands
/// of passes (a large solid area made Center Line Trace run for hours).
pub fn thin(m: &mut [bool], w: usize, h: usize) {
    let xy = |i: usize| ((i % w) as i64, (i / w) as i64);
    let mut queued = vec![false; m.len()];
    let mut cand: Vec<usize> = Vec::new();
    for i in 0..m.len() {
        if m[i] {
            let (x, y) = xy(i);
            if zs_neighbours(m, w, h, x, y).iter().any(|v| !v) {
                queued[i] = true;
                cand.push(i);
            }
        }
    }
    let mut del = Vec::new();
    let mut changed = true;
    let mut guard = 0;
    while changed && guard < 10_000 {
        guard += 1;
        changed = false;
        for step in 0..2 {
            del.clear();
            for &i in &cand {
                let (x, y) = xy(i);
                if m[i] && zs_deletes(&zs_neighbours(m, w, h, x, y), step) {
                    del.push(i);
                }
            }
            if del.is_empty() {
                continue;
            }
            changed = true;
            for &i in &del {
                m[i] = false;
            }
            // Candidates: the ones still on, plus the on neighbours of the pixels just deleted.
            let mut next: Vec<usize> = Vec::with_capacity(cand.len());
            for &i in &cand {
                if m[i] {
                    next.push(i);
                } else {
                    queued[i] = false;
                }
            }
            for &i in &del {
                let (x, y) = xy(i);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                        let j = ny as usize * w + nx as usize;
                        if m[j] && !queued[j] {
                            queued[j] = true;
                            next.push(j);
                        }
                    }
                }
            }
            cand = next;
        }
    }
}

/// Walk a one-pixel skeleton into polylines (pixel centres), pruning end branches shorter than `min_branch` pixels.
pub fn skeleton_lines(m: &[bool], w: usize, h: usize, min_branch: usize) -> Vec<Vec<Point>> {
    let idx = |x: i64, y: i64| (x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && m[y as usize * w + x as usize]).then(|| y as usize * w + x as usize);
    let neighbours = |i: usize| -> Vec<usize> {
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let mut v = Vec::with_capacity(8);
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            if let Some(j) = idx(x + dx, y + dy) {
                v.push(j);
            }
        }
        v
    };
    let center = |i: usize| Point::new((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
    let degree: Vec<u8> = (0..m.len()).map(|i| if m[i] { neighbours(i).len() as u8 } else { 0 }).collect();
    let mut used: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut lines: Vec<(Vec<usize>, bool)> = Vec::new();
    let is_node = |i: usize| degree[i] != 2;
    // Chains from nodes (ends and junctions).
    for start in (0..m.len()).filter(|&i| m[i] && is_node(i)) {
        for n0 in neighbours(start) {
            if used.contains(&(start, n0)) {
                continue;
            }
            let mut chain = vec![start, n0];
            used.insert((start, n0));
            used.insert((n0, start));
            let (mut prev, mut cur) = (start, n0);
            while !is_node(cur) {
                let Some(next) = neighbours(cur).into_iter().find(|&j| j != prev && !used.contains(&(cur, j))) else { break };
                used.insert((cur, next));
                used.insert((next, cur));
                chain.push(next);
                prev = cur;
                cur = next;
            }
            // A branch ending in a free end is a spur when short.
            let free_end = degree[*chain.last().unwrap_or(&start)] == 1 || degree[start] == 1;
            let both_free = degree[*chain.last().unwrap_or(&start)] == 1 && degree[start] == 1;
            lines.push((chain, free_end && !both_free));
        }
    }
    // Closed loops of degree-2 pixels only.
    for (start, &on) in m.iter().enumerate() {
        if !on || is_node(start) {
            continue;
        }
        let Some(n0) = neighbours(start).into_iter().find(|&j| !used.contains(&(start, j))) else { continue };
        let mut chain = vec![start, n0];
        used.insert((start, n0));
        used.insert((n0, start));
        let (mut prev, mut cur) = (start, n0);
        while let Some(next) = neighbours(cur).into_iter().find(|&j| j != prev && !used.contains(&(cur, j))) {
            used.insert((cur, next));
            used.insert((next, cur));
            chain.push(next);
            prev = cur;
            cur = next;
        }
        lines.push((chain, false));
    }
    lines
        .into_iter()
        .filter(|(c, spur)| !(*spur && c.len() < min_branch.max(3)) && c.len() >= 2)
        .map(|(c, _)| c.into_iter().map(center).collect())
        .collect()
}

/// Trace `g` with `o`.
pub fn trace(g: &Gray, o: &TraceOptions) -> Result<Traced> {
    if g.width * g.height > MAX_PIXELS {
        return invalid(format!("the image has more than {MAX_PIXELS} pixels; scale it down to trace it"));
    }
    if !(0.0..=1.334).contains(&o.smoothness) || !(o.optimize.is_finite() && o.optimize >= 0.0) {
        return invalid("smoothness must be 0..1.33 and optimize 0 or more");
    }
    let (w, h) = (g.width, g.height);
    let mut m = mask(g, o);
    despeckle(&mut m, w, h, o.ignore_less_than);
    let tol = 0.5 + o.optimize;
    let mut path = BezPath::new();
    let mut nodes = 0;
    let mut count = 0;
    if o.centerline {
        thin(&mut m, w, h);
        for line in skeleton_lines(&m, w, h, o.ignore_less_than as usize) {
            let closed = line.len() > 3 && line.first().zip(line.last()).is_some_and(|(a, b)| (*a - *b).hypot() < 1.5);
            let simple = simplify_open(&line, tol);
            nodes += smooth_into(&mut path, &simple, false, o.smoothness);
            if closed {
                path.close_path();
            }
            count += 1;
        }
    } else {
        for c in contours(&m, w, h) {
            let simple = simplify_closed(&c, tol);
            nodes += smooth_into(&mut path, &simple, true, o.smoothness);
            count += 1;
        }
    }
    Ok(Traced { path, contours: count, nodes })
}

/// A vector from pixel-edge coordinates to the image's local mm frame (centred, +Y up).
pub fn pixel_to_local(width_px: usize, height_px: usize, width_mm: f64, height_mm: f64) -> kurbo::Affine {
    let (sx, sy) = (width_mm / width_px as f64, height_mm / height_px as f64);
    kurbo::Affine::new([sx, 0.0, 0.0, -sy, -width_mm / 2.0, height_mm / 2.0])
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn gray(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Gray {
        Gray { width: w, height: h, value: (0..w * h).map(|i| f(i % w, i / w)).collect(), alpha: vec![255; w * h] }
    }

    #[test]
    fn a_square_traces_to_four_corners_without_smoothing() {
        let g = gray(20, 20, |x, y| if (5..15).contains(&x) && (5..15).contains(&y) { 0 } else { 255 });
        let t = trace(&g, &TraceOptions { smoothness: 0.0, ..TraceOptions::default() }).unwrap();
        assert_eq!(t.contours, 1);
        let b = t.path.bounding_box();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (5.0, 5.0, 15.0, 15.0));
        assert!((t.path.area().abs() - 100.0).abs() < 1e-9);
        // Smoothed fully, the square's corners still stay corners only below 4/3.
        let curvy = trace(&g, &TraceOptions { smoothness: 1.334, ..TraceOptions::default() }).unwrap();
        assert!(curvy.path.elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(..))));
    }

    #[test]
    fn a_disc_becomes_a_smooth_curve_and_holes_stay_holes() {
        let g = gray(60, 60, |x, y| {
            let d = ((x as f64 - 29.5).powi(2) + (y as f64 - 29.5).powi(2)).sqrt();
            if d < 25.0 && d > 10.0 { 0 } else { 255 }
        });
        let t = trace(&g, &TraceOptions::default()).unwrap();
        assert_eq!(t.contours, 2);
        let area = t.path.area().abs();
        let expected = std::f64::consts::PI * (25.0f64.powi(2) - 10.0f64.powi(2));
        assert!((area - expected).abs() / expected < 0.03, "{area} vs {expected}");
        assert!(t.path.elements().iter().filter(|e| matches!(e, kurbo::PathEl::CurveTo(..))).count() > 8);
        // Thresholds: a light grey ring is not traced at 128 but is with threshold 200.
        let light = gray(10, 10, |x, _| if x > 4 { 180 } else { 255 });
        assert_eq!(trace(&light, &TraceOptions::default()).unwrap().contours, 0);
        assert_eq!(trace(&light, &TraceOptions { threshold: 200, ..TraceOptions::default() }).unwrap().contours, 1);
        // Cutoff excludes the darkest.
        assert_eq!(trace(&light, &TraceOptions { threshold: 200, cutoff: 190, ..TraceOptions::default() }).unwrap().contours, 0);
    }

    #[test]
    fn speckles_are_ignored_and_diagonals_join() {
        let g = gray(20, 20, |x, y| if (x, y) == (3, 3) || (10..16).contains(&x) && (10..16).contains(&y) { 0 } else { 255 });
        assert_eq!(trace(&g, &TraceOptions { ignore_less_than: 1, ..TraceOptions::default() }).unwrap().contours, 2);
        assert_eq!(trace(&g, &TraceOptions { ignore_less_than: 2, ..TraceOptions::default() }).unwrap().contours, 1);
        // Two pixels touching at a corner make one outline.
        let diag = gray(4, 4, |x, y| if (x, y) == (1, 1) || (x, y) == (2, 2) { 0 } else { 255 });
        let m = mask(&diag, &TraceOptions::default());
        assert_eq!(contours(&m, 4, 4).len(), 1);
    }

    #[test]
    fn boundary_transparency_and_sketch() {
        let g = gray(20, 10, |x, _| if x < 10 { 0 } else { 255 });
        let t = trace(&g, &TraceOptions { boundary_px: Some([0.0, 0.0, 5.0, 10.0]), smoothness: 0.0, ..TraceOptions::default() }).unwrap();
        assert_eq!(t.path.bounding_box().x1, 5.0);
        let mut ga = gray(10, 10, |_, _| 255);
        ga.alpha = (0..100).map(|i| if i % 10 < 5 { 255 } else { 0 }).collect();
        assert_eq!(trace(&ga, &TraceOptions { transparency: true, ..TraceOptions::default() }).unwrap().contours, 1);
        // Sketch: a dark line on an uneven background is found, the gradient is not.
        let sk = gray(60, 60, |x, y| if y == 30 { (x as u8).saturating_add(20) } else { 120 + x as u8 });
        let t = trace(&sk, &TraceOptions { sketch: true, ..TraceOptions::default() }).unwrap();
        assert_eq!(t.contours, 1);
    }

    #[test]
    fn center_line_of_a_thick_stroke_is_one_open_line() {
        // A 5-pixel-thick horizontal bar.
        let g = gray(40, 15, |x, y| if (5..35).contains(&x) && (5..10).contains(&y) { 0 } else { 255 });
        let t = trace(&g, &TraceOptions { centerline: true, ..TraceOptions::default() }).unwrap();
        assert_eq!(t.contours, 1, "{:?}", t.path);
        let b = t.path.bounding_box();
        assert!(b.height() < 1.5 && b.width() > 20.0, "{b:?}");
        assert!(!t.path.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)));
        // A ring's centre line is closed.
        let ring = gray(40, 40, |x, y| {
            let d = ((x as f64 - 19.5).powi(2) + (y as f64 - 19.5).powi(2)).sqrt();
            if (10.0..14.0).contains(&d) { 0 } else { 255 }
        });
        let t = trace(&ring, &TraceOptions { centerline: true, ..TraceOptions::default() }).unwrap();
        assert!(t.contours >= 1);
        let b = t.path.bounding_box();
        assert!((b.width() - 24.0).abs() < 3.0, "{b:?}");
    }

    /// The classic full-scan Zhang-Suen, the reference for the boundary-driven [`thin`].
    fn thin_full_scan(m: &mut [bool], w: usize, h: usize) {
        loop {
            let mut changed = false;
            for step in 0..2 {
                let del: Vec<usize> = (0..m.len())
                    .filter(|&i| m[i] && zs_deletes(&zs_neighbours(m, w, h, (i % w) as i64, (i / w) as i64), step))
                    .collect();
                changed |= !del.is_empty();
                for i in del {
                    m[i] = false;
                }
            }
            if !changed {
                break;
            }
        }
    }

    #[test]
    fn thinning_matches_the_full_scan_and_is_fast_on_large_areas() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for (w, h) in [(37, 23), (64, 64), (5, 90)] {
            for density in [30u64, 60, 85] {
                let mut m: Vec<bool> = (0..w * h)
                    .map(|_| {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        seed % 100 < density
                    })
                    .collect();
                let mut reference = m.clone();
                thin(&mut m, w, h);
                thin_full_scan(&mut reference, w, h);
                assert_eq!(m, reference, "{w}x{h} at {density} %");
            }
        }
        // A 2000 x 2000 solid square: about a thousand passes, each a full scan of 4 million pixels before.
        let t = std::time::Instant::now();
        let (w, h) = (2000, 2000);
        let mut m = vec![true; w * h];
        thin(&mut m, w, h);
        assert!(m.iter().filter(|v| **v).count() < 10_000);
        assert!(t.elapsed().as_secs() < 30, "{:?}", t.elapsed());
    }

    /// A staircase of a million points simplified without recursion (it overflowed the stack).
    #[test]
    fn long_contours_simplify_without_deep_recursion() {
        let pts: Vec<Point> = (0..1_000_000).map(|i| Point::new(f64::from(i / 2 + i % 2), f64::from(i / 2))).collect();
        let open = simplify_open(&pts, 0.6);
        assert_eq!(open.first(), pts.first());
        assert_eq!(open.last(), pts.last());
        // Every point stays within the tolerance of the simplified line, which keeps points in order.
        for pts in [pts.iter().take(20_000).copied().collect::<Vec<_>>(), (0..9).map(|i| Point::new(f64::from(i), if i % 2 == 0 { 0.0 } else { 0.7 })).collect()] {
            let s = simplify_open(&pts, 0.6);
            assert!(s.windows(2).all(|w| w[0].x <= w[1].x));
            for p in &pts {
                let d = s.windows(2).map(|w| seg_dist(*p, w[0], w[1])).fold(f64::INFINITY, f64::min);
                assert!(d <= 0.6 + 1e-9, "{p:?} is {d} away");
            }
        }
    }

    #[test]
    fn pixel_frame() {
        let t = pixel_to_local(100, 50, 10.0, 5.0);
        assert_eq!(t * Point::new(0.0, 0.0), Point::new(-5.0, 2.5));
        assert_eq!(t * Point::new(100.0, 50.0), Point::new(5.0, -2.5));
    }
}
