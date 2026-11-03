//! Pause at a layer (short list #11): a layer (or sub-layer) whose cut options have `operator_pause` stops the run
//! before its first pass until the operator presses **Continue** in the app (`POST /api/job/continue`, UI only, like
//! Resume) or the **foot switch** (the profile's Start Marking input, released then pressed). Useful between a
//! colour fill and a frost pass to clean the part, flip it or change the jig.
//!
//! **How the board is kept safe (LMC).** The job is planned as usual, then encoded as stages: the passes before the
//! first paused layer, the passes from it to the next, and so on ([`encode_stages`]); each stage is one or more lists
//! (`RunPart`s) and the run records where each stage starts ([`LayerStop`]). `approve` runs every stage as a normal
//! `Controller::run_parts`, each with its own arming token: the stage ends like any job (the list finishes, the MO is
//! closed, the port goes back to the idle word, so the laser enable is off; the Done Marking pulse fires), and the
//! next stage starts like any job (`ResetList`, the laser-enable word, MO open, the open-MO delay in the list). In
//! between nothing streams and nothing can emit; the galvo rests where the stage ended and the red pointer is off. The
//! run still holds the machine (no other run, frame or foot switch cycle starts), and every gate of a run keeps
//! applying: Stop ends it at once (`aborted`, the abort sequence is sent as for a running job), a Pause holds the next
//! stage until Resume, the door is read again before the next stage streams, and a shutdown ends the run at the pause
//! instead of waiting for the operator. A pause waits at most [`MAX_WAIT`]. Not verified on the B6 yet: tested on the
//! simulator.
//!
//! Galvo jobs only: a rotary job (its strips cross every layer) and GRBL or Ruida jobs plan the layer without the
//! pause (their run summary says so). A pause before the job's first pass is ignored (Start already waits for the
//! operator). The job's time estimate and progress do not count the time spent waiting; the progress follows the clock
//! for a staged run.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use doc::Document;
use machine::DriverJob;
use plan::{EncodedPart, JobPlan, PlanError, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::notify::{Kind, Operation};
use super::now_unix;
use crate::libs::lock;
use crate::pin::{CONFIRM_POLL, CONFIRM_READS};
use crate::{AppState, EngineError, Event};

/// Longest a run waits at one layer for the operator.
pub const MAX_WAIT: Duration = Duration::from_secs(3600);

/// Where a staged run stops for the operator: before RunPart `part` (the first list of the stage), for layer `layer`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerStop {
    pub part: usize,
    pub layer_id: String,
    /// The layer's name (and the sub-layer's, when the pause is a sub-layer's).
    pub layer: String,
}

/// Whether the settings of pass `(layer, sub_layer)` ask for a pause.
fn pauses(doc: &Document, layer_id: &str, sub_layer: usize) -> Option<String> {
    let layer = doc.layers.iter().find(|l| l.id == layer_id)?;
    let settings = if sub_layer == 0 { &layer.settings } else { layer.sub_layers.get(sub_layer - 1)? };
    settings.options.operator_pause.then(|| match (sub_layer, &settings.name) {
        (0, _) => layer.name.clone(),
        (_, Some(n)) if !n.trim().is_empty() => format!("{} / {n}", layer.name),
        (n, _) => format!("{} / sub-layer {n}", layer.name),
    })
}

/// The pass indices where a stage starts and the layers they stop for: the first pass of every layer (and of every
/// sub-layer) that asks for a pause, except the job's first pass.
pub fn stops(doc: &Document, job: &JobPlan) -> Vec<(usize, String, String)> {
    let mut seen_layers = BTreeSet::new();
    let mut seen_subs = BTreeSet::new();
    let mut out = Vec::new();
    for (i, pass) in job.passes.iter().enumerate() {
        let first_of_layer = seen_layers.insert(pass.layer_id.clone());
        let first_of_sub = seen_subs.insert((pass.layer_id.clone(), pass.sub_layer));
        if i == 0 {
            continue;
        }
        let name = if first_of_layer {
            pauses(doc, &pass.layer_id, 0).or_else(|| first_of_sub.then(|| pauses(doc, &pass.layer_id, pass.sub_layer)).flatten())
        } else if first_of_sub && pass.sub_layer > 0 {
            pauses(doc, &pass.layer_id, pass.sub_layer)
        } else {
            None
        };
        if let Some(name) = name {
            out.push((i, pass.layer_id.clone(), name));
        }
    }
    out
}

