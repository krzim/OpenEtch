//! Galvo rotary marking ([`doc::Rotary`]): a planned, unwrapped job split into strips around the work, each mapped
//! onto the surface under the lens.
//!
//! The work's axis lies along field X; the design's Y is arc length on the surface at the reference diameter `D`
//! (`object_diameter_mm`, radius `R`). With a taper the radius at field X is `r(x)` (linear between the two
//! diameters) and the design keeps its proportions on the local surface: a design point `(x, y)` sits at the work's
//! angle `y / r(x)`, which is the reference arc `a = y · R / r(x)`. Without a taper `a = y`.
//!
//! Strips are bands `split_width_mm` (`w`) wide in `a`, from the design's lowest arc up: strip `k` is centred at
//! `c_k = a_min + (k + ½)·w`. For strip `k` the work is turned so `c_k` is at the top, and a point of the strip goes
//! to field `(x, r(x)·sin((a − c_k)/R))` with cylinder correction (the projection of the curved surface onto the
//! lens's plane), else `(x, r(x)·(a − c_k)/R)`. Lines are cut at the strip edges and, where the mapping bends them,
//! subdivided to 0.5 mm first. The axis position of strip `k` is `k · w / (π·D) · steps per turn of the work`
//! (roller rotaries scale the motor steps by the diameter ratio). All of this is **unverified on hardware**.

use doc::Rotary;

use crate::geom::{Point, dist};
use crate::job::{JobPlan, PartPlan, PassPlan};
use crate::{PlanError, Result, Target};

/// Longest segment kept before the curved mapping, mm.
const SUBDIVIDE_MM: f64 = 0.5;

/// Most strips one job may have.
pub const MAX_STRIPS: usize = 10_000;

/// Split `plan` (planned unwrapped) into rotary strips. Every pass is repeated per strip with the strip's part of
/// its polylines; passes with nothing in a strip are left out.
pub(crate) fn split(plan: JobPlan, r: &Rotary, target: &Target) -> Result<JobPlan> {
    let big_r = r.object_diameter_mm / 2.0;
    let arc = |p: Point| p[1] * big_r / (r.diameter_at(p[0]) / 2.0);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in plan.passes.iter().flat_map(|p| p.polylines.iter().flatten()) {
        let a = arc(*p);
        lo = lo.min(a);
        hi = hi.max(a);
    }
    if !lo.is_finite() {
        return Ok(plan);
    }
    // More than one turn of the work would mark the first strips over again.
    let turn = std::f64::consts::PI * r.object_diameter_mm;
    if hi - lo > turn * (1.0 + 1e-3) {
        return Err(PlanError::Target(format!(
            "rotary: the design is {:.1} mm around but one turn of the {} mm work is {turn:.1} mm; it would mark over itself",
            hi - lo,
            r.object_diameter_mm
        )));
    }
    let w = r.split_width_mm;
    let strips = (((hi - lo) / w).ceil() as usize).max(1);
    if strips > MAX_STRIPS {
        return Err(PlanError::Target(format!("rotary: {strips} strips of {w} mm is more than {MAX_STRIPS}")));
    }
    let steps_per_mm = r.steps_per_object_turn() / (std::f64::consts::PI * r.object_diameter_mm);
    let curved = r.cylinder_correction || r.taper.is_some();
    let mut out = JobPlan { passes: Vec::new(), stats: plan.stats, warnings: plan.warnings.clone(), parts: Vec::new() };
    for k in 0..strips {
        let c = lo + (k as f64 + 0.5) * w;
        let (band_lo, band_hi) = (c - w / 2.0, c + w / 2.0);
        // The last strip takes the top edge too.
        let band_hi = if k + 1 == strips { band_hi.max(hi) + 1e-9 } else { band_hi };
        let map = |p: Point| {
            let rx = r.diameter_at(p[0]) / 2.0;
            let phi = (arc(p) - c) / big_r;
            [p[0], if r.cylinder_correction { rx * phi.sin() } else { rx * phi }]
        };
        let part = out.parts.len();
        let mut any = false;
        for pass in &plan.passes {
            let mut polylines = Vec::new();
            let mut powers = Vec::new();
            for (i, poly) in pass.polylines.iter().enumerate() {
                let pieces = if poly.len() == 1 {
                    let a = arc(poly[0]);
                    if a >= band_lo && a < band_hi { vec![poly.clone()] } else { Vec::new() }
                } else {
                    let dense = if curved { subdivide(poly) } else { poly.clone() };
                    clip_band(&dense, &arc, band_lo, band_hi)
                };
                for piece in pieces {
                    let mapped: Vec<Point> = piece.into_iter().map(map).collect();
                    if let Some(bad) = mapped.iter().find(|p| !target.contains(**p)) {
                        return Err(PlanError::PointOutOfField { layer: pass.layer_id.clone(), point: *bad, field_mm: target.field_mm });
                    }
                    polylines.push(mapped);
                    powers.push(pass.power_of(i));
                }
            }
            if polylines.is_empty() {
                continue;
            }
            if powers.iter().all(|p| *p == pass.settings.power_pct) {
                powers.clear();
            }
            any = true;
            out.passes.push(PassPlan { polylines, powers, part, ..pass.clone() });
        }
        if any {
            let steps = (k as f64 * w * steps_per_mm).round() as i64;
            out.parts.push(PartPlan { rotary_steps: Some(steps), z_mm: None });
        }
    }
    Ok(out)
}

