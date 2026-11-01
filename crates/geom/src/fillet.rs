//! Fillets (LightBurn's Radius tool) on any corner of a subpath: between two lines (exact, as before), and between a
//! line and a curve or two curves (numeric: the arc's centre is where the two sides, offset by the radius towards the
//! inside of the turn, cross; the tangent points are the nearest points of the sides to it). A negative radius cuts a
//! concave bite: an arc of that radius centred on the corner, between where it meets the two sides.
//!
//! Each side keeps its kind: a cut line stays a line, a cut curve stays one Bézier (`subsegment`). The arc is drawn
//! with cubics, like every other generated arc.

use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, TAU};

use kurbo::{Arc, ParamCurve, PathSeg, Point, Vec2};

use crate::EPS;
use crate::ops::{Skipped, arc_segments, line_corner};
use crate::subpath::{SubPath, with_end, with_start};

/// Smallest turn (radians) that counts as a corner: nodes whose sides meet within 1° of straight are smooth.
const MIN_TURN: f64 = PI / 180.0;

/// Direction of travel at the end of a segment (`None` when it has none, a point).
fn end_dir(seg: &PathSeg) -> Option<Vec2> {
    let cands: Vec<Vec2> = match *seg {
        PathSeg::Line(l) => vec![l.p1 - l.p0],
        PathSeg::Quad(q) => vec![q.p2 - q.p1, q.p2 - q.p0],
        PathSeg::Cubic(c) => vec![c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0],
    };
    cands.into_iter().find(|v| v.hypot() > EPS).map(|v| v.normalize())
}

/// Direction of travel at the start of a segment.
fn start_dir(seg: &PathSeg) -> Option<Vec2> {
    end_dir(&seg.reverse()).map(|v| -v)
}

/// Unit tangent at parameter `t` (falls back to the end directions where the derivative vanishes).
fn tangent(seg: &PathSeg, t: f64) -> Option<Vec2> {
    let h = 1e-6;
    let (a, b) = ((t - h).max(0.0), (t + h).min(1.0));
    let d = seg.eval(b) - seg.eval(a);
    if d.hypot() > 1e-12 {
        Some(d.normalize())
    } else if t > 0.5 {
        end_dir(seg)
    } else {
        start_dir(seg)
    }
}

/// The segments meeting at node `k` (incoming, outgoing), when it is a node between two segments.
fn corner_segs(sub: &SubPath, k: usize) -> Option<(usize, usize)> {
    let n = sub.segs.len();
    let (inc, out) = if sub.closed {
        if k >= n {
            return None;
        }
        ((k + n - 1) % n, k)
    } else {
        if k == 0 || k >= n {
            return None;
        }
        (k - 1, k)
    };
    (inc != out).then_some((inc, out))
}

/// Signed turn at node `k` (radians, positive = left), when it is between two segments with directions.
fn turn_at(sub: &SubPath, k: usize) -> Option<f64> {
    let (inc, out) = corner_segs(sub, k)?;
    let (u, v) = (end_dir(&sub.segs[inc])?, start_dir(&sub.segs[out])?);
    Some(u.cross(v).atan2(u.dot(v)))
}

/// Every node where the path turns by more than 1° between two segments, of any kind (lines or curves): what a
/// fillet of "all" corners rounds.
pub fn sharp_corners(sub: &SubPath) -> Vec<usize> {
    (0..sub.node_count()).filter(|&k| turn_at(sub, k).is_some_and(|t| t.abs() > MIN_TURN)).collect()
}

/// A planned fillet at one node: cut the incoming segment at parameter `s` and the outgoing one at `t`, and put
/// `arc` (from `p_in` to `p_out`) between them.
struct Cut {
    s: f64,
    t: f64,
    p_in: Point,
    p_out: Point,
    arc: Vec<PathSeg>,
}

/// Points of `seg` offset by `r` to the left (`left`) or right of travel, at `n + 1` parameters, with them.
fn offset_samples(seg: &PathSeg, r: f64, left: bool, n: usize) -> Vec<(f64, Point)> {
    (0..=n)
        .filter_map(|i| {
            let s = i as f64 / n as f64;
            let d = tangent(seg, s)?;
            let normal = if left { Vec2::new(-d.y, d.x) } else { Vec2::new(d.y, -d.x) };
            Some((s, seg.eval(s) + normal * r))
        })
        .collect()
}

