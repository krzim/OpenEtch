//! The HTTP API (ARCHITECTURE.md "Engine API") plus `/mcp` and the static web UI, as one axum [`Router`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderName, StatusCode, header};
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
use crate::state::{FileFormat, parse_origin};
use crate::{AppState, EngineError, approve, events, mcp};

/// Largest request body (SVG imports can be large; the commands crate refuses imports over 20 MB).
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Largest file `POST /api/document/upload` accepts.
pub const MAX_UPLOAD_BYTES: usize = crate::projects::MAX_FILE_BYTES;

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
        .route("/projects", get(project_list))
        .route("/projects/save", post(project_save))
        .route("/projects/open", post(project_open))
        .route("/project", get(project_status))
        .route("/document/download", get(download))
        .route("/document/upload", post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)))
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
    drop(body);
    let origin = parse_origin(b.origin.as_ref())?;
    // Blocking thread: an import can take seconds.
    match st.blocking(move |st| st.execute(Invocation { id: b.id, params: b.params }, origin)).await {
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
    drop(body);
    let origin = parse_origin(b.origin.as_ref())?;
    match st.blocking(move |st| st.batch(b.commands, origin)).await {
        Ok(outs) => {
            let revision = outs.last().map(|o| o.revision).unwrap_or_else(|| st.revision());
            let results: Vec<&Value> = outs.iter().map(|o| &o.result).collect();
            Ok(Json(json!({ "ok": true, "results": results, "revision": revision })).into_response())
        }
        Err(EngineError::Command(e)) => Ok(command_failure(e)),
        Err(e) => Err(e),
    }
}

/// `POST /api/undo` and `/api/redo`: nothing to undo/redo is `200 {ok: false, error}`, like a failed command.
fn undo_redo_response(r: Result<u64, EngineError>) -> Response {
    match r {
        Ok(revision) => Json(json!({ "ok": true, "revision": revision })).into_response(),
        Err(EngineError::Command(e)) => command_failure(e),
        Err(e) => e.into_response(),
    }
}

async fn undo(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: OriginBody = body_or_default(&body)?;
    Ok(undo_redo_response(st.undo(parse_origin(b.origin.as_ref())?)))
}

async fn redo(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: OriginBody = body_or_default(&body)?;
    Ok(undo_redo_response(st.redo(parse_origin(b.origin.as_ref())?)))
}

async fn document(State(st): State<Arc<AppState>>) -> Json<doc::Document> {
    Json(st.document())
}

async fn history(State(st): State<Arc<AppState>>) -> Json<Vec<commands::HistoryEntry>> {
    Json(st.history())
}

async fn preview(State(st): State<Arc<AppState>>) -> Result<Response, EngineError> {
    let result = st.preview().await?;
    match &result.1 {
        Ok(p) => Ok(Json(p).into_response()),
        Err(e) => Err(EngineError::Plan(e.clone())),
    }
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
    Ok(Json(st.frame_stop().await?))
}

/// Emergency stop. Never gated (no token needed, see `security`).
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

async fn project_list(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    Ok(Json(json!(st.project_list().await?)))
}

async fn project_status(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(st.project_status())
}

/// Body of `POST /api/projects/save` and `/api/projects/open`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectBody {
    name: String,
    /// Save only: replace an existing project other than the current one (default true; the UI confirms first).
    #[serde(default)]
    overwrite: Option<bool>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn project_save(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: ProjectBody = body_required(&body)?;
    Ok(Json(st.project_save(&b.name, b.overwrite.unwrap_or(true)).await?))
}

async fn project_open(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: ProjectBody = body_required(&body)?;
    if b.overwrite.is_some() {
        return Err(EngineError::BadRequest("overwrite is only for saving".into()));
    }
    let origin = parse_origin(b.origin.as_ref())?;
    Ok(Json(st.project_open(&b.name, origin).await?))
}

/// Query of the download and upload routes, parsed by hand so every mistake is a JSON error.
type FileQuery = Result<Query<HashMap<String, String>>, QueryRejection>;

fn file_query(q: FileQuery, allowed: &[&str]) -> Result<HashMap<String, String>, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    if let Some(k) = q.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: {})", allowed.join(", "))));
    }
    Ok(q)
}

