//! Pending runs. `POST /api/runs` and the MCP tool `job_request_run` plan and encode the current document into a
//! [`Run`] that waits here; only the UI approval endpoint (module `approve`) can start one.
//!
//! A run carries its [`RunOptions`] (LightBurn's Laser window: repeat N times, run continuously, wait for the start
//! input, cut selected graphics, start here). One approval covers every iteration the options ask for; stop ends
//! them all.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use commands::Origin;
use doc::Document;
use machine::grbl::{GcodeProgram, GrblProfile};
use machine::{AxisRole, DriverJob, MachineProfile, RunPart, RunReport};
use plan::{JobOptions, Stats, Target};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::EngineError;

/// Most runs that may wait for approval at once.
pub const MAX_PENDING: usize = 8;
/// Finished runs kept for the status page.
pub const KEEP_FINISHED: usize = 20;
/// Most iterations `repeat` may ask for.
pub const MAX_REPEAT: u32 = 10_000;

/// How a run is run (LightBurn's Laser window and Cut Selected Graphics / Start Here). All optional; the default is
/// one run of the whole document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunOptions {
    /// Run the job this many times in a row (1..=10000; LightBurn's Repeat). One approval covers them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// Run again after every run until stopped, until the job counter reaches Stop At, or until "end after the
    /// current run" (LightBurn's Run Continuously). One approval.
    #[serde(default)]
    pub continuous: bool,
    /// Wait for the profile's start-marking input (a foot pedal) before every run (LightBurn's Start Marking
    /// input).
    #[serde(default)]
    pub start_input: bool,
    /// With `start_input`: the input must be released and then pressed (a full pulse, the default) rather than
    /// merely seen pressed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_pulse: Option<bool>,
    /// Cut Selected Graphics: only these object or group ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<Vec<String>>,
    /// Start Here: begin at this preview segment (`GET /api/preview` `segments` index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_segment: Option<usize>,
    /// Batch: run these saved projects one after another (instead of the open document), in this order, as one job
    /// with one approval (each project planned on its own; at most 50). Not combined with selection or from_segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<String>>,
    /// Repeat Marking with an external axis table (LightBurn's Repeat Marking): after each iteration the profile's
    /// table axis (role linear_table or rotary_table) moves on by `increment` and waits `settle_ms`; give `repeat`
    /// for the count. Unverified on hardware.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableRepeat>,
    /// Re-mark a region: clip the planned toolpath (not the design) to this polygon, field mm, at least 3 points
    /// (module `zones`). Combines with selection (Cut Selected Graphics); not with from_segment or queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Vec<[f64; 2]>>,
    /// The active device's keep-out zones, filled in by the engine when it plans the run (never from a request).
    #[serde(skip)]
    pub device_keep_out: Vec<crate::zones::DeviceZone>,
}

/// Most projects one queued run may hold.
pub const MAX_QUEUE: usize = 50;

/// Repeat Marking's table step (see [`RunOptions::table`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableRepeat {
    /// Table movement between repeats: mm for a linear table, degrees for a rotating one (> 0).
    pub increment: f64,
    /// Wait after each move before marking, ms (LightBurn's Pause After Move; at most 60000).
    #[serde(default)]
    pub settle_ms: u32,
    /// Move the table back to its start (position 0) when the job ends (LightBurn's Return to Start Position).
    #[serde(default = "yes")]
    pub return_to_start: bool,
}

fn yes() -> bool {
    true
}

impl RunOptions {
    /// How many iterations: `None` for continuous.
    pub fn iterations(&self) -> Option<u32> {
        if self.continuous { None } else { Some(self.repeat.unwrap_or(1)) }
    }

