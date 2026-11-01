//! Curve-preserving path tools: break apart, auto-join, close, reverse, duplicate segments, optimize, fillet, trim.

use std::collections::{HashMap, HashSet};

use kurbo::{
    Arc, BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, ParamCurveNearest, PathEl, PathSeg, Point, Vec2,
};

pub use crate::fillet::{fillet, sharp_corners};
use crate::flatten::{flatten, point_in_ring, polygon_area};
use crate::subpath::{SubPath, join, slice, split, with_end, with_start};
use crate::{EPS, Result, TOLERANCE, invalid};

/// Each segment as its own open path (LightBurn's Break Apart of a single shape: a rectangle becomes four lines).
pub fn segments(path: &BezPath) -> Vec<BezPath> {
    split(path).into_iter().flat_map(|s| s.segs).map(|seg| join(&[SubPath { segs: vec![seg], closed: false }])).collect()
}

/// Each subpath as its own path.
pub fn subpaths(path: &BezPath) -> Vec<BezPath> {
    split(path).into_iter().map(|s| join(&[s])).collect()
}

/// Closed subpaths grouped into islands by even-odd nesting (an outer contour with the holes directly inside it, as
/// the shape is filled), plus every open subpath on its own. Order follows the outer contours' order.
pub fn islands(path: &BezPath) -> Result<Vec<BezPath>> {
    let subs = split(path);
    let mut rings: Vec<Option<(Vec<Point>, kurbo::Rect, f64)>> = Vec::with_capacity(subs.len());
    for s in &subs {
        let ring = if s.closed {
            flatten(&join(std::slice::from_ref(s)), TOLERANCE)?.into_iter().next().filter(|l| l.closed).map(|l| {
                let b = l.points.iter().fold(kurbo::Rect::from_points(l.points[0], l.points[0]), |r, p| r.union_pt(*p));
                let a = polygon_area(&l.points).abs();
                (l.points, b, a)
            })
        } else {
            None
        };
        rings.push(ring);
    }
    let n = subs.len();
    let contains = |j: usize, i: usize| -> bool {
        let (Some((rj, bj, aj)), Some((ri, bi, ai))) = (&rings[j], &rings[i]) else { return false };
        aj > ai && bj.contains(bi.origin()) && bj.contains(Point::new(bi.x1, bi.y1)) && point_in_ring(ri[0], rj)
    };
    let depth: Vec<usize> = (0..n).map(|i| (0..n).filter(|&j| j != i && contains(j, i)).count()).collect();
    let mut holes_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        if rings[i].is_some() && depth[i] % 2 == 1 {
            let parent = (0..n)
                .filter(|&j| j != i && depth[j] + 1 == depth[i] && contains(j, i))
                .min_by(|&a, &b| {
                    let area = |k: usize| rings[k].as_ref().map_or(f64::MAX, |r| r.2);
                    area(a).total_cmp(&area(b))
                });
            if let Some(p) = parent {
                holes_of.entry(p).or_default().push(i);
                continue;
            }
        }
    }
    let assigned: HashSet<usize> = holes_of.values().flatten().copied().collect();
    let mut out = Vec::new();
    for i in 0..n {
        if assigned.contains(&i) {
            continue;
        }
        let mut group = vec![subs[i].clone()];
        for &h in holes_of.get(&i).into_iter().flatten() {
            group.push(subs[h].clone());
        }
        out.push(join(&group));
    }
    Ok(out)
}

/// An open or closed subpath tagged with where it came from (e.g. the index of its object).
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub tag: usize,
    pub sub: SubPath,
}

/// The result of [`auto_join`]: a subpath and the tags of every piece in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Joined {
    pub tags: Vec<usize>,
    pub sub: SubPath,
    /// How many joins made it (0 = untouched).
    pub joins: usize,
    /// Whether the join closed it.
    pub closed_by_join: bool,
}

