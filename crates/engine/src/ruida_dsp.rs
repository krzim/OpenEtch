//! Ruida gantries in the engine (unverified on hardware; tested on `ruida::sim`): planning a document into a Ruida
//! program, the controller's file list (LightBurn's File List window and Send, OTH-017 / JOB-065), Save RD file
//! (UX-007) and the routes for them (merged into `api::router`).
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | POST | `/api/devices/ruida` | `{display_name, profile}`: a new Ruida device from a `machine::ruida::RuidaProfile` (UI only) |
//! | GET | `/api/machine/ruida` | `?refresh=true` reads the status register and head position of an idle controller first. `{profile, controller, position}` (`controller` null while a run or frame holds it) |
//! | GET | `/api/machine/files` | `{files: [{index, name}]}`: the files stored on the controller (reads only; 409 while a run or frame holds it) |
//! | POST | `/api/machine/files/send` | `{name, ids?}`: Send. The document (or the objects `ids`) planned like a run and stored on the controller as `name` (1-32 printable ASCII); never started. Cancels pending stored-file runs. UI only |
//! | POST | `/api/machine/files/{index}/delete` | `{name}`: delete file `index` if it is still named `name`. Cancels pending stored-file runs. UI only |
//! | POST | `/api/machine/files/{index}/run` | `{name}` → `{ok, run}`: a pending run that starts file `index` (if still named `name`); approve it like any run (`POST /api/runs/{id}/approve`: the operator PIN, or Start in the app). UI only, never MCP |
//! | GET | `/api/export/rd` | `?ids=a,b`: Save RD file. The document planned for the active Ruida gantry as an `.rd` file (the bytes a run would send, checked like a run). 409 unless a Ruida gantry is active. Sends nothing to a machine |
//!
//! **Safety.** Starting a stored file fires the laser, so it is a run: a pending run whose job is
//! `machine::DriverJob::RuidaFile`, approved only through `approve` (the operator PIN or Start in the app; never the
//! foot switch, which burns only the document it framed, and never MCP, which has no tool for it and whose headers
//! these routes refuse). The approval gates apply as to any run (the stop epoch, Stop At, the Job Checklist, the
//! device generation, another run); Require Framing cannot be met for a file the engine cannot frame, so with it on a
//! stored file is refused (start it from the controller's panel, or turn Require Framing off). OpenEtch cannot read a
//! stored file back, so its contents are checked only when OpenEtch's own Send stored it. Send itself does not fire
//! (the file name makes the controller store the job); the driver still stops the upload if the controller shows a
//! running job while it arrives (see `machine::ruida::RuidaController`).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commands::Origin;
use doc::Document;
use machine::DriverJob;
use machine::ruida::{Purpose, RuidaProfile, RuidaProgram, StoredFile, check_program};
use plan::{JobOptions, JobPlan, Stats, Target};
use ruida::rd::{RdJob, RdLayer};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runs::{PreparedJob, RunOptions};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError, Event};

/// Most layers an `.rd` job holds (layer numbers are one 7-bit byte).
const MAX_RD_LAYERS: usize = ruida::rd::MAX_LAYERS;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/devices/ruida", post(device_add_ruida))
        .route("/machine/ruida", get(ruida_status))
        .route("/machine/files", get(files_list))
        .route("/machine/files/send", post(files_send))
        .route("/machine/files/{index}/delete", post(files_delete))
        .route("/machine/files/{index}/run", post(files_run))
        .route("/export/rd", get(export_rd))
}

fn color_of(doc: &Document, layer_id: &str) -> [u8; 3] {
    let hex = doc.layers.iter().find(|l| l.id == layer_id).map(|l| l.color.trim_start_matches('#').to_owned()).unwrap_or_default();
    let byte = |i: usize| hex.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0);
    if hex.len() == 6 { [byte(0), byte(2), byte(4)] } else { [0, 0, 0] }
}

