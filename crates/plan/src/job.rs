//! Document -> [`JobPlan`]: resolve each pass's settings, flatten, hatch, check the field, order travel.

use doc::{CutSettings, Document, FillMode, GalvoTiming, Layer, LayerId, ObjectId};
use lmc::PulseDecision;
use lmc::units::nearest_pulse_width;
use serde::{Deserialize, Serialize};

use crate::geom::{Point, Subpath, dist, flatten, polyline_length};
use crate::hatch::{HatchError, MAX_SCANLINES, hatch};
use crate::order::{Chunk, order};
use crate::{PlanError, Result, Target};

/// Most repetitions (`passes`) one layer or sub-layer may ask for. The commands crate refuses more too.
pub const MAX_PASSES: u32 = 1000;

/// Most mark segments (straight `MarkTo`s) one job may have. Planning stops as soon as a job passes it, so a
/// document edit cannot make the engine allocate without bound.
pub const MAX_SEGMENTS: usize = 5_000_000;

/// A whole job: passes in burn order, every polyline in field-frame mm.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JobPlan {
    pub passes: Vec<PassPlan>,
    pub stats: Stats,
}

/// One pass of one layer with one set of settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassPlan {
    pub layer_id: LayerId,
    /// 0 for the layer's own settings, `n` for `sub_layers[n - 1]`.
    pub sub_layer: usize,
    /// Repetition of those settings, `0..passes`.
    pub repeat: u32,
    pub settings: PassSettings,
    /// Polylines in marking order. A closed shape repeats its first point at the end.
    pub polylines: Vec<Vec<Point>>,
}

/// Fully resolved, already validated settings of one pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassSettings {
    pub power_pct: f64,
    pub speed_mm_s: f64,
    pub frequency_khz: f64,
    /// A width from the target's table (requests within 0.5 ns are snapped); `None` leaves the source as it is.
    pub pulse_width_ns: Option<u16>,
    /// The layer's timing, or the target's default.
    pub timing: GalvoTiming,
    /// Hatch angle used by this pass (base angle plus `repeat × angle_increment_deg`).
    pub hatch_angle_deg: f64,
}

/// Totals over a job or preview.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Stats {
    pub mark_length_mm: f64,
    /// Travel with the laser off, from the field centre through every polyline start.
    pub jump_length_mm: f64,
    /// Motion at the set speeds plus the list's delays. An estimate: acceleration is not modelled.
    pub estimated_time_s: f64,
    /// Straight mark segments (MarkTo commands).
    pub segment_count: usize,
    pub polyline_count: usize,
}

/// Plan `doc` for `target`.
///
/// Layers with `output` run in document order; each runs `settings` then every sub-layer, each repeated its own
/// `passes` times, the hatch angle growing by `angle_increment_deg` per repeat. `Line` traces outlines (closed
/// subpaths closed), `Fill` hatches closed subpaths per object with the even-odd rule, `FillThenLine` does both,
/// hatch first. Travel is ordered greedily per pass, continuing from where the previous pass ended.
///
/// Refused, never clamped: a point outside the field (naming every offending object), power above the target's
/// limit, a pulse width not in the table, settings the board cannot encode, more than [`MAX_PASSES`] passes, and
/// more than [`MAX_SEGMENTS`] mark segments in total (counted while generating; planning stops at the first pass
/// that goes over). Passes with nothing to mark are left out.
pub fn plan_job(doc: &Document, target: &Target) -> Result<JobPlan> {
    plan_job_within(doc, target, MAX_SEGMENTS)
}

