//! Per-pass cut options as polyline operations ([`doc::CutOptions`], LightBurn's layer options): dot width trim and
//! flood-fill regions of hatches, overlap removal, tabs, perforation, wobble and dot mode. All in field mm; every
//! function is pure and keeps the order of what it is given.

use crate::geom::{Point, dist};
use crate::hatch::HatchLine;
use crate::order::Chunk;

/// Cumulative arc length at each vertex.
fn arc_lengths(poly: &[Point]) -> Vec<f64> {
    let mut out = Vec::with_capacity(poly.len());
    let mut acc = 0.0;
    for (i, p) in poly.iter().enumerate() {
        if i > 0 {
            acc += dist(poly[i - 1], *p);
        }
        out.push(acc);
    }
    out
}

/// The point at arc length `s` (clamped to the polyline).
fn point_at(poly: &[Point], cum: &[f64], s: f64) -> Point {
    let i = cum.partition_point(|&c| c <= s).clamp(1, poly.len().max(2) - 1);
    if poly.len() < 2 {
        return poly[0];
    }
    let (a, b) = (poly[i - 1], poly[i]);
    let len = cum[i] - cum[i - 1];
    let t = if len > 0.0 { ((s - cum[i - 1]) / len).clamp(0.0, 1.0) } else { 0.0 };
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The part of `poly` between arc lengths `a < b`, with every vertex in between.
fn between(poly: &[Point], cum: &[f64], a: f64, b: f64) -> Vec<Point> {
    let mut out = vec![point_at(poly, cum, a)];
    for (i, &c) in cum.iter().enumerate() {
        if c > a && c < b {
            out.push(poly[i]);
        }
    }
    out.push(point_at(poly, cum, b));
    out
}

/// Fill dot width adjust: shorten both ends of every hatch line by `width_mm / 2`; lines not longer than the width
/// are dropped.
pub fn trim_hatch(lines: Vec<HatchLine>, width_mm: f64) -> Vec<HatchLine> {
    if width_mm <= 0.0 {
        return lines;
    }
    let h = width_mm / 2.0;
    lines
        .into_iter()
        .filter_map(|[a, b]| {
            let len = dist(a, b);
            if len <= width_mm {
                return None;
            }
            let (dx, dy) = ((b[0] - a[0]) / len * h, (b[1] - a[1]) / len * h);
            Some([[a[0] + dx, a[1] + dy], [b[0] - dx, b[1] - dy]])
        })
        .collect()
}

/// Flood fill: split one hatch (lines in scanline order, at `angle_deg`, `interval_mm` apart) into connected
/// regions, each a run of scanlines whose segments overlap the previous one's. Each region keeps scanline order;
/// with `bidirectional` its lines zig-zag again, else they all run the hatch's direction.
pub fn flood_regions(lines: &[HatchLine], angle_deg: f64, interval_mm: f64, bidirectional: bool) -> Vec<Vec<HatchLine>> {
    let (s, c) = angle_deg.to_radians().sin_cos();
    let along = |p: Point| p[0] * c + p[1] * s;
    let across = |p: Point| -p[0] * s + p[1] * c;
    struct Region {
        lines: Vec<HatchLine>,
        scan: i64,
        span: (f64, f64),
    }
    let mut regions: Vec<Region> = Vec::new();
    for &[a, b] in lines {
        let scan = (across(a) / interval_mm).round() as i64;
        let (u0, u1) = { let (x, y) = (along(a), along(b)); if x <= y { (x, y) } else { (y, x) } };
        // Forward along the hatch direction.
        let line = if along(a) <= along(b) { [a, b] } else { [b, a] };
        let found = regions.iter_mut().rev().find(|r| r.scan + 1 == scan && r.span.0 <= u1 && u0 <= r.span.1);
        match found {
            Some(r) => {
                r.lines.push(line);
                r.scan = scan;
                r.span = (u0, u1);
            }
            None => regions.push(Region { lines: vec![line], scan, span: (u0, u1) }),
        }
    }
    regions
        .into_iter()
        .map(|r| {
            r.lines
                .into_iter()
                .enumerate()
                .map(|(k, [a, b])| if bidirectional && k % 2 == 1 { [b, a] } else { [a, b] })
                .collect()
        })
        .collect()
}

/// Remove overlapping lines (LightBurn's "Remove Overlapping Lines"): every straight segment of the open and closed
/// chunks, in order, loses the parts that lie within `tol_mm` of an earlier kept segment running along it. A loop
/// that loses nothing stays a loop; the rest become open polylines. Hatch chunks are passed through.
pub fn remove_overlaps<T: Copy>(chunks: Vec<(Chunk, T)>, tol_mm: f64) -> Vec<(Chunk, T)> {
    let tol = tol_mm.max(1e-6);
    let mut kept: Vec<[Point; 2]> = Vec::new();
    let mut out = Vec::new();
    for (chunk, tag) in chunks {
        let (pts, closed) = match &chunk {
            Chunk::Open(p) => (p.clone(), false),
            Chunk::Closed(p) => {
                let mut q = p.clone();
                if let Some(&f) = p.first() {
                    q.push(f);
                }
                (q, true)
            }
            Chunk::Hatch { .. } => {
                out.push((chunk, tag));
                continue;
            }
        };
        let mut pieces: Vec<Vec<Point>> = Vec::new();
        let mut removed = false;
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = dist(a, b);
            if len == 0.0 {
                continue;
            }
            let free = uncovered(a, b, len, &kept, tol);
            if free.len() != 1 || free[0].0 > 0.0 || free[0].1 < len {
                removed = true;
            }
            let at = |t: f64| [a[0] + (b[0] - a[0]) * t / len, a[1] + (b[1] - a[1]) * t / len];
            for (t0, t1) in free {
                let (p, q) = (at(t0), at(t1));
                match pieces.last_mut() {
                    Some(last) if last.last().is_some_and(|l| dist(*l, p) < 1e-9) => last.push(q),
                    _ => pieces.push(vec![p, q]),
                }
            }
            kept.push([a, b]);
        }
        if !removed && closed {
            out.push((chunk, tag));
        } else {
            out.extend(pieces.into_iter().map(|p| (Chunk::Open(p), tag)));
        }
    }
    out
}

