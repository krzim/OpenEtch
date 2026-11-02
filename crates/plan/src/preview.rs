//! Previews: what a job will do, as field-frame segments. [`simulate`] reads encoded packets back (what the board
//! would run); [`preview_plan`] draws a plan directly (cheap, for the live UI) and [`preview_document`] adds a
//! summary per layer (`GET /api/preview`).

use lmc::units::units_to_speed;
use lmc::{LASER_ENABLE_BIT, ListCommand, Packet, TraceKind, TraceState, decode, trace_from};
use serde::{Deserialize, Serialize};

use doc::{Cleanup, Document, FillMode, LayerId};

use crate::geom::{Point, dist};
use crate::job::{JobPlan, Stats, pass_stats};
use crate::{PlanError, Target};

/// What the galvo does along a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// Laser off.
    Jump,
    /// Laser on (a zero-length mark is a dwell).
    Mark,
}

/// One straight motion in field-frame mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewSegment {
    pub kind: SegmentKind,
    pub from: Point,
    pub to: Point,
    /// The layer it belongs to; `None` from [`simulate`], which only sees packets.
    pub layer: Option<String>,
    /// Index of its pass in [`JobPlan::passes`] (burn order); `None` from [`simulate`]. A jump belongs to the pass
    /// whose polyline it travels to.
    #[serde(default)]
    pub pass: Option<usize>,
    /// A mark that fires: the master oscillator is open and the laser-enable port bit is set. A mark with
    /// `emits == false` moves the galvo at mark speed without light (a list bug if it ever shows up).
    pub emits: bool,
    /// The mark's own power in %, when it differs from its pass's (grayscale image runs, power scale); `None` from
    /// [`simulate`] and for marks at the pass's power.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_pct: Option<f64>,
    /// Estimated time from the start of the job to the end of this segment, s (the plan's time model: motion at the
    /// set speeds plus delays, pauses and dots; 0 from [`simulate`]). For a time slider and "start here".
    #[serde(default)]
    pub t_s: f64,
}

/// A previewed job (`GET /api/preview`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Preview {
    pub segments: Vec<PreviewSegment>,
    /// One entry per job pass in burn order, so [`PreviewSegment::pass`] indexes it; empty from [`simulate`].
    #[serde(default)]
    pub passes: Vec<PassPreview>,
    pub stats: Stats,
    /// One entry per document layer, in document order (burn order); empty from [`preview_plan`] and [`simulate`].
    #[serde(default)]
    pub layers: Vec<LayerPreview>,
    /// What the plan left out (e.g. images), from [`JobPlan::warnings`].
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// One pass of a previewed job ([`crate::PassPlan`] without its polylines, plus its share of the stats).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassPreview {
    pub layer_id: LayerId,
    /// 0 for the layer's main settings, `n` for `sub_layers[n - 1]`.
    pub sub_layer: usize,
    /// Repetition of those settings.
    pub repeat: u32,
    /// Straight mark segments.
    pub segments: usize,
    pub mark_length_mm: f64,
    /// Travel into and within the pass.
    pub jump_length_mm: f64,
    /// Motion and delays of the pass, as [`Stats::estimated_time_s`].
    pub estimated_time_s: f64,
}

/// One set of settings of a layer (its main settings or one sub-layer, as LightBurn's sub-layer tabs show them) and
/// what its passes contribute to the job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubLayerPreview {
    /// 0 for the main settings, `n` for `sub_layers[n - 1]` (as [`PassPreview::sub_layer`]).
    pub sub_layer: usize,
    /// The sub-layer's name, if it has one.
    pub name: Option<String>,
    /// Display label: "Main", the name, "Cleanup" for an unnamed cleanup, else "Sub-layer n".
    pub label: String,
    pub cleanup: Option<Cleanup>,
    pub mode: FillMode,
    pub power_pct: f64,
    pub speed_mm_s: f64,
    pub frequency_khz: f64,
    pub pulse_width_ns: Option<f64>,
    /// Repetitions asked for (per run, for a cleanup).
    pub passes: u32,
    /// Job passes planned with these settings (0 when they have nothing to mark or the layer no output).
    pub planned_passes: usize,
    pub segments: usize,
    pub mark_length_mm: f64,
    pub jump_length_mm: f64,
    pub estimated_time_s: f64,
}