    /// Refuse impossible combinations.
    pub fn check(&self) -> Result<(), EngineError> {
        if let Some(n) = self.repeat
            && !(1..=MAX_REPEAT).contains(&n)
        {
            return Err(EngineError::BadRequest(format!("repeat must be within 1..={MAX_REPEAT} (got {n})")));
        }
        if self.continuous && self.repeat.is_some_and(|n| n != 1) {
            return Err(EngineError::BadRequest("give repeat or continuous, not both".into()));
        }
        if self.full_pulse.is_some() && !self.start_input {
            return Err(EngineError::BadRequest("full_pulse needs start_input".into()));
        }
        if self.selection.as_ref().is_some_and(Vec::is_empty) {
            return Err(EngineError::BadRequest("selection is empty".into()));
        }
        // Segment numbers come from `GET /api/preview`, which plans the whole document: with a selection they
        // would point into a different job.
        if self.selection.is_some() && self.from_segment.is_some() {
            return Err(EngineError::BadRequest("give selection or from_segment, not both".into()));
        }
        if let Some(q) = &self.queue {
            if q.is_empty() || q.len() > MAX_QUEUE {
                return Err(EngineError::BadRequest(format!("queue must list 1..={MAX_QUEUE} projects")));
            }
            if self.selection.is_some() || self.from_segment.is_some() {
                return Err(EngineError::BadRequest("a queue runs whole projects: no selection or from_segment".into()));
            }
        }
        if let Some(r) = &self.region {
            if let Some(p) = crate::zones::polygon_problem(r) {
                return Err(EngineError::BadRequest(format!("region {p}")));
            }
            if self.from_segment.is_some() || self.queue.is_some() {
                return Err(EngineError::BadRequest("region re-marks part of the open document: not with from_segment or queue".into()));
            }
        }
        if let Some(t) = &self.table {
            if !(t.increment.is_finite() && t.increment > 0.0) {
                return Err(EngineError::BadRequest(format!("table.increment must be > 0 (got {})", t.increment)));
            }
            if t.settle_ms > 60_000 {
                return Err(EngineError::BadRequest("table.settle_ms must be at most 60000".into()));
            }
        }
        Ok(())
    }
}

/// Where a run is in its life: `pending` -> `approved` -> (`awaiting_confirm` ->) `running` -> `finished` |
/// `failed` | `aborted` (stopped while streaming), or `cancelled` (stopped or refused before streaming).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Waiting for a person to approve it (the operator PIN in the UI, or Start in the app without a PIN).
    Pending,
    /// Approved; about to start (or about to wait for the physical input).
    Approved,
    /// Approved; waiting for the physical confirmation input (see `pin::PhysicalConfirm`).
    AwaitingConfirm,
    /// Being streamed to the board.
    Running,
    /// Ran to the end.
    Finished,
    /// The board or link failed; the abort sequence was sent.
    Failed,
    /// Stopped by a stop request while `running` (streaming); the abort sequence was sent.
    Aborted,
    /// Never streamed: stop pressed while pending, approved or awaiting confirmation, the document changed, or the
    /// physical input was not pressed in time.
    Cancelled,
}

impl RunState {
    /// True once the run can no longer change.
    pub fn is_final(self) -> bool {
        matches!(self, RunState::Finished | RunState::Failed | RunState::Aborted | RunState::Cancelled)
    }
}

/// A planned, encoded job.
#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub state: RunState,
    pub origin: Origin,
    /// Document revision the packets were made from. Approval is refused once the document has moved on.
    pub revision: u64,
    /// The open document the packets were made from (module `documents`). Approval is refused while another one is
    /// active, also should its revision ever match.
    pub document_id: String,
    pub summary: String,
    pub stats: Stats,
    /// The job as lists, each after an optional extension-axis move (one part for a plain job). Empty for a GRBL
    /// job.
    pub parts: Arc<Vec<RunPart>>,
    /// The job as G-code, for a GRBL gantry (`parts` is then empty).
    pub gcode: Option<Arc<GcodeProgram>>,
    /// The job for a Ruida gantry (`parts` is then empty): a program planned from the document, or a file stored on
    /// the controller (`DriverJob::Ruida` / `DriverJob::RuidaFile`, module `ruida_dsp`).
    pub ruida: Option<DriverJob>,
    pub created_unix_s: u64,
    pub error: Option<String>,
    pub report: Option<RunReport>,
    /// The machine configuration generation the packets were made for (`hw::MachineConfig::generation`); a run of
    /// another generation (planned for another device) is never approved.
    pub generation: u64,
    pub options: RunOptions,
    /// Iterations that ran to the end.
    pub completed: u32,
    /// The learned correction applied to `stats.estimated_time_s` ([`PreparedJob::estimate_correction`]).
    pub estimate_correction: f64,
    /// Where the run waits for the operator before a layer (`production::layer_pause`).
    pub pauses: Vec<crate::production::layer_pause::LayerStop>,
}

impl Run {
    /// List packets over all parts (G-code lines for a GRBL job).
    pub fn packet_count(&self) -> usize {
        let ruida = match &self.ruida {
            Some(DriverJob::Ruida(p)) => p.datagram_count(),
            Some(_) => 1,
            None => 0,
        };
        self.parts.iter().map(|p| p.packets.len()).sum::<usize>() + self.gcode.as_ref().map_or(0, |g| g.len()) + ruida
    }