/// `poly` with no segment longer than [`SUBDIVIDE_MM`].
fn subdivide(poly: &[Point]) -> Vec<Point> {
    let mut out = vec![poly[0]];
    for w in poly.windows(2) {
        let n = (dist(w[0], w[1]) / SUBDIVIDE_MM).ceil().max(1.0) as usize;
        for j in 1..=n {
            let t = j as f64 / n as f64;
            out.push([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]);
        }
    }
    out
}

/// The pieces of `poly` whose `arc` value lies in `lo..hi`, cut at the band edges (linear in each segment).
fn clip_band(poly: &[Point], arc: &impl Fn(Point) -> f64, lo: f64, hi: f64) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let lerp = |a: Point, b: Point, t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    for w in poly.windows(2) {
        let (p, q) = (w[0], w[1]);
        let (ap, aq) = (arc(p), arc(q));
        // The parameter range of the segment inside the band.
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let d = aq - ap;
        if d.abs() < 1e-15 {
            if ap < lo || ap >= hi {
                (t0, t1) = (1.0, 0.0);
            }
        } else {
            let (ta, tb) = ((lo - ap) / d, (hi - ap) / d);
            let (enter, leave) = if d > 0.0 { (ta, tb) } else { (tb, ta) };
            t0 = t0.max(enter);
            t1 = t1.min(leave);
        }
        if t0 > t1 {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current.clear();
            continue;
        }
        let (a, b) = (lerp(p, q, t0), lerp(p, q, t1));
        if current.last().is_some_and(|l| dist(*l, a) < 1e-9) {
            current.push(b);
        } else {
            if current.len() >= 2 {
                out.push(std::mem::take(&mut current));
            }
            current = vec![a, b];
        }
        if t1 < 1.0 {
            out.push(std::mem::take(&mut current));
        }
    }
    if current.len() >= 2 {
        out.push(current);
    }
    out.retain(|p| p.len() >= 2 && p.windows(2).any(|w| dist(w[0], w[1]) > 1e-9));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_cuts_at_the_band_edges() {
        let id = |p: Point| p[1];
        let pieces = clip_band(&[[0.0, -5.0], [0.0, 5.0]], &id, -1.0, 2.0);
        assert_eq!(pieces, vec![vec![[0.0, -1.0], [0.0, 2.0]]]);
        // In, out, in again: two pieces.
        let zig = [[0.0, 0.0], [1.0, 5.0], [2.0, 0.0]];
        assert_eq!(clip_band(&zig, &id, -1.0, 1.0).len(), 2);
        assert!(clip_band(&[[0.0, 3.0], [1.0, 4.0]], &id, -1.0, 1.0).is_empty());
    }

    #[test]
    fn subdivision_keeps_ends() {
        let s = subdivide(&[[0.0, 0.0], [2.0, 0.0]]);
        assert_eq!(s.len(), 5);
        assert_eq!(s[4], [2.0, 0.0]);
    }
}
