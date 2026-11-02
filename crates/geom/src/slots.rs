//! Resize Slots & Tabs (LightBurn's Tools > Resize Slots in Selection, DES-024): fit a slotted or finger-jointed
//! design to another material thickness by moving the lines of its slots and tabs.
//!
//! LightBurn documents what the tool does, not how it finds slots; this is OpenEtch's reading. It works on the
//! closed outlines of parts made of straight lines (curves make an outline be skipped, as LightBurn needs sharp
//! corners), in the field frame:
//!
//! - Nodes along a straight line are merged first (LightBurn's detection breaks on extra nodes; this one does not).
//! - A **step** is a line whose length is within `old ± tolerance`, with one convex and one reflex corner, between two
//!   lines that are perpendicular to it and run the same way: the side of a slot or of a tab (finger). Its reflex
//!   end meets the *inner* level (the bottom of the slots), its convex end the *outer* level (the top of the tabs).
//! - **Adjust slot depth** moves the inner-level line at every step so the step becomes `new` long (the assembled
//!   outer size stays). **Adjust tab height** moves the outer-level line instead (the inner size stays). With both,
//!   each level moves half the change.
//! - **Adjust slot width** finds U-shaped slots (a line between two reflex corners whose neighbours run back towards
//!   each other, perpendicular to it) whose width is within `old ± tolerance` and moves both sides apart (or
//!   together) by half the change each, keeping the slot's centre: slots cut into the middle of a piece, like
//!   interlocking tray dividers.
//!
//! Moved lines keep their direction; every corner is recomputed where its two lines now meet. Holes (internal slots,
//! cutouts, mortises) are left alone, as in LightBurn, since which way they should grow is not known; so is an
//! outline that would fold over itself.

use kurbo::{BezPath, ParamCurve, PathSeg, Point, Vec2};

use crate::flatten::{flatten, point_in_ring, polygon_area};
use crate::subpath::{SubPath, join, split};
use crate::{Result, TOLERANCE, invalid};

/// What [`resize_slots`] changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotOptions {
    /// The material thickness the design was made for: the length of the slots or tabs to find, mm.
    pub old_mm: f64,
    /// The new material thickness, mm.
    pub new_mm: f64,
    /// Lengths within `old_mm ± tolerance_mm` match.
    pub tolerance_mm: f64,
    pub slot_depth: bool,
    pub slot_width: bool,
    pub tab_height: bool,
}

/// What [`resize_slots`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Resized {
    pub path: BezPath,
    /// Steps (sides of slots and tabs) made `new_mm` long.
    pub steps: usize,
    /// Slots made `new_mm` wide.
    pub widths: usize,
    /// Closed outlines left alone, with the reason (curves, a hole, a fold).
    pub skipped: Vec<String>,
}

/// Cosine below which two directions are perpendicular, and above `1 - this` parallel (about 1.1°).
const ANGLE: f64 = 0.02;
const MAX_MM: f64 = 1000.0;

fn unit(v: Vec2) -> Vec2 {
    let l = v.hypot();
    if l > 0.0 { v / l } else { v }
}

/// Resize the slots and tabs of `path` (see the module docs).
pub fn resize_slots(path: &BezPath, o: &SlotOptions) -> Result<Resized> {
    for (name, v, lo) in [("old thickness", o.old_mm, f64::MIN_POSITIVE), ("new thickness", o.new_mm, f64::MIN_POSITIVE), ("tolerance", o.tolerance_mm, 0.0)] {
        if !(v.is_finite() && v >= lo && v <= MAX_MM) {
            return invalid(format!("the {name} must be within {}..={MAX_MM} mm (got {v})", if lo > 0.0 { "0 (exclusive)" } else { "0" }));
        }
    }
    if o.tolerance_mm >= o.old_mm {
        return invalid("the tolerance must be smaller than the old thickness");
    }
    if !(o.slot_depth || o.slot_width || o.tab_height) {
        return invalid("choose at least one of slot depth, slot width and tab height");
    }
    if !crate::is_finite(path) {
        return invalid("the path has a coordinate that is not a finite number");
    }
    let mut subs = split(path);
    let rings: Vec<Option<Vec<Point>>> = subs
        .iter()
        .map(|s| {
            if !s.closed {
                return Ok(None);
            }
            Ok(flatten(&join(std::slice::from_ref(s)), TOLERANCE)?.into_iter().next().filter(|l| l.closed && l.points.len() > 2).map(|l| l.points))
        })
        .collect::<Result<_>>()?;
    let (mut steps, mut widths, mut skipped) = (0, 0, Vec::new());
    for i in 0..subs.len() {
        let Some(ring) = &rings[i] else { continue };
        let depth = rings.iter().enumerate().filter(|(j, r)| *j != i && r.as_ref().is_some_and(|r| point_in_ring(ring[0], r))).count();
        if depth % 2 == 1 {
            skipped.push(format!("outline {} is a hole (internal slots are left alone)", i + 1));
            continue;
        }
        if subs[i].segs.iter().any(|s| !matches!(s, PathSeg::Line(_))) {
            skipped.push(format!("outline {} has curves (slots need straight lines and sharp corners)", i + 1));
            continue;
        }
        let pts: Vec<Point> = subs[i].segs.iter().map(|s| s.start()).collect();
        match resize_ring(&pts, o) {
            Ok(Some((new, s, w))) => {
                steps += s;
                widths += w;
                let n = new.len();
                let segs = (0..n).map(|k| PathSeg::Line(kurbo::Line::new(new[k], new[(k + 1) % n]))).collect();
                subs[i] = SubPath { segs, closed: true };
            }
            Ok(None) => {}
            Err(why) => skipped.push(format!("outline {}: {why}", i + 1)),
        }
    }
    Ok(Resized { path: join(&subs), steps, widths, skipped })
}

