//! Offset-based toolpaths (through `crates/geom`): LightBurn's Offset Fill (concentric inward offsets of closed
//! outlines at the line interval), Kerf Offset (closed outlines moved outward or inward before they are traced) and
//! the user start point of a closed outline (Set Start Point).
//!
//! Offsets run on `geom::Region`s (even-odd, as fills read shapes), so holes move the opposite way to their outer
//! outline: a positive kerf grows a part and shrinks its holes, and an offset fill closes in on holes as well as on
//! the outer edge. Results are polygons within `geom::TOLERANCE` of the true offset; inward offsets keep the convex
//! corners sharp and round the concave ones (the true offset).

use geometry::kurbo;
use geometry::offset::{Join, offset_region};
use geometry::{Fill, Region};

use crate::geom::{Point, dist};

/// Most rings one offset fill may make (with the plan's segment budget as the real limit).
pub const MAX_OFFSET_RINGS: usize = 100_000;

/// Why an offset could not be made, for the caller to wrap with the layer.
#[derive(Debug, Clone, PartialEq)]
pub enum OffsetError {
    Geometry(String),
    /// More rings or points than the budget allows.
    OverBudget,
}

fn region_of(rings: &[Vec<Point>]) -> Result<Region, OffsetError> {
    let mut path = kurbo::BezPath::new();
    for r in rings.iter().filter(|r| r.len() >= 3) {
        path.move_to((r[0][0], r[0][1]));
        for p in &r[1..] {
            path.line_to((p[0], p[1]));
        }
        path.close_path();
    }
    Region::from_path(&path, Fill::EvenOdd).map_err(|e| OffsetError::Geometry(e.to_string()))
}

fn contours(region: &Region) -> Vec<Vec<Point>> {
    region.shapes.iter().flatten().filter(|c| c.len() >= 3).cloned().collect()
}

/// Kerf Offset: the closed `rings` (one shape, even-odd) offset by `distance_mm` (positive outward, negative
/// inward), as closed rings (first point not repeated). An inward offset can make a ring vanish.
pub fn kerf(rings: &[Vec<Point>], distance_mm: f64) -> Result<Vec<Vec<Point>>, OffsetError> {
    if distance_mm == 0.0 || rings.is_empty() {
        return Ok(rings.to_vec());
    }
    let region = region_of(rings)?;
    let out = offset_region(&region, distance_mm, Join::Miter { limit: 4.0 }).map_err(|e| OffsetError::Geometry(e.to_string()))?;
    Ok(contours(&out))
}

/// Offset Fill: the outline of the closed `rings` and its inward offsets every `interval_mm` until nothing is left,
/// outermost first, as closed rings. Stops with [`OffsetError::OverBudget`] once more than `max_points` points (or
/// [`MAX_OFFSET_RINGS`] rings) would be made.
pub fn offset_fill(rings: &[Vec<Point>], interval_mm: f64, max_points: usize) -> Result<Vec<Vec<Point>>, OffsetError> {
    if !(interval_mm.is_finite() && interval_mm > 0.0) {
        return Err(OffsetError::Geometry(format!("the line interval must be > 0 (got {interval_mm})")));
    }
    let region = region_of(rings)?;
    if region.is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Vec<Point>> = contours(&region);
    let mut points: usize = out.iter().map(Vec::len).sum();
    for k in 1.. {
        let shrunk = offset_region(&region, -(k as f64) * interval_mm, Join::Round)
            .map_err(|e| OffsetError::Geometry(e.to_string()))?;
        if shrunk.is_empty() {
            break;
        }
        let rings = contours(&shrunk);
        points += rings.iter().map(Vec::len).sum::<usize>();
        if points > max_points || out.len() + rings.len() > MAX_OFFSET_RINGS {
            return Err(OffsetError::OverBudget);
        }
        out.extend(rings);
    }
    Ok(out)
}

/// Rotate the closed `ring` so it starts at its vertex nearest `at`, and reverse its direction when `reverse`
/// (the start vertex stays first).
pub fn start_at(ring: &mut [Point], at: Point, reverse: bool) {
    if ring.is_empty() {
        return;
    }
    let i = (0..ring.len()).min_by(|a, b| dist(ring[*a], at).total_cmp(&dist(ring[*b], at))).unwrap_or(0);
    ring.rotate_left(i);
    if reverse {
        ring[1..].reverse();
    }
}

