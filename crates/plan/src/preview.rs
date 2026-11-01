//! Previews: what a job will do, as field-frame segments. [`simulate`] reads encoded packets back (what the board
//! would run); [`preview_plan`] draws a plan directly (cheap, for the live UI) and [`preview_document`] adds a
//! summary per layer (`GET /api/preview`).

use lmc::units::units_to_speed;
use lmc::{LASER_ENABLE_BIT, ListCommand, Packet, TraceKind, TraceState, decode, trace_from};
use serde::{Deserialize, Serialize};

use doc::{Document, FillMode, LayerId};

use crate::geom::{Point, dist};
use crate::job::{JobPlan, Stats, pass_stats};
use crate::Target;

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
}

/// A previewed job (`GET /api/preview`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Preview {
    pub segments: Vec<PreviewSegment>,
    pub stats: Stats,
    /// One entry per document layer, in document order (burn order); empty from [`preview_plan`] and [`simulate`].
    #[serde(default)]
    pub layers: Vec<LayerPreview>,
    /// What the plan left out (e.g. images), from [`JobPlan::warnings`].
    #[serde(default)]
    pub warnings: Vec<String>,
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
}

/// Draw `plan` without encoding: a jump from the field centre (or the previous polyline's end) to each polyline's
/// start, then its marks. Zero-length jumps are left out.
pub fn preview_plan(plan: &JobPlan) -> Preview {
    let mut segments = Vec::new();
    let mut pos: Point = [0.0, 0.0];
    for (index, pass) in plan.passes.iter().enumerate() {
        let layer = Some(pass.layer_id.clone());
        let at = |kind, from, to, emits| PreviewSegment { kind, from, to, layer: layer.clone(), pass: Some(index), emits };
        for poly in &pass.polylines {
            let Some(&first) = poly.first() else { continue };
            if first != pos {
                segments.push(at(SegmentKind::Jump, pos, first, false));
            }
            for w in poly.windows(2) {
                segments.push(at(SegmentKind::Mark, w[0], w[1], true));
            }
            pos = poly.last().copied().unwrap_or(first);
        }
    }
    Preview { segments, stats: plan.stats, layers: Vec::new(), warnings: plan.warnings.clone() }
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
        })
        .collect();
    let mut pos: Point = [0.0, 0.0];
    for pass in &plan.passes {
        let (s, _, end) = pass_stats(pass, pos);
        pos = end;
        if let Some(l) = layers.iter_mut().find(|l| l.layer_id == pass.layer_id) {
            l.passes += 1;
            l.segments += s.segment_count;
            l.mark_length_mm += s.mark_length_mm;
            l.jump_length_mm += s.jump_length_mm;
            l.estimated_time_s += s.estimated_time_s;
        }
    }
    preview.layers = layers;
    preview
}

impl Preview {
    /// Keep only the segments and layer entries of `layers` (stats stay those of the whole job).
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
    Preview { segments, stats, layers: Vec::new(), warnings: Vec::new() }
}

/// Nominal run time of a decoded list in µs, with the same model as the plan's estimate: motion at the set speeds,
/// jump delays, laser on/off delays per mark run, polygon delays between consecutive marks, and explicit waits.
fn list_time_us(cmds: &[ListCommand], target: &Target) -> f64 {
    let unit = target.galvo_unit_mm();
    let speed = |units: u16| units_to_speed(units, target.field_mm);
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
                    t += galvo_dist(pos, (x, y)) / jump_speed * 1e6;
                }
                t += jump_delay;
                pos = (x, y);
            }
            ListCommand::MarkTo { x, y, .. } => {
                if mark_speed > 0.0 {
                    t += galvo_dist(pos, (x, y)) / mark_speed * 1e6;
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
