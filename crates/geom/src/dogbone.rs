//! Dogbone and T-bone reliefs (MillMage's and CNC CAM's Dogbone tool, OTH-003): a round tool or a wide kerf cannot
//! cut the sharp inside corner of a slot or pocket, so a mating part with a square corner would not seat. A relief
//! cuts a circle of the tool's diameter past every inside corner so the corner itself is cleared:
//!
//! - **Dogbone**: the circle's centre sits on the corner's bisector, one radius from the corner, so the circle passes
//!   through the corner and bites equally into both walls (on a right angle, a half circle).
//! - **T-bone**: the circle touches one wall at the corner and bites only into the other one (the `longer` or the
//!   `shorter` of the two lines meeting there), so the first wall stays straight to the corner.
//!
//! Which corners are inside corners follows from the material: inside every closed subpath at even nesting depth
//! and outside the holes (odd depth), as the shape is filled. A corner qualifies when both lines meeting there are
//! straight and the material's angle there is more than 180° (a reflex corner of a part, any corner of a hole).
//! Open subpaths and corners touching a curve are left alone. A relief must fit on both lines (what is left of a line
//! between two reliefs cannot be negative); corners where it does not are skipped with the reason.

use kurbo::{Arc, BezPath, Line, ParamCurve, PathSeg, Point, Vec2};

use crate::flatten::{flatten, point_in_ring, polygon_area};
use crate::ops::arc_segments;
use crate::subpath::{SubPath, join, split};
use crate::{EPS, Result, TOLERANCE, invalid};

/// Which relief [`dogbone`] cuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relief {
    /// Along the corner's bisector, into both walls.
    Dogbone,
    /// Into the longer of the two lines at the corner only.
    TboneLonger,
    /// Into the shorter of the two lines at the corner only.
    TboneShorter,
}

/// A corner [`dogbone`] did not relieve.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedCorner {
    pub at: Point,
    pub reason: String,
}

/// What [`dogbone`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Dogboned {
    pub path: BezPath,
    /// The inside corners relieved.
    pub corners: Vec<Point>,
    pub skipped: Vec<SkippedCorner>,
}

/// Largest tool diameter, mm.
pub const MAX_DIAMETER_MM: f64 = 100.0;

/// The corner turns by less than this (radians) on either side: not a corner.
const MIN_TURN: f64 = 1.0 * std::f64::consts::PI / 180.0;

/// One planned relief at the corner between segment `prev` and segment `next` of a subpath.
struct Planned {
    centre: Point,
    /// A point of the circle the arc must pass through (in the material).
    probe: Point,
    /// How much of `prev` (from its end) and of `next` (from its start) the relief takes.
    trim_prev: f64,
    trim_next: f64,
}

fn unit(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l > 0.0 { v / l } else { v }
}

