//! Offsets (`i_overlay` outlines and strokes). Results are polygons: round joins and offset curves are flattened
//! to within [`TOLERANCE`](crate::TOLERANCE), so an offset circle is a many-sided polygon, not a circle. Exception:
//! `i_overlay` never steps an arc by less than 0.01π, so round joins and caps of radius above ~81 mm stray up to
//! R·(1 − cos 0.005π) from the true arc (about 0.06 mm at 500 mm, 0.12 mm at 1000 mm).
//!
//! Cost: `i_overlay` resolves every crossing between the offset pieces of the edges, so a dense zigzag outline
//! offset far past its node spacing is roughly quadratic in its node count (a 4,000-node star took ~14 s outward by
//! 500 mm and ~115 s inward by 45 mm; a 10,000-node one ~6 min outward by 1000 mm). Smooth outlines stay fast.

use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};
use kurbo::Point;

use crate::region::Region;
use crate::{Result, TOLERANCE, guard, invalid};

/// How offset corners that point away from the shape are drawn (corners pointing the other way stay sharp, as in
/// LightBurn).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Join {
    /// A circular arc around the corner (the true offset).
    Round,
    /// A sharp corner, cut off (bevelled) when the miter would reach further than `limit` times the distance
    /// (SVG's miter limit; 4 cuts corners sharper than about 29°). LightBurn's "Corner".
    Miter { limit: f64 },
    /// The corner cut off straight.
    Bevel,
}

/// Arc step (segment length / radius) that keeps a round join within [`TOLERANCE`] of the arc.
fn round_step(radius: f64) -> f64 {
    let r = radius.abs().max(TOLERANCE);
    // i_overlay clamps the step to 0.01π..0.25π itself, so arcs of radius above ~81 mm stray further than TOLERANCE
    // (by R·(1 − cos 0.005π): 0.12 mm at 1000 mm). See the module docs.
    (8.0 * TOLERANCE / r).sqrt().clamp(0.01 * std::f64::consts::PI, 0.25 * std::f64::consts::PI)
}

fn line_join(join: Join, distance: f64) -> LineJoin<f64> {
    match join {
        Join::Round => LineJoin::Round(round_step(distance)),
        Join::Bevel => LineJoin::Bevel,
        Join::Miter { limit } => {
            let limit = if limit.is_finite() { limit.max(1.0) } else { 1e6 };
            LineJoin::Miter(2.0 * (1.0 / limit).asin())
        }
    }
}

/// Grow (`distance > 0`) or shrink (`distance < 0`) a region by `distance` mm. Shrinking can make parts vanish or
/// split; 0 returns the region as it is (already cleaned of self-overlaps).
pub fn offset_region(region: &Region, distance: f64, join: Join) -> Result<Region> {
    if !distance.is_finite() {
        return invalid("the offset distance must be a finite number");
    }
    if distance == 0.0 || region.is_empty() {
        return Ok(region.clone());
    }
    crate::region::check_range(region.shapes.iter().flatten().flatten())?;
    let style = OutlineStyle::new(distance).line_join(line_join(join, distance));
    let shapes = guard("offset", || region.shapes.outline(&style))?;
    Ok(Region { shapes: shapes.into_iter().filter(|s| s.first().is_some_and(|c| c.len() >= 3)).collect() })
}

