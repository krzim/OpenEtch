//! The machine, job and device routes added for job control, framing, calibration and device settings (merged into
//! `api::router`). Request rules follow the rest of the API: routes that change the machine's configuration or
//! continue a paused job are UI-only (they refuse MCP client headers and non-`ui` origins); pause, stop and light-only
//! routes are not gated.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/job` | job counter, Stop At and the progress of the job holding the machine (see `job`) |
//! | PUT | `/api/job/settings` | `{stop_at: n \| null}` → the job status |
//! | POST | `/api/job/count/reset` | Reset Count for the active device → the job status |
//! | POST | `/api/job/end_after_current` | end a repeating / continuous job after its current iteration |
//! | POST | `/api/machine/pause` | pause the running job (`StopList`) |
//! | POST | `/api/machine/resume` | continue it (`RestartList`; UI only) |
//! | POST | `/api/machine/pointer` | `{x, y, pointer?, max_seconds?}`: hold the red dot there (light only) |
//! | POST | `/api/machine/axis` | `{z_mm}` or `{steps}`: move the board's extension axis (UI only; no emission) |
//! | GET | `/api/machine/info` | the board's serial and version words (GetSerialNo / GetVersion) |
//! | GET | `/api/machine/inputs` | `{bits, set, door_protect, door_open}`: the board's input port (`ReadPort` word 1; read only, sends nothing that can emit), to find which input a switch such as a door interlock is wired to. 409 while a run holds the board |
//! | GET | `/api/preview.png` | the preview image (`?width=` px, `?jumps=false`) |
//! | POST | `/api/calibration/pattern` | `{size_mm, layer?}`: add the lens test pattern to the document |
//! | POST | `/api/calibration/solve` | `{size_mm, measurements, apply?}`: new lens corrections (apply: UI only) |
//! | PUT | `/api/devices/{id}` | `{profile?: merge patch, display_name?}`: edit device settings (UI only) |
//! | POST | `/api/devices/{id}/duplicate` | an editable copy (UI only) |
//! | GET | `/api/devices/{id}/lbdev` | the device as a LightBurn `.lbdev` download |
//! | PUT | `/api/devices/{id}/cor` | `?name=<file>.cor`, the file as the body: load a lens correction file (UI only) |
//! | DELETE | `/api/devices/{id}/cor` | clear it (UI only) |
//! | POST | `/api/devices/import/markcfg` | `?name=`, an EzCad `markcfg7` as the body: a new device (UI only) |
//! | GET | `/api/machine/grbl` | a GRBL gantry's profile and board (version, last status report, alarm, laser mode, `$30`/`$31`, unsafe start-up blocks, messages); `board` null while a run or frame holds it |
//! | POST | `/api/machine/home` | GRBL homing cycle `$H` (UI only; moves, never fires) |
//! | POST | `/api/machine/unlock` | GRBL `$X` after an alarm (UI only) |
//! | POST | `/api/machine/jog` | `{dx, dy}` mm or `{x, y}` (field frame), `feed?` mm/min: a GRBL jog `$J=` with the laser off (UI only) |
//! | POST | `/api/machine/jog/cancel` | GRBL jog cancel (0x85); not gated |
//! | POST | `/api/machine/console` | `{line}` → `{line, lines, ok, error}`: one console line to a GRBL board (UI only). Lines that could turn the laser on (`M3`/`M4`, `S` > 0, unsafe start-up blocks, realtime characters, `$RST`) are refused (422) before anything is sent; see `machine::grbl::check_console_line` |
//! | GET | `/api/machine/ports` | the serial ports of the engine's computer (needs the `serial` build feature) |
//! | POST | `/api/devices/grbl` | `{display_name, profile}`: a new GRBL device from a GRBL profile (UI only) |
//! | GET | `/api/sim/inputs` | `{simulated, bits}`: the simulated board's input port (`bits` null on USB) |
//! | POST | `/api/sim/inputs` | `{bits: 0..=65535}` → `{ok, bits}`: set the simulated input port (word 1 of `ReadPort`) so the foot pedal (Start Marking), Stop Marking, Door Protect and Start Framing inputs can be exercised end to end. Only while the machine link is the simulator (403 on USB); refuses MCP client headers (403), as a switch press stands for a person |

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use commands::{Invocation, Origin};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::frame::Pointer;
use crate::job::JobSettings;
use crate::render::RenderOptions;
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError, Event};

