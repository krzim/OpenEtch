//! Un-rounding corners (LightBurn's Radius tool clicked on a rounded corner): a true circular arc between two sides
//! is removed and the two sides are extended until they meet, giving back the sharp corner.
//!
//! An arc is a run of consecutive curve segments that all lie on one circle (a fillet drawn by [`crate::fillet`], a
//! rounded rectangle's corner, an imported arc). Like LightBurn, only true arcs count: a free-form curve is not a
//! rounded corner (Optimize can rebuild arcs from imported polylines first). The sides keep their kind: a line is
//! lengthened, a Bézier is extended along its own polynomial (exactly undoing a fillet, which cut it with
//! `subsegment`). A negative-radius bite is undone the same way (its sides meet at the bite's centre).

use std::collections::{HashMap, HashSet};

use kurbo::{ParamCurve, PathSeg, Point, Vec2};

use crate::EPS;
use crate::subpath::{SubPath, with_end, with_start};

/// Farthest (mm) a side is extended to reach the corner.
const MAX_EXTEND_MM: f64 = 5000.0;

/// A circular arc on a subpath: segments `first, first + 1, ...` (`len` of them, wrapping round a closed subpath)
/// on one circle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArcRun {
    pub first: usize,
    pub len: usize,
    pub centre: Point,
    pub radius: f64,
}

impl ArcRun {
    /// The segment indices of the run, in order.
    pub fn segs(&self, n: usize) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).map(move |i| (self.first + i) % n)
    }

    fn contains(&self, n: usize, seg: usize) -> bool {
        self.segs(n).any(|s| s == seg)
    }
}

/// One corner [`unfillet`] made sharp.
#[derive(Debug, Clone, PartialEq)]
pub struct Unrounded {
    /// The removed arc (indices into the input subpath).
    pub arc: ArcRun,
    /// The new sharp corner.
    pub corner: Point,
}

/// An arc [`unfillet`] left alone, with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub arc: ArcRun,
    pub reason: String,
}

/// Circle through three points, `None` when they are (nearly) collinear.
fn circle3(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let (ab, ac) = (b - a, c - a);
    let d = 2.0 * ab.cross(ac);
    let scale = ab.hypot2().max(ac.hypot2());
    if d.abs() <= 1e-12 * scale {
        return None;
    }
    let ux = (ac.y * ab.hypot2() - ab.y * ac.hypot2()) / d;
    let uy = (ab.x * ac.hypot2() - ac.x * ab.hypot2()) / d;
    let centre = a + Vec2::new(ux, uy);
    let r = Vec2::new(ux, uy).hypot();
    r.is_finite().then_some((centre, r))
}

/// How far a point may be off the circle for a curve to count as a true arc: kurbo's own arcs (a quarter circle per
/// cubic in rounded rectangles: 0.027 % of the radius) and imported arcs pass, free-form curves do not.
fn arc_tolerance(r: f64) -> f64 {
    (2e-3 * r).clamp(1e-6, 0.05)
}

/// The circle a curve segment lies on, when it is a true arc (lines and free-form curves give `None`).
fn seg_circle(seg: &PathSeg) -> Option<(Point, f64)> {
    if matches!(seg, PathSeg::Line(_)) || seg.start().distance(seg.end()) <= EPS {
        return None;
    }
    let (centre, r) = circle3(seg.eval(0.0), seg.eval(0.5), seg.eval(1.0))?;
    if r > 1e5 {
        return None;
    }
    let tol = arc_tolerance(r);
    let on = [0.125, 0.25, 0.375, 0.625, 0.75, 0.875].iter().all(|&t| (seg.eval(t).distance(centre) - r).abs() <= tol);
    on.then_some((centre, r))
}

fn same_circle(a: (Point, f64), b: (Point, f64)) -> bool {
    let tol = 2.5 * arc_tolerance(a.1.max(b.1));
    a.0.distance(b.0) <= tol && (a.1 - b.1).abs() <= tol
}

