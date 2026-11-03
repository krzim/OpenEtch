//! Routes of the job log and the job ledger (module `ledger`; merged into `api::router`). Reads and verdicts are open
//! to every client (agents read and mark the ledger over MCP too); the storage settings are UI-only. Nothing here
//! approves or starts a run: Re-run opens the burned document and requests a *pending* run, which a person starts.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/job-log` | `?limit=` (default 200, at most 2000), `?q=` (every word in project, document, device, summary, layers and settings, variable values, note, error), `?verdict=ok\|scrap\|unmarked`, `?state=`, `?device=`, `?project=`, `?since_unix_s=`, `?until_unix_s=`, `?format=csv` → `{entries: [Entry]}` newest first (`Entry`: the job log line plus `id`, `verdict`, `note`, `noted_unix_s`, `noted_by`, `thumbnail`, `rerun`, `rerun_reason?`), or the CSV |
//! | GET | `/api/job-log/{id}` | one `Entry` |
//! | GET | `/api/job-log/{id}/thumbnail.png` | the thumbnail (404 when not kept) |
//! | POST | `/api/job-log/{id}/mark` | `{verdict: "ok"\|"scrap"\|null, note?, origin?}` → the `Entry` |
//! | POST | `/api/job-log/{id}/open` | `{origin?}` → `{ok, id, document_id, revision, device_mismatch?}`: the burned document opened as a new, active document |
//! | POST | `/api/job-log/{id}/rerun` | `{origin?}` → as `open` plus `run` (a pending run with the stored options) or `run_error` |
//! | GET / PUT | `/api/job-ledger/settings` | `{settings: {keep_documents, max_document_mb, keep_thumbnails, thumbnail_px}, usage: {thumbnails, thumbnail_bytes, documents, document_bytes}}`; PUT the settings (UI only) |

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commands::Origin;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ledger::{self, LedgerQuery, LedgerSettings, Verdict};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/job-log", get(list))
        .route("/job-log/{id}", get(one))
        .route("/job-log/{id}/thumbnail.png", get(thumbnail))
        .route("/job-log/{id}/mark", post(mark))
        .route("/job-log/{id}/open", post(open))
        .route("/job-log/{id}/rerun", post(rerun))
        .route("/job-ledger/settings", get(settings_view).put(settings_edit))
}

fn body<T: serde::de::DeserializeOwned>(b: &Bytes) -> Result<T, EngineError> {
    let b: &[u8] = if b.iter().all(u8::is_ascii_whitespace) { b"{}" } else { b };
    serde_json::from_slice(b).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

/// The request's origin; MCP client headers make a `ui` claim an agent's.
fn origin_of(headers: &HeaderMap, v: Option<&Value>) -> Result<Origin, EngineError> {
    let o = parse_origin(v)?;
    if has_mcp_marker(headers) && o == Origin::Ui {
        return Ok(Origin::Agent(crate::mcp::MCP_AGENT.into()));
    }
    Ok(o)
}

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    verdict: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    device: Option<String>,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    since_unix_s: Option<u64>,
    #[serde(default)]
    until_unix_s: Option<u64>,
}

async fn list(State(st): State<Arc<AppState>>, Query(x): Query<ListQuery>) -> Result<Response, EngineError> {
    let q = LedgerQuery {
        q: x.q,
        verdict: x.verdict,
        state: x.state,
        device: x.device,
        project: x.project,
        since_unix_s: x.since_unix_s,
        until_unix_s: x.until_unix_s,
        limit: x.limit,
    };
    let st2 = st.clone();
    let entries = tokio::task::spawn_blocking(move || ledger::search(&st2, &q)).await??;
    match x.format.as_deref() {
        None | Some("json") => Ok(Json(json!({ "entries": entries })).into_response()),
        Some("csv") => {
            // Oldest first, as the file is written.
            let notes: std::collections::HashMap<String, (String, String)> = entries
                .iter()
                .map(|v| {
                    let verdict = match v.verdict {
                        Some(Verdict::Ok) => "ok",
                        Some(Verdict::Scrap) => "scrap",
                        None => "",
                    };
                    (v.id.clone(), (verdict.to_string(), v.note.clone().unwrap_or_default()))
                })
                .collect();
            let rows: Vec<_> = entries.into_iter().rev().map(|v| v.entry).collect();
            let csv = crate::joblog::to_csv_with(&rows, &|id| notes.get(id).cloned());
            Ok(attachment("OpenEtchJobs.csv", "text/csv", csv.into_bytes()))
        }
        Some(f) => Err(EngineError::BadRequest(format!("unknown format '{f}' (json or csv)"))),
    }
}

fn attachment(name: &str, content_type: &str, body: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response()
}

async fn one(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    let v = tokio::task::spawn_blocking(move || ledger::get(&st, &id)).await??;
    Ok(Json(serde_json::to_value(v).unwrap_or(Value::Null)))
}

async fn thumbnail(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Response, EngineError> {
    ledger::check_id(&id)?;
    let png = st
        .libs
        .job_ledger(|l| l.thumbnail(&id))
        .ok_or_else(|| EngineError::NotFound(format!("no thumbnail of '{id}'")))?;
    // Never changes once written (a newer run has another id).
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "private, max-age=86400")], png).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkBody {
    #[serde(default)]
    verdict: Option<Verdict>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn mark(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: MarkBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    let v = tokio::task::spawn_blocking(move || ledger::mark(&st, &id, b.verdict, b.note.as_deref().unwrap_or_default(), &origin))
        .await??;
    Ok(Json(serde_json::to_value(v).unwrap_or(Value::Null)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginBody {
    #[serde(default)]
    origin: Option<Value>,
}

async fn open(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: OriginBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    Ok(Json(ledger::open_document(&st, &id, origin).await?))
}

async fn rerun(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: OriginBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    Ok(Json(ledger::rerun(&st, &id, origin).await?))
}

fn settings_json(st: &AppState) -> Value {
    st.libs.job_ledger(|l| json!({ "settings": l.settings(), "usage": l.usage() }))
}

async fn settings_view(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    Ok(Json(tokio::task::spawn_blocking(move || settings_json(&st)).await?))
}

async fn settings_edit(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let mut v: Value = body(&b)?;
    let origin = v.as_object_mut().and_then(|m| m.remove("origin"));
    if has_mcp_marker(&headers) || parse_origin(origin.as_ref())? != Origin::Ui {
        return Err(EngineError::Forbidden("the job ledger's storage settings can be changed only by a person in the OpenEtch UI".into()));
    }
    // A partial body changes only what it names.
    let mut merged = serde_json::to_value(st.libs.job_ledger(|l| l.settings())).unwrap_or(Value::Null);
    if let (Some(m), Some(patch)) = (merged.as_object_mut(), v.as_object()) {
        for (k, x) in patch {
            m.insert(k.clone(), x.clone());
        }
    }
    let s: LedgerSettings = serde_json::from_value(merged).map_err(|e| EngineError::BadRequest(format!("bad settings: {e}")))?;
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || st2.libs.job_ledger(|l| l.set_settings(s))).await??;
    Ok(Json(settings_json(&st)))
}