/// Join open pieces whose end points lie within `tolerance` mm of each other into longer subpaths (reversing pieces
/// as needed; the attached piece's end node is moved onto the chain's). A chain whose two ends meet within the
/// tolerance is closed. Closed pieces pass through unchanged. Nearest end points are joined first.
pub fn auto_join(pieces: Vec<Piece>, tolerance: f64) -> Vec<Joined> {
    let tol = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
    let cell = tol.max(1e-6);
    let mut out = Vec::new();
    let mut open: Vec<Piece> = Vec::new();
    for p in pieces {
        if p.sub.closed {
            out.push(Joined { tags: vec![p.tag], sub: p.sub, joins: 0, closed_by_join: false });
        } else {
            open.push(p);
        }
    }
    let key = |p: Point| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, p) in open.iter().enumerate() {
        grid.entry(key(p.sub.start())).or_default().push((i, false));
        grid.entry(key(p.sub.end())).or_default().push((i, true));
    }
    let mut used = vec![false; open.len()];
    // The nearest unused end point within the tolerance: (piece, is_end).
    let find = |at: Point, used: &[bool]| -> Option<(usize, bool)> {
        let (kx, ky) = key(at);
        let mut best: Option<(f64, usize, bool)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &(i, is_end) in grid.get(&(kx + dx, ky + dy)).into_iter().flatten() {
                    if used[i] {
                        continue;
                    }
                    let q = if is_end { open[i].sub.end() } else { open[i].sub.start() };
                    let d = q.distance(at);
                    if d <= tol && best.is_none_or(|b| d < b.0) {
                        best = Some((d, i, is_end));
                    }
                }
            }
        }
        best.map(|(_, i, e)| (i, e))
    };
    for i in 0..open.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut chain = open[i].sub.clone();
        let mut tags = vec![open[i].tag];
        let mut joins = 0;
        while let Some((j, is_end)) = find(chain.end(), &used) {
            used[j] = true;
            let mut s = open[j].sub.clone();
            if is_end {
                s = s.reversed();
            }
            s.segs[0] = with_start(s.segs[0], chain.end());
            chain.segs.extend(s.segs);
            tags.push(open[j].tag);
            joins += 1;
        }
        while let Some((j, is_end)) = find(chain.start(), &used) {
            used[j] = true;
            let mut s = open[j].sub.clone();
            if !is_end {
                s = s.reversed();
            }
            let n = s.segs.len() - 1;
            s.segs[n] = with_end(s.segs[n], chain.start());
            s.segs.extend(chain.segs);
            chain.segs = s.segs;
            tags.insert(0, open[j].tag);
            joins += 1;
        }
        let mut closed_by_join = false;
        if chain.start().distance(chain.end()) <= tol.max(EPS) && closable(&chain) {
            let n = chain.segs.len() - 1;
            chain.segs[n] = with_end(chain.segs[n], chain.start());
            chain.closed = true;
            closed_by_join = true;
        }
        out.push(Joined { tags, sub: chain, joins, closed_by_join });
    }
    out
}

/// Whether closing this open subpath gives a real loop (not a single straight line folded onto itself).
fn closable(s: &SubPath) -> bool {
    s.segs.len() >= 2 || !matches!(s.segs[0], PathSeg::Line(_))
}

/// How [`close`] closes a gap (LightBurn's Close Selected Paths With Tolerance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseMode {
    /// Add a straight line from the end to the start.
    JoinWithLine,
    /// Move the end nodes onto their midpoint, adding no line.
    MoveEndsTogether,
}

/// Close an open subpath whose ends are within `tolerance` mm (any distance when `None`). Returns whether it closed.
pub fn close(sub: &mut SubPath, tolerance: Option<f64>, mode: CloseMode) -> bool {
    if sub.closed {
        return false;
    }
    let gap = sub.start().distance(sub.end());
    if tolerance.is_some_and(|t| gap > t) {
        return false;
    }
    let n = sub.segs.len() - 1;
    match mode {
        _ if gap <= EPS => {
            if !closable(sub) {
                return false;
            }
            sub.segs[n] = with_end(sub.segs[n], sub.start());
        }
        CloseMode::JoinWithLine => {
            if sub.segs.len() == 1 && matches!(sub.segs[0], PathSeg::Line(_)) {
                return false;
            }
            sub.segs.push(PathSeg::Line(Line::new(sub.end(), sub.start())));
        }
        CloseMode::MoveEndsTogether => {
            if !closable(sub) {
                return false;
            }
            let m = sub.start().midpoint(sub.end());
            sub.segs[0] = with_start(sub.segs[0], m);
            sub.segs[n] = with_end(sub.segs[n], m);
        }
    }
    sub.closed = true;
    true
}

