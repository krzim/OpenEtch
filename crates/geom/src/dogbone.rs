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
//! between two reliefs cannot be negative) and may cut into no other line or curve of the shape (a nearby hole, the
//! far wall of a thin part) nor into another relief; corners where it does not are skipped with the reason. An
//! outline that crosses itself has no known inside and is left alone.

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

/// A flattened piece of the shape (`seg`: its subpath and segment) or a relief already planned (`None`).
struct Obstacle {
    seg: Option<(usize, usize)>,
    points: Vec<Point>,
    bbox: kurbo::Rect,
}

impl Obstacle {
    fn new(seg: Option<(usize, usize)>, points: Vec<Point>) -> Obstacle {
        let bbox = bounds(&points);
        Obstacle { seg, points, bbox }
    }

    /// Whether the polyline `other` crosses this one.
    fn crosses(&self, other: &[Point]) -> bool {
        let b = bounds(other);
        if b.x0 > self.bbox.x1 || b.x1 < self.bbox.x0 || b.y0 > self.bbox.y1 || b.y1 < self.bbox.y0 {
            return false;
        }
        self.points.windows(2).any(|s| other.windows(2).any(|t| segments_cross(s[0], s[1], t[0], t[1])))
    }
}

fn bounds(points: &[Point]) -> kurbo::Rect {
    points.iter().fold(kurbo::Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |r, p| {
        kurbo::Rect::new(r.x0.min(p.x), r.y0.min(p.y), r.x1.max(p.x), r.y1.max(p.y))
    })
}

/// Whether segments `ab` and `cd` cross (properly, not merely touching at an end).
fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let o = |p: Point, q: Point, r: Point| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    let tol = 1e-12;
    ((d1 > tol && d2 < -tol) || (d1 < -tol && d2 > tol)) && ((d3 > tol && d4 < -tol) || (d3 < -tol && d4 > tol))
}

/// Whether a closed ring crosses itself (rings of more than a few thousand points are not checked).
fn self_crossing(ring: &[Point]) -> bool {
    let n = ring.len();
    if n > 4000 {
        return false;
    }
    (0..n).any(|j| (j + 2..n).any(|k| !(j == 0 && k == n - 1) && segments_cross(ring[j], ring[(j + 1) % n], ring[k], ring[(k + 1) % n])))
}