/// Whether some output layer of `doc` asks for a pause (to say so where pauses are not available).
pub fn any_pause(doc: &Document) -> bool {
    doc.layers.iter().any(|l| l.output && (l.settings.options.operator_pause || l.sub_layers.iter().any(|s| s.options.operator_pause)))
}

/// A job encoded in stages: its lists in order, and where each stage after the first starts.
pub type Stages = (Vec<EncodedPart>, Vec<LayerStop>);

/// Encode `job` in stages split at the paused layers: the parts of every stage in order, and where each stage after
/// the first starts. `None` when no pause applies (or the job is a rotary job, which cannot be split by layer): encode
/// it as usual then.
pub fn encode_stages(doc: &Document, job: &JobPlan, target: &Target) -> Result<Option<Stages>, PlanError> {
    let stops = stops(doc, job);
    if stops.is_empty() || job.parts.iter().any(|p| p.rotary_steps.is_some()) {
        return Ok(None);
    }
    let mut bounds: Vec<usize> = vec![0];
    bounds.extend(stops.iter().map(|s| s.0));
    bounds.push(job.passes.len());
    let mut parts = Vec::new();
    let mut out_stops = Vec::new();
    for (k, w) in bounds.windows(2).enumerate() {
        let stage = JobPlan { passes: job.passes[w[0]..w[1]].to_vec(), parts: job.parts.clone(), ..JobPlan::default() };
        let encoded = match plan::encode_parts(&stage, target) {
            Ok(e) => e,
            // A stage with nothing to mark (its passes are all empty): no stop of its own.
            Err(PlanError::Empty) => continue,
            Err(e) => return Err(e),
        };
        if k > 0 && !parts.is_empty() {
            let (_, layer_id, layer) = &stops[k - 1];
            out_stops.push(LayerStop { part: parts.len(), layer_id: layer_id.clone(), layer: layer.clone() });
        }
        parts.extend(encoded);
    }
    if parts.is_empty() {
        return Err(PlanError::Empty);
    }
    Ok(Some((parts, out_stops)))
}

/// The stages of a run's job: `(the pause before it, its lists)`, the first without a pause. One stage for a job
/// without pauses (and for GRBL and Ruida jobs, which have none).
pub(crate) fn stages_of(job: &DriverJob, pauses: &[LayerStop]) -> Vec<(Option<LayerStop>, DriverJob)> {
    let DriverJob::Lmc(parts) = job else { return vec![(None, job.clone())] };
    if pauses.is_empty() {
        return vec![(None, job.clone())];
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut before: Option<LayerStop> = None;
    for p in pauses {
        if p.part <= start || p.part >= parts.len() {
            continue;
        }
        out.push((before.take(), DriverJob::Lmc(Arc::new(parts[start..p.part].to_vec()))));
        start = p.part;
        before = Some(p.clone());
    }
    out.push((before, DriverJob::Lmc(Arc::new(parts[start..].to_vec()))));
    out
}

/// A run waiting at a layer.
#[derive(Debug, Clone, Serialize)]
pub struct Waiting {
    pub run_id: String,
    pub layer: String,
    pub since_unix_s: u64,
    /// The foot switch continues too.
    pub foot_switch: bool,
}

/// The pause state (one field of `Production`).
#[derive(Debug, Default)]
pub struct LayerPause {
    waiting: Mutex<Option<Waiting>>,
    continue_requested: AtomicBool,
}

impl LayerPause {
    pub fn waiting(&self) -> Option<Waiting> {
        lock(&self.waiting).clone()
    }

    pub fn status_json(&self) -> Value {
        match self.waiting() {
            Some(w) => json!({ "waiting": true, "run_id": w.run_id, "layer": w.layer, "since_unix_s": w.since_unix_s, "foot_switch": w.foot_switch }),
            None => json!({ "waiting": false, "run_id": null, "layer": null, "since_unix_s": null, "foot_switch": false }),
        }
    }

    /// Continue the run waiting at a layer (`run_id`: only that run, when given). Refused when none waits.
    pub fn request_continue(&self, run_id: Option<&str>) -> Result<String, EngineError> {
        let w = self.waiting().ok_or_else(|| EngineError::Conflict("no run is waiting at a layer".into()))?;
        if run_id.is_some_and(|id| id != w.run_id) {
            return Err(EngineError::Conflict(format!("run '{}' is waiting, not '{}'", w.run_id, run_id.unwrap_or_default())));
        }
        self.continue_requested.store(true, Ordering::SeqCst);
        Ok(w.run_id)
    }
}

/// How a wait at a layer ended.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum WaitEnd {
    Continue,
    Stopped,
    /// Not continued: the reason (the engine shuts down, the wait timed out).
    Ended(String),
    Failed(String),
}