/// Remove segments that repeat an earlier segment (same kind and points within `tolerance` mm, in either direction),
/// across all `paths` in order: the first occurrence stays. A subpath that loses segments falls apart into open
/// runs. Returns the new paths and how many segments were removed.
pub fn remove_duplicate_segments(paths: &[Vec<SubPath>], tolerance: f64) -> (Vec<Vec<SubPath>>, usize) {
    let q = tolerance.max(1e-9);
    let quant = |p: Point| ((p.x / q).round() as i64, (p.y / q).round() as i64);
    let key = |s: &PathSeg| -> Vec<(i64, i64)> {
        let fwd: Vec<(i64, i64)> = match *s {
            PathSeg::Line(l) => vec![quant(l.p0), quant(l.p1)],
            PathSeg::Quad(c) => vec![quant(c.p0), quant(c.p1), quant(c.p2)],
            PathSeg::Cubic(c) => vec![quant(c.p0), quant(c.p1), quant(c.p2), quant(c.p3)],
        };
        let rev: Vec<(i64, i64)> = fwd.iter().rev().copied().collect();
        fwd.min(rev)
    };
    let mut seen: HashSet<Vec<(i64, i64)>> = HashSet::new();
    let mut removed = 0;
    let mut out = Vec::with_capacity(paths.len());
    for subs in paths {
        let mut new_subs = Vec::new();
        for s in subs {
            let keep: Vec<bool> = s.segs.iter().map(|seg| seen.insert(key(seg))).collect();
            let gone = keep.iter().filter(|k| !**k).count();
            removed += gone;
            if gone == 0 {
                new_subs.push(s.clone());
                continue;
            }
            let n = s.segs.len();
            // For a closed subpath start right after a removed segment, so no run wraps around.
            let start = if s.closed { keep.iter().position(|k| !k).map_or(0, |p| (p + 1) % n) } else { 0 };
            let mut run: Vec<PathSeg> = Vec::new();
            for k in 0..n {
                let i = (start + k) % n;
                if keep[i] {
                    run.push(s.segs[i]);
                } else if !run.is_empty() {
                    new_subs.push(SubPath { segs: std::mem::take(&mut run), closed: false });
                }
            }
            if !run.is_empty() {
                new_subs.push(SubPath { segs: run, closed: false });
            }
        }
        out.push(new_subs);
    }
    (out, removed)
}

/// Distance from `p` to the segment `a`-`b`.
fn dist_to_segment(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let len2 = ab.hypot2();
    if len2 <= 0.0 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// A curve within `tol` of its chord (control points close to the chord and not beyond its ends) as a line.
fn flat_curve(seg: &PathSeg, tol: f64) -> Option<Line> {
    let (a, b, ctrl): (Point, Point, Vec<Point>) = match *seg {
        PathSeg::Line(_) => return None,
        PathSeg::Quad(q) => (q.p0, q.p2, vec![q.p1]),
        PathSeg::Cubic(c) => (c.p0, c.p3, vec![c.p1, c.p2]),
    };
    if a.distance(b) <= EPS {
        return None;
    }
    ctrl.iter().all(|&c| dist_to_segment(c, a, b) <= tol).then(|| Line::new(a, b))
}

fn is_zero_length(seg: &PathSeg) -> bool {
    match *seg {
        PathSeg::Line(l) => l.p0.distance(l.p1) <= EPS,
        PathSeg::Quad(q) => [q.p1, q.p2].iter().all(|p| p.distance(q.p0) <= EPS),
        PathSeg::Cubic(c) => [c.p1, c.p2, c.p3].iter().all(|p| p.distance(c.p0) <= EPS),
    }
}

/// Douglas-Peucker: indices of `pts` to keep (first and last always) so no dropped point is further than `tol` from
/// the kept polyline.
fn douglas_peucker(pts: &[Point], tol: f64) -> Vec<usize> {
    let n = pts.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (mut best, mut far) = (0.0, a);
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = dist_to_segment(*p, pts[a], pts[b]);
            if d > best {
                best = d;
                far = i;
            }
        }
        if best > tol {
            keep[far] = true;
            stack.push((a, far));
            stack.push((far, b));
        }
    }
    (0..n).filter(|&i| keep[i]).collect()
}

/// Node counts before and after [`optimize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptimizeStats {
    pub nodes_before: usize,
    pub nodes_after: usize,
    pub curves_to_lines: usize,
}