/// What one layer contributes to a previewed job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerPreview {
    pub layer_id: LayerId,
    pub name: String,
    /// Display colour (CSS).
    pub color: String,
    /// The layer's own mode (sub-layers may differ).
    pub mode: FillMode,
    /// Whether the layer is in the job at all.
    pub output: bool,
    pub visible: bool,
    /// Passes planned for it (repeats and sub-layers; 0 when it has nothing to mark or no output).
    pub passes: usize,
    /// Straight mark segments.
    pub segments: usize,
    pub mark_length_mm: f64,
    /// Travel into and within its passes.
    pub jump_length_mm: f64,
    /// Time of its passes (motion and delays, as [`Stats::estimated_time_s`], without the job's start-up delay).
    pub estimated_time_s: f64,
    /// The main settings and every sub-layer, in list order, with their share of the totals above.
    #[serde(default)]
    pub sub_layers: Vec<SubLayerPreview>,
}

/// Draw `plan` without encoding: a jump from the field centre (or the previous polyline's end) to each polyline's
/// start, then its marks (a dot is a zero-length mark). Zero-length jumps are left out. Every segment carries the
/// estimated time at its end ([`PreviewSegment::t_s`]), in the same model as the plan's stats.
pub fn preview_plan(plan: &JobPlan) -> Preview {
    let mut segments = Vec::new();
    let mut passes = Vec::with_capacity(plan.passes.len());
    let mut pos: Point = [0.0, 0.0];
    // The time before the first pass (the open-MO waits): what the stats have beyond the passes' own time.
    let pass_total: f64 = {
        let mut p: Point = [0.0, 0.0];
        plan.passes
            .iter()
            .map(|pass| {
                let (_, t, end) = pass_stats(pass, p);
                p = end;
                t
            })
            .sum()
    };
    let mut t_us = (plan.stats.estimated_time_s * 1e6 - pass_total).max(0.0);
    for (index, pass) in plan.passes.iter().enumerate() {
        let (s, _, _) = pass_stats(pass, pos);
        passes.push(PassPreview {
            layer_id: pass.layer_id.clone(),
            sub_layer: pass.sub_layer,
            repeat: pass.repeat,
            segments: s.segment_count,
            mark_length_mm: s.mark_length_mm,
            jump_length_mm: s.jump_length_mm,
            estimated_time_s: s.estimated_time_s,
        });
        let layer = Some(pass.layer_id.clone());
        let at = |kind, from, to, emits, power_pct, t_us: f64| PreviewSegment {
            kind,
            from,
            to,
            layer: layer.clone(),
            pass: Some(index),
            emits,
            power_pct,
            t_s: t_us / 1e6,
        };
        let tm = &pass.settings.timing;
        t_us += pass.pause_before_us;
        for (i, poly) in pass.polylines.iter().enumerate() {
            let Some(&first) = poly.first() else { continue };
            let jump = dist(pos, first);
            if jump > 0.0 {
                t_us += pass.settings.jump_us(jump) + crate::encode::jump_delay_us(tm, jump).max(0.0);
            }
            if first != pos {
                segments.push(at(SegmentKind::Jump, pos, first, false, None, t_us));
            }
            let power = Some(pass.power_of(i)).filter(|p| *p != pass.settings.power_pct);
            t_us += tm.laser_on_delay_us.max(0.0);
            if poly.len() == 1 {
                t_us += pass.dot_time_us.unwrap_or(0.0);
                segments.push(at(SegmentKind::Mark, first, first, true, power, t_us));
            }
            let polygon = (tm.polygon_delay_us.max(0.0) / 10.0).trunc() * 10.0;
            for (k, w) in poly.windows(2).enumerate() {
                if k > 0 {
                    t_us += polygon;
                }
                t_us += pass.settings.mark_us(dist(w[0], w[1]));
                segments.push(at(SegmentKind::Mark, w[0], w[1], true, power, t_us));
            }
            t_us += tm.laser_off_delay_us.max(0.0) + tm.end_delay_us.max(0.0);
            if let Some(last) = segments.last_mut().filter(|s| s.kind == SegmentKind::Mark) {
                last.t_s = t_us / 1e6;
            }
            pos = poly.last().copied().unwrap_or(first);
        }
        t_us += pass.pause_after_us;
    }
    Preview { segments, passes, stats: plan.stats, layers: Vec::new(), warnings: plan.warnings.clone() }
}

/// Multiply every estimated time of `preview` by `factor` (the stats, each pass, layer and sub-layer, and every
/// segment's `t_s`), so a learned correction (`engine::marktime`) keeps the totals and the timeline in step.
/// A factor that is not finite and positive changes nothing.
pub fn scale_times(preview: &mut Preview, factor: f64) {
    if !(factor.is_finite() && factor > 0.0) || factor == 1.0 {
        return;
    }
    preview.stats.estimated_time_s *= factor;
    preview.passes.iter_mut().for_each(|p| p.estimated_time_s *= factor);
    for l in &mut preview.layers {
        l.estimated_time_s *= factor;
        l.sub_layers.iter_mut().for_each(|s| s.estimated_time_s *= factor);
    }
    preview.segments.iter_mut().for_each(|s| s.t_s *= factor);
}