/// Every maximal run of segments on one circle. A closed subpath that is all one circle has no runs (it has no
/// corner to sharpen).
pub fn arc_runs(sub: &SubPath) -> Vec<ArcRun> {
    let n = sub.segs.len();
    let circles: Vec<Option<(Point, f64)>> = sub.segs.iter().map(seg_circle).collect();
    let prev = |i: usize| if sub.closed { Some((i + n - 1) % n) } else { i.checked_sub(1) };
    let linked = |i: usize| match (prev(i).and_then(|p| circles[p]), circles[i]) {
        (Some(a), Some(b)) => prev(i) != Some(i) && same_circle(a, b),
        _ => false,
    };
    let mut runs = Vec::new();
    for (i, c) in circles.iter().enumerate() {
        let Some((centre, radius)) = *c else { continue };
        if linked(i) {
            continue;
        }
        let mut len = 1;
        while len < n && (sub.closed || i + len < n) && linked((i + len) % n) {
            len += 1;
        }
        runs.push(ArcRun { first: i, len, centre, radius });
    }
    runs
}

/// The arc run containing segment `seg`, if that segment is part of a true arc.
pub fn arc_at(sub: &SubPath, seg: usize) -> Option<ArcRun> {
    let n = sub.segs.len();
    arc_runs(sub).into_iter().find(|r| r.contains(n, seg))
}

/// The sides of an arc run: the segment before and the one after (`None` at the ends of an open subpath).
fn sides(sub: &SubPath, run: &ArcRun) -> Result<(usize, usize), String> {
    let n = sub.segs.len();
    if run.len >= n {
        return Err("a whole circle has no corner".into());
    }
    let last = run.first + run.len - 1;
    let before = if sub.closed { Some((run.first + n - 1) % n) } else { run.first.checked_sub(1) };
    let after = if sub.closed { Some((last + 1) % n) } else { (last + 1 < n).then_some(last + 1) };
    match (before, after) {
        (Some(a), Some(b)) if a != b => Ok((a, b)),
        (Some(_), Some(_)) => Err("the arc's two sides are the same segment".into()),
        _ => Err("the arc is at an end of an open path".into()),
    }
}

/// Derivative of a segment's polynomial at `t` (any `t`, also outside 0..1).
fn deriv(seg: &PathSeg, t: f64) -> Vec2 {
    let h = 1e-6;
    (seg.eval(t + h) - seg.eval(t - h)) / (2.0 * h)
}

/// Where side `a` extended past its end meets side `b` extended before its start: `(s, t, corner)` with
/// `a(s) == b(t)`, `s >= 1`, `t <= 0` (Béziers extended along their own polynomial).
fn corner(a: &PathSeg, b: &PathSeg) -> Option<(f64, f64, Point)> {
    let (pa, pb) = (a.eval(1.0), b.eval(0.0));
    let (da, db) = (deriv(a, 1.0), deriv(b, 0.0));
    let den = da.cross(db);
    if da.hypot() <= EPS || db.hypot() <= EPS || den.abs() <= 1e-9 * da.hypot() * db.hypot() {
        return None;
    }
    // The tangent lines' crossing: pa + da x == pb + db y.
    let w = pb - pa;
    let (x, y) = (w.cross(db) / den, w.cross(da) / den);
    if x < -1e-9 || y > 1e-9 {
        return None;
    }
    let (mut s, mut t) = (1.0 + x.max(0.0), y.min(0.0));
    let scale = 1.0 + pa.distance(pb);
    for _ in 0..50 {
        let f = a.eval(s) - b.eval(t);
        if f.hypot() <= 1e-11 * scale {
            break;
        }
        let (ja, jb) = (deriv(a, s), -deriv(b, t));
        let det = ja.cross(jb);
        if det.abs() <= 1e-18 {
            return None;
        }
        // Solve ja * dx + jb * dy = -f.
        let dx = (-f).cross(jb) / det;
        let dy = ja.cross(-f) / det;
        s += dx;
        t += dy;
        if !(s.is_finite() && t.is_finite()) {
            return None;
        }
    }
    let p = a.eval(s);
    let ok = p.distance(b.eval(t)) <= 1e-7 * scale
        && s >= 1.0 - 1e-9
        && t <= 1e-9
        && p.distance(pa) <= MAX_EXTEND_MM
        && p.distance(pb) <= MAX_EXTEND_MM;
    ok.then_some((s.max(1.0), t.min(0.0), p))
}