/// One closed outline of lines: the new corner points and the steps and widths changed, `None` when nothing
/// matched, or why it was left alone.
fn resize_ring(raw: &[Point], o: &SlotOptions) -> std::result::Result<Option<(Vec<Point>, usize, usize)>, String> {
    // Merge nodes along straight lines (and drop repeated points).
    let mut pts: Vec<Point> = Vec::with_capacity(raw.len());
    for &p in raw {
        if pts.last().is_none_or(|q: &Point| q.distance(p) > 1e-9) {
            pts.push(p);
        }
    }
    if pts.len() > 1 && pts[0].distance(pts[pts.len() - 1]) <= 1e-9 {
        pts.pop();
    }
    loop {
        let n = pts.len();
        if n < 4 {
            return Ok(None);
        }
        let straight = (0..n).find(|&k| {
            let (a, b, c) = (pts[(k + n - 1) % n], pts[k], pts[(k + 1) % n]);
            let (u, v) = (unit(b - a), unit(c - b));
            u.cross(v).abs() < 1e-9 && u.dot(v) > 0.0
        });
        match straight {
            Some(k) => {
                pts.remove(k);
            }
            None => break,
        }
    }
    // Work counter-clockwise (the material on the left); turn the result back at the end.
    let ccw = polygon_area(&pts) > 0.0;
    if !ccw {
        pts.reverse();
    }
    let n = pts.len();
    let edge = |j: usize| (pts[j % n], pts[(j + 1) % n]);
    let dir = |j: usize| {
        let (a, b) = edge(j);
        unit(b - a)
    };
    let len = |j: usize| {
        let (a, b) = edge(j);
        a.distance(b)
    };
    // Corner at vertex k (between edge k - 1 and edge k): convex when it turns left.
    let convex = |k: usize| dir((k + n - 1) % n).cross(dir(k)) > 0.0;
    let matches = |l: f64| (l - o.old_mm).abs() <= o.tolerance_mm + 1e-9;
    let perp = |a: usize, b: usize| dir(a).dot(dir(b)).abs() < ANGLE;
    // Offsets per edge: level moves (averaged when two steps move one line) and width moves (added).
    let mut level: Vec<Vec<Vec2>> = vec![Vec::new(); n];
    let mut side = vec![Vec2::ZERO; n];
    let mut n_steps = 0;
    let mut n_widths = 0;
    if o.slot_depth || o.tab_height {
        let (inner_share, outer_share) = match (o.slot_depth, o.tab_height) {
            (true, true) => (0.5, 0.5),
            (true, false) => (1.0, 0.0),
            _ => (0.0, 1.0),
        };
        for j in 0..n {
            let (prev, next) = ((j + n - 1) % n, (j + 1) % n);
            let l = len(j);
            if !matches(l) || !perp(prev, j) || !perp(j, next) || dir(prev).dot(dir(next)) < 1.0 - ANGLE {
                continue;
            }
            let (start_convex, end_convex) = (convex(j), convex(next));
            if start_convex == end_convex {
                continue;
            }
            let (a, b) = edge(j);
            // From the convex end to the reflex end.
            let (inner, outer, v) = if start_convex { (next, prev, unit(b - a)) } else { (prev, next, unit(a - b)) };
            let change = o.new_mm - l;
            if inner_share > 0.0 {
                level[inner].push(v * (change * inner_share));
            }
            if outer_share > 0.0 {
                level[outer].push(-v * (change * outer_share));
            }
            n_steps += 1;
        }
    }
    if o.slot_width {
        for j in 0..n {
            let (prev, next) = ((j + n - 1) % n, (j + 1) % n);
            let w = len(j);
            if !matches(w) || convex(j) || convex(next) || !perp(prev, j) || !perp(j, next) || dir(prev).dot(dir(next)) > -1.0 + ANGLE {
                continue;
            }
            let half = (o.new_mm - w) / 2.0;
            let d = dir(j);
            side[prev] += -d * half;
            side[next] += d * half;
            n_widths += 1;
        }
    }
    if n_steps == 0 && n_widths == 0 {
        return Ok(None);
    }
    let offset: Vec<Vec2> = (0..n)
        .map(|j| {
            let l = &level[j];
            let avg = if l.is_empty() { Vec2::ZERO } else { l.iter().fold(Vec2::ZERO, |a, b| a + *b) / l.len() as f64 };
            avg + side[j]
        })
        .collect();
    // Each corner where its two (moved) lines now meet.
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let (e0, e1) = ((k + n - 1) % n, k);
        let p0 = pts[e0] + offset[e0];
        let d0 = dir(e0);
        let p1 = pts[e1] + offset[e1];
        let d1 = dir(e1);
        let denom = d0.cross(d1);
        let corner = if denom.abs() < 1e-12 {
            pts[k] + offset[e1]
        } else {
            // p0 + d0 * t on line 1: (p1 - p0) x d1 / (d0 x d1).
            let t = (p1 - p0).cross(d1) / denom;
            p0 + d0 * t
        };
        out.push(corner);
    }
    // No line may turn around or vanish, and no two lines may cross (checked up to a few thousand corners).
    let fold = || Err("the change would fold the outline over itself".to_string());
    for j in 0..n {
        let (a, b) = (out[j], out[(j + 1) % n]);
        if (b - a).dot(dir(j)) <= 1e-9 {
            return fold();
        }
    }
    if n <= 4000 {
        for j in 0..n {
            for k in j + 2..n {
                if j == 0 && k == n - 1 {
                    continue;
                }
                if crosses(out[j], out[(j + 1) % n], out[k], out[(k + 1) % n]) {
                    return fold();
                }
            }
        }
    }
    if !ccw {
        out.reverse();
    }
    Ok(Some((out, n_steps, n_widths)))
}