    /// What the driver runs: the G-code of a GRBL job, else the LMC parts.
    pub fn driver_job(&self) -> DriverJob {
        if let Some(r) = &self.ruida {
            return r.clone();
        }
        match &self.gcode {
            Some(g) => DriverJob::Grbl(g.clone()),
            None => DriverJob::Lmc(self.parts.clone()),
        }
    }

    /// The `driver` of [`RunView`]: `lmc`, `grbl` or `ruida`.
    fn driver_name(&self) -> &'static str {
        match (&self.ruida, &self.gcode) {
            (Some(_), _) => "ruida",
            (None, Some(_)) => "grbl",
            (None, None) => "lmc",
        }
    }

    /// Drop the job's data (the run ended or was cancelled).
    fn drop_job(&mut self) {
        self.parts = Arc::new(Vec::new());
        self.gcode = None;
        self.ruida = None;
    }
}

/// The JSON view of a [`Run`] (no packets).
#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: String,
    pub state: RunState,
    pub origin: Origin,
    pub revision: u64,
    /// The open document the run was made from (`GET /api/documents`).
    pub document_id: String,
    /// True when the document changed after the run was requested; such a run cannot be approved.
    pub stale: bool,
    pub summary: String,
    pub stats: Stats,
    /// LMC list packets, or G-code lines for a GRBL job.
    pub packet_count: usize,
    /// `lmc`, `grbl` or `ruida`: the driver the run was planned for.
    pub driver: &'static str,
    pub created_unix_s: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<RunReport>,
    pub options: RunOptions,
    /// Lists the job is split into (rotary strips, Z positions; 1 for a plain job).
    pub parts: usize,
    /// Iterations that ran to the end.
    pub completed: u32,
    /// The layers the run waits at for the operator (Continue or the foot switch), in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layer_pauses: Vec<String>,
}

impl Run {
    pub fn view(&self, current_revision: u64) -> RunView {
        RunView {
            id: self.id.clone(),
            state: self.state,
            origin: self.origin.clone(),
            revision: self.revision,
            document_id: self.document_id.clone(),
            stale: self.state == RunState::Pending && self.revision != current_revision,
            summary: self.summary.clone(),
            stats: self.stats,
            packet_count: self.packet_count(),
            driver: self.driver_name(),
            created_unix_s: self.created_unix_s,
            error: self.error.clone(),
            report: self.report.clone(),
            options: self.options.clone(),
            parts: self.parts.len(),
            completed: self.completed,
            layer_pauses: self.pauses.iter().map(|p| p.layer.clone()).collect(),
        }
    }
}

/// Every run the engine knows about.
#[derive(Debug, Default)]
pub struct RunStore {
    runs: Vec<Run>,
    next: u64,
}

impl RunStore {
    /// Store a new pending run (of the `default` document until the caller sets [`Run::document_id`]). Refused when
    /// [`MAX_PENDING`] runs already wait.
    pub fn add(&mut self, origin: Origin, revision: u64, prepared: PreparedJob) -> Result<&mut Run, EngineError> {
        if self.runs.iter().filter(|r| r.state == RunState::Pending).count() >= MAX_PENDING {
            return Err(EngineError::Conflict(format!(
                "{MAX_PENDING} runs are already waiting for approval; approve or stop them first"
            )));
        }
        self.next += 1;
        let created_unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        self.runs.push(Run {
            id: format!("run-{}", self.next),
            state: RunState::Pending,
            origin,
            revision,
            document_id: crate::documents::DEFAULT_DOCUMENT.into(),
            summary: prepared.summary,
            stats: prepared.stats,
            parts: Arc::new(prepared.parts),
            gcode: prepared.gcode.map(Arc::new),
            ruida: prepared.ruida,
            created_unix_s,
            error: None,
            report: None,
            generation: prepared.generation,
            options: prepared.options,
            completed: 0,
            estimate_correction: prepared.estimate_correction,
            pauses: prepared.pauses,
        });
        self.prune();
        Ok(self.runs.last_mut().expect("just pushed"))
    }

    pub fn get(&self, id: &str) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Run> {
        self.runs.iter_mut().find(|r| r.id == id)
    }