/// [`preview_plan`] plus a [`LayerPreview`] for every layer of `doc` (the document `plan` was planned from).
pub fn preview_document(doc: &Document, plan: &JobPlan) -> Preview {
    let mut preview = preview_plan(plan);
    let mut layers: Vec<LayerPreview> = doc
        .layers
        .iter()
        .map(|l| LayerPreview {
            layer_id: l.id.clone(),
            name: l.name.clone(),
            color: l.color.clone(),
            mode: l.settings.mode.clone(),
            output: l.output,
            visible: l.visible,
            passes: 0,
            segments: 0,
            mark_length_mm: 0.0,
            jump_length_mm: 0.0,
            estimated_time_s: 0.0,
            sub_layers: std::iter::once(&l.settings)
                .chain(&l.sub_layers)
                .enumerate()
                .map(|(n, s)| SubLayerPreview {
                    sub_layer: n,
                    name: if n == 0 { None } else { s.name.clone() },
                    label: sub_layer_label(n, s),
                    cleanup: if n == 0 { None } else { s.cleanup.clone() },
                    mode: s.mode.clone(),
                    power_pct: s.power_pct,
                    speed_mm_s: s.speed_mm_s,
                    frequency_khz: s.frequency_khz,
                    pulse_width_ns: s.pulse_width_ns,
                    passes: s.passes,
                    planned_passes: 0,
                    segments: 0,
                    mark_length_mm: 0.0,
                    jump_length_mm: 0.0,
                    estimated_time_s: 0.0,
                })
                .collect(),
        })
        .collect();
    for pass in &preview.passes {
        if let Some(l) = layers.iter_mut().find(|l| l.layer_id == pass.layer_id) {
            l.passes += 1;
            l.segments += pass.segments;
            l.mark_length_mm += pass.mark_length_mm;
            l.jump_length_mm += pass.jump_length_mm;
            l.estimated_time_s += pass.estimated_time_s;
            if let Some(s) = l.sub_layers.get_mut(pass.sub_layer) {
                s.planned_passes += 1;
                s.segments += pass.segments;
                s.mark_length_mm += pass.mark_length_mm;
                s.jump_length_mm += pass.jump_length_mm;
                s.estimated_time_s += pass.estimated_time_s;
            }
        }
    }
    preview.layers = layers;
    preview
}

/// "Main" for the main settings (`n` 0); a sub-layer's name, "Cleanup" for an unnamed cleanup, else "Sub-layer n".
fn sub_layer_label(n: usize, s: &doc::CutSettings) -> String {
    match (n, &s.name, &s.cleanup) {
        (0, _, _) => "Main".into(),
        (_, Some(name), _) => name.clone(),
        (_, None, Some(_)) => "Cleanup".into(),
        (_, None, None) => format!("Sub-layer {n}"),
    }
}

