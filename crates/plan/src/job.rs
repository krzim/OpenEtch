//! Document -> [`JobPlan`]: resolve each pass's settings, flatten, hatch, raster images, check the field, order
//! travel, apply the cut options (LightBurn's layer options and Optimization Settings), and split rotary and Z jobs
//! into parts.

use std::collections::HashMap;
use std::rc::Rc;

use doc::{
    CutSettings, DitherMode, Document, FillGrouping, FillMode, GalvoTiming, Geometry, ImageAdjust, Layer, LayerId, Object, ObjectId,
    OrderBy,
};
use lmc::PulseDecision;
use lmc::units::nearest_pulse_width;
use serde::{Deserialize, Serialize};

use crate::geom::{Point, Subpath, dist, flatten, polyline_length};
use crate::hatch::{HatchError, MAX_SCANLINES, hatch};
use crate::image::{Gray, MAX_IMAGE_DOTS, Placed, RasterError, Run, raster};
use crate::cutopts;
use crate::order::{Chunk, OrderOpts, order_with};
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

/// A whole job: passes in burn order, every polyline in field-frame mm.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JobPlan {
    pub passes: Vec<PassPlan>,
    pub stats: Stats,
    /// Things the plan left out or changed that a person should know about (a Z offset without a Z axis, pulse
    /// widths dropped for a source without them, a rotary job previewed flat).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// The parts of a job that moves the extension axis between lists (rotary strips, Z positions), indexed by
    /// [`PassPlan::part`]; empty for a job that is one list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartPlan>,
}

/// One part of a multi-part job: where the extension axis goes before its list.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PartPlan {
    /// Rotary position in motor steps from the job's start (first strip = 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotary_steps: Option<i64>,
    /// Z position, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_mm: Option<f64>,
}

/// What to plan beyond the document: LightBurn's Cut Selected Graphics, the job counter (auto-rotate) and whether
/// the plan is for display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobOptions {
    /// Plan only these objects (group ids stand for their members). Unknown ids are refused.
    pub selection: Option<Vec<ObjectId>>,
    /// How many runs of this job came before (the job counter): auto-rotating fills turn by that many passes' worth.
    pub run_index: u64,
    /// A plan for display: a rotary job is drawn unwrapped (flat) with a warning instead of in strips, and needs no
    /// rotary axis.
    pub preview: bool,
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
    /// Polylines in marking order. A closed shape repeats its first point at the end. Image runs come first, one
    /// two-point polyline each, then the vectors.
    pub polylines: Vec<Vec<Point>>,
    /// Power in % of each polyline, in step with `polylines`, when they differ (grayscale images, shapes with a
    /// power scale); empty when every polyline uses `settings.power_pct`. See [`PassPlan::power_of`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub powers: Vec<f64>,
    /// Index into [`JobPlan::parts`] (0 for a one-list job).
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub part: usize,
    /// Dot mode: each one-point polyline fires in place this long, µs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dot_time_us: Option<f64>,
    /// Wait before the pass, µs (LightBurn's Pause Before).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub pause_before_us: f64,
    /// Wait after the pass, µs (Pause After).
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub pause_after_us: f64,
    /// Z position of this pass, mm (Z offset and step per pass), when the machine has a Z axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_mm: Option<f64>,
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

fn is_zero_f64(v: &f64) -> bool {
    *v == 0.0
}

