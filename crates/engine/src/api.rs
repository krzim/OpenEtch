//! The HTTP API (ARCHITECTURE.md "Engine API") plus `/mcp` and the static web UI, as one axum [`Router`].

use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use commands::{CommandError, Invocation};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

use crate::frame::FrameRequest;
use crate::render::RenderOptions;
use crate::security::{self, has_mcp_marker};
use crate::state::parse_origin;
use crate::{AppState, EngineError, approve, events, mcp};

/// Largest request body (SVG imports can be large; the commands crate refuses imports over 20 MB).
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

/// The whole app: `/api/*`, `/mcp`, and `web_dir` (with SPA fallback to `index.html`) when given, all behind
/// [`security::guard`].
pub fn router(state: Arc<AppState>, web_dir: Option<&Path>) -> Router {
    let api = Router::new()
        .route("/commands", get(commands))
        .route("/execute", post(execute))
        .route("/batch", post(batch))
        .route("/undo", post(undo))
        .route("/redo", post(redo))
        .route("/document", get(document))
        .route("/history", get(history))
        .route("/preview", get(preview))
        .route("/preview.svg", get(preview_svg))
        .route("/machine", get(machine_status))
        .route("/machine/frame", post(frame_start))
        .route("/machine/frame/stop", post(frame_stop))
        .route("/machine/stop", post(machine_stop))
        .route("/runs", post(request_run))
        .route("/runs/{id}/approve", post(approve_run))
        .route("/events", get(events::events_ws))
        .fallback(api_not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES));
    let mut app = Router::new().nest("/api", api).nest_service("/mcp", mcp::service(state.clone()));
    if let Some(dir) = web_dir {
        let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        app = app.fallback_service(spa);
    }
    app.with_state(state.clone()).layer(middleware::from_fn_with_state(state, security::guard))
}

async fn api_not_found() -> EngineError {
    EngineError::NotFound("no such API route".into())
}

/// Parse a JSON body; an empty body means `T::default()`.
fn body_or_default<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, EngineError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

/// Parse a required JSON body.
fn body_required<T: DeserializeOwned>(body: &Bytes) -> Result<T, EngineError> {
    serde_json::from_slice(body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

/// A command failure as `200 {ok: false, error}` (the contract's shape for `/api/execute` and `/api/batch`).
fn command_failure(e: CommandError) -> Response {
    Json(EngineError::Command(e).body()).into_response()
}

async fn commands(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(st.specs()))
}

#[derive(Debug, Deserialize)]
struct ExecuteBody {
    id: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    origin: Option<Value>,
}

async fn execute(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: ExecuteBody = body_required(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    match st.execute(Invocation { id: b.id, params: b.params }, origin) {
        Ok(o) => Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })).into_response()),
        Err(EngineError::Command(e)) => Ok(command_failure(e)),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Deserialize)]
struct BatchBody {
    commands: Vec<Invocation>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn batch(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: BatchBody = body_required(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    match st.batch(b.commands, origin) {
        Ok(outs) => {
            let revision = outs.last().map(|o| o.revision).unwrap_or_else(|| st.revision());
            let results: Vec<&Value> = outs.iter().map(|o| &o.result).collect();
            Ok(Json(json!({ "ok": true, "results": results, "revision": revision })).into_response())
        }
        Err(EngineError::Command(e)) => Ok(command_failure(e)),
        Err(e) => Err(e),
    }
}

async fn undo(State(st): State<Arc<AppState>>) -> Response {
    match st.undo() {
        Ok(revision) => Json(json!({ "ok": true, "revision": revision })).into_response(),
        Err(EngineError::Command(e)) => command_failure(e),
        Err(e) => e.into_response(),
    }
}

async fn redo(State(st): State<Arc<AppState>>) -> Response {
    match st.redo() {
        Ok(revision) => Json(json!({ "ok": true, "revision": revision })).into_response(),
        Err(EngineError::Command(e)) => command_failure(e),
        Err(e) => e.into_response(),
    }
}

async fn document(State(st): State<Arc<AppState>>) -> Json<doc::Document> {
    Json(st.document())
}

async fn history(State(st): State<Arc<AppState>>) -> Json<Vec<commands::HistoryEntry>> {
    Json(st.history())
}

async fn preview(State(st): State<Arc<AppState>>) -> Result<Json<plan::Preview>, EngineError> {
    let (_, preview) = st.preview().await?;
    Ok(Json(preview?))
}

#[derive(Debug, Default, Deserialize)]
struct SvgQuery {
    /// `?jumps=false` hides travel moves.
    jumps: Option<bool>,
}

async fn preview_svg(State(st): State<Arc<AppState>>, Query(q): Query<SvgQuery>) -> Result<Response, EngineError> {
    let opts = RenderOptions { show_jumps: q.jumps.unwrap_or(true) };
    let (svg, _) = st.preview_svg(opts).await?;
    Ok(([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "no-store")], svg).into_response())
}

async fn machine_status(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(st.machine_status())
}

async fn frame_start(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let req: FrameRequest = body_or_default(&body)?;
    Ok(Json(st.frame_start(req).await?))
}

async fn frame_stop(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    st.frame_stop().await?;
    Ok(Json(json!({ "ok": true })))
}

async fn machine_stop(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    Ok(Json(st.stop().await?))
}

#[derive(Debug, Default, Deserialize)]
struct OriginBody {
    #[serde(default)]
    origin: Option<Value>,
}

async fn request_run(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: OriginBody = body_or_default(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    let view = st.request_run(origin).await?;
    let body = json!({ "ok": true, "id": view.id, "run": view, "message": crate::state::APPROVAL_MESSAGE });
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// UI-only approval. Refuses anything that looks like an MCP client or names a non-UI origin.
async fn approve_run(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, EngineError> {
    if has_mcp_marker(&headers) {
        tracing::warn!(run_id = %id, "refused run approval from an MCP client");
        return Err(EngineError::Forbidden("runs can only be approved by a person in the galvo-studio UI".into()));
    }
    let b: OriginBody = body_or_default(&body)?;
    if parse_origin(b.origin.as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("runs can only be approved from the UI".into()));
    }
    Ok(Json(approve::approve(&st, &id).await?))
}
