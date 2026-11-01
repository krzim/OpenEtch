//! Paths to polylines and back, with the same subpath rules as `plan` (so "closed" means what the hatcher fills).

use kurbo::{BezPath, PathEl, Point};

use crate::{EPS, MAX_POINTS, Result, invalid};

/// One flattened subpath. A closed polyline does not repeat its first point at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Polyline {
    /// The points to draw: a closed polyline gets its first point appended.
    pub fn drawn(&self) -> Vec<Point> {
        let mut p = self.points.clone();
        if self.closed
            && let Some(&f) = p.first()
        {
            p.push(f);
        }
        p
    }

    /// Signed area (positive when counter-clockwise); 0 for open polylines.
    pub fn area(&self) -> f64 {
        if !self.closed {
            return 0.0;
        }
        polygon_area(&self.points)
    }

    pub fn length(&self) -> f64 {
        self.drawn().windows(2).map(|w| w[0].distance(w[1])).sum()
    }
}

/// Signed area of a ring (positive when counter-clockwise).
pub fn polygon_area(pts: &[Point]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        a.x * b.y - b.x * a.y
    })
    .sum::<f64>()
        / 2.0
}

/// Even-odd point-in-polygon test against a ring.
pub fn point_in_ring(p: Point, ring: &[Point]) -> bool {
    let n = ring.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Flatten `path` into polylines within `tolerance` mm. A subpath is closed when it ends with `ClosePath` or its last
/// point equals its first, and it has at least 3 distinct points (as in `plan`); repeated points are merged and
/// subpaths of fewer than 2 points are dropped. Fails past [`MAX_POINTS`] points or on non-finite coordinates.
pub fn flatten(path: &BezPath, tolerance: f64) -> Result<Vec<Polyline>> {
    if !crate::is_finite(path) {
        return invalid("the path has a coordinate that is not a finite number");
    }
    let tolerance = if tolerance.is_finite() && tolerance > 0.0 { tolerance } else { crate::TOLERANCE };
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    let mut closed_start: Option<Point> = None;
    let mut total = 0usize;
    kurbo::flatten(path, tolerance, |el| {
        if total > MAX_POINTS {
            return;
        }
        match el {
            PathEl::MoveTo(p) => {
                finish(&mut cur, false, &mut out);
                closed_start = None;
                cur.push(p);
                total += 1;
            }
            PathEl::LineTo(p) => {
                if cur.is_empty()
                    && let Some(s) = closed_start.take()
                {
                    cur.push(s);
                }
                if cur.last().is_none_or(|l| l.distance(p) > EPS) {
                    cur.push(p);
                    total += 1;
                }
            }
            PathEl::ClosePath => {
                closed_start = cur.first().copied();
                finish(&mut cur, true, &mut out);
            }
            _ => {}
        }
    });
    if total > MAX_POINTS {
        return invalid(format!("the shapes are too detailed (more than {MAX_POINTS} points when flattened)"));
    }
    finish(&mut cur, false, &mut out);
    Ok(out)
}

fn finish(pts: &mut Vec<Point>, closed: bool, out: &mut Vec<Polyline>) {
    let mut points = std::mem::take(pts);
    let mut closed = closed;
    if points.len() >= 2 && points[0].distance(points[points.len() - 1]) <= EPS {
        points.pop();
        closed = true;
    }
    if points.len() >= 2 {
        out.push(Polyline { closed: closed && points.len() >= 3, points });
    }
}

/// Polylines back to a path (`MoveTo`, `LineTo`, and `ClosePath` for closed ones).
pub fn polylines_to_path(lines: &[Polyline]) -> BezPath {
    let mut p = BezPath::new();
    for l in lines {
        let Some((&first, rest)) = l.points.split_first() else { continue };
        p.move_to(first);
        for &q in rest {
            p.line_to(q);
        }
        if l.closed {
            p.close_path();
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn rectangle_is_one_closed_ring_and_circle_is_fine() {
        let r = kurbo::Rect::new(0.0, 0.0, 10.0, 5.0).to_path(0.01);
        let f = flatten(&r, 0.01).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f[0].closed && f[0].points.len() == 4);
        assert!((f[0].area().abs() - 50.0).abs() < 1e-9);
        let c = kurbo::Circle::new((0.0, 0.0), 10.0).to_path(0.01);
        let f = flatten(&c, 0.01).unwrap();
        assert!((f[0].area().abs() - std::f64::consts::PI * 100.0).abs() < 0.5);
        for p in &f[0].points {
            assert!((p.to_vec2().hypot() - 10.0).abs() < 0.011);
        }
    }

    #[test]
    fn open_and_degenerate_subpaths() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((5.0, 0.0));
        p.move_to((1.0, 1.0)); // a lone point is dropped
        p.move_to((0.0, 0.0));
        p.line_to((1.0, 0.0));
        p.close_path(); // two points: not a ring
        let f = flatten(&p, 0.01).unwrap();
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|l| !l.closed));
        let mut bad = BezPath::new();
        bad.move_to((f64::NAN, 0.0));
        assert!(flatten(&bad, 0.01).is_err());
    }

    #[test]
    fn point_in_ring_even_odd() {
        let sq = [Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(4.0, 4.0), Point::new(0.0, 4.0)];
        assert!(point_in_ring(Point::new(2.0, 2.0), &sq));
        assert!(!point_in_ring(Point::new(5.0, 2.0), &sq));
    }
}