impl PassPlan {
    /// The power of polyline `i`: its entry in `powers`, else the pass's power.
    pub fn power_of(&self, i: usize) -> f64 {
        self.powers.get(i).copied().unwrap_or(self.settings.power_pct)
    }
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
/// Images are engraved in every pass with that pass's [`doc::ImageSettings`] whatever the mode (LightBurn's Image
/// mode; see the `image` module), before the pass's vectors, each scan run one two-point polyline; their scan angle
/// grows by `image.angle_increment_deg` per repeat. Grayscale runs carry their own power ([`PassPlan::powers`]).
///
/// Refused, never clamped: a point outside the field (naming every offending object), power above the target's
/// limit (for grayscale also the minimum power), a pulse width not in the table, settings the board cannot encode,
/// more than [`MAX_PASSES`] passes, a cleanup interval of 0, more than [`MAX_LAYER_PASSES`] passes of one layer in
/// all, an image that cannot be decoded or would need more than [`MAX_IMAGE_DOTS`] dots, and more than
/// [`MAX_SEGMENTS`] mark segments in total (counted while generating; planning stops at the first pass that goes
/// over, naming the image when one did). Passes with nothing to mark are left out.
pub fn plan_job(doc: &Document, target: &Target) -> Result<JobPlan> {
    plan_job_within(doc, target, MAX_SEGMENTS, &JobOptions::default())
}

/// [`plan_job`] with [`JobOptions`]: a selection (Cut Selected Graphics), the job counter for auto-rotate, and
/// whether the plan is for display.
///
/// Beyond [`plan_job`]: the document's [`doc::Optimization`] orders each pass (Order By group / priority, inner
/// shapes first, direction order, best start / corners / direction, reduce direction changes, remove overlapping
/// lines; with `cuts_last` line-mode layers run after the others); each layer runs `global_passes` times; settings
/// whose `options.output` is off are skipped; the cut options of each pass apply (pauses, dot mode, wobble, tabs,
/// perforation, fill grouping, dot width, flood fill, auto-rotate, Z offset and step); shapes with a power scale
/// mark at that share of the pass's power. A rotary job ([`doc::Rotary`] enabled) is split into strips (see the
/// `rotary` module) unless `preview`; a Z job into parts per Z position.
pub fn plan_job_with(doc: &Document, target: &Target, opts: &JobOptions) -> Result<JobPlan> {
    plan_job_within(doc, target, MAX_SEGMENTS, opts)
}

/// One object of an output layer, flattened, with what ordering and power scale need.
struct ShapeIn<'a> {
    object: &'a Object,
    subs: Vec<Subpath>,
    /// The object's top-level group, if any.
    group: Option<&'a str>,
    /// Sort key from the document's Order By list (group rank, priority).
    key: Vec<u64>,
    /// Power scale, 0..=1.
    scale: f64,
}

