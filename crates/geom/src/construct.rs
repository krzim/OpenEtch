//! Constructions: the convex hull of shapes (LightBurn's Create Rubber-Band Outline: "the shortest possible path that
//! can encompass all the selected graphics") and circles tangent to three circles (Tools > Generate Tangent Circle,
//! the problem of Apollonius: up to eight solutions).

use kurbo::{BezPath, Point, Vec2};

use crate::flatten::flatten;
use crate::{Result, TOLERANCE, invalid};

/// Convex hull of `points` (Andrew's monotone chain), counter-clockwise, without collinear points. Fewer than three
/// distinct non-collinear points give what is left (possibly fewer than three).
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts: Vec<Point> = points.iter().copied().filter(|p| p.x.is_finite() && p.y.is_finite()).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup_by(|a, b| a.distance(*b) <= 1e-12);
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Point, a: Point, b: Point| (a - o).cross(b - o);
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// The rubber-band outline of `paths` (field mm): the closed convex hull of their outlines flattened at
/// [`TOLERANCE`], so it lies at most that far inside the true hull of curves. Fails when the shapes have no area to
/// wrap (fewer than three non-collinear points).
pub fn rubber_band(paths: &[BezPath]) -> Result<BezPath> {
    let mut pts = Vec::new();
    for p in paths {
        for l in flatten(p, TOLERANCE)? {
            pts.extend(l.points);
        }
    }
    let hull = convex_hull(&pts);
    if hull.len() < 3 {
        return invalid("the shapes have no area to wrap (all their points lie on one line)");
    }
    let mut out = BezPath::new();
    out.move_to(hull[0]);
    for &p in &hull[1..] {
        out.line_to(p);
    }
    out.close_path();
    Ok(out)
}

/// A circle: centre and radius (mm).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Circle {
    pub center: Point,
    pub radius: f64,
}

/// Every circle tangent to all three circles (each touching from outside or inside; at most eight), sorted by
/// radius, without duplicates. Three circles with collinear centres and equal radii, nested circles and other
/// configurations can have fewer or none.
pub fn tangent_circles(c: [Circle; 3]) -> Result<Vec<Circle>> {
    if !c.iter().all(|k| k.center.x.is_finite() && k.center.y.is_finite() && k.radius.is_finite() && k.radius > 0.0) {
        return invalid("each circle needs a finite centre and a positive radius");
    }
    // Work in a frame with the first centre at the origin and the second on +X, scaled to about 1.
    let scale = c.iter().map(|k| k.center.distance(c[0].center) + k.radius).fold(0.0, f64::max);
    let o = c[0].center;
    let d = c[1].center - o;
    let ang = if d.hypot() > 1e-12 { d.atan2() } else { 0.0 };
    let (sin, cos) = ang.sin_cos();
    let to_local = |p: Point| {
        let v = (p - o) / scale;
        Point::new(v.x * cos + v.y * sin, -v.x * sin + v.y * cos)
    };
    let to_field = |p: Point| o + Vec2::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos) * scale;
    let local: Vec<(Point, f64)> = c.iter().map(|k| (to_local(k.center), k.radius / scale)).collect();
    let mut out: Vec<Circle> = Vec::new();
    for signs in 0..8u8 {
        let s: Vec<f64> = (0..3).map(|i| if signs & (1 << i) != 0 { -1.0 } else { 1.0 }).collect();
        for (p, r) in solve(&local, &s) {
            if !(r > 1e-9 && p.x.is_finite() && p.y.is_finite()) {
                continue;
            }
            // Tangent to all three: distance between centres = r + s·ri (s = -1: |r - ri|).
            let ok = local.iter().zip(&s).all(|(&(ci, ri), &si)| (p.distance(ci) - (r + si * ri).abs()).abs() < 1e-7);
            if !ok {
                continue;
            }
            let k = Circle { center: to_field(p), radius: r * scale };
            if !out
                .iter()
                .any(|q| q.center.distance(k.center) < 1e-6 * scale && (q.radius - k.radius).abs() < 1e-6 * scale)
            {
                out.push(k);
            }
        }
    }
    out.sort_by(|a, b| a.radius.total_cmp(&b.radius));
    Ok(out)
}