/// The parts `(t0, t1)` (arc lengths along `a -> b`) of segment `a b` not within `tol` of a kept segment that runs
/// along it (parallel within the tolerance over the overlap).
fn uncovered(a: Point, b: Point, len: f64, kept: &[[Point; 2]], tol: f64) -> Vec<(f64, f64)> {
    let (ux, uy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
    let mut covered: Vec<(f64, f64)> = Vec::new();
    for &[p, q] in kept {
        // Both ends of the kept segment within `tol` of the line through a b.
        let off = |r: Point| ((r[0] - a[0]) * uy - (r[1] - a[1]) * ux).abs();
        if off(p) > tol || off(q) > tol {
            continue;
        }
        let proj = |r: Point| (r[0] - a[0]) * ux + (r[1] - a[1]) * uy;
        let (s0, s1) = { let (x, y) = (proj(p), proj(q)); if x <= y { (x, y) } else { (y, x) } };
        let (s0, s1) = (s0.max(0.0), s1.min(len));
        if s1 - s0 > tol {
            covered.push((s0, s1));
        }
    }
    covered.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut free = Vec::new();
    let mut at = 0.0;
    for (s0, s1) in covered {
        if s0 > at + tol {
            free.push((at, s0));
        }
        at = at.max(s1);
    }
    if len > at + tol {
        free.push((at, len));
    }
    free
}

/// Tabs: leave `count` gaps of `size_mm` (or one every `spacing_mm` when that is positive) evenly along a closed
/// polyline, the first half a spacing from its start. Returns the marked pieces; an open polyline is returned as is.
pub fn tabs(poly: Vec<Point>, closed: bool, count: u32, spacing_mm: f64, size_mm: f64) -> Vec<Vec<Point>> {
    if !closed || poly.len() < 2 {
        return vec![poly];
    }
    let cum = arc_lengths(&poly);
    let total = *cum.last().unwrap_or(&0.0);
    let n = if spacing_mm > 0.0 { (total / spacing_mm).floor().max(1.0) } else { f64::from(count.max(1)) };
    let step = total / n;
    if size_mm >= step {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Gaps centred at (k + 0.5) * step; marks in between.
    let mut start = 0.0;
    for k in 0..n as usize {
        let centre = (k as f64 + 0.5) * step;
        let (g0, g1) = (centre - size_mm / 2.0, centre + size_mm / 2.0);
        if g0 > start {
            out.push(between(&poly, &cum, start, g0));
        }
        start = g1;
    }
    if total > start {
        out.push(between(&poly, &cum, start, total));
    }
    out
}

/// Perforation: mark `cut_mm`, skip `skip_mm`, repeating from the start of the polyline.
pub fn perforate(poly: &[Point], cut_mm: f64, skip_mm: f64) -> Vec<Vec<Point>> {
    if poly.len() < 2 {
        return vec![poly.to_vec()];
    }
    let cum = arc_lengths(poly);
    let total = *cum.last().unwrap_or(&0.0);
    let mut out = Vec::new();
    let mut s = 0.0;
    while s < total {
        let e = (s + cut_mm).min(total);
        if e > s {
            out.push(between(poly, &cum, s, e));
        }
        s = e + skip_mm;
    }
    out
}

/// Wobble: the beam circles (diameter `size_mm`, centred on the path) once every `step_mm` of travel along the
/// polyline (16 to 64 points per circle).
pub fn wobble(poly: &[Point], step_mm: f64, size_mm: f64) -> Vec<Point> {
    if poly.len() < 2 {
        return poly.to_vec();
    }
    let cum = arc_lengths(poly);
    let total = *cum.last().unwrap_or(&0.0);
    let r = size_mm / 2.0;
    let per_turn = 16.0_f64.max((std::f64::consts::PI * size_mm / 0.05).ceil()).min(64.0);
    let ds = step_mm / per_turn;
    let n = ((total / ds).ceil() as usize).max(1);
    (0..=n)
        .map(|k| {
            let s = (k as f64 * ds).min(total);
            let c = point_at(poly, &cum, s);
            let phase = 2.0 * std::f64::consts::PI * s / step_mm;
            [c[0] + r * phase.cos(), c[1] + r * phase.sin()]
        })
        .collect()
}

/// Dot mode: one point every `spacing_mm` along the polyline, starting at its first point (each becomes a dot).
pub fn dots(poly: &[Point], spacing_mm: f64) -> Vec<Point> {
    if poly.len() < 2 {
        return poly.to_vec();
    }
    let cum = arc_lengths(poly);
    let total = *cum.last().unwrap_or(&0.0);
    let n = (total / spacing_mm).floor() as usize;
    (0..=n).map(|k| point_at(poly, &cum, k as f64 * spacing_mm)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::polyline_length;

    fn square() -> Vec<Point> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]]
    }

    #[test]
    fn hatch_ends_are_trimmed() {
        let t = trim_hatch(vec![[[0.0, 0.0], [10.0, 0.0]], [[0.0, 1.0], [0.1, 1.0]]], 0.2);
        assert_eq!(t, vec![[[0.1, 0.0], [9.9, 0.0]]]);
    }

    #[test]
    fn flood_fill_splits_disconnected_regions() {
        // Two columns of short lines on the same scanlines (a U-shaped gap between them).
        let mut lines = Vec::new();
        for i in 0..4 {
            let y = f64::from(i);
            lines.push([[0.0, y], [2.0, y]]);
            lines.push([[5.0, y], [7.0, y]]);
        }
        let r = flood_regions(&lines, 0.0, 1.0, true);
        assert_eq!(r.len(), 2);
        assert!(r[0].iter().all(|l| l[0][0] < 3.0 && l[1][0] < 3.0));
        assert_eq!(r[0][1], [[2.0, 1.0], [0.0, 1.0]], "zig-zag again inside a region");
    }

    #[test]
    fn overlapping_edges_are_removed_once() {
        // Two squares sharing the edge x = 10.
        let a = Chunk::Closed(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        let b = Chunk::Closed(vec![[10.0, 0.0], [20.0, 0.0], [20.0, 10.0], [10.0, 10.0]]);
        let out = remove_overlaps(vec![(a.clone(), 0), (b, 1)], 0.01);
        assert_eq!(out[0], (a, 0), "the first is untouched");
        let total: f64 = out[1..].iter().map(|(c, _)| if let Chunk::Open(p) = c { polyline_length(p) } else { 0.0 }).sum();
        assert!((total - 30.0).abs() < 1e-9, "the second loses its shared edge: {total}");
        // A partial overlap keeps the rest of the segment.
        let c = Chunk::Open(vec![[5.0, 0.0], [15.0, 0.0]]);
        let out = remove_overlaps(vec![(Chunk::Open(vec![[0.0, 0.0], [10.0, 0.0]]), 0), (c, 1)], 0.01);
        assert_eq!(out[1], (Chunk::Open(vec![[10.0, 0.0], [15.0, 0.0]]), 1));
    }

    #[test]
    fn tabs_leave_evenly_spaced_gaps() {
        let pieces = tabs(square(), true, 4, 0.0, 1.0);
        let marked: f64 = pieces.iter().map(|p| polyline_length(p)).sum();
        assert!((marked - 36.0).abs() < 1e-9, "{marked}");
        assert_eq!(pieces.len(), 5, "4 gaps split the loop into 5 runs");
        let pieces = tabs(square(), true, 0, 20.0, 1.0);
        assert!((pieces.iter().map(|p| polyline_length(p)).sum::<f64>() - 38.0).abs() < 1e-9);
        assert_eq!(tabs(vec![[0.0, 0.0], [1.0, 0.0]], false, 4, 0.0, 0.1).len(), 1, "open lines get no tabs");
    }

    #[test]
    fn perforation_alternates_cut_and_skip() {
        let p = perforate(&[[0.0, 0.0], [10.0, 0.0]], 2.0, 1.0);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], vec![[0.0, 0.0], [2.0, 0.0]]);
        assert_eq!(p[3], vec![[9.0, 0.0], [10.0, 0.0]]);
    }

    #[test]
    fn wobble_circles_around_the_line() {
        let w = wobble(&[[0.0, 0.0], [10.0, 0.0]], 1.0, 0.5);
        assert!(w.len() > 160);
        assert!(w.iter().all(|p| p[1].abs() <= 0.25 + 1e-9));
        assert!((w.last().unwrap()[0] - 10.0).abs() < 0.3);
    }

    #[test]
    fn dots_are_spaced_along_the_path() {
        let d = dots(&square(), 5.0);
        assert_eq!(d.len(), 9);
        assert_eq!(d[1], [5.0, 0.0]);
        assert_eq!(d[3], [10.0, 5.0]);
    }
}