/// The area within `distance` mm (> 0) of an open polyline: a closed band around it with round ends.
pub fn offset_polyline(points: &[Point], distance: f64, join: Join) -> Result<Region> {
    if !(distance.is_finite() && distance > 0.0) {
        return invalid("an open path can only be offset outward, by a distance > 0");
    }
    let pts: Vec<[f64; 2]> = points.iter().map(|p| [p.x, p.y]).collect();
    if pts.len() < 2 {
        return Ok(Region::default());
    }
    crate::region::check_range(&pts)?;
    let step = round_step(distance);
    let style = StrokeStyle::new(2.0 * distance)
        .line_join(line_join(join, distance))
        .start_cap(LineCap::Round(step))
        .end_cap(LineCap::Round(step));
    let shapes = guard("stroke", || pts.stroke(style, false))?;
    Ok(Region { shapes: shapes.into_iter().filter(|s| s.first().is_some_and(|c| c.len() >= 3)).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::Fill;
    use kurbo::{BezPath, Rect, Shape};

    fn square() -> Region {
        Region::from_path(&Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01), Fill::EvenOdd).unwrap()
    }

    #[test]
    fn outward_inward_and_joins() {
        let s = square();
        let miter = offset_region(&s, 1.0, Join::Miter { limit: 4.0 }).unwrap();
        assert!((miter.area() - 144.0).abs() < 1e-3, "{}", miter.area());
        let round = offset_region(&s, 1.0, Join::Round).unwrap();
        let expect = 100.0 + 40.0 + std::f64::consts::PI;
        assert!((round.area() - expect).abs() < 0.05, "{}", round.area());
        let bevel = offset_region(&s, 1.0, Join::Bevel).unwrap();
        assert!((bevel.area() - 142.0).abs() < 1e-3, "{}", bevel.area());
        let inner = offset_region(&s, -1.0, Join::Round).unwrap();
        assert!((inner.area() - 64.0).abs() < 1e-3, "{}", inner.area());
        assert!(offset_region(&s, -6.0, Join::Round).unwrap().is_empty(), "shrunk away");
        assert_eq!(offset_region(&s, 0.0, Join::Round).unwrap(), s);
        assert!(offset_region(&s, f64::NAN, Join::Round).is_err());
    }

    #[test]
    fn holes_shrink_when_the_shape_grows() {
        let mut p = Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01);
        p.extend(Rect::new(3.0, 3.0, 7.0, 7.0).to_path(0.01));
        let r = Region::from_path(&p, Fill::EvenOdd).unwrap();
        let out = offset_region(&r, 1.0, Join::Miter { limit: 4.0 }).unwrap();
        assert!((out.area() - (144.0 - 4.0)).abs() < 1e-3, "{}", out.area());
        let gone = offset_region(&r, 2.5, Join::Miter { limit: 4.0 }).unwrap();
        assert_eq!(gone.shapes[0].len(), 1, "the hole closed up");
    }

    #[test]
    fn circles_offset_within_tolerance() {
        let c = Region::from_path(&kurbo::Circle::new((0.0, 0.0), 10.0).to_path(0.01), Fill::EvenOdd).unwrap();
        let o = offset_region(&c, 2.0, Join::Round).unwrap();
        let b = o.bounds().unwrap();
        assert!((b.width() - 24.0).abs() < 0.03, "{b:?}");
        for p in o.shapes[0][0].iter() {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 12.0).abs() < 0.03, "{r}");
        }
    }

    #[test]
    fn large_round_offsets_stay_within_the_documented_bound() {
        // The rounded corners of a square offset by 1000 mm: i_overlay's finest arc step (0.01π) bounds the error at
        // R·(1 − cos 0.005π) ≈ 0.123 mm, not TOLERANCE (the module docs say so).
        let o = offset_region(&square(), 1000.0, Join::Round).unwrap();
        let ring: Vec<Point> = o.shapes[0][0].iter().map(|p| Point::new(p[0], p[1])).collect();
        let corner = Point::new(10.0, 10.0);
        let mut worst: f64 = 0.0;
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            if a.x > 10.0 && a.y > 10.0 && b.x > 10.0 && b.y > 10.0 {
                worst = worst.max(1000.0 - a.midpoint(b).distance(corner));
                worst = worst.max((a.distance(corner) - 1000.0).abs());
            }
        }
        let bound = 1000.0 * (1.0 - (0.005 * std::f64::consts::PI).cos()) + TOLERANCE;
        assert!(worst > 0.0 && worst <= bound, "{worst} > {bound}");
        // Up to ~80 mm the arcs stay within TOLERANCE.
        let o = offset_region(&square(), 80.0, Join::Round).unwrap();
        let worst = o.shapes[0][0]
            .iter()
            .map(|p| Point::new(p[0], p[1]))
            .filter(|p| p.x > 10.0 && p.y > 10.0)
            .map(|p| (p.distance(corner) - 80.0).abs())
            .fold(0.0, f64::max);
        let ring: Vec<Point> = o.shapes[0][0].iter().map(|p| Point::new(p[0], p[1])).collect();
        let sag = (0..ring.len())
            .map(|i| (ring[i], ring[(i + 1) % ring.len()]))
            .filter(|(a, b)| a.x > 10.0 && a.y > 10.0 && b.x > 10.0 && b.y > 10.0)
            .map(|(a, b)| 80.0 - a.midpoint(b).distance(corner))
            .fold(worst, f64::max);
        assert!(sag <= TOLERANCE * 1.05, "{sag}");
    }

    #[test]
    fn open_lines_become_bands() {
        let band = offset_polyline(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0)], 1.0, Join::Round).unwrap();
        let expect = 20.0 + std::f64::consts::PI;
        assert!((band.area() - expect).abs() < 0.05, "{}", band.area());
        assert!(offset_polyline(&[Point::new(0.0, 0.0), Point::new(1.0, 0.0)], -1.0, Join::Round).is_err());
        assert!(offset_polyline(&[Point::new(0.0, 0.0)], 1.0, Join::Round).unwrap().is_empty());
        let _ = BezPath::new();
    }
}