    /// All runs, oldest first.
    pub fn views(&self, current_revision: u64) -> Vec<RunView> {
        self.runs.iter().map(|r| r.view(current_revision)).collect()
    }

    /// Runs waiting for approval, oldest first.
    pub fn pending(&self, current_revision: u64) -> Vec<RunView> {
        self.runs.iter().filter(|r| r.state == RunState::Pending).map(|r| r.view(current_revision)).collect()
    }

    /// Cancel every run that is not streaming yet (`pending`, `approved`, `awaiting_confirm`); returns how many. An
    /// approved run's task sees the stop epoch change and ends without sending anything.
    pub fn cancel_not_streaming(&mut self, reason: &str) -> usize {
        let mut n = 0;
        let waiting = |r: &&mut Run| matches!(r.state, RunState::Pending | RunState::Approved | RunState::AwaitingConfirm);
        for r in self.runs.iter_mut().filter(waiting) {
            r.state = RunState::Cancelled;
            r.error = Some(reason.to_owned());
            r.drop_job();
            n += 1;
        }
        n
    }

    /// Cancel every pending (not yet approved) run; returns how many.
    pub fn cancel_pending(&mut self, reason: &str) -> usize {
        let mut n = 0;
        for r in self.runs.iter_mut().filter(|r| r.state == RunState::Pending) {
            r.state = RunState::Cancelled;
            r.error = Some(reason.to_owned());
            r.drop_job();
            n += 1;
        }
        n
    }

    /// Forget a run that never left `pending` (or was cancelled before it was approved): one-click Start and the
    /// foot switch cycle plan a run and approve it at once, and a refused approval leaves nothing waiting.
    pub fn discard(&mut self, id: &str) {
        self.runs.retain(|r| r.id != id || !matches!(r.state, RunState::Pending | RunState::Cancelled));
    }