/// Remove redundant nodes within `tolerance` mm: zero-length segments go, curves that are straight within the
/// tolerance become lines, and runs of lines are simplified (collinear and near-collinear nodes merged,
/// Douglas-Peucker). Curves are otherwise kept as they are; arcs are not fitted. A closed subpath may start at
/// another node afterwards. A subpath that would collapse is returned unchanged.
pub fn optimize(sub: &SubPath, tolerance: f64) -> (SubPath, OptimizeStats) {
    let tol = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
    let before = sub.node_count();
    let mut curves_to_lines = 0;
    let mut segs: Vec<PathSeg> = Vec::new();
    for s in &sub.segs {
        if is_zero_length(s) {
            continue;
        }
        if let Some(l) = flat_curve(s, tol) {
            curves_to_lines += 1;
            segs.push(PathSeg::Line(l));
        } else {
            segs.push(*s);
        }
    }
    let unchanged = (sub.clone(), OptimizeStats { nodes_before: before, nodes_after: before, curves_to_lines: 0 });
    if segs.is_empty() {
        return unchanged;
    }
    for i in 1..segs.len() {
        let p = segs[i - 1].end();
        segs[i] = with_start(segs[i], p);
    }
    let closed = sub.closed;
    if closed {
        let n = segs.len() - 1;
        segs[n] = with_end(segs[n], segs[0].start());
        // Start at a curve, so no run of lines wraps around the start.
        if let Some(c) = segs.iter().position(|s| !matches!(s, PathSeg::Line(_))) {
            segs.rotate_left(c);
        }
    }
    let all_lines = segs.iter().all(|s| matches!(s, PathSeg::Line(_)));
    let mut out: Vec<PathSeg> = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if !matches!(segs[i], PathSeg::Line(_)) {
            out.push(segs[i]);
            i += 1;
            continue;
        }
        let mut j = i;
        let mut pts = vec![segs[i].start()];
        while j < segs.len() && matches!(segs[j], PathSeg::Line(_)) {
            pts.push(segs[j].end());
            j += 1;
        }
        let kept = douglas_peucker(&pts, tol);
        for w in kept.windows(2) {
            out.push(PathSeg::Line(Line::new(pts[w[0]], pts[w[1]])));
        }
        i = j;
    }
    // A closed ring of lines: the start node itself may be redundant.
    if closed && all_lines && out.len() > 3 {
        let n = out.len();
        let (prev, here, next) = (out[n - 1].start(), out[0].start(), out[0].end());
        if dist_to_segment(here, prev, next) <= tol {
            out[0] = PathSeg::Line(Line::new(prev, next));
            out.pop();
        }
    }
    let ok = if closed { out.len() >= 2 || out.iter().any(|s| !matches!(s, PathSeg::Line(_))) } else { !out.is_empty() };
    if !ok {
        return unchanged;
    }
    let res = SubPath { segs: out, closed };
    let after = res.node_count();
    (res, OptimizeStats { nodes_before: before, nodes_after: after, curves_to_lines })
}

/// Turn angle (radians, 0 = straight on) at node `k` of a subpath between two line segments, with the segments'
/// directions, or `None` when the node is not between two lines.
pub(crate) fn line_corner(sub: &SubPath, k: usize) -> Option<(Vec2, Vec2, f64, f64, f64)> {
    let n = sub.segs.len();
    let (inc, out) = if sub.closed {
        ((k + n - 1) % n, k % n)
    } else {
        if k == 0 || k >= n {
            return None;
        }
        (k - 1, k)
    };
    if inc == out {
        return None;
    }
    let (PathSeg::Line(a), PathSeg::Line(b)) = (sub.segs[inc], sub.segs[out]) else { return None };
    let (la, lb) = (a.p0.distance(a.p1), b.p0.distance(b.p1));
    if la <= EPS || lb <= EPS {
        return None;
    }
    let u = (a.p1 - a.p0) / la;
    let v = (b.p1 - b.p0) / lb;
    let turn = u.cross(v).atan2(u.dot(v));
    Some((u, v, turn, la, lb))
}

/// The nodes of `sub` that are corners between two lines turning by more than 1° (the polygon generator rounds these;
/// a fillet of "all" rounds every [`sharp_corners`] node, curves included).
pub fn line_corners(sub: &SubPath) -> Vec<usize> {
    (0..sub.node_count()).filter(|&k| line_corner(sub, k).is_some_and(|c| c.2.abs() > 1f64.to_radians())).collect()
}

/// Why [`fillet`] left a corner alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub node: usize,
    pub reason: String,
}

pub(crate) fn arc_segments(arc: &Arc, from: Point, to: Point) -> Vec<PathSeg> {
    let mut segs = Vec::new();
    let mut last = from;
    for el in arc.append_iter(1e-4) {
        if let PathEl::CurveTo(a, b, p) = el {
            segs.push(PathSeg::Cubic(CubicBez::new(last, a, b, p)));
            last = p;
        }
    }
    if let Some(n) = segs.len().checked_sub(1) {
        segs[n] = with_end(segs[n], to);
    }
    segs
}

/// What [`trim`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Trimmed {
    /// The subpaths after the trim (the trimmed one replaced by what is left of it, possibly nothing).
    pub subs: Vec<SubPath>,
    /// Index (into the input) of the subpath that was trimmed.
    pub subpath: usize,
    /// The intersections that bounded the removed piece (none when the whole subpath went).
    pub cut_at: Vec<Point>,
    /// The removed piece.
    pub removed: SubPath,
}