/// The planner's passes as `.rd` layers in machine mm (`profile.to_machine`), in order: a new layer whenever the pass
/// or the power changes (a shape with a power scale or a grayscale image makes one layer per power level, every tab
/// marked at a Tab Cut Power one of its own, at most
/// 128 in all). Refused: rotary and Z jobs, dot mode, layer pauses and the GRBL layer options (constant power, air
/// assist, overscan, overcut, the fill ramp, leads and path pauses; not built for Ruida),
/// points off the bed.
pub fn rd_job_from_plan(doc: &Document, job: &JobPlan, profile: &RuidaProfile) -> Result<RdJob, EngineError> {
    let refuse = |m: String| Err(EngineError::BadRequest(m));
    if job.parts.len() > 1 || job.passes.iter().any(|p| p.part != 0) {
        return refuse("the job moves an extension axis (rotary or Z), which a Ruida job here does not support".into());
    }
    let mut layers: Vec<RdLayer> = Vec::new();
    for (n, pass) in job.passes.iter().enumerate() {
        let name = &pass.layer_id;
        if pass.dot_time_us.is_some() {
            return refuse(format!("layer {name}: dot mode is a galvo feature"));
        }
        if pass.pause_before_us > 0.0 || pass.pause_after_us > 0.0 {
            return refuse(format!("layer {name}: pauses before or after a pass are not available on a Ruida machine yet"));
        }
        // The fill ramp is planned into the pass's powers (a Ruida target is a gantry target), so it is found from
        // the pass's own settings rather than from `pass.gantry`.
        let cut = doc
            .layers
            .iter()
            .find(|l| l.id == *name)
            .and_then(|l| if pass.sub_layer == 0 { Some(&l.settings) } else { l.sub_layers.get(pass.sub_layer - 1) });
        let ramp = cut.is_some_and(|c| c.options.ramp_length_mm > 0.0 && matches!(c.mode, doc::FillMode::Fill | doc::FillMode::FillThenLine));
        if !pass.gantry.is_default() || ramp {
            return refuse(format!(
                "layer {name}: constant power, air assist, overscan, overcut, the fill ramp, lead-ins, lead-outs and \
                 start/end pauses are GRBL layer options, not available on a Ruida machine yet"
            ));
        }
        let color = color_of(doc, name);
        let mut current: Option<(f64, usize)> = None;
        for (i, poly) in pass.polylines.iter().enumerate() {
            if poly.len() < 2 {
                continue;
            }
            let power = pass.power_of(i);
            let mut pts = Vec::with_capacity(poly.len());
            for p in poly {
                let (x, y) = profile.to_machine(p[0], p[1]).map_err(EngineError::BadRequest)?;
                pts.push([x, y]);
            }
            match current {
                Some((pw, at)) if pw == power && at == layers.len() - 1 => layers[at].polylines.push(pts),
                _ => {
                    if layers.len() >= MAX_RD_LAYERS {
                        return refuse(format!(
                            "the job needs more than {MAX_RD_LAYERS} Ruida layers (pass {n}: every pass and every change of power \
                             starts one: a power-scaled shape, a grayscale image, a tab marked at a Tab Cut Power); use fewer \
                             power levels or tabs"
                        ));
                    }
                    layers.push(RdLayer {
                        color,
                        speed_mm_s: pass.settings.speed_mm_s,
                        min_power: [power, 0.0],
                        max_power: [power, 0.0],
                        frequency: None,
                        polylines: vec![pts],
                    });
                    current = Some((power, layers.len() - 1));
                }
            }
        }
    }
    if layers.is_empty() {
        return Err(EngineError::Plan(plan::PlanError::Empty));
    }
    Ok(RdJob { layers })
}

