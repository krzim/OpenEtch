//! Positions and directions along a path by distance, for copying objects along it.

use kurbo::{BezPath, Point};

use crate::flatten::flatten;
use crate::{Result, TOLERANCE, invalid};

/// One subpath of a path, flattened, measured by arc length.
#[derive(Debug, Clone)]
pub struct Walker {
    points: Vec<Point>,
    /// Cumulative length at each point.
    cum: Vec<f64>,
    pub closed: bool,
}

impl Walker {
    /// The longest subpath of `path` (flattened at [`TOLERANCE`]).
    pub fn longest(path: &BezPath) -> Result<Walker> {
        let lines = flatten(path, TOLERANCE)?;
        let best = lines.into_iter().max_by(|a, b| a.length().total_cmp(&b.length()));
        let Some(l) = best else { return invalid("the path has no line to follow") };
        let points = l.drawn();
        let mut cum = Vec::with_capacity(points.len());
        let mut s = 0.0;
        cum.push(0.0);
        for w in points.windows(2) {
            s += w[0].distance(w[1]);
            cum.push(s);
        }
        if s <= crate::EPS {
            return invalid("the path has no length to follow");
        }
        Ok(Walker { points, cum, closed: l.closed })
    }

    pub fn length(&self) -> f64 {
        *self.cum.last().unwrap_or(&0.0)
    }

    /// Point and direction (radians from +X) at distance `s` from the start (clamped; wraps on a closed path).
    pub fn at(&self, s: f64) -> (Point, f64) {
        let len = self.length();
        let s = if self.closed { s.rem_euclid(len) } else { s.clamp(0.0, len) };
        let i = match self.cum.binary_search_by(|c| c.total_cmp(&s)) {
            Ok(i) => i.min(self.points.len() - 2),
            Err(i) => i.saturating_sub(1).min(self.points.len() - 2),
        };
        let (a, b) = (self.points[i], self.points[i + 1]);
        let seg = self.cum[i + 1] - self.cum[i];
        let t = if seg > 0.0 { (s - self.cum[i]) / seg } else { 0.0 };
        (a.lerp(b, t), (b - a).atan2())
    }

    /// Distance along the path of the point nearest `p`.
    pub fn nearest(&self, p: Point) -> f64 {
        let mut best = (f64::MAX, 0.0);
        for (i, w) in self.points.windows(2).enumerate() {
            let ab = w[1] - w[0];
            let l2 = ab.hypot2();
            let t = if l2 > 0.0 { ((p - w[0]).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let d = p.distance(w[0] + ab * t);
            if d < best.0 {
                best = (d, self.cum[i] + t * l2.sqrt());
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn walk_a_square() {
        let w = Walker::longest(&kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01)).unwrap();
        assert!(w.closed);
        assert!((w.length() - 40.0).abs() < 1e-12);
        let (p, a) = w.at(15.0);
        assert!((p - Point::new(10.0, 5.0)).hypot() < 1e-12);
        assert!((a - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        let (p, _) = w.at(45.0);
        assert!((p - Point::new(5.0, 0.0)).hypot() < 1e-12, "wraps");
        assert!((w.nearest(Point::new(11.0, 5.0)) - 15.0).abs() < 1e-12);
        assert!(Walker::longest(&BezPath::new()).is_err());
    }
}
