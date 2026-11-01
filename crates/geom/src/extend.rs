//! Extend (LightBurn's Edit Nodes "E": extend line): an open end of a path grows straight on, along its own
//! direction, until it meets the nearest other line (another shape, another subpath of the same shape, or the same
//! subpath further along).

use kurbo::{Line, PathSeg, Point, Vec2};

use crate::flatten::flatten;
use crate::subpath::{SubPath, join, with_end, with_start};
use crate::{EPS, Result, TOLERANCE, invalid};

/// What [`extend`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Extended {
    /// The subpaths after the extension.
    pub subs: Vec<SubPath>,
    /// Index of the extended subpath.
    pub subpath: usize,
    /// Whether its end (true) or its start (false) was extended.
    pub at_end: bool,
    /// The old end point and where it now ends.
    pub from: Point,
    pub to: Point,
}

/// Direction of travel leaving the path at its end (or, for `at_end == false`, backwards out of its start).
fn out_dir(sub: &SubPath, at_end: bool) -> Option<Vec2> {
    let seg = if at_end { sub.segs[sub.segs.len() - 1] } else { sub.segs[0].reverse() };
    let cands: Vec<Vec2> = match seg {
        PathSeg::Line(l) => vec![l.p1 - l.p0],
        PathSeg::Quad(q) => vec![q.p2 - q.p1, q.p2 - q.p0],
        PathSeg::Cubic(c) => vec![c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0],
    };
    cands.into_iter().find(|v| v.hypot() > EPS).map(|v| v.normalize())
}

/// Distance along the ray `p + d·λ` (λ > a hair) to the chord `a-b`, if it crosses it.
fn ray_hit(p: Point, d: Vec2, a: Point, b: Point) -> Option<f64> {
    let s = b - a;
    let den = d.cross(s);
    if den.abs() <= 1e-15 * s.hypot().max(1.0) {
        return None;
    }
    let lam = (a - p).cross(s) / den;
    let mu = (a - p).cross(d) / den;
    (lam > 1e-6 && (-1e-9..=1.0 + 1e-9).contains(&mu)).then_some(lam)
}