/// Plan `doc` for the Ruida gantry into a program (named `name` for Send), checked against the profile like a run.
fn plan_program(
    doc: &Document,
    target: &Target,
    profile: &RuidaProfile,
    selection: Option<Vec<String>>,
    from_segment: Option<usize>,
    run_index: u64,
    name: Option<&str>,
) -> Result<(RuidaProgram, JobPlan), EngineError> {
    let doc = commands::prepare_output(doc).map_err(|e| EngineError::BadRequest(format!("text: {e}")))?;
    let mut job = plan::plan_job_with(&doc, target, &JobOptions { selection, run_index, preview: false })?;
    if let Some(n) = from_segment {
        job = plan::start_at(&job, n, target)?;
    }
    let rd = rd_job_from_plan(&doc, &job, profile)?;
    let program = RuidaProgram::from_job(&rd, name).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let purpose = if name.is_some() { Purpose::Store } else { Purpose::Run };
    check_program(profile, &program, purpose).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    Ok((program, job))
}

/// Plan `doc` for a Ruida gantry (as `runs::prepare_gcode_job` does for GRBL): the program a run streams, checked
/// against the profile. Nothing is sent. Refused here: the start input, Repeat Marking tables and queues.
pub fn prepare_ruida_job(
    doc: &Document,
    target: &Target,
    profile: &RuidaProfile,
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
            return Err(EngineError::BadRequest(format!("{what} is not available on a Ruida machine yet")));
        }
    }
    let (program, job) = plan_program(doc, target, profile, options.selection.clone(), options.from_segment, run_index, None)?;
    let stats = job.stats;
    let mut summary = format!(
        "{} polylines, {:.1} mm marked, about {:.1} s, Ruida job of {} bytes",
        stats.polyline_count,
        stats.mark_length_mm,
        stats.estimated_time_s,
        program.len()
    );
    match options.iterations() {
        None => summary.push_str(", continuously"),
        Some(n) if n > 1 => summary.push_str(&format!(", {n} times")),
        _ => {}
    }
    if options.selection.is_some() {
        summary.push_str(" (selected graphics)");
    }
    Ok(PreparedJob {
        parts: Vec::new(),
        gcode: None,
        ruida: Some(DriverJob::Ruida(Arc::new(program))),
        stats,
        summary,
        generation: 0,
        options: options.clone(),
        estimate_correction: 1.0,
    })
}

/// Refuse a stored-file run while Require Framing Before Start is on (module docs). Called when the run is requested
/// and again when it is approved.
pub(crate) fn check_stored_run(st: &AppState) -> Result<(), EngineError> {
    let device = st.active_device().id;
    if crate::libs::lock(&st.libs.prefs).device(&device).require_framing {
        return Err(EngineError::Conflict(
            "Require Framing Before Start is on, and a file stored on the controller cannot be framed from OpenEtch: start it \
             from the controller's panel, or turn Require Framing off"
                .into(),
        ));
    }
    Ok(())
}

/// A Send or a Delete changed the controller's file list (and its numbering): pending stored-file runs are cancelled,
/// so an approval never starts a file other than the one the list showed when it was asked for (a name can be on
/// the list twice). Returns how many.
fn cancel_stored_runs(st: &AppState) -> usize {
    let mut store = st.runs();
    let ids: Vec<String> = store
        .pending(0)
        .into_iter()
        .filter(|v| matches!(store.get(&v.id).and_then(|r| r.ruida.as_ref()), Some(DriverJob::RuidaFile { .. })))
        .map(|v| v.id)
        .collect();
    for id in &ids {
        if let Some(r) = store.get_mut(id) {
            r.state = crate::runs::RunState::Cancelled;
            r.error = Some("the controller's file list changed (Send or Delete); start the file again from the File List".into());
        }
    }
    ids.len()
}

/// Refuse requests that look like an MCP client or name a non-UI origin.
fn ui_only(headers: &HeaderMap, origin: Option<&Value>, what: &str) -> Result<(), EngineError> {
    if has_mcp_marker(headers) {
        return Err(EngineError::Forbidden(format!("{what} only by a person in the OpenEtch UI")));
    }
    if parse_origin(origin)? != Origin::Ui {
        return Err(EngineError::Forbidden(format!("{what} only from the UI")));
    }
    Ok(())
}