/// [`plan_job`] with a segment budget of `max_segments` (tests use a small one).
pub(crate) fn plan_job_within(doc: &Document, target: &Target, max_segments: usize, opts: &JobOptions) -> Result<JobPlan> {
    target.validate()?;
    let rotary = doc.rotary.as_ref().filter(|r| r.enabled);
    if let Some(r) = rotary {
        if let Some(p) = r.problem() {
            return Err(PlanError::Target(p));
        }
        if !opts.preview && !target.rotary_axis {
            return Err(PlanError::Target(
                "rotary marking is on but the machine has no rotary axis (profile `axis` with role \"rotary\")".into(),
            ));
        }
    }
    if let Some(p) = doc.optimization.problem() {
        return Err(PlanError::Target(format!("optimization: {p}")));
    }
    let selected: Option<Vec<ObjectId>> = match &opts.selection {
        None => None,
        Some(ids) => {
            let unknown: Vec<&str> = ids
                .iter()
                .filter(|id| doc.object(id).is_none() && doc.group(id).is_none())
                .map(String::as_str)
                .collect();
            if !unknown.is_empty() {
                return Err(PlanError::Selection(format!("no object or group with id {}", unknown.join(", "))));
            }
            Some(doc.expand_group_ids(ids))
        }
    };
    let mut layers: Vec<&Layer> = doc.layers.iter().filter(|l| l.output).collect();
    // A rotary job's design wraps around the work: only X is bounded by the field (the strips are checked after
    // mapping).
    let in_field = |p: Point| if rotary.is_some() { p[0].is_finite() && p[0].abs() < target.half_field_mm() && p[1].is_finite() } else { target.contains(p) };

    // Order By keys: group rank in drawing order (ungrouped objects together), then priority.
    let order_by: Vec<OrderBy> =
        if doc.optimization.enabled { doc.optimization.order_by.iter().copied().filter(|o| *o != OrderBy::Layer).collect() } else { Vec::new() };
    let mut group_rank: HashMap<Option<&str>, u64> = HashMap::new();
    for o in &doc.objects {
        let g = doc.top_group_of(&o.id).map(|g| g.id.as_str());
        let next = group_rank.len() as u64;
        group_rank.entry(g).or_insert(next);
    }

    // Flatten every output object once and check the field before any settings work.
    let mut outside: Vec<ObjectId> = Vec::new();
    let mut shapes: Vec<ShapeIn> = Vec::new();
    let mut images: Vec<&Object> = Vec::new();
    for obj in doc.objects.iter().filter(|o| layers.iter().any(|l| l.id == o.layer)) {
        if selected.as_ref().is_some_and(|s| !s.contains(&obj.id)) {
            continue;
        }
        let subs = flatten(&obj.field_path());
        if subs.iter().flat_map(|s| &s.points).any(|&p| !in_field(p)) {
            outside.push(obj.id.clone());
        }
        if matches!(obj.geometry, Geometry::Image { .. }) {
            images.push(obj);
        } else {
            let group = doc.top_group_of(&obj.id).map(|g| g.id.as_str());
            let options = doc.object_options.get(&obj.id);
            let key = order_by
                .iter()
                .map(|o| match o {
                    OrderBy::Group => group_rank.get(&group).copied().unwrap_or(u64::MAX),
                    OrderBy::Priority => options.and_then(|o| o.priority).map_or(u64::MAX, u64::from),
                    OrderBy::Layer => 0,
                })
                .collect();
            let scale = options.map_or(1.0, |o| o.power_scale_pct / 100.0);
            shapes.push(ShapeIn { object: obj, subs, group, key, scale });
        }
    }
    if !outside.is_empty() {
        return Err(PlanError::OutOfField { objects: outside, field_mm: target.field_mm });
    }
    if doc.optimization.cuts_last {
        // Line-mode (cut) layers without images after every other layer, keeping the list order within each set.
        let is_cut = |l: &&Layer| l.settings.mode == FillMode::Line && !images.iter().any(|o| o.layer == l.id);
        let (cuts, rest): (Vec<&Layer>, Vec<&Layer>) = layers.into_iter().partition(is_cut);
        layers = rest.into_iter().chain(cuts).collect();
    }
    let ctx = PassCtx {
        order: OrderOpts::from(&doc.optimization),
        remove_overlaps: (doc.optimization.enabled && doc.optimization.remove_overlaps).then_some(doc.optimization.overlap_distance_mm),
    };

    let mut plan = JobPlan::default();
    let mut pos: Point = [0.0, 0.0];
    let mut segments = 0usize;
    let mut z_ignored = false;
    let mut pulse_dropped = false;
    for layer in layers {
        let layer_shapes: Vec<&ShapeIn> = shapes.iter().filter(|s| s.object.layer == layer.id).collect();
        // Buckets in Order By key order (stable: drawing order within a key).
        let mut keys: Vec<&Vec<u64>> = layer_shapes.iter().map(|s| &s.key).collect();
        keys.sort();
        keys.dedup();
        let buckets: Vec<Vec<&ShapeIn>> =
            keys.iter().map(|k| layer_shapes.iter().copied().filter(|s| &&s.key == k).collect()).collect();
        // Every set of settings is checked, also on a layer with nothing to mark.
        let cuts: Vec<&CutSettings> = std::iter::once(&layer.settings).chain(&layer.sub_layers).collect();
        let resolved = cuts.iter().map(|cut| resolve(&layer.id, cut, target)).collect::<Result<Vec<_>>>()?;
        let layer_images: Vec<&Object> = images.iter().copied().filter(|o| o.layer == layer.id).collect();
        if !layer_images.is_empty() {
            for cut in &cuts {
                check_image_settings(&layer.id, cut, target)?;
            }
        }
        for c in layer.sub_layers.iter().filter_map(|s| s.cleanup.as_ref()) {
            if c.every_n_passes == 0 {
                return Err(PlanError::Setting { layer: layer.id.clone(), what: "cleanup.every_n_passes", value: 0.0 });
            }
        }
        for cut in &cuts {
            if let Some(p) = cut.options.problem() {
                return Err(PlanError::Target(format!("layer {}: {p}", layer.id)));
            }
        }
        let total = layer_pass_count(layer).saturating_mul(u64::from(layer.global_passes.max(1)));
        if total > u64::from(MAX_LAYER_PASSES) {
            return Err(PlanError::TooManyPasses {
                layer: layer.id.clone(),
                passes: u32::try_from(total).unwrap_or(u32::MAX),
                max: MAX_LAYER_PASSES,
            });
        }
        if layer_shapes.is_empty() && layer_images.is_empty() {
            continue;
        }
        let decoded = layer_images.iter().map(|o| decode_image(o, &cuts)).collect::<Result<Vec<_>>>()?;
        // Image runs per (image, settings, scan angle), so repeated passes reuse them.
        let mut rasters: Rasters = HashMap::new();
        let schedule = layer_schedule(layer);
        let mut done = vec![0u32; cuts.len()];
        for (sub_layer, repeat) in (0..layer.global_passes.max(1)).flat_map(|_| schedule.iter().copied()) {
            let cut = cuts[sub_layer];
            // Repeats count on across global passes, so a turning hatch keeps turning.
            let repeat = repeat.max(done[sub_layer]);
            done[sub_layer] = repeat + 1;
            if !cut.options.output {
                continue;
            }
            let turns = if cut.options.auto_rotate {
                f64::from(repeat) + opts.run_index as f64 * f64::from(cut.passes.max(1))
            } else {
                f64::from(repeat)
            };
            let angle = cut.hatch.angle_deg + turns * cut.hatch.angle_increment_deg;
            let mut settings = PassSettings { hatch_angle_deg: angle, ..resolved[sub_layer].clone() };
            if !target.pulse_width_enabled && settings.pulse_width_ns.take().is_some() {
                pulse_dropped = true;
            }
            let z = cut.options.z_offset_mm + f64::from(repeat) * cut.options.z_step_mm;
            let z_mm = if cut.options.z_offset_mm == 0.0 && cut.options.z_step_mm == 0.0 {
                None
            } else if target.z_axis {
                Some(z)
            } else {
                z_ignored = true;
                None
            };
            let image_angle =
                (cut.image.angle_deg + f64::from(repeat) * cut.image.angle_increment_deg).rem_euclid(360.0);
            let mut polylines = Vec::new();
            let mut powers = Vec::new();
            let mut start = pos;
            for (k, image) in decoded.iter().enumerate() {
                let budget = max_segments - segments - polylines.len();
                let key = (k, sub_layer, image_angle.to_bits());
                let rows = match rasters.get(&key) {
                    Some(r) => Rc::clone(r),
                    None => {
                        let r = Rc::new(image.raster(&cut.image, image_angle, budget, max_segments)?);
                        rasters.insert(key, Rc::clone(&r));
                        r
                    }
                };
                let runs = rows.iter().map(Vec::len).sum::<usize>();
                if runs > budget {
                    return Err(PlanError::ImageSegments { object: image.object.id.clone(), max: max_segments });
                }
                let scale = doc.object_options.get(&image.object.id).map_or(1.0, |o| o.power_scale_pct / 100.0);
                for run in rows.iter().flatten() {
                    polylines.push(vec![run.from, run.to]);
                    powers.push(run_power(cut, &settings, run.level) * scale);
                    start = run.to;
                }
            }
            let budget = (max_segments - segments - polylines.len(), max_segments);
            let (vectors, scales, end) = pass_polylines(&layer.id, cut, angle, &buckets, start, budget, &ctx)?;
            powers.extend(scales.iter().map(|k| settings.power_pct * k));
            if !powers.iter().any(|p| *p != settings.power_pct) {
                powers.clear();
            }
            polylines.extend(vectors);
            if polylines.is_empty() {
                continue;
            }
            segments += segment_count(&polylines);
            if segments > max_segments {
                return Err(PlanError::TooManySegments { max: max_segments });
            }
            pos = end;
            plan.passes.push(PassPlan {
                layer_id: layer.id.clone(),
                sub_layer,
                repeat,
                settings,
                polylines,
                powers,
                part: 0,
                dot_time_us: cut.options.dot_mode.as_ref().map(|d| d.dot_time_ms * 1000.0),
                pause_before_us: cut.options.pause_before_ms * 1000.0,
                pause_after_us: cut.options.pause_after_ms * 1000.0,
                z_mm,
            });
        }
    }
    if z_ignored {
        plan.warnings.push("a Z offset or Z step is set but the machine has no board-driven Z axis; ignored".into());
    }
    if pulse_dropped {
        plan.warnings.push("the source takes no pulse width (Q-pulse off or Q-switched); pulse widths are not sent".into());
    }
    if let Some(r) = rotary {
        if opts.preview {
            plan.warnings.push(format!(
                "rotary: the preview shows the unwrapped design; a run marks it in strips of {} mm",
                r.split_width_mm
            ));
        } else {
            if plan.passes.iter().any(|p| p.z_mm.is_some()) {
                return Err(PlanError::Target("rotary marking and Z steps both need the one extension axis".into()));
            }
            plan = crate::rotary::split(plan, r, target)?;
        }
    } else if plan.passes.iter().any(|p| p.z_mm.is_some()) {
        z_parts(&mut plan);
    }
    plan.stats = plan_stats(&plan, target);
    Ok(plan)
}

