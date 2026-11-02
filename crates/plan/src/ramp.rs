//! LightBurn's Fill-mode Ramp Length for GRBL gantries (`doc::CutOptions::ramp_length_mm`): the power of a fill rises
//! from the ramp's minimum at the edge of the design to the layer's power `length` mm into the fill, so the sides of the
//! engraving slope (rubber stamps). Each hatch line is split into short collinear pieces, each with one power, in
//! [`STEPS`] steps; the G-code output turns consecutive pieces into `G1` moves with stepped `S` (and overscans the
//! whole scan line once, see `gcode`).
//!
//! Which edges ramp: by default the edges inside the fill (the outlines of holes and of islands within them, i.e. the
//! design's own sides in an inverted stamp); with Ramp Outer Edge also the outermost outlines of the filled shapes.
//! Written from LightBurn's documentation of the setting (Fill mode, "Ramp Length" / "Ramp Outer Edge").

use crate::Point;

/// Power steps across one ramp.
pub const STEPS: usize = 10;

/// The ramp's piece length never goes below this, mm (and a scan line never gets more than [`MAX_SAMPLES`] pieces).
const MIN_STEP_MM: f64 = 0.05;
const MAX_SAMPLES: usize = 20_000;

/// The edges a fill group ramps from.
#[derive(Debug, Clone, Default)]
pub struct RampEdges {
    segs: Vec<[Point; 2]>,
}

/// `p` is inside the ring `ring` (even-odd; the ring may repeat its first point at the end).
pub(crate) fn inside(p: Point, ring: &[Point]) -> bool {
    let n = ring.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
            c = !c;
        }
    }
    c
}

impl RampEdges {
    /// The edges of `rings` (closed outlines of one fill group) that ramp: every ring inside another one, and the
    /// outermost rings too with `outer_edge`.
    pub fn new(rings: &[Vec<Point>], outer_edge: bool) -> Self {
        let mut segs = Vec::new();
        for (i, r) in rings.iter().enumerate() {
            let Some(&p) = r.first() else { continue };
            let nested = rings.iter().enumerate().any(|(j, o)| j != i && o.len() >= 3 && inside(p, o));
            if !(nested || outer_edge) {
                continue;
            }
            for k in 0..r.len() {
                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                if a != b {
                    segs.push([a, b]);
                }
            }
        }
        RampEdges { segs }
    }

    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }
}

