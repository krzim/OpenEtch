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

use crate::geom::{Point, Subpath, dist, flatten_within, polyline_length};
use crate::hatch::{HatchError, MAX_SCANLINES, hatch};
use crate::image::{Gray, MAX_IMAGE_DOTS, Placed, RasterError, Run, clip_runs, raster_slice};
use crate::cutopts;
use crate::ramp::{RampEdges, ramp_line};
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
    /// The pass's GRBL layer options (constant power, air assist, overscan, overcut): read only by the G-code output
    /// (`gcode`); the LMC encoder never looks at them.
    #[serde(default, skip_serializing_if = "GantryPass::is_default")]
    pub gantry: GantryPass,
}

/// A pass's GRBL layer options ([`doc::CutOptions`] `constant_power`, `air_assist`, `overscan_mm`, `overcut_mm`, the
/// leads and path pauses), as the G-code output uses them (the fill ramp is planned into the pass's powers).
/// `overscan_mm` is set only for passes that hold scan lines (fill hatches, images).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct GantryPass {
    #[serde(default, skip_serializing_if = "is_false")]
    pub constant_power: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub air_assist: bool,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub overscan_mm: f64,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub overcut_mm: f64,
    /// Line passes only: a lead-in / lead-out cut at each path ([`doc::CutOptions::lead_in`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_in: Option<doc::Lead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_out: Option<doc::Lead>,
    /// Line passes only: a laser-off dwell before / after each path, µs.
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub start_pause_us: f64,
    #[serde(default, skip_serializing_if = "is_zero_f64")]
    pub end_pause_us: f64,
    /// The pass's first `image_runs` polylines are image scan runs (an image on a Line-mode layer): leads and path
    /// pauses are for the vector paths after them, never for raster runs. Set only when a lead or pause is.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub image_runs: usize,
    /// A plan for a gantry target (`crate::gcode::bake_gantry`): the lead-ins, lead-outs and overcuts are already in
    /// `polylines`, so the G-code output cuts them as planned and the stats, the preview and Start Here see them; the
    /// stats and preview also time the path pauses and the overscan run-in and run-out. The lead and overcut
    /// settings stay set (a Ruida job refuses them).
    #[serde(default, skip_serializing_if = "is_false")]
    pub baked: bool,
}

impl GantryPass {
    pub fn is_default(&self) -> bool {
        *self == GantryPass::default()
    }
}

fn is_false(v: &bool) -> bool {
    !*v
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
    /// How the board times this pass's motion (estimate only).
    #[serde(default)]
    pub motion: MotionTime,
}

/// How long the board takes to move, for the time estimate: the speeds its speed words stand for and the target's
/// [`Target::motion_time_scale`].
///
/// The speed words are truncated (`lmc::units::speed_to_units`), so the board is asked for a little less than the
/// layer's speed: 300 mm/s on a 153.75 mm field is 127 units/ms, 297.9 mm/s (2.35 mm/s per unit, which matters most
/// at low speeds). On top of that the owner's B6 took 1.29 to 1.33 times the commanded motion time in both runs
/// measured on 2026-11-01 (see `fixtures/b6-mopa-60w.toml`); `scale` carries that.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MotionTime {
    /// Mark speed the pass's speed word stands for, mm/s; 0 uses [`PassSettings::speed_mm_s`].
    pub mark_speed_mm_s: f64,
    /// Jump speed the pass's jump speed word stands for, mm/s; 0 uses the timing's `jump_speed_mm_s`.
    pub jump_speed_mm_s: f64,
    /// [`Target::motion_time_scale`].
    pub scale: f64,
}

impl Default for MotionTime {
    /// As commanded: the settings' speeds, scale 1.
    fn default() -> Self {
        MotionTime { mark_speed_mm_s: 0.0, jump_speed_mm_s: 0.0, scale: 1.0 }
    }
}

