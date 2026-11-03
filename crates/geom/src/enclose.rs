//! Smallest enclosing circle of a point set (Welzl's algorithm, iterative form, on a deterministically shuffled
//! copy so the expected time is linear and every build gives the same answer). Used by Fit to Stock on round stock,
//! which fits a design's real outline (its convex hull) into the disc rather than its bounding box.

use kurbo::Point;

/// The smallest circle `(centre, radius)` holding every point, or `None` for an empty or non-finite set.
pub fn min_enclosing_circle(points: &[Point]) -> Option<(Point, f64)> {
    let mut pts: Vec<Point> = points.to_vec();
    if pts.is_empty() || pts.iter().any(|p| !(p.x.is_finite() && p.y.is_finite())) {
        return None;
    }
    shuffle(&mut pts);
    // Points this far outside still count as inside (rounding in the circumcircle).
    let eps = 1e-9 * (1.0 + pts.iter().map(|p| p.x.abs().max(p.y.abs())).fold(0.0, f64::max));
    let inside = |c: (Point, f64), p: Point| c.0.distance(p) <= c.1 + eps;
    let mut c = (pts[0], 0.0);
    for i in 1..pts.len() {
        if inside(c, pts[i]) {
            continue;
        }
        c = (pts[i], 0.0);
        for j in 0..i {
            if inside(c, pts[j]) {
                continue;
            }
            c = from_two(pts[i], pts[j]);
            for k in 0..j {
                if !inside(c, pts[k]) {
                    c = from_three(pts[i], pts[j], pts[k]).unwrap_or_else(|| widest_pair(pts[i], pts[j], pts[k]));
                }
            }
        }
    }
    Some(c)
}

fn from_two(a: Point, b: Point) -> (Point, f64) {
    let m = a.midpoint(b);
    (m, a.distance(b) / 2.0)
}

/// The circle through three points (`None` when they are on one line).
fn from_three(a: Point, b: Point, c: Point) -> Option<(Point, f64)> {
    let (bx, by) = (b.x - a.x, b.y - a.y);
    let (cx, cy) = (c.x - a.x, c.y - a.y);
    let d = 2.0 * (bx * cy - by * cx);
    if d.abs() < 1e-18 {
        return None;
    }
    let b2 = bx * bx + by * by;
    let c2 = cx * cx + cy * cy;
    let ux = (cy * b2 - by * c2) / d;
    let uy = (bx * c2 - cx * b2) / d;
    let centre = Point::new(a.x + ux, a.y + uy);
    Some((centre, centre.distance(a)))
}

/// Three points on one line: the circle on the two that are furthest apart.
fn widest_pair(a: Point, b: Point, c: Point) -> (Point, f64) {
    [from_two(a, b), from_two(a, c), from_two(b, c)].into_iter().max_by(|x, y| x.1.total_cmp(&y.1)).expect("three candidates")
}

/// Fisher-Yates with SplitMix64 from a fixed seed.
fn shuffle(pts: &mut [Point]) {
    let mut s: u64 = 0x5EED_1A55_C0FF_EE00;
    for i in (1..pts.len()).rev() {
        s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = s;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        let j = (z % (i as u64 + 1)) as usize;
        pts.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_of_a_square_and_a_triangle() {
        let sq = [Point::new(0.0, 0.0), Point::new(2.0, 0.0), Point::new(2.0, 2.0), Point::new(0.0, 2.0)];
        let (c, r) = min_enclosing_circle(&sq).unwrap();
        assert!(c.distance(Point::new(1.0, 1.0)) < 1e-9 && (r - 2f64.sqrt()).abs() < 1e-9);
        // An obtuse triangle: the circle on its longest side, not the circumcircle.
        let tri = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(5.0, 1.0)];
        let (c, r) = min_enclosing_circle(&tri).unwrap();
        assert!(c.distance(Point::new(5.0, 0.0)) < 1e-9 && (r - 5.0).abs() < 1e-9, "{c:?} {r}");
        // Collinear and repeated points.
        let line = [Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(3.0, 0.0), Point::new(1.0, 0.0)];
        let (c, r) = min_enclosing_circle(&line).unwrap();
        assert!(c.distance(Point::new(1.5, 0.0)) < 1e-9 && (r - 1.5).abs() < 1e-9);
        assert!(min_enclosing_circle(&[]).is_none());
        assert_eq!(min_enclosing_circle(&[Point::new(3.0, 4.0)]), Some((Point::new(3.0, 4.0), 0.0)));
    }

    #[test]
    fn every_point_is_inside_a_ring_of_many() {
        let pts: Vec<Point> = (0..400)
            .map(|i| {
                let a = f64::from(i) * 0.37;
                let r = 5.0 + (f64::from(i) * 1.7).sin() * 2.0;
                Point::new(3.0 + r * a.cos(), -1.0 + r * a.sin())
            })
            .collect();
        let (c, r) = min_enclosing_circle(&pts).unwrap();
        assert!(pts.iter().all(|p| c.distance(*p) <= r + 1e-7));
        // Tight: at least two points on the circle.
        assert!(pts.iter().filter(|p| (c.distance(**p) - r).abs() < 1e-6).count() >= 2);
    }
}