/// Wait at `stop` until Continue, a foot switch press, a stop (the stop epoch changed), a shutdown or [`MAX_WAIT`].
pub(crate) async fn wait(st: &Arc<AppState>, run_id: &str, epoch: u64, stop: &LayerStop) -> WaitEnd {
    let profile = st.machine.profile();
    let foot = (!st.machine.is_gantry()).then_some(profile.start_mark_input_bit).flatten().map(|bit| machine::InputPort {
        bit,
        active_low: profile.start_mark_active_low,
    });
    let pause = &st.production.pause;
    pause.continue_requested.store(false, Ordering::SeqCst);
    *lock(&pause.waiting) = Some(Waiting {
        run_id: run_id.to_owned(),
        layer: stop.layer.clone(),
        since_unix_s: now_unix(),
        foot_switch: foot.is_some(),
    });
    st.job.layer_wait(Some(&stop.layer));
    st.emit(Event::machine("layer_pause", Some(stop.layer.clone())));
    super::changed(st, "layer_pause");
    let how = if foot.is_some() { "press Continue in the app or the foot switch" } else { "press Continue in the app" };
    st.production.notify.emit(
        st,
        Kind::LayerPause,
        Operation::Mark,
        &format!("Paused before layer \"{}\": {how}.", stop.layer),
        Some(run_id),
    );
    let end = wait_loop(st, epoch, foot).await;
    *lock(&pause.waiting) = None;
    pause.continue_requested.store(false, Ordering::SeqCst);
    st.job.layer_wait(None);
    st.emit(Event::machine("layer_continue", Some(stop.layer.clone())));
    super::changed(st, "layer_pause");
    end
}