/// [`plan_job`] with a segment budget of `max_segments` (tests use a small one).
pub(crate) fn plan_job_within(doc: &Document, target: &Target, max_segments: usize) -> Result<JobPlan> {
    target.validate()?;
    let layers: Vec<&Layer> = doc.layers.iter().filter(|l| l.output).collect();

    // Flatten every output object once and check the field before any settings work.
    let mut outside: Vec<ObjectId> = Vec::new();
    let mut shapes: Vec<(&str, Vec<Subpath>)> = Vec::new();
    for obj in doc.objects.iter().filter(|o| layers.iter().any(|l| l.id == o.layer)) {
        let subs = flatten(&obj.field_path());
        if subs.iter().flat_map(|s| &s.points).any(|&p| !target.contains(p)) {
            outside.push(obj.id.clone());
        }
        shapes.push((obj.layer.as_str(), subs));
    }
    if !outside.is_empty() {
        return Err(PlanError::OutOfField { objects: outside, field_mm: target.field_mm });
    }

    let mut plan = JobPlan::default();
    let mut pos: Point = [0.0, 0.0];
    let mut segments = 0usize;
    for layer in layers {
        let subs: Vec<&Vec<Subpath>> = shapes.iter().filter(|(l, _)| *l == layer.id).map(|(_, s)| s).collect();
        for (sub_layer, cut) in std::iter::once(&layer.settings).chain(&layer.sub_layers).enumerate() {
            let base = resolve(&layer.id, cut, target)?;
            for repeat in 0..cut.passes {
                let angle = cut.hatch.angle_deg + f64::from(repeat) * cut.hatch.angle_increment_deg;
                let settings = PassSettings { hatch_angle_deg: angle, ..base.clone() };
                let (polylines, end) = pass_polylines(&layer.id, cut, angle, &subs, pos, (max_segments - segments, max_segments))?;
                if polylines.is_empty() {
                    continue;
                }
                segments += segment_count(&polylines);
                if segments > max_segments {
                    return Err(PlanError::TooManySegments { max: max_segments });
                }
                pos = end;
                plan.passes.push(PassPlan { layer_id: layer.id.clone(), sub_layer, repeat, settings, polylines });
            }
        }
    }
    plan.stats = plan_stats(&plan, target);
    Ok(plan)
}

/// Validate one `CutSettings` against the target and resolve its defaults.
fn resolve(layer: &str, cut: &CutSettings, target: &Target) -> Result<PassSettings> {
    let invalid = |what: &'static str, value: f64| PlanError::Setting { layer: layer.into(), what, value };
    if cut.passes > MAX_PASSES {
        return Err(PlanError::TooManyPasses { layer: layer.into(), passes: cut.passes, max: MAX_PASSES });
    }
    if !cut.power_pct.is_finite() || cut.power_pct < 0.0 {
        return Err(invalid("power_pct", cut.power_pct));
    }
    if cut.power_pct > target.max_power_pct {
        return Err(PlanError::PowerTooHigh { layer: layer.into(), power_pct: cut.power_pct, max_pct: target.max_power_pct });
    }
    if !(cut.speed_mm_s.is_finite() && cut.speed_mm_s > 0.0) {
        return Err(invalid("speed_mm_s", cut.speed_mm_s));
    }
    if !(cut.frequency_khz.is_finite() && cut.frequency_khz > 0.0) {
        return Err(invalid("frequency_khz", cut.frequency_khz));
    }
    if matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine) && !(cut.hatch.interval_mm.is_finite() && cut.hatch.interval_mm > 0.0) {
        return Err(invalid("hatch.interval_mm", cut.hatch.interval_mm));
    }
    let pulse_width_ns = match cut.pulse_width_ns {
        None => None,
        Some(ns) => match nearest_pulse_width(ns, &target.pulse_widths_ns) {
            PulseDecision::Exact(w) | PulseDecision::Snapped { to: w, .. } => Some(w),
            PulseDecision::Refused { requested, nearest } => {
                return Err(PlanError::PulseWidth { layer: layer.into(), requested_ns: requested, nearest_ns: nearest });
            }
        },
    };
    let settings = PassSettings {
        power_pct: cut.power_pct,
        speed_mm_s: cut.speed_mm_s,
        frequency_khz: cut.frequency_khz,
        pulse_width_ns,
        timing: cut.timing.clone().unwrap_or_else(|| target.timing.clone()),
        hatch_angle_deg: cut.hatch.angle_deg,
    };
    // Everything the encoder will need must convert now, so a plan that exists can be encoded.
    crate::encode::PassWords::new(&settings, target).map_err(|e| PlanError::Encode { layer: layer.into(), source: e })?;
    Ok(settings)
}

