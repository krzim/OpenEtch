//! Curve fitting for LightBurn's Optimize Selected Shapes: "Fit shapes to arcs" (runs of short lines that lie on a
//! circle within the tolerance become true circular arcs, drawn as cubics) and "Smooth source shapes" (runs of lines
//! become a tangent-continuous chain of cubic Béziers that stays within the tolerance of every original point,
//! Schneider's "An Algorithm for Automatically Fitting Digitized Curves", Graphics Gems, 1990).
//!
//! Both work only on runs of straight segments (as LightBurn's tool does: curves are kept as they are), keep every
//! end of a run where it is, and leave a run alone when fitting would not help. Work per run is bounded.

use kurbo::{Arc, CubicBez, Line, ParamCurve, ParamCurveDeriv, PathSeg, Point, Vec2};

use crate::EPS;
use crate::ops::arc_segments;
use crate::subpath::{SubPath, with_end, with_start};

/// A unit vector along `self`, when it has a length.
trait PipeNorm {
    fn pipe_norm(self) -> Option<Vec2>;
}

impl PipeNorm for Vec2 {
    fn pipe_norm(self) -> Option<Vec2> {
        let l = self.hypot();
        (l > EPS && l.is_finite()).then(|| self / l)
    }
}

/// Runs longer than this many points are split into chunks before fitting, so fitting stays roughly linear.
const MAX_RUN: usize = 4000;

/// Fewest line segments one fitted arc replaces.
const MIN_ARC_SEGMENTS: usize = 4;

/// Radii above this (mm) are straight for arc fitting.
const MAX_ARC_RADIUS: f64 = 1e5;

/// Turns sharper than this (radians, about 60°) between lines longer than the tolerance are kept as corners when
/// smoothing.
const CORNER_TURN: f64 = 1.05;

/// The runs of a subpath: `(is_line_run, segments)`, in order, covering every segment once. A closed subpath that is
/// all lines is one run starting at its first corner.
fn runs(sub: &SubPath) -> Vec<(bool, Vec<PathSeg>)> {
    let mut segs = sub.segs.clone();
    if sub.closed
        && let Some(c) = segs.iter().position(|s| !matches!(s, PathSeg::Line(_)))
    {
        // Start at a curve so no run of lines wraps around the start.
        segs.rotate_left(c);
    }
    let mut out: Vec<(bool, Vec<PathSeg>)> = Vec::new();
    for s in segs {
        let line = matches!(s, PathSeg::Line(_));
        match out.last_mut() {
            Some((l, run)) if *l == line && (!line || run.len() < MAX_RUN) => run.push(s),
            _ => out.push((line, vec![s])),
        }
    }
    out
}

fn reassemble(sub: &SubPath, runs: Vec<Vec<PathSeg>>) -> SubPath {
    let mut segs: Vec<PathSeg> = runs.into_iter().flatten().collect();
    for i in 1..segs.len() {
        let p = segs[i - 1].end();
        segs[i] = with_start(segs[i], p);
    }
    if sub.closed && !segs.is_empty() {
        let n = segs.len() - 1;
        segs[n] = with_end(segs[n], segs[0].start());
    }
    SubPath { segs, closed: sub.closed }
}

fn points_of(run: &[PathSeg]) -> Vec<Point> {
    let mut pts = vec![run[0].start()];
    pts.extend(run.iter().map(PathSeg::end));
    pts
}

/// Circle through three points, `None` when they are (almost) collinear.
fn circle3(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (a.to_vec2().hypot2(), b.to_vec2().hypot2(), c.to_vec2().hypot2());
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let centre = Point::new(ux, uy);
    let r = centre.distance(a);
    (r.is_finite() && r <= MAX_ARC_RADIUS).then_some((centre, r))
}