async fn wait_loop(st: &Arc<AppState>, epoch: u64, foot: Option<machine::InputPort>) -> WaitEnd {
    let deadline = Instant::now() + MAX_WAIT;
    let pause = &st.production.pause;
    let (mut released, mut pressed) = (false, 0u32);
    loop {
        if st.machine.stop_epoch() != epoch {
            return WaitEnd::Stopped;
        }
        if pause.continue_requested.swap(false, Ordering::SeqCst) {
            return WaitEnd::Continue;
        }
        if st.shutting_down() {
            return WaitEnd::Ended("ended at the layer pause: the engine is shutting down".into());
        }
        if Instant::now() >= deadline {
            return WaitEnd::Ended(format!("nobody continued within {} min of the layer pause", MAX_WAIT.as_secs() / 60));
        }
        if let Some(port) = foot {
            let read = st
                .machine
                .blocking(move |m, ctl| {
                    if m.stop_epoch() != epoch {
                        return Ok(None);
                    }
                    m.ensure_ready(ctl)?;
                    m.read_inputs(ctl).map(Some)
                })
                .await;
            match read {
                Ok(Ok(Some(bits))) if !port.active(bits) => (released, pressed) = (true, 0),
                Ok(Ok(Some(_))) if released => {
                    pressed += 1;
                    if pressed >= CONFIRM_READS {
                        return WaitEnd::Continue;
                    }
                }
                Ok(Ok(Some(_))) => {}
                Ok(Ok(None)) => return WaitEnd::Stopped,
                Ok(Err(e)) => return WaitEnd::Failed(e.to_string()),
                Err(e) => return WaitEnd::Failed(e.to_string()),
            }
        }
        tokio::time::sleep(CONFIRM_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{CutSettings, Layer};

    fn layer(id: &str, pause: bool) -> Layer {
        let mut settings = CutSettings::default();
        settings.options.operator_pause = pause;
        Layer {
            id: id.into(),
            name: format!("L {id}"),
            color: "#ff0000".into(),
            visible: true,
            output: true,
            settings,
            global_passes: 1,
            sub_layers: vec![],
            extras: Default::default(),
            link: None,
            tool: None,
        }
    }

    fn pass(layer: &str, sub: usize) -> plan::PassPlan {
        let settings = json!({
            "power_pct": 10.0, "speed_mm_s": 100.0, "frequency_khz": 30.0, "pulse_width_ns": null,
            "timing": {"laser_on_delay_us": 0.0, "laser_off_delay_us": 0.0, "polygon_delay_us": 0.0, "end_delay_us": 0.0,
                       "jump_speed_mm_s": 1000.0, "min_jump_delay_us": 0.0, "max_jump_delay_us": 0.0, "jump_distance_limit_mm": 0.0},
            "hatch_angle_deg": 0.0,
        });
        serde_json::from_value(json!({
            "layer_id": layer, "sub_layer": sub, "repeat": 0, "polylines": [[[0.0, 0.0], [1.0, 0.0]]], "settings": settings,
        }))
        .unwrap()
    }

    #[test]
    fn stages_split_the_lists_at_the_pauses() {
        let parts: Vec<machine::RunPart> = (0..4).map(|_| machine::RunPart::list(vec![lmc::Packet::empty()])).collect();
        let job = DriverJob::Lmc(Arc::new(parts));
        let stop = |part: usize| LayerStop { part, layer_id: "l".into(), layer: format!("L{part}") };
        let stages = stages_of(&job, &[stop(1), stop(3)]);
        let lens: Vec<usize> = stages.iter().map(|(_, j)| if let DriverJob::Lmc(p) = j { p.len() } else { 0 }).collect();
        assert_eq!(lens, [1, 2, 1]);
        assert_eq!(stages.iter().map(|(s, _)| s.as_ref().map(|s| s.part)).collect::<Vec<_>>(), [None, Some(1), Some(3)]);
        assert_eq!(stages_of(&job, &[]).len(), 1);
        assert_eq!(stages_of(&job, &[stop(0), stop(9)]).len(), 1, "a pause at the start or past the end splits nothing");
    }

    #[test]
    fn stops_at_the_first_pass_of_paused_layers_only() {
        let mut doc = Document { layers: vec![layer("a", true), layer("b", true), layer("c", false)], ..Document::default() };
        let mut sub = CutSettings::default();
        sub.options.operator_pause = true;
        sub.name = Some("Frost".into());
        doc.layers[2].sub_layers.push(sub);
        let job = JobPlan { passes: vec![pass("a", 0), pass("a", 0), pass("b", 0), pass("b", 0), pass("c", 0), pass("c", 1), pass("c", 1)], ..JobPlan::default() };
        let s = stops(&doc, &job);
        assert_eq!(
            s,
            [(2, "b".to_owned(), "L b".to_owned()), (5, "c".to_owned(), "L c / Frost".to_owned())],
            "never before the job's first pass; once per layer or sub-layer"
        );
        assert!(any_pause(&doc));
    }
}