fn body<T: for<'de> Deserialize<'de>>(b: &Bytes) -> Result<T, EngineError> {
    serde_json::from_slice(b).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

/// Run `f` on the Ruida driver: refused unless a Ruida gantry is active and no run or frame holds the machine.
async fn with_ruida<R, F>(st: &Arc<AppState>, f: F) -> Result<R, EngineError>
where
    R: Send + 'static,
    F: FnOnce(&mut machine::RuidaDriver) -> Result<R, machine::ControllerError> + Send + 'static,
{
    st.require_ruida()?;
    st.refuse_if_shutting_down("using the controller")?;
    if st.machine.is_running() {
        return Err(EngineError::Conflict("a run is in progress".into()));
    }
    if st.machine.is_framing() {
        return Err(EngineError::Conflict("framing is on; stop framing first".into()));
    }
    let r = st
        .machine
        .blocking(move |m, ctl| {
            if m.is_running() {
                return Err(machine::ControllerError::Busy(machine::ControllerState::Running));
            }
            m.ensure_ready(ctl)?;
            match ctl.ruida_mut() {
                Some(r) => f(r),
                None => Err(machine::ControllerError::Refused("the active machine is not a Ruida gantry".into())),
            }
        })
        .await??;
    Ok(r)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuidaDeviceBody {
    #[serde(default)]
    display_name: String,
    profile: RuidaProfile,
    #[serde(default)]
    origin: Option<Value>,
}

async fn device_add_ruida(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Response, EngineError> {
    let b: RuidaDeviceBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "devices can be added")?;
    let d = st.blocking(move |st| st.devices().add_ruida(&b.display_name, b.profile)).await?;
    st.emit(Event::DevicesChanged);
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "device": d.summary() }))).into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusQuery {
    #[serde(default)]
    refresh: bool,
}

async fn ruida_status(State(st): State<Arc<AppState>>, Query(q): Query<StatusQuery>) -> Result<Json<Value>, EngineError> {
    let profile = st.machine.ruida_profile().ok_or_else(|| EngineError::Conflict("the active machine is not a Ruida gantry".into()))?;
    if q.refresh && !st.machine.is_running() && !st.machine.is_framing() {
        // Reads registers only; a failure leaves the last snapshot (and the machine's last error says why).
        let _ = with_ruida(&st, |r| r.refresh()).await;
    }
    let controller = st.machine.ruida_snapshot();
    let position = controller.as_ref().and_then(|c| c.position_mm).map(|[x, y]| {
        let (fx, fy) = profile.to_field(x, y);
        json!({ "machine": [x, y], "field": [fx, fy] })
    });
    Ok(Json(json!({
        "profile": profile,
        "controller": controller,
        "position": position,
        "running": st.machine.is_running(),
        "framing": st.machine.is_framing(),
    })))
}

