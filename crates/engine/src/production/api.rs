//! The production routes (merged into `api::router`). Reads are open to every client (MCP has its own read-only
//! tools for them); everything that changes a setting, switches the extractor, locks the design or continues a run
//! is UI-only (MCP client headers and non-`ui` origins are refused, 403).
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/production` | the cockpit's state ([`super::status_json`]): job, foot switch cycle, slots and the next values, layer pause, cycle times, maintenance, extractor, the ledger's last entries, the design lock |
//! | POST | `/api/production/lock` | `{on: bool, holder, label?, force?}`: take or renew (on) or release the cockpit's design lock (UI only; see `lock`) |
//! | POST | `/api/job/continue` | `{run_id?}`: continue a run waiting at a layer (UI only, like Resume; the foot switch continues too) |
//! | GET | `/api/production/ledger` | `?limit=` (default 200), `?run=`, `?format=csv`: the serial ledger |
//! | POST | `/api/production/remark` | `{run_id, time_unix_s?, slot, revision?}` → **201** a pending run re-marking that slot with its recorded value (approved like every run) |
//! | GET | `/api/production/meter` | the active device's hour meter and reminders |
//! | PUT | `/api/production/maintenance` | `{reminders: [{id?, name, every_hours?, every_runs?}]}` for the active device (UI only) |
//! | POST | `/api/production/maintenance/{id}/done` | the reminder was done now (UI only) |
//! | GET | `/api/production/extractor` | the active device's extractor settings and state |
//! | PUT | `/api/production/extractor` | the settings (`extractor::Config`; UI only) |
//! | POST | `/api/production/extractor/switch` | `{on}`: switch it by hand (UI only) |
//! | GET | `/api/notifications` | the targets (tokens never included: `token_set`), the event kinds, the last delivery per target |
//! | PUT | `/api/notifications` | `{waiting_delay_s?, targets: [...]}` (UI only; `token` absent keeps a target's token, `""` clears it) |
//! | POST | `/api/notifications/test` | `{id}`: send a test event to that target now → `{ok, result}` (UI only) |
//! | GET | `/api/notifications/events` | `?after=<seq>&limit=&wait_s=`: the event feed after `after`, waiting up to `wait_s` (at most 25) for news |

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commands::Origin;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{extractor, ledger, meter, notify};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError, Event};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/production", get(status))
        .route("/production/lock", post(lock_route))
        .route("/job/continue", post(continue_run))
        .route("/production/ledger", get(ledger_read))
        .route("/production/remark", post(remark))
        .route("/production/meter", get(meter_read))
        .route("/production/maintenance", axum::routing::put(maintenance_set))
        .route("/production/maintenance/{id}/done", post(maintenance_done))
        .route("/production/extractor", get(extractor_read).put(extractor_set))
        .route("/production/extractor/switch", post(extractor_switch))
        .route("/notifications", get(notifications_read).put(notifications_set))
        .route("/notifications/test", post(notifications_test))
        .route("/notifications/events", get(events_read))
}

fn body<T: DeserializeOwned>(b: &Bytes) -> Result<T, EngineError> {
    let b: &[u8] = if b.iter().all(u8::is_ascii_whitespace) { b"{}" } else { b };
    serde_json::from_slice(b).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
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

async fn status(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(super::status_json(&st))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockBody {
    on: bool,
    #[serde(default)]
    holder: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    origin: Option<Value>,
}

async fn lock_route(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: LockBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the design lock can be changed")?;
    let holder = b.holder.unwrap_or_default();
    let valid = |s: &str, max: usize| s.chars().count() <= max && !s.chars().any(char::is_control);
    if b.on {
        if holder.is_empty() || !valid(&holder, 64) {
            return Err(EngineError::BadRequest("holder must be 1-64 printable characters".into()));
        }
        let label = b.label.unwrap_or_default();
        if !valid(&label, 60) {
            return Err(EngineError::BadRequest("label must be at most 60 printable characters".into()));
        }
        let was = st.production.lock.locked();
        st.production.lock.renew(&holder, label.trim());
        if !was {
            super::changed(&st, "lock");
        }
    } else if st.production.lock.release((!holder.is_empty()).then_some(holder.as_str()), b.force) {
        super::changed(&st, "lock");
    }
    Ok(Json(st.production.lock.status_json()))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueBody {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn continue_run(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: ContinueBody = body(&b)?;
    // Continuing fires the laser again: only a person, as with Resume.
    ui_only(&headers, b.origin.as_ref(), "a run waiting at a layer can be continued")?;
    let id = st.production.pause.request_continue(b.run_id.as_deref())?;
    tracing::warn!(run_id = %id, "continue after a layer pause (UI)");
    Ok(Json(json!({ "ok": true, "run_id": id })))
}

#[derive(Debug, Default, Deserialize)]
struct LedgerQuery {
    limit: Option<usize>,
    run: Option<String>,
    format: Option<String>,
}

async fn ledger_read(State(st): State<Arc<AppState>>, Query(q): Query<LedgerQuery>) -> Result<Response, EngineError> {
    let mut entries = st.production.ledger.read(q.limit.unwrap_or(200))?;
    if let Some(run) = &q.run {
        entries.retain(|e| &e.run_id == run || e.remark_of.as_ref().is_some_and(|r| &r.run_id == run));
    }
    match q.format.as_deref() {
        None | Some("json") => Ok(Json(json!({ "entries": entries })).into_response()),
        Some("csv") => Ok((
            [
                (header::CONTENT_TYPE, "text/csv".to_string()),
                (header::CONTENT_DISPOSITION, "attachment; filename=\"OpenEtchSerialLedger.csv\"".to_string()),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            ledger::to_csv(&entries),
        )
            .into_response()),
        Some(f) => Err(EngineError::BadRequest(format!("unknown format '{f}' (json or csv)"))),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemarkBody {
    run_id: String,
    #[serde(default)]
    time_unix_s: Option<u64>,
    slot: usize,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn remark(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Response, EngineError> {
    let b: RemarkBody = body(&b)?;
    let mut origin = parse_origin(b.origin.as_ref())?;
    if has_mcp_marker(&headers) && origin == Origin::Ui {
        origin = Origin::Agent(crate::mcp::MCP_AGENT.into());
    }
    let v = super::remark::request(&st, &b.run_id, b.time_unix_s, b.slot, origin, b.revision).await?;
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

async fn meter_read(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(meter::status_json(&st, &st.active_device().id))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceBody {
    reminders: Vec<meter::ReminderInput>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn maintenance_set(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: MaintenanceBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "maintenance reminders can be changed")?;
    let device = st.active_device().id;
    st.production.meter.set_reminders(&device, b.reminders)?;
    super::changed(&st, "maintenance");
    Ok(Json(meter::status_json(&st, &device)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginBody {
    #[serde(default)]
    origin: Option<Value>,
}

async fn maintenance_done(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    b: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: OriginBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "a reminder can be marked done")?;
    let device = st.active_device().id;
    st.production.meter.done(&device, &id)?;
    super::changed(&st, "maintenance");
    Ok(Json(meter::status_json(&st, &device)))
}

async fn extractor_read(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(extractor::status_json(&st, &st.active_device().id))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractorBody {
    config: extractor::Config,
    #[serde(default)]
    origin: Option<Value>,
}

async fn extractor_set(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: ExtractorBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the extractor settings can be changed")?;
    let device = st.active_device().id;
    st.production.extractor.set_config(&device, b.config)?;
    super::changed(&st, "extractor");
    Ok(Json(extractor::status_json(&st, &device)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchBody {
    on: bool,
    #[serde(default)]
    origin: Option<Value>,
}

async fn extractor_switch(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: SwitchBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the extractor can be switched")?;
    extractor::set(&st, b.on, None).await?;
    Ok(Json(extractor::status_json(&st, &st.active_device().id)))
}

async fn notifications_read(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(st.production.notify.view())
}

async fn notifications_set(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: notify::ConfigInput = body(&b)?;
    // A target is where the feed goes: an agent must not be able to point it anywhere.
    ui_only(&headers, b.origin.as_ref(), "notification targets can be changed")?;
    let v = st.production.notify.replace(b)?;
    st.emit(Event::machine("production", Some("notifications".into())));
    Ok(Json(v))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestBody {
    id: String,
    #[serde(default)]
    origin: Option<Value>,
}

async fn notifications_test(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: TestBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "a notification target can be tested")?;
    let target = notify::target(&st, &b.id).ok_or_else(|| EngineError::NotFound(format!("no notification target '{}'", b.id)))?;
    let st2 = st.clone();
    let r = tokio::task::spawn_blocking(move || notify::test_target(&st2, &target)).await?;
    Ok(Json(match r {
        Ok(code) => json!({ "ok": true, "result": format!("HTTP {code}") }),
        Err(e) => json!({ "ok": false, "error": e }),
    }))
}

#[derive(Debug, Default, Deserialize)]
struct EventsQuery {
    after: Option<u64>,
    limit: Option<usize>,
    wait_s: Option<u64>,
}

async fn events_read(State(st): State<Arc<AppState>>, Query(q): Query<EventsQuery>) -> Json<Value> {
    let wait = Duration::from_secs(q.wait_s.unwrap_or(0).min(notify::MAX_WAIT_S));
    let (events, last) = st.production.notify.feed_wait(q.after.unwrap_or(0), q.limit.unwrap_or(50).min(notify::FEED_LEN), wait).await;
    Json(json!({ "events": events, "last_seq": last }))
}