/// The arc through `pts[0]`, `pts[mid]` and `pts[last]`, when every point and every chord's midpoint lies within
/// `tol` of it and the points advance around it in one direction; returns `(centre, radius, start angle, sweep)`.
fn arc_fit(pts: &[Point], tol: f64) -> Option<(Point, f64, f64, f64)> {
    let n = pts.len();
    let (a, b, c) = (pts[0], pts[n / 2], pts[n - 1]);
    let (centre, r) = circle3(a, b, c)?;
    let ang = |p: Point| (p - centre).atan2();
    let dir = (b - a).cross(c - b).signum();
    let mut sweep = 0.0;
    for w in pts.windows(2) {
        if ((w[0].distance(centre)) - r).abs() > tol {
            return None;
        }
        let mut d = ang(w[1]) - ang(w[0]);
        while d > std::f64::consts::PI {
            d -= std::f64::consts::TAU;
        }
        while d < -std::f64::consts::PI {
            d += std::f64::consts::TAU;
        }
        if d * dir <= 0.0 {
            return None;
        }
        // The chord's sagitta: how far its middle is from the arc.
        let mid = w[0].midpoint(w[1]);
        if (r - mid.distance(centre)).abs() > tol {
            return None;
        }
        sweep += d;
    }
    if (pts[n - 1].distance(centre) - r).abs() > tol || sweep.abs() >= std::f64::consts::TAU - 1e-6 {
        return None;
    }
    Some((centre, r, ang(a), sweep))
}

/// "Fit shapes to arcs": every run of at least 4 lines of `sub` whose points lie on a circle within `tolerance` mm
/// becomes a circular arc (cubics) between the same end points. Greedy from the start of each run, each arc as long
/// as it can be. Returns the new subpath and how many arcs were fitted.
pub fn fit_arcs(sub: &SubPath, tolerance: f64) -> (SubPath, usize) {
    let tol = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
    if tol <= 0.0 {
        return (sub.clone(), 0);
    }
    let mut fitted = 0;
    let mut pieces = Vec::new();
    for (line, run) in runs(sub) {
        if !line || run.len() < MIN_ARC_SEGMENTS {
            pieces.push(run);
            continue;
        }
        let pts = points_of(&run);
        let mut out: Vec<PathSeg> = Vec::new();
        let mut i = 0;
        while i + 1 < pts.len() {
            // Longest j with pts[i..=j] fitting an arc (grown one point at a time; the fit is re-checked in full).
            let mut best: Option<(usize, (Point, f64, f64, f64))> = None;
            let mut j = i + MIN_ARC_SEGMENTS;
            while j < pts.len() {
                match arc_fit(&pts[i..=j], tol) {
                    Some(f) => best = Some((j, f)),
                    None if best.is_some() => break,
                    None => {}
                }
                if best.is_none() {
                    break;
                }
                j += 1;
            }
            match best {
                Some((j, (centre, r, start, sweep))) => {
                    let arc = Arc::new(centre, (r, r), start, sweep, 0.0);
                    out.extend(arc_segments(&arc, pts[i], pts[j]));
                    fitted += 1;
                    i = j;
                }
                None => {
                    out.push(PathSeg::Line(Line::new(pts[i], pts[i + 1])));
                    i += 1;
                }
            }
        }
        pieces.push(out);
    }
    if fitted == 0 {
        return (sub.clone(), 0);
    }
    (reassemble(sub, pieces), fitted)
}