/// Points along a relief's arc of radius `r` about `centre`, from `from` to `to` through `probe` (as the rebuilt
/// outline draws it).
fn relief_points(centre: Point, r: f64, from: Point, to: Point, probe: Point) -> Vec<Point> {
    let tau = std::f64::consts::TAU;
    let a0 = (from - centre).atan2();
    let ccw = ((to - centre).atan2() - a0).rem_euclid(tau);
    let off = ((probe - centre).atan2() - a0).rem_euclid(tau);
    let sweep = if off < ccw { ccw } else { ccw - tau };
    let n = ((sweep.abs() / 0.05).ceil() as usize).max(8);
    let mut pts: Vec<Point> = (0..=n).map(|s| centre + Vec2::from_angle(a0 + sweep * s as f64 / n as f64) * r).collect();
    pts[0] = from;
    pts[n] = to;
    pts
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
    // Every piece of every closed outline, flattened, to check that a relief cuts into nothing else.
    let mut obstacles: Vec<Obstacle> = Vec::new();
    for (i, sub) in subs.iter().enumerate() {
        if rings[i].is_none() {
            continue;
        }
        for (k, seg) in sub.segs.iter().enumerate() {
            let points = match seg {
                PathSeg::Line(l) => vec![l.p0, l.p1],
                other => {
                    let mut p = BezPath::new();
                    p.move_to(other.start());
                    p.push(other.as_path_el());
                    flatten(&p, TOLERANCE / 10.0)?.into_iter().flat_map(|l| l.points).collect()
                }
            };
            obstacles.push(Obstacle::new(Some((i, k)), points));
        }
    }
    let mut corners = Vec::new();
    let mut skipped = Vec::new();
    for (i, sub) in subs.iter_mut().enumerate() {
        let Some(ring) = &rings[i] else { continue };
        if self_crossing(ring) {
            skipped.push(SkippedCorner { at: ring[0], reason: "the outline crosses itself, so which side is material is not known".into() });
            continue;
        }
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
            // What is left of each line between this relief and the one at its other end must stay positive.
            let left_prev = lens[k] - trims[k].0 - trims[k].1 - t_in;
            let left_next = lens[next] - trims[next].0 - trims[next].1 - t_out;
            if (t_in > 0.0 && left_prev < EPS) || (t_out > 0.0 && left_next < EPS) {
                skipped.push(SkippedCorner { at: v, reason: "the relief does not fit on the lines at this corner".into() });
                continue;
            }
            // The relief's arc may cut into no other line or curve of the shape, nor into another relief.
            let arc = relief_points(centre, r, v + e_in * t_in, v + e_out * t_out, probe);
            let hits = obstacles.iter().any(|o| o.seg != Some((i, k)) && o.seg != Some((i, next)) && o.crosses(&arc));
            if hits {
                skipped.push(SkippedCorner { at: v, reason: "the relief would cut into another edge of the shape".into() });
                continue;
            }
            obstacles.push(Obstacle::new(None, arc));
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

    /// Whether any closed outline of `p` crosses itself or another one.
    fn crossing(p: &BezPath) -> bool {
        let rings: Vec<Vec<Point>> = flatten(p, 1e-4).unwrap().into_iter().map(|l| l.points).collect();
        rings.iter().any(|r| self_crossing(r))
            || rings.iter().enumerate().any(|(i, a)| {
                rings[i + 1..].iter().any(|b| {
                    let close = |r: &Vec<Point>| r.iter().copied().chain(std::iter::once(r[0])).collect::<Vec<_>>();
                    Obstacle::new(None, close(a)).crosses(&close(b))
                })
            })
    }

    #[test]
    fn reliefs_never_overlap_or_cut_into_other_edges() {
        // A 1 mm wide slot and a 1 mm tool: one relief takes 0.71 mm of the 1 mm bottom, so only one fits.
        let narrow = poly(&[&[(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.5, 10.0), (10.5, 5.0), (9.5, 5.0), (9.5, 10.0), (0.0, 10.0)]]);
        let d = dogbone(&narrow, 1.0, Relief::Dogbone).unwrap();
        assert_eq!((d.corners.len(), d.skipped.len()), (1, 1), "{d:?}");
        assert!(!crossing(&d.path));
        // An L with 1 mm arms and a 7 mm tool: the relief fits on its lines but would break through the outer walls.
        let thin = poly(&[&[(0.0, 0.0), (10.0, 0.0), (10.0, 1.0), (1.0, 1.0), (1.0, 10.0), (0.0, 10.0)]]);
        for relief in [Relief::Dogbone, Relief::TboneLonger, Relief::TboneShorter] {
            let d = dogbone(&thin, 7.0, relief).unwrap();
            assert!(d.corners.is_empty(), "{relief:?}: {d:?}");
            assert!(d.skipped[0].reason.contains("another edge"), "{relief:?}: {:?}", d.skipped);
            // A small tool still fits.
            let d = dogbone(&thin, 1.0, relief).unwrap();
            assert_eq!(d.corners.len(), 1);
            assert!(!crossing(&d.path));
        }
        // A relief may not reach into a hole next to the corner, nor into a curve.
        // (A 2 mm relief at (12, 5) dips to y = 4.71 at x = 11.29; the hole's top is at y = 4.9 under it.)
        let mut near_hole = poly(&[&[(11.0, 4.5), (11.6, 4.5), (11.6, 4.9), (11.0, 4.9)]]);
        near_hole.extend(slotted());
        let d = dogbone(&near_hole, 2.0, Relief::Dogbone).unwrap();
        assert_eq!(d.corners, vec![Point::new(8.0, 5.0)], "{d:?}");
        assert!(!crossing(&d.path));
        // A self-crossing outline (a bow tie) has no known inside: left alone.
        let bowtie = poly(&[&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)]]);
        let d = dogbone(&bowtie, 1.0, Relief::Dogbone).unwrap();
        assert!(d.corners.is_empty() && d.skipped[0].reason.contains("crosses itself"), "{d:?}");
        // Many slots in a comb, tiny and large tools: never a crossing outline.
        let mut comb = vec![(0.0, 0.0), (200.0, 0.0), (200.0, 10.0)];
        for s in (0..50).rev() {
            let x = 4.0 * s as f64;
            comb.extend([(x + 3.0, 10.0), (x + 3.0, 4.0), (x + 1.0, 4.0), (x + 1.0, 10.0)]);
        }
        comb.push((0.0, 10.0));
        let comb = poly(&[&comb]);
        for dia in [1e-6, 0.5, 1.0, 1.5, 3.0, 50.0] {
            let d = dogbone(&comb, dia, Relief::Dogbone).unwrap();
            assert_eq!(d.corners.len() + d.skipped.len(), 100, "{dia}");
            assert!(!crossing(&d.path), "{dia}");
        }
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
