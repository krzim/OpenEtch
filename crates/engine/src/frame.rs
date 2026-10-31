//! Red-pointer framing. [`outline_for`] turns the selection (or every object on an output layer) into polylines in
//! field mm; [`start`] traces them in a loop with `Controller::frame` (light only, never the MO or laser-enable bit)
//! until [`stop`], a run, a stop request or [`FRAME_MAX_SECS`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use doc::Document;
use kurbo::PathEl;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::hw::Machine;
use crate::{EngineError, Event};

/// A frame loop ends on its own after this long.
pub const FRAME_MAX_SECS: u64 = 120;
/// Flattening tolerance for outline framing.
pub const OUTLINE_TOLERANCE_MM: f64 = 0.05;
/// Most points an outline frame may have.
pub const MAX_OUTLINE_POINTS: usize = 20_000;
/// Aim for one `Controller::frame` call to take about this long.
const TARGET_CALL_SECS: f64 = 1.5;
/// Most traced points one `Controller::frame` call may produce (loops x points).
const MAX_POINTS_PER_CALL: usize = 200_000;

/// What to trace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameMode {
    /// The bounding rectangle (default).
    #[default]
    Bounds,
    /// The objects' actual outlines.
    Outline,
}

/// Body of `POST /api/machine/frame` and input of the MCP tool `frame_start`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameRequest {
    /// Object ids to frame. Default: every object on a layer whose output is on (the whole job).
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// "bounds" (default): trace the bounding rectangle in the field frame. "outline": trace the objects' shapes.
    #[serde(default)]
    pub mode: FrameMode,
}

/// The polylines (field mm) a frame request traces. Unknown ids and empty selections are refused.
pub fn outline_for(doc: &Document, req: &FrameRequest) -> Result<Vec<Vec<(f64, f64)>>, EngineError> {
    let objects: Vec<&doc::Object> = match &req.ids {
        Some(ids) => {
            let missing: Vec<&str> = ids.iter().filter(|id| doc.object(id).is_none()).map(String::as_str).collect();
            if !missing.is_empty() {
                return Err(EngineError::NotFound(format!("no object with id {}", missing.join(", "))));
            }
            ids.iter().filter_map(|id| doc.object(id)).collect()
        }
        None => doc.objects.iter().filter(|o| doc.layer(&o.layer).is_some_and(|l| l.output)).collect(),
    };
    if objects.is_empty() {
        return Err(EngineError::BadRequest("nothing to frame: no objects selected or on an output layer".into()));
    }
    match req.mode {
        FrameMode::Bounds => {
            let b = objects.iter().map(|o| o.bounds()).reduce(|a, b| a.union(b)).expect("non-empty");
            Ok(vec![vec![(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1), (b.x0, b.y0)]])
        }
        FrameMode::Outline => {
            let mut polys: Vec<Vec<(f64, f64)>> = Vec::new();
            for o in &objects {
                let path = o.field_path();
                kurbo::flatten(&path, OUTLINE_TOLERANCE_MM, |el| match el {
                    PathEl::MoveTo(p) => polys.push(vec![(p.x, p.y)]),
                    PathEl::LineTo(p) => {
                        if let Some(last) = polys.last_mut() {
                            last.push((p.x, p.y));
                        }
                    }
                    PathEl::ClosePath => {
                        if let Some(last) = polys.last_mut()
                            && let Some(&first) = last.first()
                        {
                            last.push(first);
                        }
                    }
                    // `flatten` only emits MoveTo/LineTo/ClosePath.
                    PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
                });
            }
            polys.retain(|p| p.len() >= 2);
            let points: usize = polys.iter().map(Vec::len).sum();
            if points == 0 {
                return Err(EngineError::BadRequest("nothing to frame: the objects have no outline".into()));
            }
            if points > MAX_OUTLINE_POINTS {
                return Err(EngineError::BadRequest(format!(
                    "outline has {points} points (max {MAX_OUTLINE_POINTS}); frame with mode \"bounds\" instead"
                )));
            }
            Ok(polys)
        }
    }
}