/// Whether segments `ab` and `cd` cross or touch away from shared corners.
fn crosses(a: Point, b: Point, c: Point, d: Point) -> bool {
    let o = |p: Point, q: Point, r: Point| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    ((d1 > 1e-12 && d2 < -1e-12) || (d1 < -1e-12 && d2 > 1e-12)) && ((d3 > 1e-12 && d4 < -1e-12) || (d3 < -1e-12 && d4 > 1e-12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn poly(pts: &[(f64, f64)]) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for q in &pts[1..] {
            p.line_to(*q);
        }
        p.close_path();
        p
    }

    fn opts(old: f64, new: f64) -> SlotOptions {
        SlotOptions { old_mm: old, new_mm: new, tolerance_mm: 0.05, slot_depth: true, slot_width: false, tab_height: false }
    }

    fn corners(p: &BezPath) -> Vec<(f64, f64)> {
        split(p)[0].segs.iter().map(|s| ((s.start().x * 1e6).round() / 1e6, (s.start().y * 1e6).round() / 1e6)).collect()
    }

    /// The top edge of a 40 x 20 panel with finger joints for 3 mm stock: tabs 8 mm wide standing 3 mm proud,
    /// at both corners and in the middle (CCW).
    fn fingers() -> BezPath {
        poly(&[
            (0.0, 0.0),
            (40.0, 0.0),
            (40.0, 23.0),
            (32.0, 23.0),
            (32.0, 20.0),
            (24.0, 20.0),
            (24.0, 23.0),
            (16.0, 23.0),
            (16.0, 20.0),
            (8.0, 20.0),
            (8.0, 23.0),
            (0.0, 23.0),
        ])
    }

    #[test]
    fn slot_depth_moves_the_slot_bottoms() {
        let r = resize_slots(&fingers(), &opts(3.0, 4.0)).unwrap();
        assert_eq!(r.steps, 4, "{r:?}");
        let c = corners(&r.path);
        // The tabs' tops stay at y = 23; the slots' bottoms drop to y = 19.
        assert!(c.contains(&(32.0, 19.0)) && c.contains(&(24.0, 19.0)) && c.contains(&(16.0, 19.0)) && c.contains(&(8.0, 19.0)), "{c:?}");
        assert!(c.contains(&(40.0, 23.0)) && c.contains(&(0.0, 23.0)));
        // Tab height instead: the tops rise to y = 24, the corner tabs' outer sides grow with them.
        let r = resize_slots(&fingers(), &SlotOptions { slot_depth: false, tab_height: true, ..opts(3.0, 4.0) }).unwrap();
        let c = corners(&r.path);
        assert!(c.contains(&(40.0, 24.0)) && c.contains(&(0.0, 24.0)) && c.contains(&(16.0, 24.0)) && c.contains(&(32.0, 20.0)), "{c:?}");
        // Both: half each.
        let r = resize_slots(&fingers(), &SlotOptions { tab_height: true, ..opts(3.0, 4.0) }).unwrap();
        let c = corners(&r.path);
        assert!(c.contains(&(40.0, 23.5)) && c.contains(&(32.0, 19.5)), "{c:?}");
        // The outline direction does not matter.
        let rev = resize_slots(&fingers().reverse_subpaths(), &opts(3.0, 4.0)).unwrap();
        assert_eq!(rev.steps, 4);
        assert!((rev.path.area().abs() - resize_slots(&fingers(), &opts(3.0, 4.0)).unwrap().path.area().abs()).abs() < 1e-9);
    }

    #[test]
    fn nothing_matches_outside_the_tolerance() {
        let r = resize_slots(&fingers(), &opts(5.0, 6.0)).unwrap();
        assert_eq!((r.steps, r.widths), (0, 0));
        assert_eq!(r.path, fingers());
        // Within the tolerance: steps of 3.04 match 3 ± 0.05.
        let r = resize_slots(&fingers(), &SlotOptions { old_mm: 3.04, ..opts(3.0, 4.0) }).unwrap();
        assert_eq!(r.steps, 4);
    }

    #[test]
    fn slot_width_widens_u_slots_around_their_centre() {
        // A 60 x 30 divider with a 3 mm wide, 15 mm deep slot in the middle of its top edge, with an extra node on
        // the top edge (merged first).
        let p = poly(&[(0.0, 0.0), (60.0, 0.0), (60.0, 30.0), (31.5, 30.0), (31.5, 15.0), (28.5, 15.0), (28.5, 30.0), (10.0, 30.0), (0.0, 30.0)]);
        let o = SlotOptions { slot_depth: false, slot_width: true, ..opts(3.0, 3.6) };
        let r = resize_slots(&p, &o).unwrap();
        assert_eq!((r.steps, r.widths), (0, 1));
        let c = corners(&r.path);
        assert!(c.contains(&(31.8, 15.0)) && c.contains(&(28.2, 15.0)) && c.contains(&(31.8, 30.0)) && c.contains(&(28.2, 30.0)), "{c:?}");
        // The 15 mm deep sides are not steps of 3 mm, so slot depth changes nothing here.
        assert_eq!(resize_slots(&p, &opts(3.0, 3.6)).unwrap().steps, 0);
    }

    #[test]
    fn holes_curves_and_folds_are_left_alone() {
        let mut p = fingers();
        // A 3 x 3 hole: its sides would match, but holes are skipped.
        p.extend(poly(&[(10.0, 5.0), (13.0, 5.0), (13.0, 8.0), (10.0, 8.0)]));
        let r = resize_slots(&p, &opts(3.0, 4.0)).unwrap();
        assert_eq!(r.steps, 4);
        assert!(r.skipped.iter().any(|s| s.contains("hole")), "{:?}", r.skipped);
        // Making the slots deeper than the panel folds it.
        let r = resize_slots(&fingers(), &opts(3.0, 30.0)).unwrap();
        assert_eq!(r.steps, 0);
        assert!(r.skipped.iter().any(|s| s.contains("fold")), "{:?}", r.skipped);
        assert!(resize_slots(&fingers(), &SlotOptions { slot_depth: false, ..opts(3.0, 4.0) }).is_err(), "nothing to adjust");
        assert!(resize_slots(&fingers(), &SlotOptions { tolerance_mm: 3.0, ..opts(3.0, 4.0) }).is_err());
    }
}