/// Split a plan whose passes set Z positions into parts: a new part whenever the Z position changes.
fn z_parts(plan: &mut JobPlan) {
    let mut current: Option<Option<f64>> = None;
    for pass in &mut plan.passes {
        if current != Some(pass.z_mm) {
            current = Some(pass.z_mm);
            plan.parts.push(PartPlan { rotary_steps: None, z_mm: pass.z_mm });
        }
        pass.part = plan.parts.len() - 1;
    }
}

/// Ordering of a pass: the document's optimization.
struct PassCtx {
    order: OrderOpts,
    /// Remove overlapping lines within this distance, mm.
    remove_overlaps: Option<f64>,
}

/// Image runs by (image, settings index, scan angle bits), rows in burn order.
type Rasters = HashMap<(usize, usize, u64), Rc<Vec<Vec<Run>>>>;

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

/// Refuse image settings that cannot be planned (checked for layers that hold images): a line interval that is not
/// positive, a dot width that is negative or wider than the interval, halftone cells that are not positive, and for
/// grayscale a minimum power outside 0..=100 or above the target's limit.
fn check_image_settings(layer: &str, cut: &CutSettings, target: &Target) -> Result<()> {
    let s = &cut.image;
    let invalid = |what: &'static str, value: f64| PlanError::Setting { layer: layer.into(), what, value };
    if !s.pass_through && !(s.interval_mm.is_finite() && s.interval_mm > 0.0) {
        return Err(invalid("image.interval_mm", s.interval_mm));
    }
    if !s.angle_deg.is_finite() {
        return Err(invalid("image.angle_deg", s.angle_deg));
    }
    if !s.angle_increment_deg.is_finite() {
        return Err(invalid("image.angle_increment_deg", s.angle_increment_deg));
    }
    let widest = if s.pass_through { f64::INFINITY } else { s.interval_mm };
    if !(s.dot_width_mm.is_finite() && s.dot_width_mm >= 0.0 && s.dot_width_mm <= widest) {
        return Err(invalid("image.dot_width_mm", s.dot_width_mm));
    }
    if matches!(s.dither, DitherMode::Halftone)
        && !(s.halftone_cells_per_inch.is_finite() && s.halftone_cells_per_inch > 0.0 && s.halftone_angle_deg.is_finite())
    {
        return Err(invalid("image.halftone_cells_per_inch", s.halftone_cells_per_inch));
    }
    if matches!(s.dither, DitherMode::Grayscale) {
        if !(s.min_power_pct.is_finite() && (0.0..=100.0).contains(&s.min_power_pct)) {
            return Err(invalid("image.min_power_pct", s.min_power_pct));
        }
        if s.min_power_pct > target.max_power_pct {
            return Err(PlanError::PowerTooHigh {
                layer: layer.into(),
                power_pct: s.min_power_pct,
                max_pct: target.max_power_pct,
            });
        }
    }
    Ok(())
}

