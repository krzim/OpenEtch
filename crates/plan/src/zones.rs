//! Areas applied to a planned job (field mm), after planning and before encoding:
//!
//! - [`clip_to_region`]: Re-mark a region. Only the parts of the planned toolpath inside a polygon stay (the design
//!   is not changed): every mark is cut where it crosses the polygon's edge, dots outside it are dropped, and travel
//!   is planned again between what is left. Fills keep their scan lines, so a re-marked patch lines up with the
//!   first marking.
//! - [`keep_out_hit`]: the first mark of a job that touches a keep-out zone (a jig wall, a clamp), so the engine can
//!   refuse the job before anything is sent.
//!
//! Polygons are rings of points (closed implicitly) and use the even-odd rule.

use crate::geom::Point;
use crate::job::{JobPlan, plan_stats};
use crate::{PlanError, Result, Target};

/// Axis-aligned bounds of a ring: (min x, min y, max x, max y).
fn ring_box(ring: &[Point]) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in ring {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b
}

fn boxes_meet(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// Even-odd point in ring.
pub fn inside(p: Point, ring: &[Point]) -> bool {
    let n = ring.len();
    let mut c = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

/// Parameters `t` in (0, 1) where segment `a`-`b` crosses an edge of `ring`.
fn crossings(a: Point, b: Point, ring: &[Point]) -> Vec<f64> {
    let n = ring.len();
    let mut out = Vec::new();
    let d = [b[0] - a[0], b[1] - a[1]];
    for i in 0..n {
        let (p, q) = (ring[i], ring[(i + 1) % n]);
        let e = [q[0] - p[0], q[1] - p[1]];
        let den = d[0] * e[1] - d[1] * e[0];
        if den.abs() < 1e-18 {
            continue;
        }
        let w = [p[0] - a[0], p[1] - a[1]];
        let t = (w[0] * e[1] - w[1] * e[0]) / den;
        let u = (w[0] * d[1] - w[1] * d[0]) / den;
        if t > 1e-12 && t < 1.0 - 1e-12 && (-1e-12..=1.0 + 1e-12).contains(&u) {
            out.push(t);
        }
    }
    out
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

/// The pieces of polyline `poly` inside `ring` (a one-point dot stays when it is inside).
fn clip_polyline(poly: &[Point], ring: &[Point]) -> Vec<Vec<Point>> {
    if poly.len() == 1 {
        return if inside(poly[0], ring) { vec![poly.to_vec()] } else { vec![] };
    }
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    for w in poly.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut ts = crossings(a, b, ring);
        ts.push(0.0);
        ts.push(1.0);
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
        for s in ts.windows(2) {
            let mid = lerp(a, b, (s[0] + s[1]) / 2.0);
            let (p, q) = (lerp(a, b, s[0]), lerp(a, b, s[1]));
            if inside(mid, ring) {
                if cur.last() != Some(&p) {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    cur.push(p);
                }
                cur.push(q);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out.retain(|p| p.len() >= 2);
    out
}

/// Keep only the toolpath inside `region` (see the module docs). The stats are recomputed for `target`; passes
/// left with nothing are dropped. Refused when the plan is split into parts (rotary or Z jobs), the region is not a
/// polygon, or nothing is left to mark.
pub fn clip_to_region(plan: &JobPlan, region: &[Point], target: &Target) -> Result<JobPlan> {
    if region.len() < 3 || region.iter().any(|p| !(p[0].is_finite() && p[1].is_finite())) {
        return Err(PlanError::Selection("the region must be a polygon of at least 3 finite points".into()));
    }
    if plan.parts.len() > 1 {
        return Err(PlanError::Selection("re-marking a region is not available for rotary or Z-stepped jobs".into()));
    }
    let rb = ring_box(region);
    let mut out = JobPlan { passes: Vec::new(), stats: Default::default(), warnings: plan.warnings.clone(), parts: plan.parts.clone(), rasters: Vec::new() };
    for pass in &plan.passes {
        let mut p = pass.clone();
        p.polylines.clear();
        p.powers.clear();
        let mut image_runs = 0;
        for (i, poly) in pass.polylines.iter().enumerate() {
            if poly.is_empty() || !boxes_meet(ring_box(poly), rb) {
                continue;
            }
            for piece in clip_polyline(poly, region) {
                if i < pass.gantry.image_runs {
                    image_runs += 1;
                }
                if !pass.powers.is_empty() {
                    p.powers.push(pass.power_of(i));
                }
                p.polylines.push(piece);
            }
        }
        if p.polylines.is_empty() {
            continue;
        }
        p.gantry.image_runs = image_runs;
        out.passes.push(p);
    }
    if out.passes.is_empty() {
        return Err(PlanError::Selection("nothing of the job lies inside the region".into()));
    }
    out.stats = plan_stats(&out, target);
    Ok(out)
}

/// A mark of a job inside a keep-out zone.
#[derive(Debug, Clone, PartialEq)]
pub struct KeepOutHit {
    /// The zone's name.
    pub zone: String,
    /// The layer of the pass that marks there.
    pub layer: String,
    /// A point of the mark inside the zone (or where it enters it), field mm.
    pub point: Point,
}

/// The first mark of `plan` that touches one of `zones` (`(name, ring)`), or `None`. Jumps never count: the laser
/// is off while travelling.
pub fn keep_out_hit(plan: &JobPlan, zones: &[(String, Vec<Point>)]) -> Option<KeepOutHit> {
    let zones: Vec<(&String, &Vec<Point>, [f64; 4])> = zones.iter().filter(|(_, r)| r.len() >= 3).map(|(n, r)| (n, r, ring_box(r))).collect();
    if zones.is_empty() {
        return None;
    }
    for pass in &plan.passes {
        for poly in &pass.polylines {
            let Some(&first) = poly.first() else { continue };
            let pb = ring_box(poly);
            for (name, ring, zb) in &zones {
                if !boxes_meet(pb, *zb) {
                    continue;
                }
                let hit = |p: Point| KeepOutHit { zone: (*name).clone(), layer: pass.layer_id.clone(), point: p };
                if inside(first, ring) {
                    return Some(hit(first));
                }
                for w in poly.windows(2) {
                    if !boxes_meet(ring_box(w), *zb) {
                        continue;
                    }
                    if inside(w[1], ring) {
                        return Some(hit(w[1]));
                    }
                    if let Some(t) = crossings(w[0], w[1], ring).into_iter().reduce(f64::min) {
                        return Some(hit(lerp(w[0], w[1], t)));
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{PassPlan, PassSettings};
    use crate::target::tests::target;

    fn pass(polys: Vec<Vec<Point>>) -> PassPlan {
        PassPlan {
            layer_id: "layer-1".into(),
            sub_layer: 0,
            repeat: 0,
            settings: PassSettings {
                power_pct: 20.0,
                speed_mm_s: 1000.0,
                frequency_khz: 30.0,
                pulse_width_ns: None,
                timing: target().timing,
                hatch_angle_deg: 0.0,
                motion: Default::default(),
            },
            polylines: polys,
            powers: vec![],
            part: 0,
            dot_time_us: None,
            pause_before_us: 0.0,
            pause_after_us: 0.0,
            z_mm: None,
            gantry: Default::default(),
        }
    }

    fn square(c: Point, h: f64) -> Vec<Point> {
        vec![[c[0] - h, c[1] - h], [c[0] + h, c[1] - h], [c[0] + h, c[1] + h], [c[0] - h, c[1] + h]]
    }

    #[test]
    fn clipping_keeps_the_inside_of_every_mark() {
        let plan = JobPlan {
            passes: vec![
                pass(vec![vec![[-10.0, 0.0], [10.0, 0.0]], vec![[-10.0, 5.0], [10.0, 5.0]], vec![[1.0, 0.5]], vec![[8.0, 8.0]]]),
                pass(vec![vec![[20.0, 20.0], [30.0, 20.0]]]),
            ],
            ..JobPlan::default()
        };
        let out = clip_to_region(&plan, &square([0.0, 0.0], 2.0), &target()).unwrap();
        assert_eq!(out.passes.len(), 1, "the second pass is outside");
        let p = &out.passes[0].polylines;
        assert_eq!(p.len(), 2, "{p:?}");
        assert_eq!(p[0], vec![[-2.0, 0.0], [2.0, 0.0]]);
        assert_eq!(p[1], vec![[1.0, 0.5]]);
        assert!((out.stats.mark_length_mm - 4.0).abs() < 1e-9);
        // A polyline that leaves and comes back splits in two; a dot inside stays.
        let zig = JobPlan { passes: vec![pass(vec![vec![[-1.0, 0.0], [5.0, 0.0], [5.0, 1.0], [-1.0, 1.0]], vec![[0.5, 0.5]]])], ..JobPlan::default() };
        let out = clip_to_region(&zig, &square([0.0, 0.0], 2.0), &target()).unwrap();
        let p = &out.passes[0].polylines;
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[0], vec![[-1.0, 0.0], [2.0, 0.0]]);
        assert_eq!(p[1], vec![[2.0, 1.0], [-1.0, 1.0]]);
        assert_eq!(p[2], vec![[0.5, 0.5]]);
        assert!(clip_to_region(&zig, &square([50.0, 50.0], 1.0), &target()).is_err());
        assert!(clip_to_region(&zig, &[[0.0, 0.0], [1.0, 1.0]], &target()).is_err());
    }

    #[test]
    fn keep_out_finds_marks_not_jumps() {
        let zone = vec![("Clamp".to_string(), square([0.0, 0.0], 1.0))];
        // The two marks pass either side; the jump between them crosses the zone.
        let ok = JobPlan { passes: vec![pass(vec![vec![[-5.0, 3.0], [5.0, 3.0]], vec![[5.0, -3.0], [-5.0, -3.0]]])], ..JobPlan::default() };
        assert_eq!(keep_out_hit(&ok, &zone), None);
        let through = JobPlan { passes: vec![pass(vec![vec![[-5.0, 0.0], [5.0, 0.0]]])], ..JobPlan::default() };
        let h = keep_out_hit(&through, &zone).unwrap();
        assert_eq!((h.zone.as_str(), h.layer.as_str()), ("Clamp", "layer-1"));
        assert!((h.point[0] + 1.0).abs() < 1e-9 && h.point[1].abs() < 1e-9, "{h:?}");
        let dot = JobPlan { passes: vec![pass(vec![vec![[0.2, 0.2]]])], ..JobPlan::default() };
        assert!(keep_out_hit(&dot, &zone).is_some());
    }
}