/// Make the given arcs sharp corners again: each arc is removed and its two sides are extended until they meet.
/// Arcs are skipped (with the reason) when they have no two different sides, the extended sides do not meet ahead
/// of both, or they overlap an arc already made sharp in this call (a side may be lengthened at each end once).
pub fn unfillet(sub: &SubPath, arcs: &[ArcRun]) -> (SubPath, Vec<Unrounded>, Vec<Kept>) {
    let n = sub.segs.len();
    let (mut done, mut kept) = (Vec::new(), Vec::new());
    let mut removed: HashSet<usize> = HashSet::new();
    let mut new_end: HashMap<usize, (f64, Point)> = HashMap::new();
    let mut new_start: HashMap<usize, (f64, Point)> = HashMap::new();
    for run in arcs {
        let (a, b) = match sides(sub, run) {
            Ok(s) => s,
            Err(reason) => {
                kept.push(Kept { arc: *run, reason });
                continue;
            }
        };
        let segs: Vec<usize> = run.segs(n).collect();
        let clash = segs.iter().any(|s| removed.contains(s) || new_end.contains_key(s) || new_start.contains_key(s))
            || removed.contains(&a)
            || removed.contains(&b)
            || new_end.contains_key(&a)
            || new_start.contains_key(&b);
        if clash {
            kept.push(Kept { arc: *run, reason: "it touches a corner already made sharp".into() });
            continue;
        }
        match corner(&sub.segs[a], &sub.segs[b]) {
            Some((s, t, p)) => {
                removed.extend(segs);
                new_end.insert(a, (s, p));
                new_start.insert(b, (t, p));
                done.push(Unrounded { arc: *run, corner: p });
            }
            None => kept.push(Kept { arc: *run, reason: "the sides do not meet when extended".into() }),
        }
    }
    if done.is_empty() {
        return (sub.clone(), done, kept);
    }
    let mut out = Vec::with_capacity(n);
    for (i, seg) in sub.segs.iter().enumerate() {
        if removed.contains(&i) {
            continue;
        }
        let (t0, start) = new_start.get(&i).map_or((0.0, None), |&(t, p)| (t, Some(p)));
        let (t1, end) = new_end.get(&i).map_or((1.0, None), |&(s, p)| (s, Some(p)));
        let mut s = if t0 == 0.0 && t1 == 1.0 { *seg } else { seg.subsegment(t0..t1) };
        if let Some(p) = start {
            s = with_start(s, p);
        }
        if let Some(p) = end {
            s = with_end(s, p);
        }
        out.push(s);
    }
    for i in 1..out.len() {
        let p = out[i - 1].end();
        out[i] = with_start(out[i], p);
    }
    if sub.closed && !out.is_empty() {
        let last = out.len() - 1;
        out[last] = with_end(out[last], out[0].start());
    }
    (SubPath { segs: out, closed: sub.closed }, done, kept)
}