/// The power of an image run at `level` (255 = the pass's power): grayscale runs lie between the minimum power
/// (level 1 is just above it) and the pass's power; every other mode fires at the pass's power.
fn run_power(cut: &CutSettings, settings: &PassSettings, level: u8) -> f64 {
    if matches!(cut.image.dither, DitherMode::Grayscale) {
        let min = cut.image.min_power_pct;
        min + (settings.power_pct - min) * f64::from(level) / 255.0
    } else {
        settings.power_pct
    }
}

/// One image object of a layer, decoded and adjusted, ready to raster for any of the layer's settings.
struct DecodedImage<'a> {
    object: &'a Object,
    gray: Gray,
    width_mm: f64,
    height_mm: f64,
}

impl DecodedImage<'_> {
    fn raster(&self, s: &doc::ImageSettings, angle_deg: f64, budget: usize, max: usize) -> Result<Vec<Vec<Run>>> {
        let placed =
            Placed { gray: &self.gray, width_mm: self.width_mm, height_mm: self.height_mm, transform: self.object.transform };
        raster(&placed, s, angle_deg, budget).map_err(|e| match e {
            RasterError::TooManyDots { rows, cols } => PlanError::ImageTooLarge {
                object: self.object.id.clone(),
                rows,
                cols,
                max: MAX_IMAGE_DOTS,
            },
            RasterError::OverBudget => PlanError::ImageSegments { object: self.object.id.clone(), max },
        })
    }
}