/// Largest correction or markcfg file accepted.
const MAX_MACHINE_FILE: usize = 4 * 1024 * 1024;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/job", get(job_status))
        .route("/job/settings", put(job_settings))
        .route("/job/count/reset", post(job_count_reset))
        .route("/job/end_after_current", post(job_end_after_current))
        .route("/machine/pause", post(machine_pause))
        .route("/machine/resume", post(machine_resume))
        .route("/machine/pointer", post(machine_pointer))
        .route("/machine/axis", post(machine_axis))
        .route("/machine/info", get(machine_info))
        .route("/machine/inputs", get(machine_inputs))
        .route("/preview.png", get(preview_png))
        .route("/calibration/pattern", post(calibration_pattern))
        .route("/calibration/solve", post(calibration_solve))
        .route("/devices/{id}/duplicate", post(device_duplicate))
        .route("/devices/{id}/lbdev", get(device_lbdev))
        .route(
            "/devices/{id}/cor",
            put(device_cor_load).delete(device_cor_clear).layer(DefaultBodyLimit::max(MAX_MACHINE_FILE)),
        )
        .route("/devices/import/markcfg", post(device_import_markcfg).layer(DefaultBodyLimit::max(MAX_MACHINE_FILE)))
        .route("/devices/grbl", post(device_add_grbl))
        .route("/machine/grbl", get(grbl_status))
        .route("/machine/home", post(grbl_home))
        .route("/machine/unlock", post(grbl_unlock))
        .route("/machine/jog", post(grbl_jog))
        .route("/machine/jog/cancel", post(grbl_jog_cancel))
        .route("/machine/console", post(grbl_console))
        .route("/machine/ports", get(serial_ports))
        .route("/sim/inputs", get(sim_inputs).post(sim_set_inputs))
}

async fn serial_ports() -> Json<Value> {
    Json(json!({ "ports": crate::hw::serial_ports(), "serial_support": cfg!(feature = "serial") }))
}

async fn grbl_status(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let profile = st.machine.grbl_profile().ok_or_else(|| EngineError::Conflict("the active machine is not a GRBL gantry".into()))?;
    Ok(Json(json!({ "profile": profile, "board": st.machine.grbl_snapshot(), "running": st.machine.is_running(), "framing": st.machine.is_framing() })))
}

/// Run `f` on the GRBL driver: refused unless a GRBL gantry is active and no run or frame holds the machine (a
/// request never waits behind a run for the driver).
async fn with_grbl<R, F>(st: &Arc<AppState>, f: F) -> Result<R, EngineError>
where
    R: Send + 'static,
    F: FnOnce(&mut machine::GrblDriver) -> Result<R, machine::ControllerError> + Send + 'static,
{
    st.require_grbl()?;
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
            match ctl.grbl_mut() {
                Some(g) => f(g),
                None => Err(machine::ControllerError::Refused("the active machine is not a GRBL gantry".into())),
            }
        })
        .await??;
    Ok(r)
}

/// Homing (`$H`): motion, never emission; UI only like the galvo's axis moves.
async fn grbl_home(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: OriginOnly = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "the machine can be homed")?;
    with_grbl(&st, |g| g.home()).await?;
    st.emit(Event::machine("homed", None));
    Ok(Json(json!({ "ok": true })))
}

async fn grbl_unlock(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: OriginOnly = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "the machine can be unlocked")?;
    with_grbl(&st, |g| g.unlock()).await?;
    st.emit(Event::machine("unlocked", None));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct JogBody {
    #[serde(default)]
    dx: Option<f64>,
    #[serde(default)]
    dy: Option<f64>,
    #[serde(default)]
    x: Option<f64>,
    #[serde(default)]
    y: Option<f64>,
    /// mm/min (default the profile's frame feed).
    #[serde(default)]
    feed: Option<f64>,
    #[serde(default)]
    origin: Option<Value>,
}

