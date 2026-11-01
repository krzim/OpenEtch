//! Document -> [`JobPlan`]: resolve each pass's settings, flatten, hatch, check the field, order travel.

use doc::{CutSettings, Document, FillMode, GalvoTiming, Geometry, Layer, LayerId, ObjectId};
use lmc::PulseDecision;
use lmc::units::nearest_pulse_width;
use serde::{Deserialize, Serialize};

use crate::geom::{Point, Subpath, dist, flatten, polyline_length};
use crate::hatch::{HatchError, MAX_SCANLINES, hatch};
use crate::order::{Chunk, order};
use crate::{PlanError, Result, Target};

/// Most repetitions (`passes`) one layer or sub-layer may ask for. The commands crate refuses more too.
pub const MAX_PASSES: u32 = 1000;

/// Most passes (repetitions of the main settings, of cleanups and of sub-layers together) one layer may plan, so a
/// layer of many sub-layers or a cleanup after every one of many main passes cannot make planning loop without bound.
pub const MAX_LAYER_PASSES: u32 = 100_000;

/// Most mark segments (straight `MarkTo`s) one job may have. Planning stops as soon as a job passes it, so a
/// document edit cannot make the engine allocate without bound: at this size a plan (polylines, plus a preview or
/// the encoded packets) stays around a couple of hundred MB at most, which a Raspberry Pi can hold.
pub const MAX_SEGMENTS: usize = 2_000_000;

/// The warning a plan carries when output layers hold images, which are skipped.
pub const IMAGE_WARNING: &str = "image engraving is not supported yet";

/// A whole job: passes in burn order, every polyline in field-frame mm.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JobPlan {
    pub passes: Vec<PassPlan>,
    pub stats: Stats,
    /// Things left out of the job, e.g. [`IMAGE_WARNING`] with the number of images skipped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