/// Decode and adjust an image object. When none of the layer's settings uses pass-through and even the finest line
/// interval spans 4 or more pixels, the image is shrunk right away (by half the pixels per dot, so a dot still spans
/// two), which keeps a large photo's memory down; the enhance radius is scaled with it.
fn decode_image<'a>(o: &'a Object, cuts: &[&CutSettings]) -> Result<DecodedImage<'a>> {
    let Geometry::Image { width_mm, height_mm, data_base64, adjust, .. } = &o.geometry else {
        unreachable!("only image objects are decoded")
    };
    let problem = |reason: String| PlanError::Image { object: o.id.clone(), reason };
    if let Some(p) = adjust.problem() {
        return Err(problem(p));
    }
    // The finest dot in the image's own (local) millimetres: the interval over the transform's largest stretch.
    let [a, b, c, d, _, _] = o.transform.as_coeffs();
    let half = (a * a + b * b + c * c + d * d) / 2.0;
    let det = a * d - b * c;
    let stretch = (half + (half * half - det * det).max(0.0).sqrt()).sqrt();
    let finest = if cuts.iter().any(|c| c.image.pass_through) {
        None
    } else {
        cuts.iter().map(|c| c.image.interval_mm).fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v))))
    };
    let dot_local_mm = finest.map(|f| f / stretch).filter(|v| v.is_finite() && *v > 0.0);
    let (mut gray, factor) = crate::image::decode(data_base64, *width_mm, dot_local_mm).map_err(problem)?;
    let scaled = ImageAdjust { enhance_radius: adjust.enhance_radius / factor as f64, ..adjust.clone() };
    crate::image::adjust(&mut gray, &scaled);
    Ok(DecodedImage { object: o, gray, width_mm: *width_mm, height_mm: *height_mm })
}

/// Mark segments of `polylines` (each polyline of n points has n - 1; a one-point dot counts as one).
fn segment_count(polylines: &[Vec<Point>]) -> usize {
    polylines.iter().map(|p| p.len().saturating_sub(1).max(usize::from(p.len() == 1))).sum()
}