/// Length of one trace of `outline`, including the jumps between polylines and back to the start.
fn loop_length_mm(outline: &[Vec<(f64, f64)>]) -> f64 {
    let d = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let mut len = 0.0;
    let mut pos: Option<(f64, f64)> = None;
    for poly in outline {
        for &p in poly {
            if let Some(q) = pos {
                len += d(q, p);
            }
            pos = Some(p);
        }
    }
    if let (Some(first), Some(last)) = (outline.first().and_then(|p| p.first()), pos) {
        len += d(last, *first);
    }
    len
}

/// Loops per `Controller::frame` call and the call's expected duration.
fn loops_per_call(outline: &[Vec<(f64, f64)>], speed_mm_s: f64) -> (u32, Duration) {
    let speed = if speed_mm_s.is_finite() && speed_mm_s > 0.0 { speed_mm_s } else { 1000.0 };
    let t_loop = (loop_length_mm(outline) / speed).max(0.005);
    let points: usize = outline.iter().map(Vec::len).sum::<usize>().max(1);
    let by_time = (TARGET_CALL_SECS / t_loop).ceil() as usize;
    let by_points = MAX_POINTS_PER_CALL / points;
    let loops = by_time.min(by_points).clamp(1, 1000) as u32;
    (loops, Duration::from_secs_f64(t_loop * f64::from(loops)))
}

/// Start tracing `outline` with the red pointer, replacing any frame already running. Returns once the first
/// trace has been sent (so out-of-field outlines are reported to the caller); the loop continues in the background.
pub async fn start(
    machine: &Arc<Machine>,
    events: &broadcast::Sender<Event>,
    outline: Vec<Vec<(f64, f64)>>,
) -> Result<Value, EngineError> {
    if machine.is_running() {
        return Err(EngineError::Conflict("a run is in progress; framing is not available".into()));
    }
    let generation = machine.bump_frame_gen();
    let (loops, call_time) = loops_per_call(&outline, machine.profile().red_light_speed_mm_s);
    let outline = Arc::new(outline);
    let points: usize = outline.iter().map(Vec::len).sum();
    let first = frame_once(machine, generation, outline.clone(), loops).await;
    match first {
        Ok(Some(packets)) => {
            machine.set_framing_gen(generation);
            let _ = events.send(Event::machine("framing", None));
            let m = machine.clone();
            let ev = events.clone();
            tokio::spawn(async move { frame_loop(m, ev, generation, outline, loops, call_time).await });
            Ok(json!({ "ok": true, "framing": true, "points": points, "loops_per_call": loops, "packets": packets,
                "max_seconds": FRAME_MAX_SECS }))
        }
        Ok(None) => Err(EngineError::Conflict("framing was cancelled before it started".into())),
        Err(e) => {
            machine.set_error(Some(e.to_string()));
            let _ = events.send(Event::machine("error", Some(e.to_string())));
            Err(e)
        }
    }
}

/// One `Controller::frame` call, unless the loop `generation` has been superseded or a run holds the machine.
async fn frame_once(
    machine: &Arc<Machine>,
    generation: u64,
    outline: Arc<Vec<Vec<(f64, f64)>>>,
    loops: u32,
) -> Result<Option<usize>, EngineError> {
    machine
        .blocking(move |m, ctl| {
            if m.frame_gen() != generation || m.is_running() {
                return Ok(None);
            }
            m.ensure_ready(ctl)?;
            Ok(Some(ctl.frame(&outline, loops)?))
        })
        .await?
}