/// `name` made safe for a `Content-Disposition` header: an ASCII `filename` (other characters become `_`) and an
/// RFC 5987 `filename*` with the full UTF-8 name.
fn content_disposition(stem: &str, ext: &str) -> String {
    let stem: String = stem.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\')).collect();
    let stem = match stem.trim() {
        "" => "Untitled",
        s => s,
    };
    let file = format!("{stem}.{ext}");
    let ascii: String = file
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') { c } else { '_' })
        .collect();
    let mut encoded = String::new();
    for b in file.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// `GET /api/document/download?format=openetch|lbrn2` (default openetch): the document as a file. A LightBurn
/// export also sends `X-OpenEtch-Warnings: <count>` (the warnings themselves come from `doc.export_lbrn2`).
async fn download(State(st): State<Arc<AppState>>, q: FileQuery) -> Result<Response, EngineError> {
    let q = file_query(q, &["format"])?;
    let format = FileFormat::parse(q.get("format").map_or("openetch", String::as_str))?;
    let file = st.download(format)?;
    let content_type = match format {
        FileFormat::OpenEtch => "application/json; charset=utf-8",
        FileFormat::Lbrn2 => "application/xml; charset=utf-8",
    };
    let disposition = content_disposition(&file.stem, format.extension());
    let headers = [
        (header::CONTENT_TYPE, content_type.to_owned()),
        (header::CONTENT_DISPOSITION, disposition),
        (header::CACHE_CONTROL, "no-store".to_owned()),
        (HeaderName::from_static("x-openetch-warnings"), file.warnings.len().to_string()),
    ];
    Ok((headers, file.content).into_response())
}

/// `POST /api/document/upload?format=openetch|lbrn2&mode=replace|merge[&field_mm=150]` with the raw file as the
/// body (at most [`MAX_UPLOAD_BYTES`], UTF-8). Runs `doc.load` or `doc.import_lbrn2` with origin `ui`; answers like
/// `/api/execute` (a failing command is 200 `{ok: false, error}`).
async fn upload(
    State(st): State<Arc<AppState>>,
    q: FileQuery,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, EngineError> {
    let q = file_query(q, &["format", "mode", "field_mm"])?;
    let body = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => {
            EngineError::TooLarge(format!("the file is larger than {} MB", MAX_UPLOAD_BYTES / (1024 * 1024)))
        }
        _ => EngineError::BadRequest(format!("cannot read the upload: {e}")),
    })?;
    let format = FileFormat::parse(q.get("format").map(String::as_str).ok_or_else(|| {
        EngineError::BadRequest("the format query parameter is required (openetch or lbrn2)".into())
    })?)?;
    let merge = match q.get("mode").map(String::as_str) {
        None | Some("replace") => false,
        Some("merge") => true,
        Some(other) => return Err(EngineError::BadRequest(format!("unknown mode '{other}' (replace or merge)"))),
    };
    let field_mm = match q.get("field_mm") {
        None => None,
        // `parse` accepts "NaN" and "inf", which would become JSON null (the document's field) downstream.
        Some(v) => match v.parse::<f64>() {
            Ok(f) if f.is_finite() => Some(f),
            _ => return Err(EngineError::BadRequest(format!("field_mm '{v}' is not a finite number"))),
        },
    };
    if body.is_empty() {
        return Err(EngineError::BadRequest("the upload is empty".into()));
    }
    // Large files are converted and imported on a blocking thread; the body is moved, not copied.
    let result = st
        .blocking(move |st| {
            // No copy when the body buffer is not shared (the usual case).
            let mut text = String::from_utf8(Vec::from(body)).map_err(|_| {
                EngineError::BadRequest("the file is not UTF-8 text (expected .openetch JSON or .lbrn2 XML)".into())
            })?;
            // LightBurn and some editors write a byte-order mark.
            if text.starts_with('\u{feff}') {
                text.drain(..'\u{feff}'.len_utf8());
            }
            st.upload(format, merge, field_mm, text, commands::Origin::Ui)
        })
        .await;
    match result {
        Ok(o) => Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })).into_response()),
        Err(EngineError::Command(e)) => Ok(command_failure(e)),
        Err(e) => Err(e),
    }
}

/// Body of `POST /api/runs/{id}/approve`.
#[derive(Debug, Default, Deserialize)]
struct ApproveBody {
    /// The operator PIN (`openetch set-pin`).
    #[serde(default)]
    pin: Option<String>,
    #[serde(default)]
    origin: Option<Value>,
}

/// Approval by a person: `{"pin": "<operator PIN>"}`. Refuses anything that looks like an MCP client or names a
/// non-UI origin; PIN refusals are 403 with `code` (`pin_not_set`, `pin_required`, `pin_invalid`, `pin_locked`).
async fn approve_run(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, EngineError> {
    if has_mcp_marker(&headers) {
        tracing::warn!(run_id = %id, "refused run approval from an MCP client");
        return Err(EngineError::Forbidden("runs can only be approved by a person in the OpenEtch UI".into()));
    }
    let b: ApproveBody = body_or_default(&body)?;
    if parse_origin(b.origin.as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("runs can only be approved from the UI".into()));
    }
    Ok(Json(approve::approve(&st, &id, b.pin.as_deref()).await?))
}