/// `passes` times, the hatch angle growing by `angle_increment_deg` per repeat. A cleanup sub-layer
/// ([`doc::Cleanup`]) runs between the main passes instead: after main passes n, 2n, ... and after the last one, its
/// own `passes` each time (its repeats count on across runs, so its hatch keeps turning); the other sub-layers follow
/// in order (see [`layer_schedule`]). `Line` traces outlines (closed subpaths closed), `Fill` hatches closed
/// subpaths per object with the even-odd rule, `FillThenLine` does both, hatch first. Travel is ordered greedily per pass, continuing from where the previous pass ended.
///
/// Images are skipped (image engraving is not supported yet), with a warning in [`JobPlan::warnings`].
///
/// Refused, never clamped: a point outside the field (naming every offending object), power above the target's
/// limit, a pulse width not in the table, settings the board cannot encode, more than [`MAX_PASSES`] passes, a
/// cleanup interval of 0, more than [`MAX_LAYER_PASSES`] passes of one layer in all, and more than [`MAX_SEGMENTS`]
/// mark segments in total (counted while generating; planning stops at the first pass that goes over). Passes with
/// nothing to mark are left out.
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
    let mut images = 0usize;
    for obj in doc.objects.iter().filter(|o| layers.iter().any(|l| l.id == o.layer)) {
        if matches!(obj.geometry, Geometry::Image { .. }) {
            images += 1;
            continue;
        }
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
    if images > 0 {
        plan.warnings.push(format!("{IMAGE_WARNING}: {images} image(s) on output layers were skipped"));
    }
    let mut pos: Point = [0.0, 0.0];
    let mut segments = 0usize;
    for layer in layers {
        let subs: Vec<&Vec<Subpath>> = shapes.iter().filter(|(l, _)| *l == layer.id).map(|(_, s)| s).collect();
        // Every set of settings is checked, also on a layer with nothing to mark.
        let cuts: Vec<&CutSettings> = std::iter::once(&layer.settings).chain(&layer.sub_layers).collect();
        let resolved = cuts.iter().map(|cut| resolve(&layer.id, cut, target)).collect::<Result<Vec<_>>>()?;
        for c in layer.sub_layers.iter().filter_map(|s| s.cleanup.as_ref()) {
            if c.every_n_passes == 0 {
                return Err(PlanError::Setting { layer: layer.id.clone(), what: "cleanup.every_n_passes", value: 0.0 });
            }
        }
        let total = layer_pass_count(layer);
        if total > u64::from(MAX_LAYER_PASSES) {
            return Err(PlanError::TooManyPasses {
                layer: layer.id.clone(),
                passes: u32::try_from(total).unwrap_or(u32::MAX),
                max: MAX_LAYER_PASSES,
            });
        }
        if subs.is_empty() {
            continue;
        }
        for (sub_layer, repeat) in layer_schedule(layer) {
            let cut = cuts[sub_layer];
            let angle = cut.hatch.angle_deg + f64::from(repeat) * cut.hatch.angle_increment_deg;
            let settings = PassSettings { hatch_angle_deg: angle, ..resolved[sub_layer].clone() };
            let (polylines, end) =
                pass_polylines(&layer.id, cut, angle, &subs, pos, (max_segments - segments, max_segments))?;
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
    plan.stats = plan_stats(&plan, target);
    Ok(plan)
}

/// The passes of one layer in burn order, as `(sub_layer, repeat)` (`sub_layer` 0 for the main settings, n for
/// `sub_layers[n - 1]`): each main pass, followed by every cleanup sub-layer due after it ([`doc::Cleanup::runs_after`],
/// its `passes` each time, `repeat` counting on across its runs), then every other sub-layer `passes` times in order.
/// The main settings' own `cleanup` means nothing and is ignored.
pub fn layer_schedule(layer: &Layer) -> Vec<(usize, u32)> {
    let main = layer.settings.passes;
    let cleanups: Vec<(usize, &CutSettings, &doc::Cleanup)> =
        layer.sub_layers.iter().enumerate().filter_map(|(i, s)| s.cleanup.as_ref().map(|c| (i + 1, s, c))).collect();
    let mut runs = vec![0u32; layer.sub_layers.len() + 1];
    let mut out = Vec::new();
    for k in 1..=main {
        out.push((0, k - 1));
        for &(n, s, c) in &cleanups {
            if k % c.every_n_passes.max(1) == 0 || k == main {
                out.extend((0..s.passes).map(|r| (n, runs[n].saturating_mul(s.passes).saturating_add(r))));
                runs[n] += 1;
            }
        }
    }
    for (i, s) in layer.sub_layers.iter().enumerate().filter(|(_, s)| s.cleanup.is_none()) {
        out.extend((0..s.passes).map(|r| (i + 1, r)));
    }
    out
}

/// How many passes [`layer_schedule`] gives `layer`, without listing them.
fn layer_pass_count(layer: &Layer) -> u64 {
    let main = u64::from(layer.settings.passes);
    layer.sub_layers.iter().fold(main, |total, s| {
        let runs = match &s.cleanup {
            Some(c) => main.div_ceil(u64::from(c.every_n_passes.max(1))),
            None => 1,
        };
        total.saturating_add(runs.saturating_mul(u64::from(s.passes)))
    })
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
        return Err(PlanError::PowerTooHigh {
            layer: layer.into(),
            power_pct: cut.power_pct,
            max_pct: target.max_power_pct,
        });
    }
    if !(cut.speed_mm_s.is_finite() && cut.speed_mm_s > 0.0) {
        return Err(invalid("speed_mm_s", cut.speed_mm_s));
    }
    if !(cut.frequency_khz.is_finite() && cut.frequency_khz > 0.0) {
        return Err(invalid("frequency_khz", cut.frequency_khz));
    }
    if matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine)
        && !(cut.hatch.interval_mm.is_finite() && cut.hatch.interval_mm > 0.0)
    {
        return Err(invalid("hatch.interval_mm", cut.hatch.interval_mm));
    }
    let pulse_width_ns = match cut.pulse_width_ns {
        None => None,
        Some(ns) => match nearest_pulse_width(ns, &target.pulse_widths_ns) {
            PulseDecision::Exact(w) | PulseDecision::Snapped { to: w, .. } => Some(w),
            PulseDecision::Refused { requested, nearest } => {
                return Err(PlanError::PulseWidth {
                    layer: layer.into(),
                    requested_ns: requested,
                    nearest_ns: nearest,
                });
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
    crate::encode::PassWords::new(&settings, target)
        .map_err(|e| PlanError::Encode { layer: layer.into(), source: e })?;
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
                let lines = hatch(&rings, a, cut.hatch.interval_mm, cut.hatch.bidirectional, budget - lines_made)
                    .map_err(|e| match e {
                        HatchError::TooManyScanlines => {
                            PlanError::TooManyHatchLines { layer: layer.into(), max: MAX_SCANLINES }
                        }
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
        let (p, t, end) = pass_stats(pass, pos);
        s.jump_length_mm += p.jump_length_mm;
        s.mark_length_mm += p.mark_length_mm;
        s.segment_count += p.segment_count;
        s.polyline_count += p.polyline_count;
        time_us += t;
        pos = end;
    }
    s.estimated_time_s = time_us / 1e6;
    s
}

/// Lengths, counts and time of one pass that starts with the galvo at `pos` (the jump to its first polyline is
/// counted). Returns the stats, the time in µs (also in the stats, in s) and where the pass ends.
pub(crate) fn pass_stats(pass: &PassPlan, mut pos: Point) -> (Stats, f64, Point) {
    let mut s = Stats::default();
    let mut time_us = 0.0;
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
    s.estimated_time_s = time_us / 1e6;
    (s, time_us, pos)
}
