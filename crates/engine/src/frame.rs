//! Red-pointer framing. [`outline_for`] turns the selection (or every object on an output layer, or only the tool
//! layers) into polylines in field mm, in LightBurn's framing styles: bounds, hull (rubber band) or contour, for all
//! shapes at once or each on its own; [`start`] traces them with `Controller::frame_styled` (light only, never the MO
//! or laser-enable bit: the red pointer, the focus light or both, with or without blanking between shapes, at the
//! frame speed) once or in a loop until [`stop`], a run, a stop request or the time limit. Live framing re-reads the
//! outline whenever the document changes, so the dot follows edits. A red-dot jog ([`point_outline`]) is a frame of
//! one point.

use std::sync::Arc;
use std::time::{Duration, Instant};

use doc::Document;
use kurbo::PathEl;
use machine::{ControllerError, ControllerState, LinkError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use machine::FrameStyle;

use crate::hw::Machine;
use crate::{EngineError, Event};

/// A frame loop ends on its own after this long unless the request says otherwise.
pub const FRAME_MAX_SECS: u64 = 120;
/// Longest time limit a request may ask for.
pub const FRAME_LIMIT_SECS: u64 = 3600;

/// Live framing: given the document revision the frame was made from, the new revision and outline when the
/// document has changed since (and the outline can still be made).
pub type LiveSource = Arc<dyn Fn(u64) -> Option<(u64, Vec<Vec<(f64, f64)>>)> + Send + Sync>;

/// How a frame runs once its outline is known.
#[derive(Clone)]
pub struct FrameRun {
    pub style: FrameStyle,
    /// Trace in a loop (true) or once.
    pub continuous: bool,
    pub max_secs: u64,
    /// Re-read the outline when the document changes (with the revision it was made from).
    pub live: Option<(u64, LiveSource)>,
}
/// Flattening tolerance for outline framing.
pub const OUTLINE_TOLERANCE_MM: f64 = 0.05;
/// Most points an outline frame may have.
pub const MAX_OUTLINE_POINTS: usize = 20_000;
/// Aim for one `Controller::frame` call to take about this long.
const TARGET_CALL_SECS: f64 = 1.5;
/// Most traced points one `Controller::frame` call may produce (loops x points).
const MAX_POINTS_PER_CALL: usize = 200_000;

/// What to trace (LightBurn's framing styles).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameMode {
    /// The bounding rectangle (default).
    #[default]
    Bounds,
    /// The convex hull: a rubber band around the shapes.
    Hull,
    /// The objects' actual outlines (LightBurn's Contour; "contour" is accepted).
    #[serde(alias = "contour")]
    Outline,
}

/// Which pointer outputs light while framing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Pointer {
    /// The red aiming pointer (default).
    #[default]
    Red,
    /// The profile's focus light (`focus_light_port_bit`).
    Focus,
    /// Both.
    Both,
}

/// Body of `POST /api/machine/frame` and input of the MCP tool `frame_start`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameRequest {
    /// Object ids to frame. Default: every object on a layer whose output is on (the whole job).
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    /// "bounds" (default): the bounding rectangle in the field frame; "hull": a rubber band (convex hull) around the
    /// shapes; "outline" (or "contour"): the shapes themselves.
    #[serde(default)]
    pub mode: FrameMode,
    /// Bounds or hull: one frame per object instead of one around all of them (LightBurn's Frame Individually).
    #[serde(default)]
    pub individually: bool,
    /// Outline: skip shapes inside other shapes (letter counters), LightBurn's Outside Shapes Only.
    #[serde(default)]
    pub outside_only: bool,
    /// Frame only the objects on tool layers: layers whose output is off (a jig or fixture outline). Ignored when
    /// `ids` is given.
    #[serde(default)]
    pub tool_layers_only: bool,
    /// Trace in a loop until stopped (default) or once.
    #[serde(default)]
    pub continuous: Option<bool>,
    /// Pointer travel speed, mm/s (default the profile's frame speed; at most its jump speed limit).
    #[serde(default)]
    pub speed_mm_s: Option<f64>,
    /// "red" (default), "focus" (the profile's focus light) or "both".
    #[serde(default)]
    pub pointer: Pointer,
    /// Pointer off while travelling between shapes (default the profile's `blanking`).
    #[serde(default)]
    pub blanking: Option<bool>,
    /// Follow the document: when it changes while framing, the frame is traced again from the new outline.
    #[serde(default)]
    pub live: bool,
    /// Stop on its own after this many seconds (default 120, at most 3600).
    #[serde(default)]
    pub max_seconds: Option<u64>,
    /// Frame images by their content (LightBurn's Image Bounds): the outline of their non-transparent, non-white
    /// pixels (a convex hull) instead of their rectangle.
    #[serde(default)]
    pub image_bounds: bool,
}