/// The rest of `plan` from preview segment `segment` on ([`preview_plan`]'s numbering: LightBurn's "Start Here" on
/// the preview slider). A jump starts the job at the polyline it travels to; a mark starts it at that mark's start
/// point, on the same polyline. Earlier passes and polylines are dropped; the stats are recomputed for `target`.
/// Refused for a multi-part plan (rotary or Z) and for an index past the end.
pub fn start_at(plan: &JobPlan, segment: usize, target: &Target) -> Result<JobPlan, PlanError> {
    if plan.parts.len() > 1 {
        return Err(PlanError::Target("start here is not available for rotary or Z-stepped jobs".into()));
    }
    let mut n = 0usize;
    let mut pos: Point = [0.0, 0.0];
    for (pi, pass) in plan.passes.iter().enumerate() {
        for (li, poly) in pass.polylines.iter().enumerate() {
            let Some(&first) = poly.first() else { continue };
            // (first point index of the remainder) if `segment` falls in this polyline.
            let mut cut: Option<usize> = None;
            if first != pos {
                if n == segment {
                    cut = Some(0);
                }
                n += 1;
            }
            let marks = poly.len().saturating_sub(1).max(usize::from(poly.len() == 1));
            if cut.is_none() && segment < n + marks {
                cut = Some(if poly.len() == 1 { 0 } else { segment - n });
            }
            if let Some(k) = cut {
                let mut out = JobPlan { passes: plan.passes[pi..].to_vec(), stats: Stats::default(), warnings: plan.warnings.clone(), parts: plan.parts.clone() };
                let p = &mut out.passes[0];
                p.polylines.drain(..li);
                if !p.powers.is_empty() {
                    p.powers.drain(..li);
                }
                p.polylines[0].drain(..k);
                p.pause_before_us = 0.0;
                p.gantry.image_runs = p.gantry.image_runs.saturating_sub(li);
                if k > 0 && (p.gantry.lead_in.is_some() || p.gantry.lead_out.is_some()) {
                    // A GRBL lead at a path resumed part-way would be measured from the resume point, and for a
                    // closed outline (now open) on the wrong side, into the part: that polyline is cut on its own,
                    // without leads.
                    let mut head = p.clone();
                    head.polylines.truncate(1);
                    head.powers.truncate(1);
                    head.gantry.lead_in = None;
                    head.gantry.lead_out = None;
                    p.polylines.remove(0);
                    if !p.powers.is_empty() {
                        p.powers.remove(0);
                    }
                    p.gantry.image_runs = p.gantry.image_runs.saturating_sub(1);
                    if p.polylines.is_empty() {
                        out.passes[0] = head;
                    } else {
                        head.pause_after_us = 0.0;
                        out.passes.insert(0, head);
                    }
                }
                out.stats = crate::job::plan_stats(&out, target);
                return Ok(out);
            }
            n += marks;
            pos = poly.last().copied().unwrap_or(first);
        }
    }
    Err(PlanError::Target(format!("segment {segment} is past the end of the job ({n} segments)")))
}

impl Preview {
    /// Keep only the segments and layer entries of `layers` (stats stay those of the whole job, and `passes` stays
    /// whole so the segments' pass indices still point into it).
    pub fn retain_layers(&mut self, layers: &[&str]) {
        self.segments.retain(|s| s.layer.as_deref().is_some_and(|l| layers.contains(&l)));
        self.layers.retain(|l| layers.contains(&l.layer_id.as_str()));
    }
}

/// Read packets back into field-frame segments with `lmc::trace_from`, starting from the state `machine`'s run
/// prelude leaves the board in: galvo at the field centre, MO open (`Fiber_SetMo(1)`) and the port at the
/// laser-enable bit. The list itself never opens the MO (see the `encode` module), so a mark emits while the list
/// keeps the laser-enable bit set.
pub fn simulate(packets: &[Packet], target: &Target) -> Preview {
    let start = TraceState { mo_open: true, port: LASER_ENABLE_BIT, ..TraceState::default() };
    let (trace, _) = trace_from(packets, start);
    let segments: Vec<PreviewSegment> = trace
        .iter()
        .map(|s| PreviewSegment {
            kind: match s.kind {
                TraceKind::Jump => SegmentKind::Jump,
                TraceKind::Mark => SegmentKind::Mark,
            },
            from: target.from_galvo(s.from),
            to: target.from_galvo(s.to),
            layer: None,
            pass: None,
            emits: s.emits(),
            power_pct: None,
            t_s: 0.0,
        })
        .collect();
    let mut stats = Stats::default();
    let mut in_run = false;
    for s in &segments {
        let len = dist(s.from, s.to);
        match s.kind {
            SegmentKind::Jump => {
                stats.jump_length_mm += len;
                in_run = false;
            }
            SegmentKind::Mark => {
                stats.mark_length_mm += len;
                stats.segment_count += 1;
                if !in_run {
                    stats.polyline_count += 1;
                }
                in_run = true;
            }
        }
    }
    stats.estimated_time_s = list_time_us(&decode(packets), target) / 1e6;
    Preview { segments, passes: Vec::new(), stats, layers: Vec::new(), warnings: Vec::new() }
}