/// Sample points of a segment with their positions `index + t`.
fn sample(seg: &PathSeg, index: usize, out: &mut Vec<(Point, f64)>) {
    let n = match seg {
        PathSeg::Line(_) => 1,
        _ => ((seg.arclen(1e-3) / 0.1).ceil() as usize).clamp(8, 256),
    };
    for k in 0..n {
        let t = k as f64 / n as f64;
        out.push((seg.eval(t), index as f64 + t));
    }
}

fn cross(a: Vec2, b: Vec2) -> f64 {
    a.cross(b)
}

/// Parameters `(λ, μ)` where chords `a-b` and `c-d` cross (both in 0..=1), if they do and are not parallel.
fn chord_hit(a: Point, b: Point, c: Point, d: Point) -> Option<(f64, f64)> {
    let (r, s) = (b - a, d - c);
    let den = cross(r, s);
    if den.abs() <= 1e-15 * r.hypot().max(1.0) * s.hypot().max(1.0) {
        return None;
    }
    let lam = cross(c - a, s) / den;
    let mu = cross(c - a, r) / den;
    let e = 1e-9;
    ((-e..=1.0 + e).contains(&lam) && (-e..=1.0 + e).contains(&mu)).then_some((lam.clamp(0.0, 1.0), mu))
}

/// Remove the piece of the subpath nearest `at` (within `pick` mm) between the nearest intersections on either side
/// of the point (LightBurn's Trim): intersections with `others` (polylines in the same frame), the object's other
/// subpaths and the subpath itself. With no intersection on a side, the piece runs to that end of an open
/// subpath; a subpath with no intersections at all (or a closed one with only one) is removed entirely. At most
/// 200,000 sample chords are compared with themselves.
pub fn trim(subs: &[SubPath], others: &[Vec<Point>], at: Point, pick: f64) -> Result<Trimmed> {
    let mut best: Option<(f64, usize, f64)> = None;
    for (i, s) in subs.iter().enumerate() {
        for (j, seg) in s.segs.iter().enumerate() {
            let near = seg.nearest(at, 1e-9);
            let d = near.distance_sq.sqrt();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, i, j as f64 + near.t));
            }
        }
    }
    let Some((d, si, click)) = best else { return invalid("the shape has no segments to trim") };
    if d > pick {
        return invalid(format!("no line of the shape within {pick} mm of the point (nearest is {d:.3} mm away)"));
    }
    let sub = &subs[si];
    let mut samples = Vec::new();
    for (j, seg) in sub.segs.iter().enumerate() {
        sample(seg, j, &mut samples);
    }
    samples.push((sub.end(), sub.segs.len() as f64));
    let chords = samples.len() - 1;
    if chords > 200_000 {
        return invalid("the shape is too detailed to trim");
    }
    let bbox = samples.iter().fold(kurbo::Rect::from_points(samples[0].0, samples[0].0), |r, p| r.union_pt(p.0));
    let mut hits: Vec<(f64, Point)> = Vec::new();
    let mut lines: Vec<&[Point]> = others.iter().map(Vec::as_slice).collect();
    let mut own: Vec<Vec<Point>> = Vec::new();
    for (i, s) in subs.iter().enumerate() {
        if i != si {
            let mut pts = Vec::new();
            for (j, seg) in s.segs.iter().enumerate() {
                sample(seg, j, &mut pts);
            }
            let mut p: Vec<Point> = pts.into_iter().map(|x| x.0).collect();
            p.push(s.end());
            own.push(p);
        }
    }
    lines.extend(own.iter().map(Vec::as_slice));
    for line in lines {
        for w in line.windows(2) {
            let (c, dd) = (w[0], w[1]);
            let cb = kurbo::Rect::from_points(c, dd);
            if cb.x1 < bbox.x0 || cb.x0 > bbox.x1 || cb.y1 < bbox.y0 || cb.y0 > bbox.y1 {
                continue;
            }
            for k in 0..chords {
                let (a, b) = (samples[k], samples[k + 1]);
                if let Some((lam, _)) = chord_hit(a.0, b.0, c, dd) {
                    hits.push((a.1 + (b.1 - a.1) * lam, a.0 + (b.0 - a.0) * lam));
                }
            }
        }
    }
    // Self-intersections between chords that are not neighbours.
    if chords <= 20_000 {
        for p in 0..chords {
            for q in p + 2..chords {
                if sub.closed && p == 0 && q == chords - 1 {
                    continue;
                }
                let (a, b, c, dd) = (samples[p], samples[p + 1], samples[q], samples[q + 1]);
                if let Some((lam, mu)) = chord_hit(a.0, b.0, c.0, dd.0) {
                    let pt = a.0 + (b.0 - a.0) * lam;
                    hits.push((a.1 + (b.1 - a.1) * lam, pt));
                    hits.push((c.1 + (dd.1 - c.1) * mu.clamp(0.0, 1.0), pt));
                }
            }
        }
    }
    let total = sub.segs.len() as f64;
    hits.retain(|h| h.0.is_finite());
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.dedup_by(|a, b| (a.0 - b.0).abs() <= 1e-9);
    if !sub.closed {
        // Hits at the very ends do not bound anything.
        hits.retain(|h| h.0 > 1e-9 && h.0 < total - 1e-9);
    }
    let below = hits.iter().rev().find(|h| h.0 < click - 1e-9).copied();
    let above = hits.iter().find(|h| h.0 > click + 1e-9).copied();
    let mut rest: Vec<SubPath> = Vec::new();
    let (removed, cut_at) = if sub.closed {
        let (lo, hi) = match (below, above) {
            (Some(lo), Some(hi)) => (lo, hi),
            (None, Some(hi)) => (*hits.last().unwrap_or(&hi), hi),
            (Some(lo), None) => (lo, hits[0]),
            (None, None) => (( -1.0, at), (-1.0, at)),
        };
        if hits.len() < 2 || (lo.0 - hi.0).abs() <= 1e-9 {
            (sub.clone(), vec![])
        } else {
            // Keep hi → (wrap) → lo; remove lo → hi.
            let keep = wrap_slice(sub, hi.0, lo.0);
            let gone = wrap_slice(sub, lo.0, hi.0);
            if !keep.is_empty() {
                rest.push(SubPath { segs: keep, closed: false });
            }
            (SubPath { segs: gone, closed: false }, vec![lo.1, hi.1])
        }
    } else {
        match (below, above) {
            (None, None) => (sub.clone(), vec![]),
            _ => {
                let lo = below.map_or(0.0, |h| h.0);
                let hi = above.map_or(total, |h| h.0);
                let head = slice(&sub.segs, 0.0, lo);
                let tail = slice(&sub.segs, hi, total);
                for piece in [head, tail] {
                    if !piece.is_empty() {
                        rest.push(SubPath { segs: piece, closed: false });
                    }
                }
                let cuts = below.into_iter().chain(above).map(|h| h.1).collect();
                (SubPath { segs: slice(&sub.segs, lo, hi), closed: false }, cuts)
            }
        }
    };
    let mut out: Vec<SubPath> = Vec::with_capacity(subs.len() + 1);
    for (i, s) in subs.iter().enumerate() {
        if i == si {
            out.extend(rest.iter().cloned());
        } else {
            out.push(s.clone());
        }
    }
    Ok(Trimmed { subs: out, subpath: si, cut_at, removed })
}

