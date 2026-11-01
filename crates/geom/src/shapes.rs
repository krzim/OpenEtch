//! Shape generators: regular polygons and stars (optionally with rounded corners), polylines.

use std::f64::consts::PI;

use kurbo::{BezPath, Line, PathSeg, Point};

use crate::ops::{fillet, line_corners};
use crate::subpath::{SubPath, join};
use crate::{Result, invalid};

/// Most sides (or star points) a generated polygon may have.
pub const MAX_SIDES: u32 = 1000;

/// Corners of a regular polygon with `sides` corners on a circle of `radius`, centred on the origin, the first corner
/// straight up (+Y), counter-clockwise. With `star_inner_ratio` (0 < r < 1) it is a star of `sides` points whose
/// inner corners lie at `radius * r`.
pub fn polygon_points(sides: u32, radius: f64, star_inner_ratio: Option<f64>) -> Result<Vec<Point>> {
    polygon_points_at(sides, radius, radius, star_inner_ratio, PI / 2.0)
}

/// As [`polygon_points`] on an ellipse of radii `rx` x `ry` (each corner `(rx cos a, ry sin a)`), the first corner at
/// angle `start` (radians, counter-clockwise from +X). LightBurn's Polygon and Star shapes start half a step past +Y
/// (`PI / 2 + PI / sides`), so an edge (a star's inner corner) is centred on +Y.
pub fn polygon_points_at(
    sides: u32,
    rx: f64,
    ry: f64,
    star_inner_ratio: Option<f64>,
    start: f64,
) -> Result<Vec<Point>> {
    if !(3..=MAX_SIDES).contains(&sides) {
        return invalid(format!("sides must be 3..={MAX_SIDES} (got {sides})"));
    }
    if !(rx.is_finite() && rx > 0.0 && ry.is_finite() && ry > 0.0) {
        return invalid("the radius must be > 0");
    }
    if let Some(r) = star_inner_ratio
        && !(r.is_finite() && r > 0.0 && r < 1.0)
    {
        return invalid("star_inner_ratio must be between 0 and 1 (exclusive)");
    }
    if !start.is_finite() {
        return invalid("the start angle must be finite");
    }
    let n = sides as usize;
    let at = |r: f64, a: f64| Point::new(rx * r * a.cos(), ry * r * a.sin());
    let pts = match star_inner_ratio {
        None => (0..n).map(|i| at(1.0, start + 2.0 * PI * i as f64 / n as f64)).collect(),
        Some(r) => (0..2 * n)
            .map(|i| at(if i % 2 == 0 { 1.0 } else { r }, start + PI * i as f64 / n as f64))
            .collect(),
    };
    Ok(pts)
}

/// A closed polygon through `points` with every corner rounded by `corner_radius` (corners where it does not fit
/// stay sharp; the second value counts them).
pub fn polygon_path(points: &[Point], corner_radius: f64) -> (BezPath, usize) {
    let sub = ring(points);
    if corner_radius == 0.0 || points.len() < 3 {
        return (join(&[sub]), 0);
    }
    let corners = line_corners(&sub);
    let (rounded, _, skipped) = fillet(&sub, &corners, corner_radius);
    (join(&[rounded]), skipped.len())
}

fn ring(points: &[Point]) -> SubPath {
    let n = points.len();
    let segs = (0..n).map(|i| PathSeg::Line(Line::new(points[i], points[(i + 1) % n]))).collect();
    SubPath { segs, closed: true }
}

/// A polyline through `points` (consecutive repeats dropped), closed back to the first point when `closed`.
pub fn polyline(points: &[Point], closed: bool) -> Result<BezPath> {
    let mut pts: Vec<Point> = Vec::with_capacity(points.len());
    for &p in points {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return invalid("every point must be finite");
        }
        if pts.last().is_none_or(|l| l.distance(p) > crate::EPS) {
            pts.push(p);
        }
    }
    if closed && pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) <= crate::EPS {
        pts.pop();
    }
    if pts.len() < 2 {
        return invalid("a line needs at least 2 distinct points");
    }
    if closed && pts.len() < 3 {
        return invalid("a closed line needs at least 3 distinct points");
    }
    let mut p = BezPath::new();
    p.move_to(pts[0]);
    for &q in &pts[1..] {
        p.line_to(q);
    }
    if closed {
        p.close_path();
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn hexagon_and_star() {
        let h = polygon_points(6, 10.0, None).unwrap();
        assert_eq!(h.len(), 6);
        assert!((h[0].y - 10.0).abs() < 1e-12 && h[0].x.abs() < 1e-12, "first corner up");
        let (p, _) = polygon_path(&h, 0.0);
        let expect = 1.5 * 3f64.sqrt() * 100.0;
        assert!((p.area() - expect).abs() < 1e-9, "counter-clockwise: {}", p.area());
        let s = polygon_points(5, 10.0, Some(0.5)).unwrap();
        assert_eq!(s.len(), 10);
        assert!((s[1].to_vec2().hypot() - 5.0).abs() < 1e-12);
        assert!(polygon_points(2, 1.0, None).is_err());
        assert!(polygon_points(MAX_SIDES + 1, 1.0, None).is_err());
        assert!(polygon_points(5, 1.0, Some(1.0)).is_err());
        assert!(polygon_points(5, -1.0, None).is_err());
    }

    #[test]
    fn rounded_polygon() {
        let sq = polygon_points(4, 10.0, None).unwrap();
        let (p, skipped) = polygon_path(&sq, 1.0);
        assert_eq!(skipped, 0);
        let (sharp, _) = polygon_path(&sq, 0.0);
        assert!(p.area() < sharp.area() && p.area() > sharp.area() - 1.0);
        let (_, skipped) = polygon_path(&sq, 100.0);
        assert_eq!(skipped, 4, "too big to fit stays sharp");
    }

    #[test]
    fn polylines() {
        let p = polyline(&[Point::new(0.0, 0.0), Point::new(0.0, 0.0), Point::new(5.0, 0.0)], false).unwrap();
        assert_eq!(p.elements().len(), 2);
        assert!(polyline(&[Point::new(0.0, 0.0)], false).is_err());
        assert!(polyline(&[Point::new(0.0, 0.0), Point::new(1.0, 0.0)], true).is_err());
        assert!(polyline(&[Point::new(f64::NAN, 0.0), Point::new(1.0, 0.0)], false).is_err());
        let t = polyline(&[Point::new(0.0, 0.0), Point::new(4.0, 0.0), Point::new(0.0, 3.0), Point::new(0.0, 0.0)], true).unwrap();
        assert!((t.area().abs() - 6.0).abs() < 1e-12);
    }
}