impl MotionTime {
    /// The board's view of `s` on `target`: the speeds its truncated words give, and the target's scale.
    pub(crate) fn for_pass(s: &PassSettings, target: &Target) -> Self {
        let field = target.transform().speed_field_mm();
        let word = |mm_s: f64| lmc::units::speed_to_units(mm_s, field).map_or(0.0, |u| lmc::units::units_to_speed(u, field));
        MotionTime { mark_speed_mm_s: word(s.speed_mm_s), jump_speed_mm_s: word(s.timing.jump_speed_mm_s), scale: target.motion_time_scale }
    }
}

impl PassSettings {
    /// Estimated time to mark `mm`, µs ([`MotionTime`]).
    pub fn mark_us(&self, mm: f64) -> f64 {
        let v = if self.motion.mark_speed_mm_s > 0.0 { self.motion.mark_speed_mm_s } else { self.speed_mm_s };
        mm / v * 1e6 * self.motion.scale
    }

    /// Estimated time to jump `mm`, µs, without the jump delay ([`MotionTime`]).
    pub fn jump_us(&self, mm: f64) -> f64 {
        let v = if self.motion.jump_speed_mm_s > 0.0 { self.motion.jump_speed_mm_s } else { self.timing.jump_speed_mm_s };
        mm / v * 1e6 * self.motion.scale
    }
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
    /// The user start point (field mm) and reverse flag of the shape's closed outlines (Set Start Point).
    start: Option<(Point, bool)>,
    /// Tabs placed by hand on the shape (field mm; LightBurn's Add Tabs tool).
    tabs: Vec<Point>,
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
    // Tool layers (T1 / T2) never output, whatever their output flag says.
    let mut layers: Vec<&Layer> = doc.layers.iter().filter(|l| l.emits()).collect();
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