/// Of a closed subpath, the run from position `a` forward to `b`, wrapping past the start when `b <= a`.
fn wrap_slice(sub: &SubPath, a: f64, b: f64) -> Vec<PathSeg> {
    let n = sub.segs.len() as f64;
    let mut out = if b > a {
        slice(&sub.segs, a, b)
    } else {
        let mut v = slice(&sub.segs, a, n);
        v.extend(slice(&sub.segs, 0.0, b));
        v
    };
    for i in 1..out.len() {
        let p = out[i - 1].end();
        out[i] = with_start(out[i], p);
    }
    out
}

/// Every subpath reversed (the order of subpaths is kept).
pub fn reverse(subs: &[SubPath]) -> Vec<SubPath> {
    subs.iter().map(SubPath::reversed).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn line(a: (f64, f64), b: (f64, f64)) -> SubPath {
        SubPath { segs: vec![PathSeg::Line(Line::new(a, b))], closed: false }
    }

    fn rect_sub(w: f64, h: f64) -> SubPath {
        split(&kurbo::Rect::new(0.0, 0.0, w, h).to_path(0.01)).remove(0)
    }

    #[test]
    fn break_apart_modes() {
        let r = kurbo::Rect::new(0.0, 0.0, 4.0, 2.0).to_path(0.01);
        assert_eq!(segments(&r).len(), 4);
        let mut p = kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
        p.extend(kurbo::Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.01)); // hole
        p.extend(kurbo::Rect::new(4.0, 4.0, 6.0, 6.0).to_path(0.01)); // island in the hole
        p.extend(kurbo::Rect::new(20.0, 0.0, 30.0, 10.0).to_path(0.01));
        p.move_to((40.0, 0.0));
        p.line_to((50.0, 0.0));
        assert_eq!(subpaths(&p).len(), 5);
        let isl = islands(&p).unwrap();
        assert_eq!(isl.len(), 4, "outer+hole, inner island, second square, open line");
        assert_eq!(split(&isl[0]).len(), 2);
    }

    #[test]
    fn auto_join_chains_and_closes() {
        // Four loose sides of a square, one reversed and one a little off.
        let pieces = vec![
            Piece { tag: 0, sub: line((0.0, 0.0), (10.0, 0.0)) },
            Piece { tag: 1, sub: line((10.0, 10.0), (10.0, 0.02)) },
            Piece { tag: 2, sub: line((10.0, 10.0), (0.0, 10.0)) },
            Piece { tag: 3, sub: line((0.0, 10.0), (0.0, 0.0)) },
            Piece { tag: 4, sub: line((50.0, 0.0), (60.0, 0.0)) },
        ];
        let j = auto_join(pieces, 0.05);
        assert_eq!(j.len(), 2);
        assert!(j[0].sub.closed && j[0].closed_by_join);
        assert_eq!(j[0].sub.segs.len(), 4);
        assert_eq!(j[0].joins, 3);
        let mut tags = j[0].tags.clone();
        tags.sort_unstable();
        assert_eq!(tags, [0, 1, 2, 3]);
        assert!(!j[1].sub.closed && j[1].joins == 0);
        // Outside the tolerance nothing joins.
        let j = auto_join(vec![Piece { tag: 0, sub: line((0.0, 0.0), (1.0, 0.0)) }, Piece { tag: 1, sub: line((1.1, 0.0), (2.0, 0.0)) }], 0.05);
        assert_eq!(j.len(), 2);
        assert!(auto_join(vec![], 1.0).is_empty());
    }

    #[test]
    fn close_with_and_without_tolerance() {
        let mut s = SubPath { segs: vec![PathSeg::Line(Line::new((0.0, 0.0), (10.0, 0.0))), PathSeg::Line(Line::new((10.0, 0.0), (0.0, 0.1)))], closed: false };
        let mut t = s.clone();
        assert!(!close(&mut s, Some(0.05), CloseMode::JoinWithLine));
        assert!(close(&mut s, Some(0.2), CloseMode::JoinWithLine));
        assert_eq!(s.segs.len(), 3);
        assert!(close(&mut t, None, CloseMode::MoveEndsTogether));
        assert_eq!(t.segs.len(), 2);
        assert!(t.start().distance(t.end()) < 1e-12);
        let mut single = line((0.0, 0.0), (1.0, 0.0));
        assert!(!close(&mut single, None, CloseMode::JoinWithLine), "a single line cannot be closed");
    }

    #[test]
    fn duplicate_segments_go_and_rings_fall_apart() {
        let a = rect_sub(10.0, 10.0);
        let b = line((10.0, 0.0), (0.0, 0.0)); // the square's bottom, reversed
        let (out, removed) = remove_duplicate_segments(&[vec![a.clone()], vec![b], vec![a.clone()]], 1e-4);
        assert_eq!(removed, 5);
        assert!(out[1].is_empty() && out[2].is_empty());
        assert_eq!(out[0], vec![a]);
        let (out, _) = remove_duplicate_segments(&[vec![line((0.0, 0.0), (10.0, 0.0))], vec![rect_sub(10.0, 10.0)]], 1e-4);
        assert_eq!(out[1].len(), 1);
        assert!(!out[1][0].closed && out[1][0].segs.len() == 3);
    }

    #[test]
    fn optimize_merges_collinear_and_flat_curves() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        for i in 1..=10 {
            p.line_to((i as f64, if i % 2 == 0 { 0.0 } else { 0.001 }));
        }
        p.curve_to((11.0, 0.0), (12.0, 0.0), (13.0, 0.0));
        p.line_to((13.0, 0.0)); // zero length
        let s = split(&p).remove(0);
        let (o, st) = optimize(&s, 0.01);
        assert_eq!(o.segs.len(), 1, "{o:?}");
        assert_eq!(st.curves_to_lines, 1);
        assert_eq!((st.nodes_before, st.nodes_after), (12, 2));
        // A square with extra nodes on its sides.
        let mut q = BezPath::new();
        q.move_to((5.0, 0.0));
        for p in [(10.0, 0.0), (10.0, 5.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)] {
            q.line_to(p);
        }
        q.close_path();
        let (o, _) = optimize(&split(&q).remove(0), 0.01);
        assert_eq!(o.segs.len(), 4);
        assert!(o.closed);
        // Curves stay curves.
        let c = split(&kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01)).remove(0);
        let (oc, _) = optimize(&c, 0.01);
        assert_eq!(oc, c);
    }

    #[test]
    fn fillet_rounds_corners_that_fit() {
        let sq = rect_sub(10.0, 10.0);
        assert_eq!(line_corners(&sq), [0, 1, 2, 3]);
        let (f, applied, skipped) = fillet(&sq, &[0, 1, 2, 3], 2.0);
        assert_eq!(applied, [0, 1, 2, 3]);
        assert!(skipped.is_empty());
        let area = join(std::slice::from_ref(&f)).area().abs();
        let expect = 100.0 - 4.0 * (4.0 - std::f64::consts::PI);
        assert!((area - expect).abs() < 2e-3, "{area} vs {expect}");
        assert!(f.closed);
        let (_, applied, skipped) = fillet(&sq, &[0, 1], 6.0);
        assert!(applied.is_empty() && skipped.len() == 2, "6 mm on a 10 mm side shared by two corners does not fit");
        let (_, applied, _) = fillet(&sq, &[0], 6.0);
        assert_eq!(applied, [0]);
        let (b, applied, _) = fillet(&sq, &[2], -2.0);
        assert_eq!(applied, [2]);
        let area = join(&[b]).area().abs();
        assert!((area - (100.0 - std::f64::consts::PI)).abs() < 1e-3, "bite: {area}");
        let (_, _, skipped) = fillet(&sq, &[9], 1.0);
        assert_eq!(skipped[0].reason, "no such node");
        let open = SubPath { segs: vec![PathSeg::Line(Line::new((0.0, 0.0), (10.0, 0.0))), PathSeg::Line(Line::new((10.0, 0.0), (10.0, 10.0)))], closed: false };
        assert_eq!(line_corners(&open), [1]);
        let (f, applied, _) = fillet(&open, &[0, 1], 1.0);
        assert_eq!(applied, [1]);
        assert!(!f.closed && f.start() == Point::new(0.0, 0.0) && f.end() == Point::new(10.0, 10.0));
    }

    #[test]
    fn trim_between_intersections() {
        // A horizontal line crossed by two verticals: clicking the middle removes the middle.
        let h = vec![line((0.0, 0.0), (30.0, 0.0))];
        let others = vec![vec![Point::new(10.0, -5.0), Point::new(10.0, 5.0)], vec![Point::new(20.0, -5.0), Point::new(20.0, 5.0)]];
        let t = trim(&h, &others, Point::new(15.0, 0.1), 1.0).unwrap();
        assert_eq!(t.subs.len(), 2);
        assert!((t.subs[0].end().x - 10.0).abs() < 1e-9 && (t.subs[1].start().x - 20.0).abs() < 1e-9);
        // Clicking an end piece removes it to the end.
        let t = trim(&h, &others, Point::new(25.0, 0.0), 1.0).unwrap();
        assert_eq!(t.subs.len(), 1);
        assert!((t.subs[0].end().x - 20.0).abs() < 1e-9);
        // No intersections: the whole line goes.
        let t = trim(&h, &[], Point::new(5.0, 0.0), 1.0).unwrap();
        assert!(t.subs.is_empty());
        assert!(trim(&h, &others, Point::new(15.0, 9.0), 1.0).is_err(), "too far");
        // A square crossed by a line: the clicked side's piece goes, the rest stays one open path.
        let sq = vec![rect_sub(10.0, 10.0)];
        let cut = vec![vec![Point::new(5.0, -5.0), Point::new(5.0, 15.0)]];
        let t = trim(&sq, &cut, Point::new(0.0, 5.0), 1.0).unwrap();
        assert_eq!(t.subs.len(), 1);
        assert!(!t.subs[0].closed);
        assert!((t.subs[0].length() - 20.0).abs() < 1e-6, "{}", t.subs[0].length());
        assert!((t.removed.length() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn trim_curves_and_self_intersections() {
        let c = split(&kurbo::Circle::new((0.0, 0.0), 10.0).to_path(0.01));
        let cut = vec![vec![Point::new(-20.0, 0.0), Point::new(20.0, 0.0)]];
        let t = trim(&c, &cut, Point::new(0.0, 10.0), 1.0).unwrap();
        assert_eq!(t.subs.len(), 1);
        assert!(t.subs[0].segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))), "curves stay curves");
        assert!((t.subs[0].length() - std::f64::consts::PI * 10.0).abs() < 0.05);
        // A figure-eight trims at its own crossing.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((10.0, 0.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        let s = split(&p);
        let t = trim(&s, &[], Point::new(10.0, 5.0), 1.0).unwrap();
        assert_eq!(t.subs.len(), 1);
        assert!((t.removed.length() - (10.0 + 2.0 * 50f64.sqrt() / 1.0)).abs() < 1e-6, "{}", t.removed.length());
    }

    #[test]
    fn reverse_flips_direction() {
        let sq = rect_sub(10.0, 10.0);
        let r = reverse(std::slice::from_ref(&sq));
        assert!(join(&r).area() * join(&[sq]).area() < 0.0);
    }
}