/// Solutions `(centre, r)` of `|p - ci|² = (r + si·ri)²` for i = 0..3 (centres with c0 at the origin, c1 on +X).
fn solve(c: &[(Point, f64)], s: &[f64]) -> Vec<(Point, f64)> {
    let k = |i: usize| c[i].0.to_vec2().hypot2() - c[i].1 * c[i].1;
    // Subtracting equation 0 from i: 2(xi - x0)x + 2(yi - y0)y + 2(si ri - s0 r0) r = ki - k0.
    let row = |i: usize| {
        let (ci, ri) = c[i];
        let (c0, r0) = c[0];
        (2.0 * (ci.x - c0.x), 2.0 * (ci.y - c0.y), 2.0 * (s[i] * ri - s[0] * r0), k(i) - k(0))
    };
    let (a1, b1, e1, f1) = row(1);
    let (a2, b2, e2, f2) = row(2);
    let (c0, r0, s0) = (c[0].0, c[0].1, s[0]);
    let mut out = Vec::new();
    let det = a1 * b2 - a2 * b1;
    if det.abs() > 1e-12 {
        // x = px + qx r, y = py + qy r.
        let px = (f1 * b2 - f2 * b1) / det;
        let qx = (-e1 * b2 + e2 * b1) / det;
        let py = (a1 * f2 - a2 * f1) / det;
        let qy = (-a1 * e2 + a2 * e1) / det;
        // (px + qx r - x0)² + (py + qy r - y0)² = (r + s0 r0)².
        let (ux, uy) = (px - c0.x, py - c0.y);
        let qa = qx * qx + qy * qy - 1.0;
        let qb = 2.0 * (ux * qx + uy * qy) - 2.0 * s0 * r0;
        let qc = ux * ux + uy * uy - r0 * r0;
        for r in quadratic(qa, qb, qc) {
            out.push((Point::new(px + qx * r, py + qy * r), r));
        }
    } else {
        // Collinear centres (all on the X axis here): x and r from the two rows, y from equation 0.
        let det2 = a1 * e2 - a2 * e1;
        if det2.abs() > 1e-12 {
            let x = (f1 * e2 - f2 * e1) / det2;
            let r = (a1 * f2 - a2 * f1) / det2;
            let yy = (r + s0 * r0).powi(2) - (x - c0.x).powi(2);
            if yy >= -1e-12 {
                let y = yy.max(0.0).sqrt();
                out.push((Point::new(x, y), r));
                out.push((Point::new(x, -y), r));
            }
        } else if a1.abs() > 1e-12 {
            // Rows dependent: x = (f1 - e1 r) / a1 and y free; equation 0 is then a family. Use the symmetric pair
            // through r from equation 0 with y = 0 only (the circles on the line of centres).
            let (m, n) = (f1 / a1, -e1 / a1);
            let (ux, qa) = (m - c0.x, n * n - 1.0);
            for r in quadratic(qa, 2.0 * ux * n - 2.0 * s0 * r0, ux * ux - r0 * r0) {
                out.push((Point::new(m + n * r, 0.0), r));
            }
        }
    }
    out
}

fn quadratic(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        return if b.abs() > 1e-15 { vec![-c / b] } else { vec![] };
    }
    let disc = b * b - 4.0 * a * c;
    if disc < -1e-12 {
        return vec![];
    }
    let sq = disc.max(0.0).sqrt();
    vec![(-b + sq) / (2.0 * a), (-b - sq) / (2.0 * a)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn hull_wraps_shapes() {
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(4.0, 0.0),
            Point::new(2.0, 1.0),
            Point::new(4.0, 4.0),
            Point::new(0.0, 4.0),
            Point::new(2.0, 0.0),
        ];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 4, "{h:?}");
        let a = kurbo::Circle::new((0.0, 0.0), 1.0).to_path(0.01);
        let b = kurbo::Circle::new((10.0, 0.0), 1.0).to_path(0.01);
        let band = rubber_band(&[a, b]).unwrap();
        // A stadium: 2 x 10 rectangle plus a unit circle.
        let expect = 20.0 + std::f64::consts::PI;
        assert!(band.area() < expect && band.area() > expect - 0.1, "{}", band.area());
        let mut line = BezPath::new();
        line.move_to((0.0, 0.0));
        line.line_to((5.0, 0.0));
        assert!(rubber_band(&[line]).is_err());
    }

    fn check(sol: &[Circle], c: &[Circle; 3]) {
        for s in sol {
            for k in c {
                let d = s.center.distance(k.center);
                assert!(
                    (d - (s.radius + k.radius)).abs() < 1e-6 || (d - (s.radius - k.radius).abs()).abs() < 1e-6,
                    "{s:?} {k:?}"
                );
            }
        }
    }

    #[test]
    fn apollonius_has_eight_solutions_for_separate_circles() {
        let c = [
            Circle { center: Point::new(0.0, 0.0), radius: 1.0 },
            Circle { center: Point::new(10.0, 0.0), radius: 2.0 },
            Circle { center: Point::new(4.0, 8.0), radius: 1.5 },
        ];
        let sol = tangent_circles(c).unwrap();
        assert_eq!(sol.len(), 8, "{sol:?}");
        check(&sol, &c);
        // Three equal circles in a row: solutions exist (the two lines are not circles).
        let row = [
            Circle { center: Point::new(0.0, 0.0), radius: 1.0 },
            Circle { center: Point::new(5.0, 0.0), radius: 1.0 },
            Circle { center: Point::new(10.0, 0.0), radius: 1.0 },
        ];
        let sol = tangent_circles(row).unwrap();
        assert!(!sol.is_empty());
        check(&sol, &row);
        assert!(tangent_circles([row[0], row[1], Circle { center: Point::new(1.0, 1.0), radius: -1.0 }]).is_err());
    }
}