/// Mark segments of `polylines` (each polyline of n points has n - 1).
fn segment_count(polylines: &[Vec<Point>]) -> usize {
    polylines.iter().map(|p| p.len().saturating_sub(1)).sum()
}

/// The ordered polylines of one pass over the layer's flattened objects, and where the galvo ends. Refused with
/// [`PlanError::TooManySegments`] as soon as the hatch lines alone pass `budget`, the segments left of the job's
/// `max` (the caller checks the total).
fn pass_polylines(
    layer: &str,
    cut: &CutSettings,
    angle: f64,
    objects: &[&Vec<Subpath>],
    start: Point,
    (budget, max): (usize, usize),
) -> Result<(Vec<Vec<Point>>, Point)> {
    let fill = matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine);
    let line = matches!(cut.mode, FillMode::Line | FillMode::FillThenLine);
    let mut out = Vec::new();
    let mut pos = start;
    if fill {
        let mut chunks = Vec::new();
        let mut lines_made = 0usize;
        for subs in objects {
            let rings: Vec<Vec<Point>> = subs.iter().filter(|s| s.closed).map(|s| s.points.clone()).collect();
            let mut angles = vec![angle];
            if cut.hatch.cross_hatch {
                angles.push(angle + 90.0);
            }
            for a in angles {
                let lines = hatch(&rings, a, cut.hatch.interval_mm, cut.hatch.bidirectional, budget - lines_made).map_err(|e| match e {
                    HatchError::TooManyScanlines => PlanError::TooManyHatchLines { layer: layer.into(), max: MAX_SCANLINES },
                    HatchError::OverBudget => PlanError::TooManySegments { max },
                })?;
                lines_made += lines.len();
                if !lines.is_empty() {
                    chunks.push(Chunk::Hatch { lines, bidirectional: cut.hatch.bidirectional });
                }
            }
        }
        let (polylines, end) = order(chunks, pos);
        out.extend(polylines);
        pos = end;
    }
    if line {
        let chunks = objects
            .iter()
            .flat_map(|subs| subs.iter())
            .map(|s| if s.closed { Chunk::Closed(s.points.clone()) } else { Chunk::Open(s.points.clone()) })
            .collect();
        let (polylines, end) = order(chunks, pos);
        out.extend(polylines);
        pos = end;
    }
    Ok((out, pos))
}

/// Lengths, counts and an estimated time for a plan, following the encoder's list layout.
fn plan_stats(plan: &JobPlan, target: &Target) -> Stats {
    let mut s = Stats::default();
    let mut pos: Point = [0.0, 0.0];
    let mut time_us = if plan.passes.is_empty() { 0.0 } else { target.open_mo_delay_us };
    for pass in &plan.passes {
        let t = &pass.settings.timing;
        for p in &pass.polylines {
            let (Some(&first), Some(&last)) = (p.first(), p.last()) else { continue };
            let jump = dist(pos, first);
            let mark = polyline_length(p);
            let segments = p.len().saturating_sub(1);
            s.jump_length_mm += jump;
            s.mark_length_mm += mark;
            s.segment_count += segments;
            s.polyline_count += 1;
            if jump > 0.0 {
                time_us += jump / t.jump_speed_mm_s * 1e6 + crate::encode::jump_delay_us(t, jump).max(0.0);
            }
            time_us += mark / pass.settings.speed_mm_s * 1e6;
            time_us += t.laser_on_delay_us.max(0.0) + t.laser_off_delay_us.max(0.0) + t.end_delay_us.max(0.0);
            #[allow(clippy::cast_precision_loss)]
            let corners = segments.saturating_sub(1) as f64;
            // As encoded: whole 10 µs units (`lmc::units::polygon_delay_units`).
            time_us += corners * (t.polygon_delay_us.max(0.0) / 10.0).trunc() * 10.0;
            pos = last;
        }
    }
    s.estimated_time_s = time_us / 1e6;
    s
}