/// The ordered polylines of one pass over the layer's flattened objects (in Order By buckets), each with its power
/// scale, and where the galvo ends. Refused with [`PlanError::TooManySegments`] as soon as the hatch lines alone pass
/// `budget`, the segments left of the job's `max` (the caller checks the total).
fn pass_polylines(
    layer: &str,
    cut: &CutSettings,
    angle: f64,
    buckets: &[Vec<&ShapeIn>],
    start: Point,
    (budget, max): (usize, usize),
    ctx: &PassCtx,
) -> Result<(Vec<Vec<Point>>, Vec<f64>, Point)> {
    let fill = matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine);
    let line = matches!(cut.mode, FillMode::Line | FillMode::FillThenLine);
    let o = &cut.options;
    let mut out = Vec::new();
    let mut scales = Vec::new();
    let mut pos = start;
    let mut lines_made = 0usize;
    for bucket in buckets {
        if fill {
            // Fill grouping: which shapes share one hatch.
            let groups: Vec<Vec<&ShapeIn>> = match o.fill_grouping {
                FillGrouping::Individually => bucket.iter().map(|s| vec![*s]).collect(),
                FillGrouping::AllAtOnce => vec![bucket.clone()],
                FillGrouping::ByGroup => {
                    let mut groups: Vec<(Option<&str>, Vec<&ShapeIn>)> = Vec::new();
                    for s in bucket {
                        match s.group.and_then(|g| groups.iter_mut().find(|(k, _)| *k == Some(g))) {
                            Some((_, members)) => members.push(s),
                            None => groups.push((s.group, vec![*s])),
                        }
                    }
                    groups.into_iter().map(|(_, m)| m).collect()
                }
            };
            let mut chunks: Vec<(Chunk, f64)> = Vec::new();
            for group in groups {
                let rings: Vec<Vec<Point>> =
                    group.iter().flat_map(|s| s.subs.iter()).filter(|s| s.closed).map(|s| s.points.clone()).collect();
                let scale = group.first().map_or(1.0, |s| s.scale);
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
                    let lines = cutopts::trim_hatch(lines, o.fill_dot_width_mm);
                    if lines.is_empty() {
                        continue;
                    }
                    if o.flood_fill {
                        for region in cutopts::flood_regions(&lines, a, cut.hatch.interval_mm, cut.hatch.bidirectional) {
                            chunks.push((Chunk::Hatch { lines: region, bidirectional: cut.hatch.bidirectional }, scale));
                        }
                    } else {
                        chunks.push((Chunk::Hatch { lines, bidirectional: cut.hatch.bidirectional }, scale));
                    }
                }
            }
            let (polylines, end) = order_with(chunks, pos, &ctx.order);
            for (p, k) in polylines {
                out.push(p);
                scales.push(k);
            }
            pos = end;
        }
        if line {
            let mut chunks: Vec<(Chunk, f64)> = bucket
                .iter()
                .flat_map(|s| s.subs.iter().map(move |sub| (sub, s.scale)))
                .map(|(s, k)| (if s.closed { Chunk::Closed(s.points.clone()) } else { Chunk::Open(s.points.clone()) }, k))
                .collect();
            if let Some(tol) = ctx.remove_overlaps {
                chunks = cutopts::remove_overlaps(chunks, tol);
            }
            let (polylines, end) = order_with(chunks, pos, &ctx.order);
            for (p, k) in polylines {
                for piece in line_options(p, o) {
                    out.push(piece);
                    scales.push(k);
                }
            }
            pos = end;
        }
    }
    Ok((out, scales, pos))
}

/// The line options of one ordered outline, in order: tabs (closed outlines), perforation, wobble, dot mode.
fn line_options(p: Vec<Point>, o: &doc::CutOptions) -> Vec<Vec<Point>> {
    let closed = p.len() > 2 && p.first() == p.last();
    let mut pieces = match &o.tabs {
        Some(t) => cutopts::tabs(p, closed, t.count, t.spacing_mm, t.size_mm),
        None => vec![p],
    };
    if let Some(perf) = &o.perforation {
        pieces = pieces.iter().flat_map(|p| cutopts::perforate(p, perf.cut_mm, perf.skip_mm)).collect();
    }
    if let Some(w) = &o.wobble {
        pieces = pieces.iter().map(|p| cutopts::wobble(p, w.step_mm, w.size_mm)).collect();
    }
    if let Some(d) = &o.dot_mode {
        return pieces.iter().flat_map(|p| cutopts::dots(p, d.dot_spacing_mm)).map(|p| vec![p]).collect();
    }
    pieces.retain(|p| p.len() >= 2);
    pieces
}

/// Lengths, counts and an estimated time for a plan, following the encoder's list layout.
pub(crate) fn plan_stats(plan: &JobPlan, target: &Target) -> Stats {
    let mut s = Stats::default();
    let mut pos: Point = [0.0, 0.0];
    // Every part's list waits the open-MO delay again.
    let lists = plan.parts.len().max(1) as f64;
    let mut time_us = if plan.passes.is_empty() { 0.0 } else { target.open_mo_delay_us * lists };
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
    let mut time_us = pass.pause_before_us + pass.pause_after_us;
    let t = &pass.settings.timing;
    for p in &pass.polylines {
        let (Some(&first), Some(&last)) = (p.first(), p.last()) else { continue };
        let jump = dist(pos, first);
        let mark = polyline_length(p);
        let segments = p.len().saturating_sub(1).max(usize::from(p.len() == 1));
        if p.len() == 1 {
            time_us += pass.dot_time_us.unwrap_or(0.0);
        }
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