fn offset_point(seg: &PathSeg, s: f64, r: f64, left: bool) -> Option<Point> {
    let d = tangent(seg, s)?;
    let normal = if left { Vec2::new(-d.y, d.x) } else { Vec2::new(d.y, -d.x) };
    Some(seg.eval(s) + normal * r)
}

/// Where chords `a-b` and `c-d` cross, as parameters on each (both within 0..=1).
fn cross_params(a: Point, b: Point, c: Point, d: Point) -> Option<(f64, f64)> {
    let (r, q) = (b - a, d - c);
    let den = r.cross(q);
    if den.abs() <= 1e-18 {
        return None;
    }
    let lam = (c - a).cross(q) / den;
    let mu = (c - a).cross(r) / den;
    let e = 1e-9;
    ((-e..=1.0 + e).contains(&lam) && (-e..=1.0 + e).contains(&mu)).then_some((lam.clamp(0.0, 1.0), mu.clamp(0.0, 1.0)))
}

/// The fillet centre of radius `r` between `a` (arriving) and `b` (leaving) on the `left` side: parameters `(s, t)`
/// with `offset_a(s) == offset_b(t)`, the crossing nearest the corner (largest `s`, then smallest `t`), refined by
/// Newton steps. `s >= s_min` and `t <= t_max` (room shared with neighbouring fillets).
fn fillet_params(a: &PathSeg, b: &PathSeg, r: f64, left: bool, s_min: f64, t_max: f64) -> Option<(f64, f64, Point)> {
    const N: usize = 96;
    let pa = offset_samples(a, r, left, N);
    let pb = offset_samples(b, r, left, N);
    let mut best: Option<(f64, f64)> = None;
    for wa in pa.windows(2).rev() {
        for wb in pb.windows(2) {
            if let Some((lam, mu)) = cross_params(wa[0].1, wa[1].1, wb[0].1, wb[1].1) {
                let s = wa[0].0 + (wa[1].0 - wa[0].0) * lam;
                let t = wb[0].0 + (wb[1].0 - wb[0].0) * mu;
                if best.is_none_or(|(bs, bt)| s > bs + 1e-12 || ((s - bs).abs() <= 1e-12 && t < bt)) {
                    best = Some((s, t));
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    let (mut s, mut t) = best?;
    // Newton on F(s, t) = offset_a(s) - offset_b(t).
    for _ in 0..20 {
        let f = offset_point(a, s, r, left)? - offset_point(b, t, r, left)?;
        if f.hypot() < 1e-10 {
            break;
        }
        let h = 1e-7;
        let (s0, s1) = ((s - h).max(0.0), (s + h).min(1.0));
        let (t0, t1) = ((t - h).max(0.0), (t + h).min(1.0));
        let ds = (offset_point(a, s1, r, left)? - offset_point(a, s0, r, left)?) / (s1 - s0);
        let dt = -(offset_point(b, t1, r, left)? - offset_point(b, t0, r, left)?) / (t1 - t0);
        let det = ds.cross(dt);
        if det.abs() < 1e-18 {
            break;
        }
        // Solve ds * x + dt * y = -f.
        let x = (-f).cross(dt) / det;
        let y = ds.cross(-f) / det;
        s = (s + x).clamp(0.0, 1.0);
        t = (t + y).clamp(0.0, 1.0);
    }
    let c = offset_point(a, s, r, left)?;
    let miss = (c - offset_point(b, t, r, left)?).hypot();
    let tol = 1e-6 * r.max(1.0);
    if miss > tol || s < s_min - 1e-9 || t > t_max + 1e-9 || s <= 1e-9 && t >= 1.0 - 1e-9 {
        return None;
    }
    // The centre must be a radius away from both sides (no closer point elsewhere on the side being cut).
    if (a.eval(s).distance(c) - r).abs() > tol || (b.eval(t).distance(c) - r).abs() > tol {
        return None;
    }
    Some((s, t, c))
}

/// Parameter on `seg` nearest its `from_end` (1.0) or start (0.0) where the distance to `c` reaches `r`.
fn distance_hit(seg: &PathSeg, c: Point, r: f64, from_end: bool) -> Option<f64> {
    const N: usize = 256;
    let at = |i: usize| if from_end { 1.0 - i as f64 / N as f64 } else { i as f64 / N as f64 };
    let f = |t: f64| seg.eval(t).distance(c) - r;
    let mut prev = at(0);
    for i in 1..=N {
        let cur = at(i);
        if f(cur) >= 0.0 {
            // Bisection between prev (inside) and cur (outside).
            let (mut lo, mut hi) = (prev, cur);
            for _ in 0..60 {
                let mid = 0.5 * (lo + hi);
                if f(mid) >= 0.0 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            return Some(hi);
        }
        prev = cur;
    }
    None
}

/// Round the given corner nodes of one subpath with circular arcs of `radius` mm (LightBurn's Radius tool). A
/// negative radius cuts a concave "bite" of that radius centred on the corner instead. A corner is a node between
/// two segments (lines or curves) that turn by more than 1°; one is skipped (with the reason) when it is not such a
/// corner, or the arc does not fit on its sides: between two lines the arc must fit on each line, sharing a line with
/// a neighbouring fillet half and half; a curve side may be cut anywhere but, when the neighbouring corner is
/// filleted too, only in its half nearest this corner.
pub fn fillet(sub: &SubPath, corners: &[usize], radius: f64) -> (SubPath, Vec<usize>, Vec<Skipped>) {
    let mut skipped = Vec::new();
    if !(radius.is_finite() && radius != 0.0) {
        let all = corners.iter().map(|&k| Skipped { node: k, reason: "the radius must be a non-zero number".into() });
        return (sub.clone(), vec![], all.collect());
    }
    let wanted: HashSet<usize> = corners.iter().copied().collect();
    let r = radius.abs();
    let n = sub.segs.len();
    let mut ks: Vec<usize> = wanted.iter().copied().collect();
    ks.sort_unstable();
    let mut plan: HashMap<usize, Cut> = HashMap::new();
    for &k in &ks {
        if k >= sub.node_count() {
            skipped.push(Skipped { node: k, reason: "no such node".into() });
            continue;
        }
        let (Some((inc, out)), Some(turn)) = (corner_segs(sub, k), turn_at(sub, k)) else {
            skipped.push(Skipped { node: k, reason: "not a corner between two segments".into() });
            continue;
        };
        if turn.abs() <= MIN_TURN {
            skipped.push(Skipped { node: k, reason: "the sides meet (almost) straight".into() });
            continue;
        }
        let neighbour = |m: usize| wanted.contains(&m) && corner_segs(sub, m).is_some();
        let prev_node = if sub.closed { (k + n - 1) % n } else { k.wrapping_sub(1) };
        let next_node = if sub.closed { (k + 1) % n } else { k + 1 };
        let (share_a, share_b) = (neighbour(prev_node), neighbour(next_node));
        let (a, b) = (sub.segs[inc], sub.segs[out]);
        let c = sub.node(k);
        let cut = if let Some((u, v, turn, la, lb)) = line_corner(sub, k) {
            line_line(c, u, v, turn, la, lb, radius, share_a, share_b)
        } else if radius > 0.0 {
            let left = turn > 0.0;
            let (s_min, t_max) = (if share_a { 0.5 } else { 0.0 }, if share_b { 0.5 } else { 1.0 });
            fillet_params(&a, &b, r, left, s_min, t_max).map(|(s, t, centre)| {
                let (p_in, p_out) = (a.eval(s), b.eval(t));
                let start = (p_in - centre).atan2();
                let raw = (p_in - centre).cross(p_out - centre).atan2((p_in - centre).dot(p_out - centre));
                let sweep = if left && raw < 0.0 {
                    raw + TAU
                } else if !left && raw > 0.0 {
                    raw - TAU
                } else {
                    raw
                };
                let arc = Arc::new(centre, (r, r), start, sweep, 0.0);
                Cut { s, t, p_in, p_out, arc: arc_segments(&arc, p_in, p_out) }
            })
        } else {
            let s = distance_hit(&a, c, r, true).filter(|s| *s >= if share_a { 0.5 } else { 0.0 });
            let t = distance_hit(&b, c, r, false).filter(|t| *t <= if share_b { 0.5 } else { 1.0 });
            s.zip(t).map(|(s, t)| {
                let (p_in, p_out) = (a.eval(s), b.eval(t));
                let (d1, d2) = (p_in - c, p_out - c);
                let arc = Arc::new(c, (r, r), d1.atan2(), d1.cross(d2).atan2(d1.dot(d2)), 0.0);
                Cut { s, t, p_in, p_out, arc: arc_segments(&arc, p_in, p_out) }
            })
        };
        match cut {
            Some(cut) => {
                plan.insert(k, cut);
            }
            None => {
                skipped.push(Skipped { node: k, reason: format!("a radius of {r} mm does not fit at this corner") })
            }
        }
    }
    let mut out = Vec::new();
    for (i, seg) in sub.segs.iter().enumerate() {
        let start_node = i;
        let end_node = if i + 1 == n && sub.closed { 0 } else { i + 1 };
        let a = plan.get(&start_node).map_or(0.0, |c| c.t);
        let b = plan.get(&end_node).map_or(1.0, |c| c.s);
        if let Some(c) = plan.get(&start_node) {
            out.extend(c.arc.iter().copied());
        }
        if b - a <= 1e-12 {
            continue;
        }
        let mut s = if a == 0.0 && b == 1.0 { *seg } else { seg.subsegment(a..b) };
        if let Some(c) = plan.get(&start_node) {
            s = with_start(s, c.p_out);
        }
        if let Some(c) = plan.get(&end_node) {
            s = with_end(s, c.p_in);
        }
        if s.start().distance(s.end()) > EPS || !matches!(s, PathSeg::Line(_)) {
            out.push(s);
        }
    }
    for i in 1..out.len() {
        let p = out[i - 1].end();
        out[i] = with_start(out[i], p);
    }
    if sub.closed && !out.is_empty() {
        let last = out.len() - 1;
        out[last] = with_end(out[last], out[0].start());
    }
    let mut applied: Vec<usize> = plan.keys().copied().collect();
    applied.sort_unstable();
    (SubPath { segs: out, closed: sub.closed }, applied, skipped)
}

/// The exact fillet (or bite) between two lines meeting at `c` with directions `u` (arriving) and `v` (leaving).
#[allow(clippy::too_many_arguments)]
fn line_line(
    c: Point,
    u: Vec2,
    v: Vec2,
    turn: f64,
    la: f64,
    lb: f64,
    radius: f64,
    share_a: bool,
    share_b: bool,
) -> Option<Cut> {
    let r = radius.abs();
    let t = if radius > 0.0 { r * (turn.abs() / 2.0).tan() } else { r };
    let room_a = la * if share_a { 0.5 } else { 1.0 };
    let room_b = lb * if share_b { 0.5 } else { 1.0 };
    if !(t.is_finite() && t <= room_a + 1e-9 && t <= room_b + 1e-9) {
        return None;
    }
    let (p_in, p_out) = (c - u * t, c + v * t);
    let arc = if radius > 0.0 {
        let left = turn > 0.0;
        let normal = if left { Vec2::new(-u.y, u.x) } else { Vec2::new(u.y, -u.x) };
        let center = p_in + normal * r;
        Arc::new(center, (r, r), (p_in - center).atan2(), turn, 0.0)
    } else {
        let sweep = (-u).cross(v).atan2((-u).dot(v));
        Arc::new(c, (r, r), (-u).atan2(), sweep, 0.0)
    };
    let s = (1.0 - t / la).clamp(0.0, 1.0);
    let tt = (t / lb).clamp(0.0, 1.0);
    Some(Cut { s, t: tt, p_in, p_out, arc: arc_segments(&arc, p_in, p_out) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subpath::{join, split};
    use kurbo::{BezPath, ParamCurveNearest, Shape};

    /// A line into a quarter-circle-ish curve: the corner at (10, 0) turns left by 90°.
    fn line_curve() -> SubPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.curve_to((10.0, 5.0), (15.0, 10.0), (20.0, 10.0));
        split(&p).remove(0)
    }

    fn dist_to(sub: &SubPath, p: Point) -> f64 {
        sub.segs.iter().map(|s| s.nearest(p, 1e-9).distance_sq.sqrt()).fold(f64::MAX, f64::min)
    }

    #[test]
    fn line_curve_corner_is_rounded_tangent_to_both_sides() {
        let sub = line_curve();
        assert_eq!(sharp_corners(&sub), vec![1]);
        let (f, applied, skipped) = fillet(&sub, &[1], 2.0);
        assert_eq!(applied, vec![1], "{skipped:?}");
        assert!(f.segs.iter().any(|s| matches!(s, PathSeg::Line(_))), "the line side stays a line");
        // The arc's middle lies 2 mm from the centre, which lies 2 mm from both original sides.
        let first_arc = f.segs.iter().position(|s| matches!(s, PathSeg::Cubic(_))).unwrap();
        let p_in = f.segs[first_arc].start();
        assert!(p_in.y.abs() < 1e-9 && p_in.x < 10.0 && p_in.x > 7.0, "{p_in:?}");
        let centre = Point::new(p_in.x, 2.0);
        assert!((dist_to(&sub, centre) - 2.0).abs() < 1e-5, "{}", dist_to(&sub, centre));
        // Connected, ends unchanged.
        for w in f.segs.windows(2) {
            assert!(w[0].end().distance(w[1].start()) < 1e-9);
        }
        assert_eq!(f.start(), Point::new(0.0, 0.0));
        assert!(f.end().distance(Point::new(20.0, 10.0)) < 1e-9);
        // Every point of the fillet stays within the original corner region (it only cuts the corner off).
        let flat = crate::flatten::flatten(&join(std::slice::from_ref(&f)), 0.01).unwrap();
        for p in &flat[0].points {
            assert!(dist_to(&sub, *p) < 2.0 * (1.0 - (PI / 4.0).cos()) + 0.05 || p.y < 1e-6 || p.x > 9.0, "{p:?}");
        }
    }

    #[test]
    fn curve_curve_corner_and_bites() {
        // Two arcs meeting at a sharp point (a lens / leaf shape).
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.curve_to((5.0, 5.0), (15.0, 5.0), (20.0, 0.0));
        p.curve_to((15.0, -5.0), (5.0, -5.0), (0.0, 0.0));
        p.close_path();
        let sub = split(&p).remove(0);
        let corners = sharp_corners(&sub);
        assert_eq!(corners, vec![0, 1]);
        let before = join(std::slice::from_ref(&sub)).area().abs();
        let (f, applied, _) = fillet(&sub, &corners, 1.0);
        assert_eq!(applied, vec![0, 1]);
        let after = join(std::slice::from_ref(&f)).area().abs();
        assert!(after < before && after > before - 2.0, "{before} {after}");
        assert!(f.closed);
        let (bite, applied, _) = fillet(&sub, &corners, -1.0);
        assert_eq!(applied, vec![0, 1]);
        assert!(join(&[bite]).area().abs() < before);
        // Too big to fit: skipped with a reason.
        let (_, applied, skipped) = fillet(&sub, &[0], 50.0);
        assert!(applied.is_empty());
        assert!(skipped[0].reason.contains("does not fit"));
    }

    #[test]
    fn line_line_matches_the_exact_construction() {
        let sq = split(&kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01)).remove(0);
        let (f, applied, _) = fillet(&sq, &sharp_corners(&sq), 1.0);
        assert_eq!(applied.len(), 4);
        let expect = 100.0 - 4.0 * (1.0 - PI / 4.0);
        assert!((join(&[f]).area().abs() - expect).abs() < 1e-3);
        // Smooth nodes are not corners.
        let c = split(&kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01)).remove(0);
        assert!(sharp_corners(&c).is_empty());
        let (_, applied, skipped) = fillet(&c, &[0], 1.0);
        assert!(applied.is_empty() && skipped[0].reason.contains("straight"));
    }
}