/// Every arc of `sub` that can be made sharp, smallest radius first (fillets are usually the small arcs): what
/// "all rounded corners" means.
pub fn unfillet_all(sub: &SubPath) -> (SubPath, Vec<Unrounded>, Vec<Kept>) {
    let mut runs = arc_runs(sub);
    runs.sort_by(|a, b| a.radius.total_cmp(&b.radius));
    unfillet(sub, &runs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fillet::{fillet, sharp_corners};
    use crate::subpath::{join, split};
    use kurbo::{BezPath, Rect, RoundedRect, Shape};

    fn square() -> SubPath {
        split(&Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01)).remove(0)
    }

    fn same_shape(a: &SubPath, b: &SubPath) {
        assert_eq!(a.segs.len(), b.segs.len(), "{a:?}\n{b:?}");
        let pa: Vec<Point> = a.segs.iter().map(|s| s.start()).collect();
        for s in &b.segs {
            assert!(pa.iter().any(|p| p.distance(s.start()) < 1e-6), "{:?} not in {pa:?}", s.start());
        }
    }

    #[test]
    fn filleted_square_comes_back_sharp() {
        let sq = square();
        let (f, applied, _) = fillet(&sq, &sharp_corners(&sq), 2.0);
        assert_eq!(applied.len(), 4);
        let runs = arc_runs(&f);
        assert_eq!(runs.len(), 4, "{runs:?}");
        assert!(runs.iter().all(|r| (r.radius - 2.0).abs() < 1e-3));
        // One corner (click on its arc).
        let arc_seg = f.segs.iter().position(|s| !matches!(s, PathSeg::Line(_))).unwrap();
        let run = arc_at(&f, arc_seg).unwrap();
        let (one, done, kept) = unfillet(&f, &[run]);
        assert_eq!(done.len(), 1, "{kept:?}");
        assert_eq!(arc_runs(&one).len(), 3);
        let c = done[0].corner;
        assert!([(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)].iter().any(|&q| c.distance(q.into()) < 1e-9), "{c:?}");
        // All corners: the square again.
        let (back, done, _) = unfillet_all(&f);
        assert_eq!(done.len(), 4);
        same_shape(&back, &sq);
        assert!(back.segs.iter().all(|s| matches!(s, PathSeg::Line(_))));
        assert!((join(&[back]).area().abs() - 100.0).abs() < 1e-6);
    }

    #[test]
    fn bites_and_curve_sides_are_undone() {
        let sq = square();
        let (bite, _, _) = fillet(&sq, &[2], -3.0);
        let (back, done, _) = unfillet_all(&bite);
        assert_eq!(done.len(), 1);
        assert!(done[0].corner.distance(Point::new(10.0, 10.0)) < 1e-9);
        same_shape(&back, &sq);

        // A line into a curve: the curve is extended along its own polynomial, back to the original corner.
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.curve_to((10.0, 5.0), (15.0, 10.0), (20.0, 10.0));
        let lc = split(&p).remove(0);
        let (f, applied, _) = fillet(&lc, &[1], 2.0);
        assert_eq!(applied, vec![1]);
        let (back, done, kept) = unfillet_all(&f);
        assert_eq!(done.len(), 1, "{kept:?}");
        assert!(done[0].corner.distance(Point::new(10.0, 0.0)) < 1e-6, "{:?}", done[0].corner);
        assert_eq!(back.segs.len(), 2);
        if let PathSeg::Cubic(c) = back.segs[1] {
            for (got, want) in [(c.p1, (10.0, 5.0)), (c.p2, (15.0, 10.0)), (c.p3, (20.0, 10.0))] {
                assert!(got.distance(want.into()) < 1e-5, "{c:?}");
            }
        } else {
            panic!("the curve side stays a curve");
        }
    }

    #[test]
    fn rounded_rectangle_and_non_corners() {
        let rr = split(&RoundedRect::new(0.0, 0.0, 20.0, 10.0, 3.0).to_path(0.01)).remove(0);
        let (back, done, _) = unfillet_all(&rr);
        assert_eq!(done.len(), 4);
        assert!((join(&[back]).area().abs() - 200.0).abs() < 1e-6);
        // A circle has no corner; an ellipse has no true arc; a plain square has nothing to undo.
        let c = split(&kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01)).remove(0);
        assert!(arc_runs(&c).is_empty());
        let e = split(&kurbo::Ellipse::new((0.0, 0.0), (8.0, 3.0), 0.0).to_path(0.01)).remove(0);
        assert!(arc_runs(&e).is_empty());
        let (same, done, _) = unfillet_all(&square());
        assert!(done.is_empty() && same == square());
        // A "D": the arc's two sides are the same line.
        let mut d = BezPath::new();
        d.move_to((0.0, -5.0));
        d.extend(kurbo::Arc::new((0.0, 0.0), (5.0, 5.0), -std::f64::consts::FRAC_PI_2, std::f64::consts::PI, 0.0).append_iter(1e-4));
        d.close_path();
        let d = split(&d).remove(0);
        let (_, done, kept) = unfillet_all(&d);
        assert!(done.is_empty());
        assert!(kept[0].reason.contains("same segment"), "{kept:?}");
    }

    #[test]
    fn open_path_ends_and_parallel_sides() {
        // A U-turn: two parallel lines joined by a semicircle never meet.
        let mut u = BezPath::new();
        u.move_to((0.0, 0.0));
        u.line_to((10.0, 0.0));
        u.extend(kurbo::Arc::new((10.0, 5.0), (5.0, 5.0), -std::f64::consts::FRAC_PI_2, std::f64::consts::PI, 0.0).append_iter(1e-4));
        u.line_to((0.0, 10.0));
        let u = split(&u).remove(0);
        let (_, done, kept) = unfillet_all(&u);
        assert!(done.is_empty());
        assert!(kept[0].reason.contains("do not meet"), "{kept:?}");
        // An arc at the end of an open path has only one side.
        let mut e = BezPath::new();
        e.move_to((0.0, 0.0));
        e.line_to((10.0, 0.0));
        e.extend(kurbo::Arc::new((10.0, 5.0), (5.0, 5.0), -std::f64::consts::FRAC_PI_2, 1.0, 0.0).append_iter(1e-4));
        let (_, done, kept) = unfillet_all(&split(&e).remove(0));
        assert!(done.is_empty() && kept[0].reason.contains("end of an open path"));
    }
}