/// Distance from `p` to the nearest vertex of `ring`.
pub fn ring_distance(ring: &[Point], p: Point) -> f64 {
    ring.iter().map(|q| dist(*q, p)).fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: f64, y0: f64, s: f64) -> Vec<Point> {
        vec![[x0, y0], [x0 + s, y0], [x0 + s, y0 + s], [x0, y0 + s]]
    }

    fn area(r: &[Point]) -> f64 {
        let n = r.len();
        (0..n).map(|i| r[i][0] * r[(i + 1) % n][1] - r[(i + 1) % n][0] * r[i][1]).sum::<f64>() / 2.0
    }

    #[test]
    fn offset_fill_makes_concentric_rings() {
        let rings = offset_fill(&[square(0.0, 0.0, 10.0)], 1.0, 1_000_000).unwrap();
        // 10 mm square at 1 mm: the outline, then 8x8, 6x6, 4x4, 2x2 (5 mm inset is a point: gone).
        assert_eq!(rings.len(), 5, "{rings:?}");
        let areas: Vec<f64> = rings.iter().map(|r| area(r).abs()).collect();
        for (a, side) in areas.iter().zip([10.0, 8.0, 6.0, 4.0, 2.0]) {
            assert!((a - side * side).abs() < 1e-6, "{areas:?}");
        }
    }

    #[test]
    fn offset_fill_closes_in_on_holes() {
        let rings = offset_fill(&[square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)], 1.0, 1_000_000).unwrap();
        // Outer + hole, then outer 8 with hole 4, outer 6 with hole 6 = gone (touching), ...
        assert!(rings.len() >= 4, "{}", rings.len());
        let holes = rings.iter().filter(|r| area(r) < 0.0).count();
        assert!(holes >= 2, "holes grow as the fill closes in on them");
    }

    #[test]
    fn offset_fill_budget_and_bad_interval() {
        assert_eq!(offset_fill(&[square(0.0, 0.0, 10.0)], 0.01, 100), Err(OffsetError::OverBudget));
        assert!(offset_fill(&[square(0.0, 0.0, 10.0)], 0.0, 100).is_err());
        assert!(offset_fill(&[], 1.0, 100).unwrap().is_empty());
    }

    #[test]
    fn kerf_grows_parts_and_shrinks_holes() {
        let out = kerf(&[square(0.0, 0.0, 10.0)], 0.5).unwrap();
        assert_eq!(out.len(), 1);
        assert!((area(&out[0]).abs() - 121.0).abs() < 1e-6);
        let inward = kerf(&[square(0.0, 0.0, 10.0)], -0.5).unwrap();
        assert!((area(&inward[0]).abs() - 81.0).abs() < 1e-6);
        let with_hole = kerf(&[square(0.0, 0.0, 10.0), square(4.0, 4.0, 2.0)], 0.5).unwrap();
        let hole = with_hole.iter().map(|r| area(r).abs()).fold(f64::INFINITY, f64::min);
        assert!((hole - 1.0).abs() < 1e-6, "the 2 mm hole shrinks to 1 mm: {hole}");
        assert!(kerf(&[square(0.0, 0.0, 10.0)], -6.0).unwrap().is_empty(), "shrunk away");
        assert_eq!(kerf(&[square(0.0, 0.0, 1.0)], 0.0).unwrap(), vec![square(0.0, 0.0, 1.0)]);
    }

    #[test]
    fn start_point_rotates_and_reverses() {
        let mut r = square(0.0, 0.0, 10.0);
        start_at(&mut r, [10.2, 9.9], false);
        assert_eq!(r, vec![[10.0, 10.0], [0.0, 10.0], [0.0, 0.0], [10.0, 0.0]]);
        let mut r = square(0.0, 0.0, 10.0);
        start_at(&mut r, [10.0, 10.0], true);
        assert_eq!(r, vec![[10.0, 10.0], [10.0, 0.0], [0.0, 0.0], [0.0, 10.0]]);
        assert_eq!(ring_distance(&r, [0.0, 0.0]), 0.0);
    }
}