async fn files_list(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let files = with_ruida(&st, |r| r.list_files()).await?;
    Ok(Json(json!({ "ok": true, "files": files })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    name: String,
    #[serde(default)]
    ids: Option<Vec<String>>,
    #[serde(default)]
    origin: Option<Value>,
}

/// Send (JOB-065): plan the document like a run and store it on the controller. Never starts it.
async fn files_send(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: SendBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "jobs can be sent to the controller")?;
    st.require_ruida()?;
    machine::ruida::check_name(&b.name).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let config = st.machine.config();
    let profile = config.ruida.clone().ok_or_else(|| EngineError::Conflict("the active machine is not a Ruida gantry".into()))?;
    let doc = st.document();
    let run_index = st.job.count(&st.active_device().id);
    let name = b.name.clone();
    let (program, _) = tokio::task::spawn_blocking(move || {
        plan_program(&doc, &config.target, &profile, b.ids.filter(|v| !v.is_empty()), None, run_index, Some(&name))
    })
    .await??;
    let result = with_ruida(&st, move |r| r.upload_file(&program)).await;
    // Even a failed Send may have changed the list (a half file stored, then removed).
    cancel_stored_runs(&st);
    let file: StoredFile = result?;
    tracing::info!(index = file.index, name = %file.name, "job sent to the Ruida controller");
    st.emit(Event::machine("controller_files_changed", None));
    Ok(Json(json!({ "ok": true, "file": file })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBody {
    name: String,
    #[serde(default)]
    origin: Option<Value>,
}

async fn files_delete(
    State(st): State<Arc<AppState>>,
    UrlPath(index): UrlPath<u16>,
    headers: HeaderMap,
    b: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: FileBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "files on the controller can be deleted")?;
    let name = b.name.clone();
    with_ruida(&st, move |r| r.delete_file(index, &name)).await?;
    cancel_stored_runs(&st);
    tracing::info!(index, name = %b.name, "file deleted on the Ruida controller");
    st.emit(Event::machine("controller_files_changed", None));
    Ok(Json(json!({ "ok": true })))
}

/// A pending run that starts stored file `index` (OTH-017's Start). It waits for approval like any run.
async fn files_run(
    State(st): State<Arc<AppState>>,
    UrlPath(index): UrlPath<u16>,
    headers: HeaderMap,
    b: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: FileBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "a file on the controller can be started")?;
    st.refuse_if_shutting_down("requesting a run")?;
    st.require_ruida()?;
    check_stored_run(&st)?;
    machine::ruida::check_name(&b.name).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    if index == 0 {
        return Err(EngineError::BadRequest("file numbers start at 1".into()));
    }
    let (revision, document_id) = st.active_revision_and_document();
    let generation = st.machine.generation();
    let prepared = PreparedJob {
        parts: Vec::new(),
        gcode: None,
        ruida: Some(DriverJob::RuidaFile { index, name: b.name.clone() }),
        stats: Stats::default(),
        summary: format!(
            "file {index} \"{}\" stored on the Ruida controller (run as stored: OpenEtch cannot read it back)",
            b.name
        ),
        generation,
        options: RunOptions::default(),
        estimate_correction: 1.0,
    };
    let view = {
        let mut store = st.runs();
        st.refuse_if_shutting_down("requesting a run")?;
        let run = store.add(Origin::Ui, revision, prepared)?;
        run.document_id = document_id;
        run.view(revision)
    };
    tracing::info!(run_id = %view.id, index, name = %b.name, "stored-file run requested; waiting for approval in the UI");
    st.emit(Event::RunPending { run_id: view.id.clone(), origin: Origin::Ui, summary: view.summary.clone() });
    Ok(Json(json!({ "ok": true, "run": view })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportQuery {
    #[serde(default)]
    ids: Option<String>,
}

/// Save RD file (UX-007): the program a run would send, as a scrambled `.rd` file.
async fn export_rd(State(st): State<Arc<AppState>>, Query(q): Query<ExportQuery>) -> Result<Response, EngineError> {
    let config = st.machine.config();
    let Some(profile) = config.ruida.clone() else {
        return Err(EngineError::Conflict("RD export needs a Ruida gantry as the active device".into()));
    };
    let selection = q.ids.map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>());
    let doc = st.document();
    let stem = crate::userfiles::download_stem(&st);
    let bytes = tokio::task::spawn_blocking(move || {
        plan_program(&doc, &config.target, &profile, selection, None, 0, None).map(|(p, _)| p.rd_file(profile.magic))
    })
    .await??;
    let file: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    let file = if file.trim().is_empty() { "Untitled".to_owned() } else { file.trim().to_owned() };
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file}.rd\"")),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        bytes,
    )
        .into_response())
}

/// The import parameters an `.rd` file gets from the active Ruida device (its origin corner and bed), so a file
/// written for it lands where it was (`userfiles` import).
pub(crate) fn import_params(st: &AppState) -> Vec<(String, String)> {
    let Some(r) = st.machine.ruida_profile() else { return Vec::new() };
    let origin = match r.mirror() {
        (true, true) => "top_right",
        (false, true) => "top_left",
        (false, false) => "bottom_left",
        (true, false) => "bottom_right",
    };
    vec![
        ("rd_origin".into(), origin.into()),
        ("bed_width_mm".into(), r.work_area_mm[0].to_string()),
        ("bed_height_mm".into(), r.work_area_mm[1].to_string()),
    ]
}