/// "Smooth source shapes": every run of lines of `sub` becomes cubic Béziers that pass within `tolerance` mm of
/// every one of its points, tangent-continuous except at real corners (turns over about 60° between lines longer
/// than the tolerance). Larger tolerances smooth more (and round off small details). Returns the new subpath and how
/// many runs were smoothed.
pub fn smooth(sub: &SubPath, tolerance: f64) -> (SubPath, usize) {
    let tol = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
    if tol <= 0.0 {
        return (sub.clone(), 0);
    }
    let all_lines = sub.segs.iter().all(|s| matches!(s, PathSeg::Line(_)));
    let mut smoothed = 0;
    let mut pieces = Vec::new();
    for (line, run) in runs(sub) {
        if !line || run.len() < 2 {
            pieces.push(run);
            continue;
        }
        let mut pts = points_of(&run);
        let ring = sub.closed && all_lines && pieces.is_empty() && run.len() == sub.segs.len();
        // A closed ring of lines: start at a corner if it has one, else smooth across the start too.
        let mut wrap_tangent = None;
        if ring {
            pts.pop();
            let n = pts.len();
            let corner = (0..n).find(|&k| is_corner(pts[(k + n - 1) % n], pts[k], pts[(k + 1) % n], tol));
            match corner {
                Some(k) => pts.rotate_left(k),
                None => wrap_tangent = (pts[1] - pts[n - 1]).pipe_norm(),
            }
            let first = pts[0];
            pts.push(first);
        }
        let mut out = Vec::new();
        let mut start = 0;
        for k in 1..pts.len() {
            let corner = k + 1 == pts.len() || is_corner(pts[k - 1], pts[k], pts[k + 1], tol);
            if corner {
                let part = &pts[start..=k];
                let t1 = if start == 0 { wrap_tangent } else { None }.or_else(|| (part[1] - part[0]).pipe_norm());
                let t2 = if k + 1 == pts.len() { wrap_tangent.map(|t| -t) } else { None }
                    .or_else(|| (part[part.len() - 2] - part[part.len() - 1]).pipe_norm());
                match (t1, t2) {
                    (Some(t1), Some(t2)) if part.len() > 2 => fit_cubic(part, t1, t2, tol, 0, &mut out),
                    _ => out.extend(part.windows(2).map(|w| PathSeg::Line(Line::new(w[0], w[1])))),
                }
                start = k;
            }
        }
        if out.len() < run.len() || out.iter().any(|s| matches!(s, PathSeg::Cubic(_))) {
            smoothed += 1;
            pieces.push(out);
        } else {
            pieces.push(run);
        }
    }
    if smoothed == 0 {
        return (sub.clone(), 0);
    }
    (reassemble(sub, pieces), smoothed)
}

fn is_corner(a: Point, b: Point, c: Point, tol: f64) -> bool {
    let (u, v) = (b - a, c - b);
    if u.hypot() <= tol * 2.0 || v.hypot() <= tol * 2.0 {
        return false;
    }
    u.cross(v).atan2(u.dot(v)).abs() > CORNER_TURN
}

fn bernstein(u: f64) -> [f64; 4] {
    let m = 1.0 - u;
    [m * m * m, 3.0 * u * m * m, 3.0 * u * u * m, u * u * u]
}

fn chord_params(pts: &[Point]) -> Vec<f64> {
    let mut u = vec![0.0];
    for w in pts.windows(2) {
        let last = *u.last().unwrap_or(&0.0);
        u.push(last + w[0].distance(w[1]));
    }
    let total = *u.last().unwrap_or(&1.0);
    if total <= EPS {
        return (0..pts.len()).map(|i| i as f64 / (pts.len() - 1).max(1) as f64).collect();
    }
    u.iter().map(|v| v / total).collect()
}