/// Extend the open end of `subs` nearest `at` (within `pick` mm) straight on until it meets the nearest line of
/// `others` (polylines in the same frame) or of `subs` themselves, at most `max_len` mm away. A line at that end is
/// lengthened; a curve gets a new line after it. Fails when no open end is near `at` or nothing is hit.
pub fn extend(subs: &[SubPath], others: &[Vec<Point>], at: Point, pick: f64, max_len: f64) -> Result<Extended> {
    let mut best: Option<(f64, usize, bool)> = None;
    for (i, s) in subs.iter().enumerate().filter(|(_, s)| !s.closed) {
        for (end, p) in [(false, s.start()), (true, s.end())] {
            let d = p.distance(at);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, i, end));
            }
        }
    }
    let Some((d, si, at_end)) = best else { return invalid("the shape has no open ends to extend") };
    if d > pick {
        return invalid(format!("no open end within {pick} mm of the point (nearest is {d:.3} mm away)"));
    }
    let sub = &subs[si];
    let from = if at_end { sub.end() } else { sub.start() };
    let Some(dir) = out_dir(sub, at_end) else { return invalid("that end has no direction to extend in") };
    let mut lines: Vec<Vec<Point>> = others.to_vec();
    for s in subs {
        for l in flatten(&join(std::slice::from_ref(s)), TOLERANCE)? {
            lines.push(l.drawn());
        }
    }
    let mut nearest: Option<f64> = None;
    for line in &lines {
        for w in line.windows(2) {
            if let Some(lam) = ray_hit(from, dir, w[0], w[1])
                && lam <= max_len
                && nearest.is_none_or(|n| lam < n)
            {
                nearest = Some(lam);
            }
        }
    }
    let Some(len) = nearest else { return invalid(format!("nothing to extend to within {max_len} mm")) };
    let to = from + dir * len;
    let mut segs = sub.segs.clone();
    if at_end {
        let n = segs.len() - 1;
        match segs[n] {
            PathSeg::Line(_) => segs[n] = with_end(segs[n], to),
            _ => segs.push(PathSeg::Line(Line::new(from, to))),
        }
    } else {
        match segs[0] {
            PathSeg::Line(_) => segs[0] = with_start(segs[0], to),
            _ => segs.insert(0, PathSeg::Line(Line::new(to, from))),
        }
    }
    let mut out = subs.to_vec();
    out[si] = SubPath { segs, closed: false };
    Ok(Extended { subs: out, subpath: si, at_end, from, to })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subpath::split;
    use kurbo::BezPath;

    fn line(a: (f64, f64), b: (f64, f64)) -> SubPath {
        let mut p = BezPath::new();
        p.move_to(a);
        p.line_to(b);
        split(&p).remove(0)
    }

    #[test]
    fn extends_a_line_to_the_nearest_crossing() {
        let sub = line((0.0, 0.0), (5.0, 0.0));
        let walls = vec![
            vec![Point::new(10.0, -5.0), Point::new(10.0, 5.0)],
            vec![Point::new(20.0, -5.0), Point::new(20.0, 5.0)],
        ];
        let e = extend(std::slice::from_ref(&sub), &walls, Point::new(5.2, 0.1), 1.0, 1000.0).unwrap();
        assert!(e.at_end);
        assert_eq!(e.to, Point::new(10.0, 0.0));
        assert_eq!(e.subs[0].segs.len(), 1, "the line is lengthened");
        assert_eq!(e.subs[0].end(), Point::new(10.0, 0.0));
        // The start extends backwards (nothing there).
        assert!(extend(std::slice::from_ref(&sub), &walls, Point::new(0.0, 0.0), 1.0, 1000.0).is_err());
        let back = vec![vec![Point::new(-3.0, -1.0), Point::new(-3.0, 1.0)]];
        let e = extend(std::slice::from_ref(&sub), &back, Point::new(0.0, 0.0), 1.0, 1000.0).unwrap();
        assert!(!e.at_end && e.subs[0].start() == Point::new(-3.0, 0.0));
        // Too far, or no end near the point.
        assert!(extend(std::slice::from_ref(&sub), &walls, Point::new(5.0, 0.0), 1.0, 2.0).is_err());
        assert!(extend(&[sub], &walls, Point::new(2.5, 3.0), 1.0, 100.0).is_err());
    }

    #[test]
    fn curve_end_gets_a_new_line_and_hits_itself() {
        // A hook: a line up, then a curve ending pointing down at the line's own start region.
        let mut p = BezPath::new();
        p.move_to((0.0, -10.0));
        p.line_to((0.0, 10.0));
        p.curve_to((0.0, 15.0), (10.0, 15.0), (10.0, 10.0));
        let sub = split(&p).remove(0);
        // Its end points down (-Y); a horizontal wall below.
        let wall = vec![vec![Point::new(-20.0, -5.0), Point::new(20.0, -5.0)]];
        let e = extend(std::slice::from_ref(&sub), &wall, Point::new(10.0, 10.0), 0.5, 100.0).unwrap();
        assert_eq!(e.subs[0].segs.len(), 3);
        assert!(e.to.distance(Point::new(10.0, -5.0)) < 1e-9, "{:?}", e.to);
        // Extending the start backwards (down) with nothing below: fails; with the wall, it lands on it.
        let e = extend(&[sub], &wall, Point::new(0.0, -10.0), 0.5, 100.0);
        assert!(e.is_err(), "the wall is above the start going down");
        let closed = split(&kurbo::Shape::to_path(&kurbo::Rect::new(0.0, 0.0, 1.0, 1.0), 0.01)).remove(0);
        assert!(extend(&[closed], &[], Point::new(0.0, 0.0), 1.0, 10.0).is_err());
    }
}
