//! Previews: what a job will do, as field-frame segments. [`simulate`] reads encoded packets back (what the board
//! would run); [`preview_plan`] draws a plan directly (cheap, for the live UI).

use lmc::units::units_to_speed;
use lmc::{ListCommand, Packet, TraceKind, TraceState, decode, trace_from};
use serde::{Deserialize, Serialize};

use crate::geom::{Point, dist};
use crate::job::{JobPlan, Stats};
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
    /// A mark that fires: the master oscillator is open and the laser-enable port bit is set. A mark with
    /// `emits == false` moves the galvo at mark speed without light (a list bug if it ever shows up).
    pub emits: bool,
}

/// A previewed job (`GET /api/preview`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Preview {
    pub segments: Vec<PreviewSegment>,
    pub stats: Stats,
}

/// Draw `plan` without encoding: a jump from the field centre (or the previous polyline's end) to each polyline's
/// start, then its marks. Zero-length jumps are left out.
pub fn preview_plan(plan: &JobPlan) -> Preview {
    let mut segments = Vec::new();
    let mut pos: Point = [0.0, 0.0];
    for pass in &plan.passes {
        let layer = Some(pass.layer_id.clone());
        for poly in &pass.polylines {
            let Some(&first) = poly.first() else { continue };
            if first != pos {
                segments.push(PreviewSegment { kind: SegmentKind::Jump, from: pos, to: first, layer: layer.clone(), emits: false });
            }
            for w in poly.windows(2) {
                segments.push(PreviewSegment { kind: SegmentKind::Mark, from: w[0], to: w[1], layer: layer.clone(), emits: true });
            }
            pos = poly.last().copied().unwrap_or(first);
        }
    }
    Preview { segments, stats: plan.stats }
}

/// Read packets back into field-frame segments with `lmc::trace_from`, starting from the field centre with the MO
/// closed and the port at 0 (so only marks the list itself enables count as emitting).
pub fn simulate(packets: &[Packet], target: &Target) -> Preview {
    let (trace, _) = trace_from(packets, TraceState::default());
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
    Preview { segments, stats }
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
            ListCommand::PolygonDelay(d) => polygon = d.micros().max(0.0),
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