    // Flatten every output object once (at the document's or device's curve tolerance) and check the field before
    // any settings work.
    let curve_tolerance = target.curve_tolerance(doc);
    let mut outside: Vec<ObjectId> = Vec::new();
    let mut shapes: Vec<ShapeIn> = Vec::new();
    let mut images: Vec<&Object> = Vec::new();
    for obj in doc.objects.iter().filter(|o| layers.iter().any(|l| l.id == o.layer)) {
        if selected.as_ref().is_some_and(|s| !s.contains(&obj.id)) {
            continue;
        }
        let subs = flatten_within(&obj.field_path(), curve_tolerance);
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
            let start = options.and_then(|o| o.start.as_ref()).map(|s| {
                let p = obj.transform * kurbo::Point::new(s.local[0], s.local[1]);
                ([p.x, p.y], s.reverse)
            });
            // (A document edited by hand may hold more tabs than tabs.add allows, or ones that are not numbers.)
            let tabs = options
                .map(|o| {
                    o.tabs
                        .iter()
                        .map(|t| obj.transform * kurbo::Point::new(t[0], t[1]))
                        .filter(|p| p.is_finite())
                        .take(doc::MAX_MANUAL_TABS)
                        .map(|p| [p.x, p.y])
                        .collect()
                })
                .unwrap_or_default();
            shapes.push(ShapeIn { object: obj, subs, group, key, scale, start, tabs });
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
        gantry: target.gantry,
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
        let decoded = layer_images
            .iter()
            .map(|o| decode_image(o, &cuts).map(|d| DecodedImage { mask: mask_polygons(doc, o), ..d }))
            .collect::<Result<Vec<_>>>()?;
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
            let slice = slice_level(layer, cut, repeat, done[0]);
            let mut polylines = Vec::new();
            let mut powers = Vec::new();
            let mut start = pos;
            for (k, image) in decoded.iter().enumerate() {
                let budget = max_segments - segments - polylines.len();
                let key = (k, sub_layer, image_angle.to_bits(), slice.to_bits());
                let rows = match rasters.get(&key) {
                    Some(r) => Rc::clone(r),
                    None => {
                        let r = Rc::new(image.raster(&cut.image, image_angle, budget, max_segments, slice)?);
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
            let image_runs = polylines.len();
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
            let o = &cut.options;
            let path_extras = cut.mode == FillMode::Line
                && (o.lead_in.is_some() || o.lead_out.is_some() || o.start_pause_ms > 0.0 || o.end_pause_ms > 0.0);
            if target.gantry && path_extras && (o.tabs.as_ref().is_some_and(|t| t.enabled) || o.perforation.is_some()) {
                // Tabs (automatic or placed by hand, while the layer's Tabs / Bridges switch is on) and perforation
                // cut a path into pieces; a lead (and a dwell) at every piece would cut across the bridges and the
                // skips. Tab settings kept with the switch off cut nothing, so they do not count.
                return Err(PlanError::Target(format!(
                    "layer {}: lead-in, lead-out and start/end pauses cannot be combined with tabs or perforation",
                    layer.id
                )));
            }
            let leads = cut.mode == FillMode::Line && (o.lead_in.is_some() || o.lead_out.is_some());
            if target.gantry
                && o.wobble.is_some()
                && matches!(cut.mode, FillMode::Line | FillMode::FillThenLine)
                && (leads || o.overcut_mm > 0.0)
            {
                // Wobble turns every outline into a run of small circles: a lead would leave along the first circle
                // instead of the outline, and an overcut would retrace circles, not the outline's start.
                return Err(PlanError::Target(format!(
                    "layer {}: wobble cannot be combined with a lead-in, lead-out or overcut (they would follow the wobble's                      circles, not the outline): turn one of them off",
                    layer.id
                )));
            }
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
                gantry: GantryPass {
                    constant_power: cut.options.constant_power,
                    air_assist: cut.options.air_assist,
                    // Scan lines: image runs, and the hatch lines of fill modes (Offset Fill traces closed rings).
                    overscan_mm: if !decoded.is_empty() || matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine) {
                        cut.options.overscan_mm
                    } else {
                        0.0
                    },
                    overcut_mm: cut.options.overcut_mm,
                    // Lead-ins, lead-outs and path pauses are Line-mode settings (LightBurn's Advanced tab).
                    lead_in: if cut.mode == FillMode::Line { cut.options.lead_in } else { None },
                    lead_out: if cut.mode == FillMode::Line { cut.options.lead_out } else { None },
                    start_pause_us: if cut.mode == FillMode::Line { cut.options.start_pause_ms * 1000.0 } else { 0.0 },
                    end_pause_us: if cut.mode == FillMode::Line { cut.options.end_pause_ms * 1000.0 } else { 0.0 },
                    image_runs: if path_extras { image_runs } else { 0 },
                    baked: false,
                },
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
    if target.gantry {
        crate::gcode::bake_gantry(&mut plan);
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
    /// The job is for a GRBL gantry (`Target::gantry`): the planner applies the gantry-only fill ramp.
    gantry: bool,
}

/// Image runs by (image, settings index, scan angle bits), rows in burn order.
type Rasters = HashMap<(usize, usize, u64, u32), Rc<Vec<Vec<Run>>>>;

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

/// The darkness level (0..1) above which a 3D Sliced pass fires (LightBurn's 3D Sliced mode; 0 for every other
/// mode). Main pass `repeat` (0-based, counted on across global passes) of N slices at (N - 1 - repeat mod N) / N,
/// so the first pass engraves only the deepest (darkest) areas and the last everything not white. A cleanup
/// sub-layer cleans the slice of the last main pass (`main_done` main passes so far), or everything with
/// "Clean whole image" (on the main settings or the cleanup's own); another sub-layer slices over its own passes.
pub(crate) fn slice_level(layer: &Layer, cut: &CutSettings, repeat: u32, main_done: u32) -> f32 {
    if cut.image.dither != doc::DitherMode::Slice3d {
        return 0.0;
    }
    let level = |n: u32, k: u32| {
        let n = n.max(1);
        (n - 1 - k % n) as f32 / n as f32
    };
    if cut.cleanup.is_some() {
        if layer.settings.image.clean_whole_image || cut.image.clean_whole_image {
            return 0.0;
        }
        return level(layer.settings.passes, main_done.saturating_sub(1));
    }
    level(cut.passes, repeat)
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
        motion: MotionTime::default(),
    };
    // A gantry has no LMC speed words: its passes are timed as commanded (scaled by the target's motion_time_scale).
    if target.gantry {
        let motion = MotionTime { scale: target.motion_time_scale, ..MotionTime::default() };
        return Ok(PassSettings { motion, ..settings });
    }
    let settings = PassSettings { motion: MotionTime::for_pass(&settings, target), ..settings };
    // Everything the encoder will need must convert now, so a plan that exists can be encoded.
    crate::encode::PassWords::new(&settings, target).map_err(|e| PlanError::Encode { layer: layer.into(), source: e })?;
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
    /// Apply Mask to Image: the mask's closed outlines in the field frame (even-odd); runs are clipped to them.
    mask: Option<Vec<Vec<Point>>>,
}

/// The outlines of image `o`'s mask ([`doc::Document::image_mask`]; a group stands for its members), flattened.
fn mask_polygons(doc: &doc::Document, o: &Object) -> Option<Vec<Vec<Point>>> {
    let m = doc.image_mask(&o.id)?;
    let ids = doc.expand_group_ids(std::slice::from_ref(m));
    let polys = ids
        .iter()
        .filter_map(|id| doc.object(id))
        .filter(|m| !matches!(m.geometry, Geometry::Image { .. }))
        .flat_map(|m| crate::geom::flatten(&m.field_path()))
        .filter(|s| s.points.len() >= 3)
        .map(|s| s.points)
        .collect();
    Some(polys)
}

impl DecodedImage<'_> {
    fn raster(&self, s: &doc::ImageSettings, angle_deg: f64, budget: usize, max: usize, slice: f32) -> Result<Vec<Vec<Run>>> {
        let placed =
            Placed { gray: &self.gray, width_mm: self.width_mm, height_mm: self.height_mm, transform: self.object.transform };
        let rows = raster_slice(&placed, s, angle_deg, budget, slice).map_err(|e| match e {
            RasterError::TooManyDots { rows, cols } => PlanError::ImageTooLarge {
                object: self.object.id.clone(),
                rows,
                cols,
                max: MAX_IMAGE_DOTS,
            },
            RasterError::OverBudget => PlanError::ImageSegments { object: self.object.id.clone(), max },
        })?;
        Ok(match &self.mask {
            Some(mask) => clip_runs(&rows, mask),
            None => rows,
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
    Ok(DecodedImage { object: o, gray, width_mm: *width_mm, height_mm: *height_mm, mask: None })
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
    let fill = matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine | FillMode::OffsetFill);
    let line = matches!(cut.mode, FillMode::Line | FillMode::FillThenLine);
    let o = &cut.options;
    // LightBurn's Ramp Length: hatch fills only (Offset Fill has none), GRBL gantries only.
    let ramp = ctx.gantry && o.ramp_length_mm > 0.0 && cut.power_pct > 0.0 && matches!(cut.mode, FillMode::Fill | FillMode::FillThenLine);
    let mut out = Vec::new();
    let mut scales = Vec::new();
    let mut pos = start;
    let mut lines_made = 0usize;
    let tabs = o.tabs.as_ref().filter(|t| t.enabled);
    let (line_chunks_of, tab_info) = if line {
        line_buckets(layer, buckets, o, !(ctx.order.enabled && ctx.order.best_start), ctx.remove_overlaps.is_some(), max)?
    } else {
        (Vec::new(), Vec::new())
    };
    let mut line_iter = line_chunks_of.into_iter();
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
            // One hatch marks at one power: shapes with different power scales do not share one.
            let groups: Vec<Vec<&ShapeIn>> = groups.into_iter().flat_map(by_scale).collect();
            // Chunks tagged with their power scale and their group (whose edges a GRBL ramp measures from).
            let mut chunks: Vec<(Chunk, (f64, usize))> = Vec::new();
            let mut ramp_edges: Vec<RampEdges> = Vec::new();
            for (gi, group) in groups.into_iter().enumerate() {
                let rings: Vec<Vec<Point>> =
                    group.iter().flat_map(|s| s.subs.iter()).filter(|s| s.closed).map(|s| s.points.clone()).collect();
                let scale = group.first().map_or(1.0, |s| s.scale);
                ramp_edges.push(if ramp { RampEdges::new(&rings, o.ramp_outer_edge) } else { RampEdges::default() });
                let scale = (scale, gi);
                if cut.mode == FillMode::OffsetFill {
                    // LightBurn's Offset Fill: concentric inward offsets at the line interval (no scan angle).
                    let made = crate::offsets::offset_fill(&rings, cut.hatch.interval_mm, budget.saturating_sub(lines_made))
                        .map_err(|e| offset_error(layer, "offset fill", e, max))?;
                    lines_made += made.iter().map(Vec::len).sum::<usize>();
                    chunks.extend(made.into_iter().map(|r| (Chunk::Closed(r), scale)));
                    continue;
                }
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
            for (p, (k, gi)) in polylines {
                match (ramp, p.as_slice()) {
                    (true, &[a, b]) => {
                        // Power at the edge (never above this line's own), rising to the line's power.
                        let full = cut.power_pct * k;
                        let edge = o.ramp_min_power_pct.min(full);
                        for (piece, f) in ramp_line(a, b, &ramp_edges[gi], o.ramp_length_mm) {
                            out.push(piece.to_vec());
                            scales.push((edge + (full - edge) * f) / cut.power_pct);
                        }
                    }
                    _ => {
                        out.push(p);
                        scales.push(k);
                    }
                }
            }
            pos = end;
        }
        if line {
            let chunks = line_iter.next().unwrap_or_default();
            let chunks = match ctx.remove_overlaps {
                Some(tol) => cutopts::remove_overlaps(chunks, tol),
                None => chunks,
            };
            let (polylines, end) = order_with(chunks, pos, &ctx.order);
            for (p, (k, i)) in polylines {
                for (piece, tab) in line_options(p, o, &tab_info[i]) {
                    out.push(piece);
                    scales.push(if tab { k * tabs.map_or(0.0, |t| t.cut_power_pct) / 100.0 } else { k });
                }
            }
            pos = end;
        }
    }
    Ok((out, scales, pos))
}

/// Line chunks of every bucket (tagged with the power scale and an index into the returned [`ChunkTabs`]), made
/// before any is ordered so Skip Inner Shapes sees the whole pass. A tab placed by hand belongs to the shape's closed
/// outline nearest it. With `keep_rings` (Remove Overlapping Lines may open the outlines) each closed outline is kept
/// too, so the pieces left of it still get its tabs.
#[allow(clippy::type_complexity)]
fn line_buckets(
    layer: &str,
    buckets: &[Vec<&ShapeIn>],
    o: &doc::CutOptions,
    user_start: bool,
    keep_rings: bool,
    max: usize,
) -> Result<(Vec<Vec<(Chunk, (f64, usize))>>, Vec<ChunkTabs>)> {
    let tabs = o.tabs.as_ref().filter(|t| t.enabled);
    let mut per_bucket: Vec<Vec<(Chunk, (f64, usize))>> = Vec::with_capacity(buckets.len());
    let mut info: Vec<ChunkTabs> = Vec::new();
    for bucket in buckets {
        let mut chunks: Vec<(Chunk, (f64, usize))> = Vec::new();
        for s in bucket {
            let first = chunks.len();
            for c in line_chunks(layer, s, o.kerf_offset_mm, user_start, max)? {
                let ring = match &c {
                    Chunk::Closed(r) if keep_rings && tabs.is_some() && !r.is_empty() => Some(r.iter().chain(std::iter::once(&r[0])).copied().collect()),
                    _ => None,
                };
                chunks.push((c, (s.scale, info.len())));
                info.push(ChunkTabs { manual: Vec::new(), auto: s.tabs.is_empty(), inner: false, ring });
            }
            if tabs.is_none() {
                continue;
            }
            for &t in &s.tabs {
                let nearest = chunks[first..]
                    .iter()
                    .filter_map(|(c, (_, k))| match c {
                        Chunk::Closed(r) => Some((ring_distance(r, t), *k)),
                        _ => None,
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, k)) = nearest {
                    info[k].manual.push(t);
                }
            }
        }
        per_bucket.push(chunks);
    }
    if tabs.is_some_and(|t| t.skip_inner) {
        let rings: Vec<(usize, &[Point])> = per_bucket
            .iter()
            .flatten()
            .filter_map(|(c, (_, k))| match c {
                Chunk::Closed(r) => Some((*k, r.as_slice())),
                _ => None,
            })
            .collect();
        let flags = cutopts::inner_rings(&rings.iter().map(|(_, r)| *r).collect::<Vec<_>>());
        for ((k, _), inner) in rings.iter().zip(flags) {
            info[*k].inner = inner;
        }
    }
    // Automatic tabs by spacing scale with the outline's length: a tiny spacing on a long outline would make
    // millions of pieces before the segment budget is counted, so it is refused here.
    if let Some(t) = tabs.filter(|t| t.generation == doc::TabGeneration::Auto && t.spacing_mm > 0.0) {
        let mut pieces = 0.0;
        for (c, (_, k)) in per_bucket.iter().flatten() {
            let Chunk::Closed(r) = c else { continue };
            if !info[*k].auto || (t.skip_inner && info[*k].inner) || r.len() < 2 {
                continue;
            }
            let len = polyline_length(r) + dist(r[r.len() - 1], r[0]);
            let mut n = (len / t.spacing_mm).floor().max(1.0);
            if t.max_tabs > 0 {
                n = n.min(f64::from(t.max_tabs));
            }
            if t.size_mm < len / n {
                pieces += n;
            }
            if pieces > max as f64 {
                return Err(PlanError::TooManySegments { max });
            }
        }
    }
    Ok((per_bucket, info))
}

/// What the tabs of one line chunk need (see [`line_buckets`]).
struct ChunkTabs {
    /// Tabs placed by hand that belong to this outline (field mm).
    manual: Vec<Point>,
    /// Automatic tabs may go here: the shape has no hand-placed tabs.
    auto: bool,
    /// Inside another closed outline of the pass (Skip Inner Shapes).
    inner: bool,
    /// The closed outline (ending where it starts), kept while Remove Overlapping Lines may open it.
    ring: Option<Vec<Point>>,
}

/// Distance from `p` to the nearest point of the closed ring `r`.
fn ring_distance(r: &[Point], p: Point) -> f64 {
    let n = r.len();
    (0..n)
        .map(|i| {
            let (a, b) = (r[i], r[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            dist(p, [a[0] + dx * t, a[1] + dy * t])
        })
        .fold(f64::INFINITY, f64::min)
}

/// `group` split by power scale, in order of each scale's first shape (one group when all share one).
fn by_scale<'s, 'a>(group: Vec<&'s ShapeIn<'a>>) -> Vec<Vec<&'s ShapeIn<'a>>> {
    let mut out: Vec<(f64, Vec<&ShapeIn>)> = Vec::new();
    for s in group {
        match out.iter_mut().find(|(k, _)| *k == s.scale) {
            Some((_, members)) => members.push(s),
            None => out.push((s.scale, vec![s])),
        }
    }
    out.into_iter().map(|(_, m)| m).collect()
}

/// A shape's outlines as line chunks: closed outlines offset by the kerf (LightBurn's Kerf Offset; open ones stay
/// as they are) and, with `user_start`, started at the shape's start point (Set Start Point; the outline nearest
/// it starts at its nearest vertex, reversed when asked).
fn line_chunks(layer: &str, s: &ShapeIn, kerf_mm: f64, user_start: bool, max: usize) -> Result<Vec<Chunk>> {
    let open = s.subs.iter().filter(|p| !p.closed).map(|p| Chunk::Open(p.points.clone()));
    let mut closed: Vec<Vec<Point>> = s.subs.iter().filter(|p| p.closed).map(|p| p.points.clone()).collect();
    if kerf_mm != 0.0 && !closed.is_empty() {
        closed = crate::offsets::kerf(&closed, kerf_mm).map_err(|e| offset_error(layer, "kerf offset", e, max))?;
    }
    if user_start
        && let Some((at, reverse)) = s.start
        && let Some(i) = (0..closed.len()).min_by(|a, b| {
            crate::offsets::ring_distance(&closed[*a], at).total_cmp(&crate::offsets::ring_distance(&closed[*b], at))
        })
    {
        crate::offsets::start_at(&mut closed[i], at, reverse);
    }
    Ok(closed.into_iter().map(Chunk::Closed).chain(open).collect())
}

fn offset_error(layer: &str, what: &str, e: crate::offsets::OffsetError, max: usize) -> PlanError {
    match e {
        crate::offsets::OffsetError::OverBudget => PlanError::TooManySegments { max },
        crate::offsets::OffsetError::Geometry(m) => PlanError::Target(format!("layer {layer}: {what}: {m}")),
    }
}

/// The line options of one ordered outline, in order: tabs (closed outlines; LightBurn's Tabs / Bridges: automatic
/// ones unless the generation is manual, the shape has tabs placed by hand or Skip Inner Shapes applies, and the
/// hand-placed ones), perforation, wobble, dot mode. Each piece says whether it is a tab (marked at the tab cut
/// power; without one, tabs are left out).
fn line_options(p: Vec<Point>, o: &doc::CutOptions, info: &ChunkTabs) -> Vec<(Vec<Point>, bool)> {
    let closed = p.len() > 2 && p.first() == p.last();
    let mut pieces = match o.tabs.as_ref().filter(|t| t.enabled) {
        Some(t) => {
            let auto = (t.generation == doc::TabGeneration::Auto && info.auto && !(t.skip_inner && info.inner))
                .then_some(cutopts::AutoTabs { count: t.count, spacing_mm: t.spacing_mm, max_tabs: t.max_tabs });
            match &info.ring {
                // What Remove Overlapping Lines left of a closed outline keeps that outline's tabs.
                Some(ring) if !closed => cutopts::tab_pieces_of_part(p, ring, auto, &info.manual, t.size_mm, t.cut_power_pct > 0.0),
                _ => cutopts::tab_pieces(p, closed, auto, &info.manual, t.size_mm, t.cut_power_pct > 0.0),
            }
        }
        None => vec![(p, false)],
    };
    if let Some(perf) = &o.perforation {
        pieces = pieces.iter().flat_map(|(p, t)| cutopts::perforate(p, perf.cut_mm, perf.skip_mm).into_iter().map(|q| (q, *t))).collect();
    }
    if let Some(w) = &o.wobble {
        pieces = pieces.iter().map(|(p, t)| (cutopts::wobble(p, w.step_mm, w.size_mm), *t)).collect();
    }
    if let Some(d) = &o.dot_mode {
        return pieces.iter().flat_map(|(p, t)| cutopts::dots(p, d.dot_spacing_mm).into_iter().map(|q| (vec![q], *t))).collect();
    }
    pieces.retain(|(p, _)| p.len() >= 2);
    pieces
}

/// Lengths, counts and an estimated time for a plan, following the encoder's list layout.
///
/// The time model (JOB-050): the open-MO wait of every list; per polyline the jump (at the speed the jump word
/// stands for), its jump delay, the marks (at the speed the mark word stands for), the laser-on (when positive),
/// laser-off and end delays, and a polygon delay per corner; both motion times scaled by
/// [`Target::motion_time_scale`]. On the owner's B6 this put both runs measured on 2026-11-01 within 3 % of the
/// board's own `GetMarkTime` (a 30 x 30 mm fill at 1000 mm/s: 23.57 s measured; an "F" outline at 300 mm/s: 0.777 s).
/// The engine multiplies the result by a correction learned from the device's finished runs (`engine::marktime`).
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
            time_us += pass.settings.jump_us(jump) + crate::encode::jump_delay_us(t, jump).max(0.0);
        }
        time_us += pass.settings.mark_us(mark);
        time_us += t.laser_on_delay_us.max(0.0) + t.laser_off_delay_us.max(0.0) + t.end_delay_us.max(0.0);
        #[allow(clippy::cast_precision_loss)]
        let corners = segments.saturating_sub(1) as f64;
        // As encoded: whole 10 µs units (`lmc::units::polygon_delay_units`).
        time_us += corners * (t.polygon_delay_us.max(0.0) / 10.0).trunc() * 10.0;
        pos = last;
    }
    time_us += crate::gcode::extra_us(pass);
    s.estimated_time_s = time_us / 1e6;
    (s, time_us, pos)
}