/// Least-squares cubic with end tangents `t1` (out of the start) and `t2` (into the end, pointing backwards).
fn generate(pts: &[Point], u: &[f64], t1: Vec2, t2: Vec2) -> CubicBez {
    let (p0, p3) = (pts[0], pts[pts.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (p, &ui) in pts.iter().zip(u) {
        let b = bernstein(ui);
        let (a0, a1) = (t1 * b[1], t2 * b[2]);
        c00 += a0.dot(a0);
        c01 += a0.dot(a1);
        c11 += a1.dot(a1);
        let tmp = *p - (p0.to_vec2() * (b[0] + b[1]) + p3.to_vec2() * (b[2] + b[3])).to_point();
        x0 += a0.dot(tmp);
        x1 += a1.dot(tmp);
    }
    let det = c00 * c11 - c01 * c01;
    let seg = p0.distance(p3);
    let (mut al, mut ar) =
        if det.abs() > 1e-12 { ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det) } else { (0.0, 0.0) };
    if !(al.is_finite() && ar.is_finite()) || al < 1e-6 * seg || ar < 1e-6 * seg {
        al = seg / 3.0;
        ar = seg / 3.0;
    }
    CubicBez::new(p0, p0 + t1 * al, p3 + t2 * ar, p3)
}

/// The largest distance of a point from the curve at its parameter, and where.
fn max_error(c: &CubicBez, pts: &[Point], u: &[f64]) -> (f64, usize) {
    let mut best = (0.0, pts.len() / 2);
    for (i, (p, &ui)) in pts.iter().zip(u).enumerate().skip(1).take(pts.len().saturating_sub(2)) {
        let d = c.eval(ui).distance(*p);
        if d > best.0 {
            best = (d, i);
        }
    }
    best
}

/// One Newton step per parameter towards the nearest point of the curve.
fn reparameterize(c: &CubicBez, pts: &[Point], u: &[f64]) -> Vec<f64> {
    let d1 = c.deriv();
    let d2 = d1.deriv();
    pts.iter()
        .zip(u)
        .map(|(p, &ui)| {
            let q = c.eval(ui) - *p;
            let q1 = d1.eval(ui).to_vec2();
            let q2 = d2.eval(ui).to_vec2();
            let den = q1.dot(q1) + q.dot(q2);
            if den.abs() < 1e-12 { ui } else { (ui - q.dot(q1) / den).clamp(0.0, 1.0) }
        })
        .collect()
}

fn fit_cubic(pts: &[Point], t1: Vec2, t2: Vec2, tol: f64, depth: usize, out: &mut Vec<PathSeg>) {
    if pts.len() == 2 || depth > 24 {
        if pts.len() == 2 {
            let d = pts[0].distance(pts[1]) / 3.0;
            out.push(PathSeg::Cubic(CubicBez::new(pts[0], pts[0] + t1 * d, pts[1] + t2 * d, pts[1])));
        } else {
            out.extend(pts.windows(2).map(|w| PathSeg::Line(Line::new(w[0], w[1]))));
        }
        return;
    }
    let mut u = chord_params(pts);
    let mut c = generate(pts, &u, t1, t2);
    let (mut err, mut split) = max_error(&c, pts, &u);
    if err <= tol {
        out.push(PathSeg::Cubic(c));
        return;
    }
    if err <= tol * 4.0 {
        for _ in 0..6 {
            u = reparameterize(&c, pts, &u);
            c = generate(pts, &u, t1, t2);
            (err, split) = max_error(&c, pts, &u);
            if err <= tol {
                out.push(PathSeg::Cubic(c));
                return;
            }
        }
    }
    let split = split.clamp(1, pts.len() - 2);
    let tc = (pts[split - 1] - pts[split + 1]).pipe_norm().unwrap_or_else(|| -t1);
    fit_cubic(&pts[..=split], t1, tc, tol, depth + 1, out);
    fit_cubic(&pts[split..], -tc, t2, tol, depth + 1, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subpath::{join, split};
    use kurbo::{BezPath, ParamCurveNearest, Shape};

    /// A polyline approximation of a circle arc (or a whole circle when `closed`).
    fn polyline_arc(r: f64, from: f64, to: f64, n: usize, closed: bool) -> SubPath {
        let mut p = BezPath::new();
        for i in 0..=n {
            if closed && i == n {
                break;
            }
            let a = from + (to - from) * i as f64 / n as f64;
            let q = Point::new(r * a.cos(), r * a.sin());
            if i == 0 { p.move_to(q) } else { p.line_to(q) }
        }
        if closed {
            p.close_path();
        }
        split(&p).remove(0)
    }

    fn max_dev(a: &SubPath, b: &SubPath) -> f64 {
        let flat = crate::flatten::flatten(&join(std::slice::from_ref(a)), 0.001).unwrap();
        flat.iter()
            .flat_map(|l| l.points.clone())
            .map(|p| b.segs.iter().map(|s| s.nearest(p, 1e-9).distance_sq.sqrt()).fold(f64::MAX, f64::min))
            .fold(0.0, f64::max)
    }

    #[test]
    fn segmented_arc_becomes_a_true_arc() {
        let sub = polyline_arc(10.0, 0.0, std::f64::consts::FRAC_PI_2, 40, false);
        let (f, n) = fit_arcs(&sub, 0.05);
        assert_eq!(n, 1);
        assert!(f.segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))));
        assert!(f.segs.len() <= 2, "{}", f.segs.len());
        assert_eq!(f.start(), sub.start());
        assert!(f.end().distance(sub.end()) < 1e-12);
        // On the true circle.
        for t in [0.1, 0.5, 0.9] {
            assert!((f.segs[0].eval(t).to_vec2().hypot() - 10.0).abs() < 1e-3);
        }
        // A whole segmented circle: arcs all round, area close to the circle's.
        let ring = polyline_arc(5.0, 0.0, std::f64::consts::TAU, 72, true);
        let (f, n) = fit_arcs(&ring, 0.02);
        assert!(n >= 1 && f.closed);
        assert!((join(&[f]).area().abs() - std::f64::consts::PI * 25.0).abs() < 0.2);
        // Straight lines are not arcs; too few segments are left alone.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        for i in 1..10 {
            p.line_to((i as f64, 0.0));
        }
        assert_eq!(fit_arcs(&split(&p)[0], 0.05).1, 0);
        assert_eq!(fit_arcs(&polyline_arc(10.0, 0.0, 1.0, 3, false), 0.05).1, 0);
    }

    #[test]
    fn mixed_run_keeps_corners_and_fits_the_arc_part() {
        // A line, then a segmented semicircle, then a line: a slot end.
        let mut p = BezPath::new();
        p.move_to((-20.0, -5.0));
        p.line_to((0.0, -5.0));
        for i in 1..=30 {
            let a = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 30.0;
            p.line_to((5.0 * a.cos(), 5.0 * a.sin()));
        }
        p.line_to((-20.0, 5.0));
        let sub = split(&p).remove(0);
        let (f, n) = fit_arcs(&sub, 0.01);
        assert_eq!(n, 1);
        assert!(matches!(f.segs[0], PathSeg::Line(_)) && matches!(f.segs[f.segs.len() - 1], PathSeg::Line(_)));
        assert!(max_dev(&f, &sub) < 0.02, "{}", max_dev(&f, &sub));
    }

    #[test]
    fn smoothing_stays_within_tolerance_and_keeps_corners() {
        // A jagged wave: a sine sampled with alternating noise.
        let mut p = BezPath::new();
        for i in 0..=100 {
            let x = i as f64 * 0.5;
            let y = (x / 5.0).sin() * 5.0 + if i % 2 == 0 { 0.03 } else { -0.03 };
            if i == 0 { p.move_to((x, y)) } else { p.line_to((x, y)) }
        }
        let sub = split(&p).remove(0);
        let (f, n) = smooth(&sub, 0.1);
        assert_eq!(n, 1);
        assert!(f.segs.len() < 20, "{}", f.segs.len());
        assert!(f.segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))));
        // Every original point lies within the tolerance of the result.
        for q in crate::flatten::flatten(&join(std::slice::from_ref(&sub)), 0.001).unwrap()[0].points.iter() {
            let d = f.segs.iter().map(|s| s.nearest(*q, 1e-9).distance_sq.sqrt()).fold(f64::MAX, f64::min);
            assert!(d <= 0.1 + 1e-6, "{q:?} {d}");
        }
        // Tangent-continuous at the joints.
        for w in f.segs.windows(2) {
            let (PathSeg::Cubic(a), PathSeg::Cubic(b)) = (w[0], w[1]) else { panic!() };
            let (u, v) = ((a.p3 - a.p2).normalize(), (b.p1 - b.p0).normalize());
            assert!(u.cross(v).abs() < 1e-6 && u.dot(v) > 0.0);
        }
        // A square keeps its four corners (each side is one curve or line between them).
        let sq = split(&kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01)).remove(0);
        let (f, _) = smooth(&sq, 0.5);
        assert_eq!(f.segs.len(), 4);
        assert!((join(&[f]).area().abs() - 100.0).abs() < 1e-6);
        // A closed segmented circle smooths into a few curves.
        let ring = polyline_arc(5.0, 0.0, std::f64::consts::TAU, 64, true);
        let (f, n) = smooth(&ring, 0.05);
        assert_eq!(n, 1);
        assert!(f.closed && f.segs.len() < 16, "{}", f.segs.len());
        assert!((join(&[f]).area().abs() - std::f64::consts::PI * 25.0).abs() < 0.5);
    }
}