/// The polylines (field mm) a frame request traces. Unknown ids and empty selections are refused.
pub fn outline_for(doc: &Document, req: &FrameRequest) -> Result<Vec<Vec<(f64, f64)>>, EngineError> {
    let objects: Vec<&doc::Object> = match &req.ids {
        Some(ids) => {
            let missing: Vec<&str> =
                ids.iter().filter(|id| doc.object(id).is_none() && doc.group(id).is_none()).map(String::as_str).collect();
            if !missing.is_empty() {
                return Err(EngineError::NotFound(format!("no object with id {}", missing.join(", "))));
            }
            doc.expand_group_ids(ids).iter().filter_map(|id| doc.object(id)).collect()
        }
        // Tool layers (T1 / T2) and other layers with output off; a tool layer never has output on.
        None if req.tool_layers_only => doc.objects.iter().filter(|o| doc.layer(&o.layer).is_some_and(|l| !l.emits())).collect(),
        // The job, with what sits on the tool layers T1 / T2 (LightBurn frames those with the job).
        None => doc.objects.iter().filter(|o| doc.layer(&o.layer).is_some_and(|l| l.output || l.tool.is_some())).collect(),
    };
    if objects.is_empty() {
        let what = if req.tool_layers_only { "no objects on tool layers (layers with output off)" } else { "no objects selected or on an output layer" };
        return Err(EngineError::BadRequest(format!("nothing to frame: {what}")));
    }
    let rect = |b: kurbo::Rect| vec![(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1), (b.x0, b.y0)];
    let flattened = |o: &doc::Object| frame_polys(o, req.image_bounds);
    let bounds = |o: &doc::Object| -> Option<kurbo::Rect> {
        let pts: Vec<(f64, f64)> = flattened(o).into_iter().flatten().collect();
        let first = *pts.first()?;
        Some(pts.iter().fold(kurbo::Rect::from_points(first, first), |r, p| r.union_pt(*p)))
    };
    let polys = match req.mode {
        FrameMode::Bounds if req.individually => objects.iter().filter_map(|o| bounds(o)).map(rect).collect(),
        FrameMode::Bounds => match objects.iter().filter_map(|o| bounds(o)).reduce(|a, b| a.union(b)) {
            Some(b) => vec![rect(b)],
            None => Vec::new(),
        },
        FrameMode::Hull => {
            let groups: Vec<Vec<&doc::Object>> =
                if req.individually { objects.iter().map(|o| vec![*o]).collect() } else { vec![objects.clone()] };
            groups
                .iter()
                .filter_map(|g| {
                    let pts: Vec<(f64, f64)> = g.iter().flat_map(|o| flattened(o)).flatten().collect();
                    let mut h = convex_hull(pts);
                    let first = *h.first()?;
                    h.push(first);
                    Some(h)
                })
                .collect()
        }
        FrameMode::Outline => {
            let mut polys: Vec<Vec<(f64, f64)>> = objects.iter().flat_map(|o| flattened(o)).collect();
            polys.retain(|p| p.len() >= 2);
            if req.outside_only {
                polys = outside_only(polys);
            }
            polys
        }
    };
    let polys: Vec<Vec<(f64, f64)>> = polys.into_iter().filter(|p| !p.is_empty()).collect();
    let points: usize = polys.iter().map(Vec::len).sum();
    if points == 0 {
        return Err(EngineError::BadRequest("nothing to frame: the objects have no outline".into()));
    }
    if points > MAX_OUTLINE_POINTS {
        return Err(EngineError::BadRequest(format!(
            "outline has {points} points (max {MAX_OUTLINE_POINTS}); frame with mode \"bounds\" or \"hull\" instead"
        )));
    }
    Ok(polys)
}