/// Add reliefs of `diameter_mm` to every inside corner of `path` (see the module docs).
pub fn dogbone(path: &BezPath, diameter_mm: f64, relief: Relief) -> Result<Dogboned> {
    if !(diameter_mm.is_finite() && diameter_mm > 0.0 && diameter_mm <= MAX_DIAMETER_MM) {
        return invalid(format!("the tool diameter must be > 0 and at most {MAX_DIAMETER_MM} mm (got {diameter_mm})"));
    }
    if !crate::is_finite(path) {
        return invalid("the path has a coordinate that is not a finite number");
    }
    let r = diameter_mm / 2.0;
    let mut subs = split(path);
    // Each closed subpath's outline, for its orientation and nesting.
    let rings: Vec<Option<Vec<Point>>> = subs
        .iter()
        .map(|s| {
            if !s.closed {
                return Ok(None);
            }
            Ok(flatten(&join(std::slice::from_ref(s)), TOLERANCE)?.into_iter().next().filter(|l| l.closed && l.points.len() > 2).map(|l| l.points))
        })
        .collect::<Result<_>>()?;
    let mut corners = Vec::new();
    let mut skipped = Vec::new();
    for (i, sub) in subs.iter_mut().enumerate() {
        let Some(ring) = &rings[i] else { continue };
        let probe = ring[0];
        let depth = rings.iter().enumerate().filter(|(j, o)| *j != i && o.as_ref().is_some_and(|o| point_in_ring(probe, o))).count();
        let material_left = (polygon_area(ring) > 0.0) == (depth % 2 == 0);
        let n = sub.segs.len();
        let lens: Vec<f64> = sub.segs.iter().map(|s| if let PathSeg::Line(l) = s { l.p0.distance(l.p1) } else { 0.0 }).collect();
        let mut trims = vec![(0.0f64, 0.0f64); n];
        let mut planned: Vec<Option<Planned>> = (0..n).map(|_| None).collect();
        for k in 0..n {
            // The corner at the end of segment `k`, where segment `k + 1` starts.
            let next = (k + 1) % n;
            let (PathSeg::Line(a), PathSeg::Line(b)) = (sub.segs[k], sub.segs[next]) else { continue };
            if n < 2 {
                continue;
            }
            let v = a.p1;
            let u = unit(a.p1 - a.p0);
            let w = unit(b.p1 - b.p0);
            let cross = u.cross(w);
            let inside = if material_left { cross < -MIN_TURN.sin() } else { cross > MIN_TURN.sin() };
            if !inside {
                continue;
            }
            let (e_in, e_out) = (-u, w);
            let (centre, probe, t_in, t_out) = match relief {
                Relief::Dogbone => {
                    let d = unit(e_in + e_out);
                    let c = v + d * r;
                    (c, v, 2.0 * r * e_in.dot(d), 2.0 * r * e_out.dot(d))
                }
                Relief::TboneLonger | Relief::TboneShorter => {
                    // The relief bites into wall `b` only; it touches wall `a` at the corner.
                    let prev_is_b = (lens[k] >= lens[next]) == (relief == Relief::TboneLonger);
                    let (ea, eb) = if prev_is_b { (e_out, e_in) } else { (e_in, e_out) };
                    let na = unit(eb - ea * eb.dot(ea));
                    let nb = unit(ea - eb * ea.dot(eb));
                    let c = v + na * r;
                    let tb = 2.0 * r * eb.dot(na);
                    let probe = c - nb * r;
                    if prev_is_b { (c, probe, tb, 0.0) } else { (c, probe, 0.0, tb) }
                }
            };
            if trims[k].1 + t_in > lens[k] - EPS || trims[next].0 + t_out > lens[next] - EPS && t_out > 0.0 {
                skipped.push(SkippedCorner { at: v, reason: "the relief does not fit on the lines at this corner".into() });
                continue;
            }
            trims[k].1 += t_in;
            trims[next].0 += t_out;
            planned[k] = Some(Planned { centre, probe, trim_prev: t_in, trim_next: t_out });
            corners.push(v);
        }
        if planned.iter().all(Option::is_none) {
            continue;
        }
        // Rebuild: each line shortened at the reliefs, each relief an arc between them.
        let trimmed: Vec<PathSeg> = sub
            .segs
            .iter()
            .zip(&trims)
            .map(|(s, (t0, t1))| match s {
                PathSeg::Line(l) => {
                    let d = unit(l.p1 - l.p0);
                    PathSeg::Line(Line::new(l.p0 + d * *t0, l.p1 - d * *t1))
                }
                other => *other,
            })
            .collect();
        let mut segs = Vec::with_capacity(n * 3);
        for k in 0..n {
            let s = trimmed[k];
            if s.start().distance(s.end()) > EPS {
                segs.push(s);
            }
            if let Some(p) = &planned[k] {
                let from = trimmed[k].end();
                let to = trimmed[(k + 1) % n].start();
                debug_assert!(p.trim_prev >= 0.0 && p.trim_next >= 0.0);
                let a0 = (from - p.centre).atan2();
                let a1 = (to - p.centre).atan2();
                let ap = (p.probe - p.centre).atan2();
                let tau = std::f64::consts::TAU;
                let ccw = (a1 - a0).rem_euclid(tau);
                let off = (ap - a0).rem_euclid(tau);
                let sweep = if off < ccw { ccw } else { ccw - tau };
                let arc = Arc { center: p.centre, radii: Vec2::new(r, r), start_angle: a0, sweep_angle: sweep, x_rotation: 0.0 };
                segs.extend(arc_segments(&arc, from, to));
            }
        }
        // Every segment starts where the last one ended; snap away rounding.
        for k in 1..segs.len() {
            let p = segs[k - 1].end();
            segs[k] = crate::subpath::with_start(segs[k], p);
        }
        if let (Some(first), Some(last)) = (segs.first().copied(), segs.len().checked_sub(1)) {
            segs[last] = crate::subpath::with_end(segs[last], first.start());
        }
        *sub = SubPath { segs, closed: true };
    }
    Ok(Dogboned { path: join(&subs), corners, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn poly(rings: &[&[(f64, f64)]]) -> BezPath {
        let mut p = BezPath::new();
        for r in rings {
            p.move_to(r[0]);
            for q in &r[1..] {
                p.line_to(*q);
            }
            p.close_path();
        }
        p
    }

    /// A 20 x 10 part with a 4 mm wide, 5 mm deep slot in its top edge (CCW).
    fn slotted() -> BezPath {
        poly(&[&[(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (12.0, 10.0), (12.0, 5.0), (8.0, 5.0), (8.0, 10.0), (0.0, 10.0)]])
    }

    #[test]
    fn dogbones_go_into_the_slot_corners_only() {
        let d = dogbone(&slotted(), 1.0, Relief::Dogbone).unwrap();
        assert_eq!(d.corners, vec![Point::new(12.0, 5.0), Point::new(8.0, 5.0)], "the two inside corners");
        assert!(d.skipped.is_empty());
        // A right-angle dogbone of radius r adds a half circle minus the corner's square: pi r^2 / 2 - r^2 / 2 ... less
        // material: the area shrinks by the circle part outside the slot.
        let before = slotted().area();
        let after = d.path.area();
        let r: f64 = 0.5;
        let removed = before - after;
        // The circle (centre r from the corner on the bisector) outside the slot's quarter: the half disc beyond the
        // chord through the two wall points, minus the triangle between that chord and the corner.
        let expected = 2.0 * (std::f64::consts::PI * r * r / 2.0 - r * r);
        assert!((removed - expected).abs() < 1e-3, "{removed} vs {expected}");
        // The relief passes through the corner itself: the corner point is on the new outline.
        let flat = flatten(&d.path, 1e-4).unwrap();
        let near = flat[0].points.iter().map(|p| p.distance(Point::new(8.0, 5.0))).fold(f64::INFINITY, f64::min);
        assert!(near < 1e-3, "{near}");
        // The orientation of the outline does not matter.
        let rev = dogbone(&slotted().reverse_subpaths(), 1.0, Relief::Dogbone).unwrap();
        assert_eq!(rev.corners.len(), 2);
        assert!((rev.path.area().abs() - after.abs()).abs() < 1e-6);
    }

    #[test]
    fn holes_get_reliefs_in_every_corner() {
        // A 30 x 30 plate with a 10 x 6 rectangular hole (a mortise): all four hole corners are inside corners.
        let p = poly(&[&[(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)], &[(10.0, 10.0), (20.0, 10.0), (20.0, 16.0), (10.0, 16.0)]]);
        let d = dogbone(&p, 2.0, Relief::Dogbone).unwrap();
        assert_eq!(d.corners.len(), 4, "{:?}", d.corners);
        assert!(d.corners.iter().all(|c| c.x >= 10.0 && c.x <= 20.0), "the plate's own corners are outside corners");
        // A circle that does not fit is skipped: on a 1 mm wide slot a 3 mm tool needs more than the walls give.
        let tight = poly(&[&[(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.5, 10.0), (10.5, 9.0), (9.5, 9.0), (9.5, 10.0), (0.0, 10.0)]]);
        let d = dogbone(&tight, 3.0, Relief::Dogbone).unwrap();
        assert!(!d.skipped.is_empty());
    }

    #[test]
    fn tbones_bite_into_one_wall() {
        let d = dogbone(&slotted(), 1.0, Relief::TboneLonger).unwrap();
        assert_eq!(d.corners.len(), 2);
        let flat = flatten(&d.path, 1e-4).unwrap();
        let pts = &flat[0].points;
        // The slot's sides (5 mm) are longer than its bottom (4 mm): the relief goes into the sides, so the outline
        // reaches 0.5 mm past x = 8 and x = 12 near the bottom, and the bottom stays at y = 5 exactly.
        let min_x = pts.iter().filter(|p| p.y < 7.0 && p.x > 5.0 && p.x < 15.0).map(|p| p.x).fold(f64::INFINITY, f64::min);
        assert!((min_x - 7.5).abs() < 1e-3, "{min_x}");
        assert!(pts.iter().filter(|p| p.x > 7.5 && p.x < 12.5).all(|p| p.y >= 5.0 - 1e-6));
        let d = dogbone(&slotted(), 1.0, Relief::TboneShorter).unwrap();
        let flat = flatten(&d.path, 1e-4).unwrap();
        let min_y = flat[0].points.iter().filter(|p| p.x > 7.0 && p.x < 13.0).map(|p| p.y).fold(f64::INFINITY, f64::min);
        assert!((min_y - 4.5).abs() < 1e-3, "into the bottom: {min_y}");
    }

    #[test]
    fn bad_input_is_refused() {
        assert!(dogbone(&slotted(), 0.0, Relief::Dogbone).is_err());
        assert!(dogbone(&slotted(), f64::NAN, Relief::Dogbone).is_err());
        // An open path has no inside corners.
        let mut open = BezPath::new();
        open.move_to((0.0, 0.0));
        open.line_to((10.0, 0.0));
        open.line_to((10.0, 10.0));
        let d = dogbone(&open, 1.0, Relief::Dogbone).unwrap();
        assert!(d.corners.is_empty());
    }
}