/// Estimated run time of a decoded list in µs, with the same model as the plan's estimate: motion at the speeds the
/// words give (times [`Target::motion_time_scale`]), jump delays, laser on/off delays per mark run, polygon delays
/// between consecutive marks, and explicit waits.
fn list_time_us(cmds: &[ListCommand], target: &Target) -> f64 {
    let scale = target.motion_time_scale;
    let unit = target.galvo_unit_mm();
    let speed = |units: u16| units_to_speed(units, target.transform().speed_field_mm());
    let (mut jump_speed, mut mark_speed) = (0.0, 0.0);
    let (mut on, mut off, mut polygon, mut jump_delay) = (0.0, 0.0, 0.0, 0.0);
    let mut pos = (lmc::CENTER, lmc::CENTER);
    let mut prev_mark = false;
    let mut t = 0.0;
    let galvo_dist = |a: (u16, u16), b: (u16, u16)| {
        (f64::from(a.0) - f64::from(b.0)).hypot(f64::from(a.1) - f64::from(b.1)) * unit
    };
    for c in cmds {
        let mut is_mark = false;
        match *c {
            ListCommand::JumpSpeed { units } => jump_speed = speed(units),
            ListCommand::MarkSpeed { units } => mark_speed = speed(units),
            ListCommand::LaserOnDelay(d) => on = d.micros().max(0.0),
            ListCommand::LaserOffDelay(d) => off = d.micros().max(0.0),
            ListCommand::PolygonDelay { time_10us } => polygon = f64::from(time_10us) * 10.0,
            ListCommand::JumpDelay(d) => jump_delay = d.micros().max(0.0),
            ListCommand::DelayTime { time_10us } | ListCommand::LaserOnPoint { time_10us } => t += f64::from(time_10us) * 10.0,
            ListCommand::JumpTo { x, y, .. } => {
                if jump_speed > 0.0 {
                    t += galvo_dist(pos, (x, y)) / jump_speed * 1e6 * scale;
                }
                t += jump_delay;
                pos = (x, y);
            }
            ListCommand::MarkTo { x, y, .. } => {
                if mark_speed > 0.0 {
                    t += galvo_dist(pos, (x, y)) / mark_speed * 1e6 * scale;
                }
                t += if prev_mark { polygon } else { on + off };
                is_mark = true;
                pos = (x, y);
            }
            _ => {}
        }
        prev_mark = is_mark;
    }
    t
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_job;
    use crate::target::tests::target;
    use doc::{Geometry, Layer, Object};
    use kurbo::Affine;

    fn rect(id: &str, layer: &str, at: (f64, f64)) -> Object {
        let g = Geometry::Rect { width_mm: 10.0, height_mm: 10.0, corner_radius_mm: 0.0 };
        Object::new(id.into(), id, layer.into(), g, Affine::translate(at))
    }

    #[test]
    fn per_layer_totals_add_up_and_segments_carry_their_pass() {
        let mut d = Document::default();
        let mut engrave = Layer::new("layer-7".into(), "Engrave", "#ff0000");
        engrave.settings.mode = FillMode::Fill;
        engrave.settings.passes = 2;
        d.layers.push(engrave);
        d.layers.push(Layer { output: false, ..Layer::new("layer-8".into(), "Off", "#0000ff") });
        d.objects = vec![rect("obj-a", "layer-1", (-20.0, 0.0)), rect("obj-b", "layer-7", (20.0, 0.0)), rect("obj-c", "layer-8", (0.0, 30.0))];
        let plan = plan_job(&d, &target()).unwrap();
        let p = preview_document(&d, &plan);
        assert_eq!(p.layers.iter().map(|l| l.layer_id.as_str()).collect::<Vec<_>>(), ["layer-1", "layer-7", "layer-8"]);
        let (line, fill, off) = (&p.layers[0], &p.layers[1], &p.layers[2]);
        assert_eq!((line.passes, fill.passes, off.passes), (1, 2, 0));
        assert!(!off.output && off.segments == 0 && off.estimated_time_s == 0.0);
        assert_eq!(line.segments + fill.segments, p.stats.segment_count);
        assert!((line.mark_length_mm + fill.mark_length_mm - p.stats.mark_length_mm).abs() < 1e-9);
        assert!((line.jump_length_mm + fill.jump_length_mm - p.stats.jump_length_mm).abs() < 1e-9);
        let total = line.estimated_time_s + fill.estimated_time_s;
        assert!(total > 0.0 && total <= p.stats.estimated_time_s, "{total} vs {}", p.stats.estimated_time_s);
        // Every segment names its layer and pass, and the pass index points at that layer's pass.
        for s in &p.segments {
            let pass = s.pass.expect("planned segments carry their pass");
            assert_eq!(s.layer.as_deref(), Some(plan.passes[pass].layer_id.as_str()));
        }
        assert!(p.segments.iter().any(|s| s.pass == Some(2)), "the second fill pass is there");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["layers"][1]["mode"], "fill");
        assert!(v["segments"][0]["pass"].is_u64());

        let mut only = p.clone();
        only.retain_layers(&["layer-7"]);
        assert!(only.segments.iter().all(|s| s.layer.as_deref() == Some("layer-7")));
        assert_eq!(only.segments.len(), p.segments.iter().filter(|s| s.layer.as_deref() == Some("layer-7")).count());
        assert_eq!(only.layers.len(), 1);
        assert_eq!(only.stats, p.stats, "stats stay the whole job's");
    }
}