fn dist_to_seg(p: Point, [a, b]: [Point; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (a[0] + t * dx - p[0], a[1] + t * dy - p[1]);
    (x * x + y * y).sqrt()
}

/// The hatch line `a` -> `b` split for a ramp of `length` mm: collinear pieces in order from `a`, each with its power
/// as a fraction of the full ramp (0 at an edge, 1 from `length` mm away), stepped in [`STEPS`] levels. A line no
/// ramping edge comes near is one piece at 1.
pub fn ramp_line(a: Point, b: Point, edges: &RampEdges, length: f64) -> Vec<([Point; 2], f64)> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if !(length > 0.0 && len > 1e-9 && len.is_finite()) || edges.is_empty() {
        return vec![([a, b], 1.0)];
    }
    let u = [dx / len, dy / len];
    let along = |p: Point| (p[0] - a[0]) * u[0] + (p[1] - a[1]) * u[1];
    let across = |p: Point| (p[0] - a[0]) * u[1] - (p[1] - a[1]) * u[0];
    // The edges that come within `length` of the line, and the stretches of the line (in mm from `a`) they can reach:
    // outside an edge's projection widened by `length`, every point is further than `length` from it.
    let mut near: Vec<[Point; 2]> = Vec::new();
    let mut spans: Vec<(f64, f64)> = Vec::new();
    for &s in &edges.segs {
        let (c0, c1) = (across(s[0]), across(s[1]));
        if (c0 > length && c1 > length) || (c0 < -length && c1 < -length) {
            continue;
        }
        let (t0, t1) = (along(s[0]), along(s[1]));
        let (lo, hi) = ((t0.min(t1) - length).max(0.0), (t0.max(t1) + length).min(len));
        if lo < hi {
            near.push(s);
            spans.push((lo, hi));
        }
    }
    if near.is_empty() {
        return vec![([a, b], 1.0)];
    }
    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (lo, hi) in spans {
        match merged.last_mut() {
            Some(m) if lo <= m.1 => m.1 = m.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    let covered: f64 = merged.iter().map(|(lo, hi)| hi - lo).sum();
    let step = (length / STEPS as f64).max(MIN_STEP_MM).max(covered / MAX_SAMPLES as f64);
    let level = |t: f64| {
        let p = [a[0] + u[0] * t, a[1] + u[1] * t];
        let d = near.iter().map(|s| dist_to_seg(p, *s)).fold(f64::INFINITY, f64::min);
        // Rounded down: a step never marks hotter than the slope at its middle.
        ((d / length).clamp(0.0, 1.0) * STEPS as f64).floor() / STEPS as f64
    };
    // Stretches as (from, to, level), full power between the spans; neighbours of one level merge.
    let mut out: Vec<(f64, f64, f64)> = Vec::new();
    let mut push = |from: f64, to: f64, f: f64| {
        if to - from <= 1e-9 {
            return;
        }
        match out.last_mut() {
            Some(last) if last.2 == f && (last.1 - from).abs() <= 1e-9 => last.1 = to,
            _ => out.push((from, to, f)),
        }
    };
    let mut t = 0.0;
    for (lo, hi) in merged {
        push(t, lo, 1.0);
        let n = ((hi - lo) / step).ceil().max(1.0) as usize;
        let h = (hi - lo) / n as f64;
        for i in 0..n {
            let (s0, s1) = (lo + h * i as f64, if i + 1 == n { hi } else { lo + h * (i + 1) as f64 });
            push(s0, s1, level((s0 + s1) / 2.0));
        }
        t = hi;
    }
    push(t, len, 1.0);
    let at = |t: f64| if t >= len { b } else { [a[0] + u[0] * t, a[1] + u[1] * t] };
    out.into_iter().map(|(s0, s1, f)| ([at(s0), at(s1)], f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(c: Point, half: f64) -> Vec<Point> {
        vec![[c[0] - half, c[1] - half], [c[0] + half, c[1] - half], [c[0] + half, c[1] + half], [c[0] - half, c[1] + half]]
    }

    #[test]
    fn only_inner_edges_ramp_unless_outer_edge() {
        let rings = vec![square([0.0, 0.0], 10.0), square([0.0, 0.0], 2.0)];
        let inner = RampEdges::new(&rings, false);
        assert_eq!(inner.segs.len(), 4, "the hole's four sides");
        assert_eq!(RampEdges::new(&rings, true).segs.len(), 8);
        assert!(RampEdges::new(&rings[..1], false).is_empty(), "a lone shape has no inner edge");
    }

    #[test]
    fn power_steps_up_away_from_the_hole() {
        // A scan line from the outer edge to the hole's left side at x = -2: the last 1 mm before the hole ramps down.
        let rings = vec![square([0.0, 0.0], 10.0), square([0.0, 0.0], 2.0)];
        let e = RampEdges::new(&rings, false);
        let pieces = ramp_line([-10.0, 0.0], [-2.0, 0.0], &e, 1.0);
        assert_eq!(pieces[0].0[0], [-10.0, 0.0]);
        assert_eq!(pieces.last().unwrap().0[1], [-2.0, 0.0]);
        for w in pieces.windows(2) {
            assert_eq!(w[0].0[1], w[1].0[0], "pieces chain");
            assert!(w[1].1 < w[0].1, "falls towards the hole: {pieces:?}");
        }
        assert_eq!(pieces[0], ([[-10.0, 0.0], [-3.0, 0.0]], 1.0));
        assert!(pieces.last().unwrap().1 <= 0.1, "{pieces:?}");
        assert!(pieces.len() >= STEPS - 1 && pieces.len() <= STEPS + 2, "{pieces:?}");
        // Far from every ramping edge: one piece at full power.
        assert_eq!(ramp_line([-10.0, 8.0], [10.0, 8.0], &e, 1.0), vec![([[-10.0, 8.0], [10.0, 8.0]], 1.0)]);
        // With the outer edge on, the start ramps too.
        let both = RampEdges::new(&rings, true);
        let p = ramp_line([-10.0, 8.0], [10.0, 8.0], &both, 1.0);
        assert!(p.first().unwrap().1 < 0.1 && p.last().unwrap().1 < 0.1, "{p:?}");
        assert!(p.iter().any(|(_, f)| *f == 1.0));
    }
}