/// A red-dot jog: the dot held at `(x, y)` (field mm).
pub fn point_outline(x: f64, y: f64) -> Vec<Vec<(f64, f64)>> {
    vec![vec![(x, y)]]
}

/// What framing traces for an object: its outline, or with `image_bounds` an image's content outline (when it has
/// any content; else its rectangle).
fn frame_polys(o: &doc::Object, image_bounds: bool) -> Vec<Vec<(f64, f64)>> {
    if image_bounds
        && let Some(hull) = image_content_outline(o)
    {
        return vec![hull];
    }
    flattened(o)
}

/// Largest side an image is reduced to before its content is measured.
const IMAGE_BOUNDS_PX: u32 = 512;

/// The convex hull (closed, field mm) of an image's "ink": pixels that are not transparent (alpha above 16) and not
/// white (luma below 245), as LightBurn estimates Image Bounds from transparency and whitespace. `None` for a
/// non-image, an image that cannot be decoded, or one with no ink.
pub fn image_content_outline(o: &doc::Object) -> Option<Vec<(f64, f64)>> {
    use base64::Engine as _;
    let doc::Geometry::Image { width_mm, height_mm, data_base64, .. } = &o.geometry else { return None };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data_base64.as_bytes()).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width().max(img.height()) > IMAGE_BOUNDS_PX { img.thumbnail(IMAGE_BOUNDS_PX, IMAGE_BOUNDS_PX) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w == 0 || h == 0 {
        return None;
    }
    let (sx, sy) = (width_mm / f64::from(w), height_mm / f64::from(h));
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for (x, y, p) in rgba.enumerate_pixels() {
        let [r, g, b, a] = p.0;
        let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
        if a <= 16 || luma >= 245.0 {
            continue;
        }
        // Pixel corners in the image's local frame: centred, +Y up (row 0 at the top).
        let (x0, x1) = (f64::from(x) * sx - width_mm / 2.0, f64::from(x + 1) * sx - width_mm / 2.0);
        let (y0, y1) = (height_mm / 2.0 - f64::from(y) * sy, height_mm / 2.0 - f64::from(y + 1) * sy);
        for (lx, ly) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
            let q = o.transform * kurbo::Point::new(lx, ly);
            pts.push((q.x, q.y));
        }
    }
    let mut hull = convex_hull(pts);
    let first = *hull.first()?;
    hull.push(first);
    Some(hull)
}