async fn frame_loop(
    machine: Arc<Machine>,
    events: broadcast::Sender<Event>,
    generation: u64,
    outline: Arc<Vec<Vec<(f64, f64)>>>,
    loops: u32,
    call_time: Duration,
) {
    let deadline = Instant::now() + Duration::from_secs(FRAME_MAX_SECS);
    let current = |m: &Machine| m.frame_gen() == generation;
    'outer: loop {
        // Wait for the trace just sent to finish: its expected time, then poll until the board is idle.
        let waited = Instant::now();
        while waited.elapsed() < call_time {
            if !current(&machine) {
                break 'outer;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let poll_until = Instant::now() + Duration::from_secs(5);
        loop {
            if !current(&machine) {
                break 'outer;
            }
            let idle = machine.blocking(|_, ctl| ctl.status().map(|s| s.idle())).await;
            match idle {
                Ok(Ok(true)) => break,
                Ok(Ok(false)) if Instant::now() < poll_until => tokio::time::sleep(Duration::from_millis(50)).await,
                Ok(Ok(false)) => break,
                Ok(Err(e)) => {
                    report_error(&machine, &events, e.to_string());
                    break 'outer;
                }
                Err(e) => {
                    report_error(&machine, &events, e.to_string());
                    break 'outer;
                }
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        match frame_once(&machine, generation, outline.clone(), loops).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                report_error(&machine, &events, e.to_string());
                break;
            }
        }
    }
    // Only the loop that still owns the frame turns the pointer off; a superseding frame or run handles its own.
    if current(&machine) {
        let _ = machine.blocking(|_, ctl| ctl.stop_frame()).await;
        machine.set_framing_gen(0);
        let _ = events.send(Event::machine("frame_stopped", None));
    }
}

fn report_error(machine: &Machine, events: &broadcast::Sender<Event>, e: String) {
    tracing::warn!(error = %e, "frame loop stopped");
    machine.set_error(Some(e.clone()));
    let _ = events.send(Event::machine("error", Some(e)));
}

/// End framing: stop the loop and turn the pointer off (`Controller::stop_frame`).
pub async fn stop(machine: &Arc<Machine>, events: &broadcast::Sender<Event>) -> Result<(), EngineError> {
    let was = machine.is_framing();
    machine.bump_frame_gen();
    if !machine.is_running() {
        machine.blocking(|_, ctl| ctl.stop_frame()).await??;
    }
    if was {
        let _ = events.send(Event::machine("frame_stopped", None));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Geometry, Object};
    use kurbo::Affine;

    fn doc_with_rect(x: f64) -> Document {
        let mut d = Document::default();
        d.objects.push(Object {
            id: "obj-9".into(),
            name: "r".into(),
            layer: "layer-1".into(),
            geometry: Geometry::Rect { width_mm: 10.0, height_mm: 4.0, corner_radius_mm: 0.0 },
            transform: Affine::translate((x, 0.0)),
        });
        d
    }

    #[test]
    fn bounds_frame_is_a_closed_rectangle() {
        let o = outline_for(&doc_with_rect(5.0), &FrameRequest::default()).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].len(), 5);
        assert_eq!(o[0][0], o[0][4]);
        let xs: Vec<f64> = o[0].iter().map(|p| p.0).collect();
        assert!(xs.iter().any(|&x| (x - 0.0).abs() < 1e-9) && xs.iter().any(|&x| (x - 10.0).abs() < 1e-9));
    }

    #[test]
    fn outline_frame_follows_the_shape() {
        let req = FrameRequest { ids: Some(vec!["obj-9".into()]), mode: FrameMode::Outline };
        let o = outline_for(&doc_with_rect(0.0), &req).unwrap();
        assert!(o[0].len() >= 5);
    }

    #[test]
    fn empty_and_unknown_selections_are_refused() {
        assert!(matches!(outline_for(&Document::default(), &FrameRequest::default()), Err(EngineError::BadRequest(_))));
        let req = FrameRequest { ids: Some(vec!["obj-404".into()]), mode: FrameMode::Bounds };
        assert!(matches!(outline_for(&doc_with_rect(0.0), &req), Err(EngineError::NotFound(_))));
    }

    #[test]
    fn loops_scale_with_outline_length() {
        let small = vec![vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)]];
        let (loops, t) = loops_per_call(&small, 3000.0);
        assert!(loops > 1 && t.as_secs_f64() < 3.0);
        let big = vec![vec![(-70.0, -70.0), (70.0, -70.0), (70.0, 70.0), (-70.0, 70.0), (-70.0, -70.0)]];
        assert!(loops_per_call(&big, 3000.0).0 < loops);
    }
}