    /// The state of a run that holds or is about to hold the machine (`approved`, `awaiting_confirm`, `running`),
    /// if any, as its name.
    pub fn busy_state(&self) -> Option<&'static str> {
        self.runs.iter().find_map(|r| match r.state {
            RunState::Approved => Some("approved"),
            RunState::AwaitingConfirm => Some("waiting for its physical confirmation"),
            RunState::Running => Some("running"),
            _ => None,
        })
    }

    /// Drop the oldest finished runs beyond [`KEEP_FINISHED`] (and their packets).
    fn prune(&mut self) {
        let finished = |r: &Run| r.state.is_final();
        let mut excess = self.runs.iter().filter(|r| finished(r)).count().saturating_sub(KEEP_FINISHED);
        self.runs.retain(|r| {
            if excess > 0 && finished(r) {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Move an approved run to a non-final state (`awaiting_confirm`, `running`).
    pub fn set_state(&mut self, id: &str, state: RunState) {
        if let Some(r) = self.get_mut(id).filter(|r| !r.state.is_final()) {
            r.state = state;
        }
    }

    /// Record how an approved run ended (`state` is final).
    pub fn finish(&mut self, id: &str, state: RunState, report: Option<RunReport>, error: Option<String>) {
        debug_assert!(state.is_final());
        if let Some(r) = self.get_mut(id) {
            r.state = state;
            r.report = report;
            r.error = error;
            r.drop_job();
        }
        self.prune();
    }
}

/// A job ready to be stored as a run.
#[derive(Debug, Clone)]
pub struct PreparedJob {
    pub parts: Vec<RunPart>,
    /// The G-code of a GRBL job (`parts` is then empty).
    pub gcode: Option<GcodeProgram>,
    /// The job of a Ruida gantry (`parts` is then empty; module `ruida_dsp`).
    pub ruida: Option<DriverJob>,
    pub stats: Stats,
    pub summary: String,
    /// The machine configuration generation it was planned for (set by the caller; 0 from [`prepare_job`]).
    pub generation: u64,
    pub options: RunOptions,
    /// The factor `stats.estimated_time_s` was multiplied by (the device's learned correction, module `marktime`);
    /// 1 from [`prepare_job`]. The planner's own estimate is `stats.estimated_time_s / estimate_correction`.
    pub estimate_correction: f64,
    /// Where the run waits for the operator before a layer (`production::layer_pause`); empty for most jobs.
    pub pauses: Vec<crate::production::layer_pause::LayerStop>,
}

impl PreparedJob {
    /// Apply the device's learned estimate correction (module `marktime`) to the estimate and the summary. A factor
    /// that is not finite and positive is ignored.
    pub fn with_correction(mut self, factor: f64) -> Self {
        if factor.is_finite() && factor > 0.0 {
            let before = about(self.stats.estimated_time_s);
            self.stats.estimated_time_s *= factor;
            self.estimate_correction *= factor;
            self.summary = self.summary.replacen(&before, &about(self.stats.estimated_time_s), 1);
        }
        self
    }

    /// The planner's estimate before the learned correction, s.
    pub fn planned_time_s(&self) -> f64 {
        self.stats.estimated_time_s / self.estimate_correction
    }
}

/// "about 12.3 s": the time in run summaries.
fn about(seconds: f64) -> String {
    format!("about {seconds:.1} s")
}

/// The layers' names as people see them (`"Fill", "Outline"`), each once, in first-use order; a layer the document
/// no longer has shows its id.
fn layer_names<'a>(doc: &Document, ids: impl Iterator<Item = &'a str>) -> String {
    let mut seen = Vec::new();
    for id in ids {
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    let names: Vec<String> = seen
        .iter()
        .map(|id| match doc.layers.iter().find(|l| l.id == *id) {
            Some(l) => format!("\"{}\"", l.name),
            None => (*id).to_owned(),
        })
        .collect();
    names.join(", ")
}

/// Plan, encode and check `doc` for this machine. Nothing is sent anywhere: the result is only data.
///
/// Besides `plan`'s own refusals, the packets go through [`machine::check_job`] (the profile's hard limits) and a
/// read-back with [`plan::simulate`], so a run that would be refused at approval time is refused now instead.
pub fn prepare_job(doc: &Document, target: &Target, profile: &MachineProfile) -> Result<PreparedJob, EngineError> {
    prepare_job_with(doc, target, profile, &RunOptions::default(), 0)
}

/// [`prepare_job`] with [`RunOptions`] (selection, start here) and the job counter (`run_index`, for auto-rotating
/// fills). A rotary job's strips and a Z job's positions become [`RunPart`]s with the axis position as the board's
/// step word (rotary: steps from the start, which must fit 0..=65535; Z: the profile axis's `z_steps`), and a
/// rotary job ends with a move back to step 0.
pub fn prepare_job_with(
    doc: &Document,
    target: &Target,
    profile: &MachineProfile,
    options: &RunOptions,
    run_index: u64,
) -> Result<PreparedJob, EngineError> {
    options.check()?;
    if options.table.is_some() && profile.axis.as_ref().is_none_or(|a| !matches!(a.role, AxisRole::LinearTable | AxisRole::RotaryTable)) {
        return Err(EngineError::BadRequest(
            "table: Repeat Marking needs a table axis in the profile (axis role linear_table or rotary_table)".into(),
        ));
    }
    if options.start_input && profile.start_mark_input_bit.is_none() {
        return Err(EngineError::BadRequest("start_input: the machine profile names no start-marking input bit".into()));
    }
    if options.start_input && profile.start_mark_input_bit == profile.stop_mark_input_bit {
        return Err(EngineError::BadRequest(
            "start_input: the profile's start- and stop-marking inputs are the same bit (each run would stop as it starts)".into(),
        ));
    }
    let job_options = JobOptions { selection: options.selection.clone(), run_index, preview: false };
    let mut job = plan::plan_job_with(doc, target, &job_options)?;
    if let Some(n) = options.from_segment {
        job = plan::start_at(&job, n, target)?;
    }
    crate::zones::apply(&mut job, doc, options.region.as_deref(), &options.device_keep_out, target)?;
    // Pause at a layer (module `production::layer_pause`): the job in stages, each starting at a paused layer.
    let (encoded, pauses) = match crate::production::layer_pause::encode_stages(doc, &job, target)? {
        Some(staged) => staged,
        None => (plan::encode_parts(&job, target)?, Vec::new()),
    };
    let mut parts = Vec::with_capacity(encoded.len());
    let mut rotary = false;
    for e in encoded {
        machine::check_job(profile, &e.packets)?;
        let axis_position = match (e.rotary_steps, e.z_mm) {
            (Some(steps), _) => {
                rotary = true;
                if profile.axis.as_ref().is_none_or(|a| a.role != AxisRole::Rotary) {
                    return Err(EngineError::BadRequest("the job turns a rotary but the profile has no rotary axis".into()));
                }
                Some(u16::try_from(steps).map_err(|_| {
                    EngineError::BadRequest(format!("rotary position {steps} steps does not fit one 16-bit word (0..=65535)"))
                })?)
            }
            (None, Some(z)) => {
                let axis = profile.axis.as_ref().ok_or_else(|| EngineError::BadRequest("the job sets Z but the profile has no Z axis".into()))?;
                Some(axis.z_steps(z).map_err(EngineError::BadRequest)?)
            }
            (None, None) => None,
        };
        parts.push(RunPart { axis_position, packets: e.packets });
    }
    if rotary {
        parts.push(RunPart::axis_only(0));
    }
    let marks: usize = parts
        .iter()
        .map(|p| plan::simulate(&p.packets, target).segments.iter().filter(|s| s.kind == plan::SegmentKind::Mark).count())
        .sum();
    if marks == 0 {
        return Err(EngineError::Plan(plan::PlanError::Empty));
    }
    let stats = job.stats;
    let mut summary = format!(
        "{} polylines on {}, {:.1} mm marked, {}",
        stats.polyline_count,
        layer_names(doc, job.passes.iter().map(|p| p.layer_id.as_str())),
        stats.mark_length_mm,
        about(stats.estimated_time_s)
    );
    if job.parts.len() > 1 {
        summary.push_str(&format!(", in {} parts", job.parts.len()));
    }
    if !pauses.is_empty() {
        let names: Vec<String> = pauses.iter().map(|p| format!("\"{}\"", p.layer)).collect();
        summary.push_str(&format!(", waiting for the operator before {}", names.join(", ")));
    } else if rotary && crate::production::layer_pause::any_pause(doc) {
        summary.push_str(" (layer pauses are not available for a rotary job)");
    }
    match options.iterations() {
        None => summary.push_str(", continuously"),
        Some(n) if n > 1 => summary.push_str(&format!(", {n} times")),
        _ => {}
    }
    if options.selection.is_some() {
        summary.push_str(" (selected graphics)");
    }
    if options.region.is_some() {
        summary.push_str(" (re-marking a region)");
    }
    if let Some(n) = options.from_segment {
        summary.push_str(&format!(" (from segment {n})"));
    }
    Ok(PreparedJob { parts, gcode: None, ruida: None, stats, summary, generation: 0, options: options.clone(), estimate_correction: 1.0, pauses })
}

/// Plan `doc` for a GRBL gantry: the same planning as [`prepare_job_with`] on the bed's target, then G-code
/// (`plan::gcode_lines`) checked against the GRBL profile (`machine::grbl::check_program`, the GRBL counterpart of
/// `check_job`). Nothing is sent anywhere. Refused here: the start input, Repeat Marking tables and queues (galvo
/// features not built for GRBL yet), dot mode, rotary and Z jobs, feeds or powers above the profile's limits.
pub fn prepare_gcode_job(
    doc: &Document,
    target: &Target,
    grbl: &GrblProfile,
    options: &RunOptions,
    run_index: u64,
) -> Result<PreparedJob, EngineError> {
    options.check()?;
    for (set, what) in [
        (options.start_input, "start_input (a foot pedal input)"),
        (options.table.is_some(), "table (Repeat Marking)"),
        (options.queue.is_some(), "queue (batch of projects)"),
    ] {
        if set {
            return Err(EngineError::BadRequest(format!("{what} is not available on a GRBL machine yet")));
        }
    }
    let job_options = JobOptions { selection: options.selection.clone(), run_index, preview: false };
    let mut job = plan::plan_job_with(doc, target, &job_options)?;
    if let Some(n) = options.from_segment {
        job = plan::start_at(&job, n, target)?;
    }
    crate::zones::apply(&mut job, doc, options.region.as_deref(), &options.device_keep_out, target)?;
    let lines = plan::gcode_lines(&job, &crate::hw::gcode_settings(grbl))?;
    let program = GcodeProgram::new(lines);
    machine::grbl::check_program(grbl, &program).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let stats = job.stats;
    let mut summary = format!(
        "{} polylines on {}, {:.1} mm marked, {}, {} G-code lines",
        stats.polyline_count,
        layer_names(doc, job.passes.iter().map(|p| p.layer_id.as_str())),
        stats.mark_length_mm,
        about(stats.estimated_time_s),
        program.len()
    );
    if crate::production::layer_pause::any_pause(doc) {
        summary.push_str(" (layer pauses are galvo-only and were left out)");
    }
    match options.iterations() {
        None => summary.push_str(", continuously"),
        Some(n) if n > 1 => summary.push_str(&format!(", {n} times")),
        _ => {}
    }
    if options.selection.is_some() {
        summary.push_str(" (selected graphics)");
    }
    if options.region.is_some() {
        summary.push_str(" (re-marking a region)");
    }
    if let Some(n) = options.from_segment {
        summary.push_str(&format!(" (from segment {n})"));
    }
    Ok(PreparedJob {
        parts: Vec::new(),
        gcode: Some(program),
        ruida: None,
        stats,
        summary,
        generation: 0,
        options: options.clone(),
        estimate_correction: 1.0,
        pauses: Vec::new(),
    })
}

/// Batch / run multiple files: plan every `(name, document)` on its own (as [`prepare_job_with`] does, without
/// repeats) and join their parts into one job, in order, so one approval runs them one after another. The summary
/// names the projects. Refused when any of them cannot be planned (the error names it).
pub fn prepare_queue(
    docs: &[(String, Document)],
    target: &Target,
    profile: &MachineProfile,
    options: &RunOptions,
    run_index: u64,
) -> Result<PreparedJob, EngineError> {
    options.check()?;
    let single = RunOptions { queue: None, repeat: None, continuous: false, table: None, ..options.clone() };
    let mut parts = Vec::new();
    let mut pauses = Vec::new();
    let mut stats = Stats::default();
    for (name, doc) in docs {
        let doc = commands::prepare_output(doc).map_err(|e| EngineError::BadRequest(format!("project \"{name}\": text: {e}")))?;
        let p = prepare_job_with(&doc, target, profile, &single, run_index).map_err(|e| match e {
            EngineError::Plan(pe) => EngineError::BadRequest(format!("project \"{name}\": {pe}")),
            EngineError::BadRequest(m) => EngineError::BadRequest(format!("project \"{name}\": {m}")),
            other => other,
        })?;
        let offset = parts.len();
        pauses.extend(p.pauses.into_iter().map(|s| crate::production::layer_pause::LayerStop { part: s.part + offset, ..s }));
        parts.extend(p.parts);
        stats.polyline_count += p.stats.polyline_count;
        stats.mark_length_mm += p.stats.mark_length_mm;
        stats.jump_length_mm += p.stats.jump_length_mm;
        stats.estimated_time_s += p.stats.estimated_time_s;
        stats.segment_count += p.stats.segment_count;
    }
    let names: Vec<String> = docs.iter().map(|(n, _)| format!("\"{n}\"")).collect();
    let mut summary = format!(
        "queue of {} projects ({}), {} polylines, {:.1} mm marked, {}",
        docs.len(),
        names.join(", "),
        stats.polyline_count,
        stats.mark_length_mm,
        about(stats.estimated_time_s)
    );
    match options.iterations() {
        None => summary.push_str(", continuously"),
        Some(n) if n > 1 => summary.push_str(&format!(", {n} times")),
        _ => {}
    }
    Ok(PreparedJob { parts, gcode: None, ruida: None, stats, summary, generation: 0, options: options.clone(), estimate_correction: 1.0, pauses })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn prepared() -> PreparedJob {
        PreparedJob {
            parts: vec![RunPart::list(vec![lmc::Packet::empty()])],
            gcode: None,
            ruida: None,
            stats: Stats::default(),
            summary: "s".into(),
            generation: 0,
            options: RunOptions::default(),
            estimate_correction: 1.0,
            pauses: Vec::new(),
        }
    }

    #[test]
    fn the_learned_correction_scales_the_estimate_and_the_summary() {
        let mut p = prepared();
        p.stats.estimated_time_s = 20.0;
        p.summary = format!("600 polylines on Fill, 18000.0 mm marked, {}", about(20.0));
        let c = p.with_correction(1.1).with_correction(f64::NAN);
        assert!((c.stats.estimated_time_s - 22.0).abs() < 1e-9);
        assert!((c.estimate_correction - 1.1).abs() < 1e-12 && (c.planned_time_s() - 20.0).abs() < 1e-9);
        assert_eq!(c.summary, "600 polylines on Fill, 18000.0 mm marked, about 22.0 s");
    }

    #[test]
    fn run_options_are_checked() {
        assert!(RunOptions::default().check().is_ok());
        assert_eq!(RunOptions::default().iterations(), Some(1));
        assert_eq!(RunOptions { continuous: true, ..RunOptions::default() }.iterations(), None);
        assert_eq!(RunOptions { repeat: Some(5), ..RunOptions::default() }.iterations(), Some(5));
        for bad in [
            RunOptions { repeat: Some(0), ..RunOptions::default() },
            RunOptions { repeat: Some(3), continuous: true, ..RunOptions::default() },
            RunOptions { full_pulse: Some(false), ..RunOptions::default() },
            RunOptions { selection: Some(vec![]), ..RunOptions::default() },
            RunOptions { selection: Some(vec!["obj-1".into()]), from_segment: Some(3), ..RunOptions::default() },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        assert!(serde_json::from_value::<RunOptions>(serde_json::json!({"repeats": 2})).is_err());
    }

    #[test]
    fn summaries_name_layers_by_their_names() {
        // Regression: the pending run read "on layer-1, layer-21" instead of the layer names.
        let mut d = Document::default();
        d.layers[0].name = "Fill".into();
        let mut outline = d.layers[0].clone();
        outline.id = "layer-21".into();
        outline.name = "Outline".into();
        d.layers.push(outline);
        let ids = ["layer-1", "layer-21", "layer-1", "layer-9"];
        assert_eq!(layer_names(&d, ids.into_iter()), "\"Fill\", \"Outline\", layer-9");
    }

    #[test]
    fn pending_runs_are_capped_and_cancelled() {
        let mut store = RunStore::default();
        for _ in 0..MAX_PENDING {
            store.add(Origin::Ui, 1, prepared()).unwrap();
        }
        assert!(matches!(store.add(Origin::Ui, 1, prepared()), Err(EngineError::Conflict(_))));
        assert_eq!(store.cancel_not_streaming("stop"), MAX_PENDING);
        assert!(store.pending(1).is_empty());
        assert!(store.add(Origin::Ui, 1, prepared()).is_ok());
    }

    #[test]
    fn stop_cancels_every_run_not_yet_streaming() {
        let mut store = RunStore::default();
        let ids: Vec<String> = (0..5).map(|_| store.add(Origin::Ui, 1, prepared()).unwrap().id.clone()).collect();
        store.set_state(&ids[1], RunState::Approved);
        store.set_state(&ids[2], RunState::AwaitingConfirm);
        store.set_state(&ids[3], RunState::Running);
        store.finish(&ids[4], RunState::Finished, None, None);
        assert_eq!(store.cancel_not_streaming("stop"), 3);
        let states: Vec<RunState> = ids.iter().map(|id| store.get(id).unwrap().state).collect();
        assert_eq!(
            states,
            [RunState::Cancelled, RunState::Cancelled, RunState::Cancelled, RunState::Running, RunState::Finished]
        );
    }

    #[test]
    fn views_mark_stale_runs() {
        let mut store = RunStore::default();
        let id = store.add(Origin::Ui, 3, prepared()).unwrap().id.clone();
        assert!(!store.get(&id).unwrap().view(3).stale);
        assert!(store.get(&id).unwrap().view(4).stale);
    }

    #[test]
    fn finished_runs_are_pruned() {
        let mut store = RunStore::default();
        for _ in 0..(KEEP_FINISHED + 5) {
            let id = store.add(Origin::Ui, 1, prepared()).unwrap().id.clone();
            store.finish(&id, RunState::Failed, None, Some("x".into()));
        }
        assert_eq!(store.views(1).len(), KEEP_FINISHED);
    }

    #[test]
    fn states_serialise_as_the_contract_names() {
        let names: Vec<Value> = [
            RunState::Pending,
            RunState::Approved,
            RunState::AwaitingConfirm,
            RunState::Running,
            RunState::Finished,
            RunState::Failed,
            RunState::Aborted,
            RunState::Cancelled,
        ]
        .iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
        assert_eq!(
            Value::Array(names),
            serde_json::json!([
                "pending",
                "approved",
                "awaiting_confirm",
                "running",
                "finished",
                "failed",
                "aborted",
                "cancelled"
            ])
        );
    }

    #[test]
    fn final_states_do_not_move() {
        let mut store = RunStore::default();
        let id = store.add(Origin::Ui, 1, prepared()).unwrap().id.clone();
        store.set_state(&id, RunState::Running);
        store.finish(&id, RunState::Aborted, None, Some("stop".into()));
        store.set_state(&id, RunState::Running);
        let v = store.get(&id).unwrap().view(1);
        assert_eq!((v.state, v.packet_count, v.error.as_deref()), (RunState::Aborted, 0, Some("stop")));
    }
}