/// An object's outline flattened to polylines (closed subpaths end where they start).
fn flattened(o: &doc::Object) -> Vec<Vec<(f64, f64)>> {
    let mut polys: Vec<Vec<(f64, f64)>> = Vec::new();
    kurbo::flatten(o.field_path(), OUTLINE_TOLERANCE_MM, |el| match el {
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
    polys
}

/// Andrew's monotone chain: the convex hull, counter-clockwise, without the closing point.
pub fn convex_hull(mut pts: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    pts.retain(|p| p.0.is_finite() && p.1.is_finite());
    pts.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: (f64, f64), a: (f64, f64), b: (f64, f64)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut hull: Vec<(f64, f64)> = Vec::with_capacity(pts.len() * 2);
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &(f64, f64)>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Keep the polylines that no other closed polyline encloses (LightBurn's Outside Shapes Only).
fn outside_only(polys: Vec<Vec<(f64, f64)>>) -> Vec<Vec<(f64, f64)>> {
    let closed = |p: &Vec<(f64, f64)>| p.len() > 2 && p.first() == p.last();
    let inside = |pt: (f64, f64), ring: &[(f64, f64)]| {
        let mut c = false;
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (ring[i], ring[(i + n - 1) % n]);
            if (a.1 > pt.1) != (b.1 > pt.1) && pt.0 < (b.0 - a.0) * (pt.1 - a.1) / (b.1 - a.1) + a.0 {
                c = !c;
            }
        }
        c
    };
    polys
        .iter()
        .enumerate()
        .filter(|(i, p)| {
            let Some(&probe) = p.first() else { return false };
            !polys.iter().enumerate().any(|(j, q)| j != *i && closed(q) && inside(probe, q))
        })
        .map(|(_, p)| p.clone())
        .collect()
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

/// Start tracing `outline`, replacing any frame already running. Returns once the first trace has been sent (so
/// out-of-field outlines are reported to the caller); the loop continues in the background.
pub async fn start(
    machine: &Arc<Machine>,
    events: &broadcast::Sender<Event>,
    outline: Vec<Vec<(f64, f64)>>,
    run: FrameRun,
) -> Result<Value, EngineError> {
    if machine.is_running() {
        return Err(EngineError::Conflict("a run is in progress; framing is not available".into()));
    }
    let generation = machine.bump_frame_gen();
    let speed = run.style.speed_mm_s.unwrap_or(machine.profile().red_light_speed_mm_s);
    let (loops, call_time) = if run.continuous { loops_per_call(&outline, speed) } else { (1, single_time(&outline, speed)) };
    let outline = Arc::new(outline);
    let points: usize = outline.iter().map(Vec::len).sum();
    let first = frame_once(machine, generation, outline.clone(), loops, run.style, true).await;
    match first {
        Ok(Some(packets)) => {
            machine.set_framing_gen(generation);
            let _ = events.send(Event::machine("framing", None));
            let m = machine.clone();
            let ev = events.clone();
            let max_seconds = run.max_secs;
            let continuous = run.continuous;
            let live = run.live.is_some();
            tokio::spawn(async move { frame_loop(m, ev, generation, outline, (loops, call_time), run).await });
            Ok(json!({ "ok": true, "framing": true, "points": points, "loops_per_call": loops, "packets": packets,
                "max_seconds": max_seconds, "continuous": continuous, "live": live, "frame_gen": generation }))
        }
        Ok(None) => Err(EngineError::Conflict("framing was cancelled before it started".into())),
        Err(e) => {
            machine.set_error(Some(e.to_string()));
            let _ = events.send(Event::machine("error", Some(e.to_string())));
            Err(e)
        }
    }
}

/// Time of one trace of `outline` at `speed` (at least 5 ms).
fn single_time(outline: &[Vec<(f64, f64)>], speed: f64) -> Duration {
    let speed = if speed.is_finite() && speed > 0.0 { speed } else { 1000.0 };
    Duration::from_secs_f64((loop_length_mm(outline) / speed).max(0.005))
}

/// One `Controller::frame_styled` call, unless the loop `generation` has been superseded or a run holds the machine.
/// `first`: the loop's first call, which discards the input reads earlier frames took while they streamed (with the
/// controller held, so none of theirs is left): a press counts only for the frame that was showing.
async fn frame_once(
    machine: &Arc<Machine>,
    generation: u64,
    outline: Arc<Vec<Vec<(f64, f64)>>>,
    loops: u32,
    style: FrameStyle,
    first: bool,
) -> Result<Option<usize>, EngineError> {
    machine
        .blocking(move |m, ctl| {
            if m.frame_gen() != generation || m.is_running() {
                return Ok(None);
            }
            if first {
                m.clear_input_samples();
            }
            m.ensure_ready(ctl)?;
            Ok(Some(ctl.frame_styled(&outline, loops, &style)?))
        })
        .await?
}

async fn frame_loop(
    machine: Arc<Machine>,
    events: broadcast::Sender<Event>,
    generation: u64,
    mut outline: Arc<Vec<Vec<(f64, f64)>>>,
    (mut loops, mut call_time): (u32, Duration),
    run: FrameRun,
) {
    let deadline = Instant::now() + Duration::from_secs(run.max_secs);
    let current = |m: &Machine| m.frame_gen() == generation;
    let mut live = run.live.clone();
    let speed = run.style.speed_mm_s.unwrap_or(machine.profile().red_light_speed_mm_s);
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
        if Instant::now() >= deadline || !run.continuous {
            break;
        }
        if let Some((revision, source)) = live.as_mut()
            && let Some((new_revision, new_outline)) = source(*revision)
        {
            *revision = new_revision;
            (loops, call_time) = loops_per_call(&new_outline, speed);
            outline = Arc::new(new_outline);
            let _ = events.send(Event::machine("frame_updated", None));
        }
        match frame_once(&machine, generation, outline.clone(), loops, run.style, false).await {
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

/// End framing: stop the loop and turn the pointer off (`Controller::stop_frame`). Returns whether a board was
/// connected: with no board (never initialised and the link disconnected, as `Machine::abort` treats it) there is
/// nothing to turn off, and that is not an error.
pub async fn stop(machine: &Arc<Machine>, events: &broadcast::Sender<Event>) -> Result<bool, EngineError> {
    let was = machine.is_framing();
    machine.bump_frame_gen();
    let mut connected = true;
    if !machine.is_running() {
        connected = machine
            .blocking(|_, ctl| {
                let initialised = ctl.state() != ControllerState::Uninitialized;
                match ctl.stop_frame() {
                    Ok(()) => Ok(true),
                    Err(ControllerError::Link(LinkError::Disconnected | LinkError::NotFound(_))) if !initialised => {
                        Ok(false)
                    }
                    Err(e) => Err(e),
                }
            })
            .await??;
    }
    if was {
        let _ = events.send(Event::machine("frame_stopped", None));
    }
    Ok(connected)
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
            group: None,
            extras: Default::default(),
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
        let req = FrameRequest { ids: Some(vec!["obj-9".into()]), mode: FrameMode::Outline, ..FrameRequest::default() };
        let o = outline_for(&doc_with_rect(0.0), &req).unwrap();
        assert!(o[0].len() >= 5);
    }

    #[test]
    fn empty_and_unknown_selections_are_refused() {
        assert!(matches!(outline_for(&Document::default(), &FrameRequest::default()), Err(EngineError::BadRequest(_))));
        let req = FrameRequest { ids: Some(vec!["obj-404".into()]), mode: FrameMode::Bounds, ..FrameRequest::default() };
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

    fn two_rects() -> Document {
        let mut d = doc_with_rect(0.0);
        let mut o = d.objects[0].clone();
        o.id = "obj-10".into();
        o.transform = Affine::translate((30.0, 10.0));
        d.objects.push(o);
        d
    }

    #[test]
    fn hull_individually_and_tool_layers() {
        let d = two_rects();
        let hull = outline_for(&d, &FrameRequest { mode: FrameMode::Hull, ..FrameRequest::default() }).unwrap();
        assert_eq!(hull.len(), 1);
        assert_eq!(hull[0].first(), hull[0].last(), "closed");
        assert_eq!(hull[0].len(), 7, "a hexagon around two offset rectangles");
        let each = outline_for(&d, &FrameRequest { individually: true, ..FrameRequest::default() }).unwrap();
        assert_eq!(each.len(), 2);
        let each_hull = outline_for(&d, &FrameRequest { mode: FrameMode::Hull, individually: true, ..FrameRequest::default() }).unwrap();
        assert_eq!(each_hull.len(), 2);
        assert!(outline_for(&d, &FrameRequest { tool_layers_only: true, ..FrameRequest::default() }).is_err());
        let mut jig = d.clone();
        let mut tool = doc::Layer::new("layer-9".into(), "T1", "#888");
        tool.output = false;
        jig.layers.push(tool);
        jig.objects[1].layer = "layer-9".into();
        let t = outline_for(&jig, &FrameRequest { tool_layers_only: true, ..FrameRequest::default() }).unwrap();
        assert!(t[0].iter().all(|p| p.0 >= 25.0), "only the jig outline");
    }

    #[test]
    fn outside_only_skips_counters() {
        let mut d = doc_with_rect(0.0);
        let mut inner = d.objects[0].clone();
        inner.id = "obj-10".into();
        inner.geometry = Geometry::Rect { width_mm: 2.0, height_mm: 1.0, corner_radius_mm: 0.0 };
        d.objects.push(inner);
        let all = outline_for(&d, &FrameRequest { mode: FrameMode::Outline, ..FrameRequest::default() }).unwrap();
        let outer = outline_for(&d, &FrameRequest { mode: FrameMode::Outline, outside_only: true, ..FrameRequest::default() }).unwrap();
        assert_eq!((all.len(), outer.len()), (2, 1));
    }

    #[test]
    fn hull_of_a_square_and_the_contour_alias() {
        let h = convex_hull(vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.5, 0.5)]);
        assert_eq!(h.len(), 4);
        let r: FrameRequest = serde_json::from_value(serde_json::json!({"mode": "contour"})).unwrap();
        assert_eq!(r.mode, FrameMode::Outline);
        assert!(serde_json::from_value::<FrameRequest>(serde_json::json!({"mode": "hull", "pointer": "both", "live": true})).is_ok());
    }
}