/// Jog (`$J=`) with the laser off: by `{dx, dy}` mm, or to `{x, y}` in the field frame. UI only.
async fn grbl_jog(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: JogBody = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "the head can be jogged")?;
    let feed = b.feed;
    match (b.dx.or(b.dy), b.x, b.y) {
        (Some(_), None, None) => {
            let (dx, dy) = (b.dx.unwrap_or(0.0), b.dy.unwrap_or(0.0));
            with_grbl(&st, move |g| g.jog(dx, dy, feed)).await?;
        }
        (None, Some(x), Some(y)) => with_grbl(&st, move |g| g.jog_to(x, y, feed)).await?,
        _ => return Err(EngineError::BadRequest("give dx/dy (a step) or x and y (a position)".into())),
    }
    Ok(Json(json!({ "ok": true })))
}

/// Jog cancel (0x85): stops motion only, so it is not gated (like pause and stop).
async fn grbl_jog_cancel(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    st.require_grbl()?;
    if st.machine.is_running() || st.machine.is_framing() {
        return Ok(Json(json!({ "ok": true, "sent": false })));
    }
    st.machine
        .blocking(|_, ctl| match ctl.grbl_mut() {
            Some(g) => g.jog_cancel(),
            None => Ok(()),
        })
        .await??;
    Ok(Json(json!({ "ok": true, "sent": true })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConsoleBody {
    line: String,
    #[serde(default)]
    origin: Option<Value>,
}

/// One console line to the GRBL board. UI only; the console policy refuses anything that could fire.
async fn grbl_console(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: ConsoleBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body (expected {{\"line\": ...}}): {e}")))?;
    ui_only(&headers, b.origin.as_ref(), "console lines can be sent")?;
    // Checked before waiting for the driver, so a refused line never even queues.
    machine::grbl::check_console_line(&b.line).map_err(|e| EngineError::Machine(machine::grbl::GrblError::Console(e).into()))?;
    let line = b.line.clone();
    let reply = with_grbl(&st, move |g| g.console(&line)).await?;
    tracing::info!(line = %reply.line, ok = reply.ok, "GRBL console");
    Ok(Json(json!(reply)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrblDeviceBody {
    #[serde(default)]
    display_name: String,
    profile: machine::grbl::GrblProfile,
    #[serde(default)]
    origin: Option<Value>,
}

async fn device_add_grbl(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Response, EngineError> {
    let b: GrblDeviceBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    ui_only(&headers, b.origin.as_ref(), "devices can be added")?;
    let d = st.blocking(move |st| st.devices().add_grbl(&b.display_name, b.profile)).await?;
    st.emit(Event::DevicesChanged);
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "device": d.summary() }))).into_response())
}

async fn sim_inputs(State(st): State<Arc<AppState>>) -> Json<Value> {
    let counters = st.machine.sim_counters();
    Json(json!({ "simulated": counters.is_some(), "bits": counters.map(|c| c.inputs()) }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimInputsBody {
    bits: u16,
}

async fn sim_set_inputs(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    if has_mcp_marker(&headers) {
        return Err(EngineError::Forbidden("the simulated inputs stand for switches a person presses; not from MCP".into()));
    }
    let counters = match st.machine.sim_counters() {
        Some(c) if st.machine.kind() == crate::hw::LinkKind::Simulated => c,
        _ => return Err(EngineError::Forbidden("the machine link is a real board (USB): its inputs cannot be set".into())),
    };
    let b: SimInputsBody =
        serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body (expected {{\"bits\": 0..=65535}}): {e}")))?;
    counters.set_inputs(b.bits);
    tracing::info!(bits = b.bits, "simulated inputs set");
    Ok(Json(json!({ "ok": true, "bits": b.bits })))
}

/// Parse a JSON body; an empty body means `T::default()`.
fn body_or_default<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, EngineError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

/// Refuse requests that look like an MCP client or name a non-UI origin (`?origin=` or the body's `origin`).
fn ui_only(headers: &HeaderMap, origin: Option<&Value>, what: &str) -> Result<(), EngineError> {
    if has_mcp_marker(headers) {
        return Err(EngineError::Forbidden(format!("{what} only by a person in the OpenEtch UI")));
    }
    if parse_origin(origin)? != Origin::Ui {
        return Err(EngineError::Forbidden(format!("{what} only from the UI")));
    }
    Ok(())
}

async fn job_status(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(st.job_status())
}

async fn job_settings(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let s: JobSettings = body_or_default(&body)?;
    if let Some(v) = s.stop_at {
        st.job.set_stop_at(v);
    }
    st.emit(Event::machine("job_count", None));
    Ok(Json(st.job_status()))
}

async fn job_count_reset(State(st): State<Arc<AppState>>) -> Json<Value> {
    st.job.reset(&st.active_device().id);
    st.emit(Event::machine("job_count", Some("0".into())));
    Json(st.job_status())
}

async fn job_end_after_current(State(st): State<Arc<AppState>>) -> Json<Value> {
    let active = st.job.end_after_current();
    Json(json!({ "ok": true, "active": active }))
}

/// Pause: never gated (it only stops motion).
async fn machine_pause(State(st): State<Arc<AppState>>) -> Json<Value> {
    let running = st.machine.is_running();
    if running {
        st.machine.pause();
        st.emit(Event::machine("paused", None));
    }
    Json(json!({ "ok": true, "running": running }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginOnly {
    #[serde(default)]
    origin: Option<Value>,
}

/// Resume continues an approved run, so only a person in the UI may.
async fn machine_resume(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: OriginOnly = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "a paused job can be resumed")?;
    if !st.machine.is_running() {
        return Err(EngineError::Conflict("no job is running".into()));
    }
    st.machine.resume();
    st.emit(Event::machine("resumed", None));
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PointerBody {
    x: f64,
    y: f64,
    #[serde(default)]
    pointer: Pointer,
    #[serde(default)]
    max_seconds: Option<u64>,
}

async fn machine_pointer(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: PointerBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    Ok(Json(st.pointer_to(b.x, b.y, b.max_seconds, b.pointer).await?))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AxisBody {
    #[serde(default)]
    z_mm: Option<f64>,
    #[serde(default)]
    steps: Option<u16>,
    #[serde(default)]
    origin: Option<Value>,
}

/// Move the board's extension axis (a Z lift or a rotary). UI only: motion near the work.
async fn machine_axis(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: AxisBody = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "the axis can be moved")?;
    let profile = st.machine.profile();
    let axis = profile.axis.clone().ok_or_else(|| EngineError::BadRequest("the machine profile has no extension axis".into()))?;
    let position = match (b.z_mm, b.steps) {
        (Some(z), None) => axis.z_steps(z).map_err(EngineError::BadRequest)?,
        // Raw steps stay within a Z lift's configured travel like z_mm does (a rotary turns freely).
        (None, Some(s)) if axis.role == machine::AxisRole::Z => {
            let top = axis.z_steps(axis.max_mm).map_err(EngineError::BadRequest)?;
            if s > top {
                return Err(EngineError::BadRequest(format!("steps {s} is beyond the Z travel (0..={top} steps)")));
            }
            s
        }
        (None, Some(s)) => s,
        _ => return Err(EngineError::BadRequest("give z_mm or steps".into())),
    };
    // Claim the machine as a run would, so no run is approved while the axis moves (activate_device does the same).
    if !st.machine.try_begin_run() {
        return Err(EngineError::Conflict("a run is in progress".into()));
    }
    struct Claim<'a>(&'a crate::hw::Machine);
    impl Drop for Claim<'_> {
        fn drop(&mut self) {
            self.0.end_run();
        }
    }
    let _claim = Claim(&st.machine);
    st.machine.bump_frame_gen();
    st.machine.blocking(move |m, ctl| m.ensure_ready(ctl).and_then(|()| ctl.move_axis(position))).await??;
    st.emit(Event::machine("axis_moved", Some(position.to_string())));
    Ok(Json(json!({ "ok": true, "position": position })))
}

/// Controller info (LightBurn's Get Controller Info): the raw words of `GetSerialNo` and `GetVersion` (their meaning
/// is not decoded) and the serial as hex, as meerk40t shows it.
async fn machine_info(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let info = st.machine.blocking(|m, ctl| m.board_info(ctl)).await?;
    let profile = st.machine.profile();
    let mut v = json!({
        "connected": info.is_some(),
        "profile": profile.name,
        "laser_source": profile.laser_source,
        "laser_kind": profile.laser_kind,
        "fiber_kind": profile.fiber_kind,
        "axis": profile.axis,
    });
    if let Some(i) = info {
        let hex: String = i.serial.iter().map(|w| format!("{w:04x}")).collect();
        v["serial_words"] = json!(i.serial);
        v["version_words"] = json!(i.version);
        v["serial_hex"] = json!(format!("0x{hex}"));
    }
    Ok(Json(v))
}

async fn machine_inputs(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    if st.machine.is_running() {
        return Err(EngineError::Conflict("a run holds the board; read the inputs when it is idle".into()));
    }
    let bits = st
        .machine
        .blocking(|m, ctl| {
            m.ensure_ready(ctl)?;
            m.read_inputs(ctl)
        })
        .await??;
    let door = st.machine.profile().io.door_protect;
    Ok(Json(json!({
        "bits": bits,
        "set": (0..16).filter(|b| bits & (1u16 << b) != 0).collect::<Vec<u16>>(),
        "door_protect": door,
        "door_open": door.map(|d| d.active(bits)),
    })))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PngQuery {
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    jumps: Option<bool>,
}

/// The preview rendered as PNG (LightBurn's Save Preview Image).
async fn preview_png(State(st): State<Arc<AppState>>, Query(q): Query<PngQuery>) -> Result<Response, EngineError> {
    let width = q.width.unwrap_or(1600);
    if !(64..=8192).contains(&width) {
        return Err(EngineError::BadRequest("width must be within 64..=8192 px".into()));
    }
    let (svg, _) = st.preview_svg(RenderOptions { show_jumps: q.jumps.unwrap_or(true) }).await?;
    let png = tokio::task::spawn_blocking(move || crate::render::svg_to_png(&svg, width)).await??;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"preview.png\""),
        ],
        png,
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternBody {
    size_mm: f64,
    #[serde(default)]
    layer: Option<String>,
}

/// The lens calibration test pattern (a square with centre lines and diagonals, `lmc::lens_test_pattern`) added to
/// the document as one group, ready to request as a run (it fires only after the usual PIN approval).
async fn calibration_pattern(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: PatternBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let field = st.machine.profile().field_mm;
    if !(b.size_mm.is_finite() && b.size_mm >= 5.0 && b.size_mm < field * 0.95) {
        return Err(EngineError::BadRequest(format!("size_mm must be within 5..{} mm", field * 0.95)));
    }
    let mut invs: Vec<Invocation> = lmc::lens_test_pattern(b.size_mm)
        .iter()
        .enumerate()
        .map(|(i, poly)| {
            let mut d = String::new();
            for (k, (x, y)) in poly.iter().enumerate() {
                d.push_str(&format!("{}{x} {y} ", if k == 0 { "M" } else { "L" }));
            }
            let mut params = json!({ "svg_path_data": d.trim(), "name": format!("Lens test {}", i + 1) });
            if let Some(l) = &b.layer {
                params["layer"] = json!(l);
            }
            Invocation { id: "shape.path".into(), params }
        })
        .collect();
    let n = invs.len();
    let outcomes = st.blocking(move |st| st.batch(std::mem::take(&mut invs), Origin::Ui)).await?;
    let ids: Vec<Value> = outcomes.iter().filter_map(|o| o.result["created"].get(0).cloned()).collect();
    let group = st
        .blocking({
            let ids = ids.clone();
            move |st| st.execute(Invocation { id: "group.create".into(), params: json!({ "ids": ids, "name": "Lens calibration" }) }, Origin::Ui)
        })
        .await?;
    Ok(Json(json!({ "ok": true, "created": ids, "group": group.result, "paths": n, "size_mm": b.size_mm,
        "measure": ["top_mm", "bottom_mm", "middle_x_mm", "left_mm", "right_mm", "middle_y_mm", "diagonal_up_mm", "diagonal_down_mm"] })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolveBody {
    size_mm: f64,
    measurements: lmc::LensMeasurements,
    #[serde(default)]
    apply: bool,
    #[serde(default)]
    origin: Option<Value>,
}

/// New lens corrections from the measured test pattern (`lmc::solve_lens_calibration`) for the active device. With
/// `apply` (UI only) they are stored in the device's profile and the machine is set up again with them.
async fn calibration_solve(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: SolveBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let device = st.active_device();
    let profile = device.profile.clone().ok_or_else(|| EngineError::BadRequest("the active device has no machine profile".into()))?;
    let next = lmc::solve_lens_calibration(&profile.calibration, profile.field_mm, b.size_mm, &b.measurements)
        .map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let mut v = json!({ "ok": true, "device": device.id, "current": profile.calibration, "calibration": next, "applied": false });
    if b.apply {
        ui_only(&headers, b.origin.as_ref(), "device settings can be changed")?;
        let patch = json!({ "calibration": next });
        st.edit_device(&device.id, Some(patch), None).await?;
        v["applied"] = json!(true);
    }
    Ok(Json(v))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceEditBody {
    #[serde(default)]
    profile: Option<Value>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    origin: Option<Value>,
}

/// `PUT /api/devices/{id}` (registered in `api`): edit a device's settings.
pub(crate) async fn device_edit(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: DeviceEditBody = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "device settings can be changed")?;
    if b.profile.is_none() && b.display_name.is_none() {
        return Err(EngineError::BadRequest("give profile and/or display_name".into()));
    }
    Ok(Json(st.edit_device(&id, b.profile, b.display_name).await?))
}

async fn device_duplicate(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, EngineError> {
    let b: OriginOnly = body_or_default(&body)?;
    ui_only(&headers, b.origin.as_ref(), "devices can be added")?;
    let d = st.blocking(move |st| st.devices().duplicate(&id)).await?;
    st.emit(Event::DevicesChanged);
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "device": d.summary() })) ).into_response())
}

async fn device_lbdev(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Response, EngineError> {
    let (name, text) = st.devices().lbdev(&id)?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        text,
    )
        .into_response())
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    origin: Option<String>,
}

fn query_origin(q: &FileQuery) -> Option<Value> {
    q.origin.as_ref().map(|o| Value::String(o.clone()))
}

async fn device_cor_load(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, EngineError> {
    ui_only(&headers, query_origin(&q).as_ref(), "correction files can be changed")?;
    st.check_device_editable(&id)?;
    let name = q.name.clone().ok_or_else(|| EngineError::BadRequest("give ?name=<file>.cor".into()))?;
    let bytes = body.to_vec();
    let target = id.clone();
    let d = st.blocking(move |st| st.devices().set_cor(&target, Some((&name, &bytes)))).await?;
    st.device_settings_changed(&id).await
        .map(|mut v| {
            v["device"] = d.summary();
            Json(v)
        })
}

async fn device_cor_clear(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, EngineError> {
    ui_only(&headers, query_origin(&q).as_ref(), "correction files can be changed")?;
    st.check_device_editable(&id)?;
    let target = id.clone();
    st.blocking(move |st| st.devices().set_cor(&target, None)).await?;
    Ok(Json(st.device_settings_changed(&id).await?))
}

async fn device_import_markcfg(
    State(st): State<Arc<AppState>>,
    Query(q): Query<FileQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, EngineError> {
    ui_only(&headers, query_origin(&q).as_ref(), "devices can be added")?;
    let name = q.name.unwrap_or_default();
    let bytes = body.to_vec();
    let d = st.blocking(move |st| st.devices().import_markcfg(&bytes, &name)).await?;
    st.emit(Event::DevicesChanged);
    Ok((StatusCode::CREATED, Json(json!({ "ok": true, "device": d.summary() }))).into_response())
}
