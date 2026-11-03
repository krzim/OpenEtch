//! The HTTP API (ARCHITECTURE.md "Engine API") plus `/mcp` and the static web UI, as one axum [`Router`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use commands::{CommandError, Invocation};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

use crate::frame::FrameRequest;
use crate::pin::ApprovalMode;
use crate::render::RenderOptions;
use crate::security::{self, has_mcp_marker};
use crate::state::{FileFormat, UploadFormat, UploadOptions, parse_origin};
use crate::{AppState, EngineError, approve, events, mcp};

/// Largest request body. Large files go through `POST /api/document/upload` (50 MB, the commands' SVG limit too).
pub const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Largest file `POST /api/document/upload` accepts.
pub const MAX_UPLOAD_BYTES: usize = crate::projects::MAX_FILE_BYTES;

/// The whole app: `/api/*`, `/mcp`, and `web_dir` (see [`serve_web`]) when given, all behind [`security::guard`].
pub fn router(state: Arc<AppState>, web_dir: Option<&Path>) -> Router {
    let api = Router::new()
        .route("/commands", get(commands))
        .route("/execute", post(execute))
        .route("/batch", post(batch))
        .route("/undo", post(undo))
        .route("/redo", post(redo))
        .route("/document", get(document))
        .route("/document/sync", get(document_sync))
        .route("/history", get(history))
        .route("/history/goto", post(history_goto))
        .route("/history/delete", post(history_delete))
        .route("/preview", get(preview))
        .route("/preview.bin", get(preview_bin))
        .route("/preview/stats", get(preview_stats))
        // What the browser plans the display preview with (crates/webcore; module preview_context).
        .route("/preview/context", get(crate::preview_context::handler))
        .route("/fill_regions", get(fill_regions))
        .route("/preview.svg", get(preview_svg))
        .route("/machine", get(machine_status))
        .route("/machine/frame", post(frame_start))
        .route("/machine/frame/stop", post(frame_stop))
        .route("/machine/stop", post(machine_stop))
        .route("/runs", post(request_run))
        .route("/runs/{id}/approve", post(approve_run))
        .route("/job/start", post(start_job))
        .route("/events", get(events::events_ws))
        .route("/projects", get(project_list))
        .route("/projects/save", post(project_save))
        .route("/projects/open", post(project_open))
        .route("/project", get(project_status))
        .route("/document/download", get(download))
        .route("/document/upload", post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)))
        .route("/lightburn", get(lightburn))
        .route(
            "/lightburn/prefs",
            post(lightburn_upload)
                .delete(lightburn_delete)
                .layer(DefaultBodyLimit::max(crate::lightburn::MAX_PREFS_UPLOAD_BYTES)),
        )
        .route("/devices", get(devices_list))
        .route(
            "/devices/import",
            post(devices_import).layer(DefaultBodyLimit::max(crate::lightburn::MAX_PREFS_UPLOAD_BYTES)),
        )
        .route("/devices/{id}", get(device_details).delete(device_remove).put(crate::api_machine::device_edit))
        .route("/devices/{id}/activate", post(device_activate))
        .merge(crate::api_machine::routes())
        .merge(crate::ruida_dsp::routes())
        .merge(crate::api_libs::routes())
        .merge(crate::userfiles::routes())
        .merge(crate::documents::routes())
        .merge(crate::artlib::routes())
        .merge(crate::folders::routes())
        .merge(crate::test_presets::routes())
        .fallback(api_not_found)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), attach_sync))
        .layer(compression());
    let mut app = Router::new().nest("/api", api).nest_service("/mcp", mcp::service(state.clone()));
    if let Some(dir) = web_dir {
        let dir: Arc<PathBuf> = Arc::new(dir.to_path_buf());
        app = app.fallback(move |req: Request<Body>| {
            let dir = dir.clone();
            async move { serve_web(&dir, req).await }
        });
    }
    app.with_state(state.clone()).layer(middleware::from_fn_with_state(state, security::guard))
}

/// `Cache-Control` for the hashed bundles under `/assets/` (their names change with their content).
pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// `Cache-Control` for everything else the UI is made of (`index.html`, the manifest, icons, source maps): always
/// revalidated (`Last-Modified`), so an app window, which has no reload button, never keeps a stale page.
pub const CACHE_REVALIDATE: &str = "no-cache";

/// A request for a page of the single-page app (`/`, `/editor/x`): a GET or HEAD whose last path segment has no
/// file extension, outside `/assets/`. Only these fall back to `index.html`; a missing `/assets/x.js`, `favicon.ico`
/// or `manifest.webmanifest` is a 404, never a page that a script tag or the browser would take for the file.
pub fn is_navigation(method: &Method, path: &str) -> bool {
    (method == Method::GET || method == Method::HEAD)
        && !path.starts_with("/assets/")
        && !path.rsplit('/').next().unwrap_or_default().contains('.')
}

/// The `Cache-Control` for a static answer (`served` is the path of the file actually sent), or `None` for failures,
/// which are never cached.
pub fn static_cache_control(served: &str, status: StatusCode) -> Option<&'static str> {
    if !(status.is_success() || status == StatusCode::NOT_MODIFIED) {
        return None;
    }
    Some(if served.starts_with("/assets/") && !served.ends_with(".map") { CACHE_IMMUTABLE } else { CACHE_REVALIDATE })
}

/// Serve the built web UI from `dir`: files as they are, with the `.br`/`.gz` copies written at build time (web/
/// vite.config.ts) when the browser accepts them (`Content-Encoding`, `Vary: Accept-Encoding`); `index.html` for
/// page navigations only ([`is_navigation`]); `Cache-Control` from [`static_cache_control`].
pub async fn serve_web(dir: &Path, req: Request<Body>) -> Response {
    let path = req.uri().path().to_owned();
    let navigation = is_navigation(req.method(), &path);
    let (method, headers) = (req.method().clone(), req.headers().clone());
    let served = ServeDir::new(dir).precompressed_br().precompressed_gzip().try_call(req).await;
    let (mut res, mut file) = match served {
        Ok(r) => (r.map(Body::new), path.clone()),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("cannot read the web UI: {e}")).into_response(),
    };
    if res.status() == StatusCode::NOT_FOUND && navigation {
        let mut index = Request::new(Body::empty());
        *index.method_mut() = method;
        *index.headers_mut() = headers;
        res = match ServeFile::new(dir.join("index.html")).precompressed_br().precompressed_gzip().try_call(index).await {
            Ok(r) => r.map(Body::new),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("cannot read the web UI: {e}")).into_response(),
        };
        file = "/index.html".to_owned();
    }
    if let Some(cc) = static_cache_control(&file, res.status()) {
        res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
        res.headers_mut().append(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    res
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

async fn document(State(st): State<Arc<AppState>>) -> Result<Json<doc::Document>, EngineError> {
    // Blocking thread: a large document takes a while to copy.
    Ok(Json(st.blocking(|st| Ok(st.document_for_client())).await?))
}

/// Query of `GET /api/document/sync`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncQuery {
    /// The revision the client holds (none: send the whole document).
    since: Option<u64>,
    /// The engine run that revision is from (`epoch` of an earlier answer).
    epoch: Option<String>,
}

/// `GET /api/document/sync?since=<revision>&epoch=<epoch>`: the changes since the client's revision, or the whole
/// document, with the timeline and project status (module `docsync`).
async fn document_sync(
    State(st): State<Arc<AppState>>,
    q: Result<Query<SyncQuery>, QueryRejection>,
) -> Result<Response, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    let since = q.since.zip(q.epoch).map(|(r, e)| (e, r));
    let body = st.blocking(move |st| sync_json(st, since)).await?;
    Ok(([(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], body).into_response())
}

fn sync_json(st: &AppState, since: Option<(String, u64)>) -> Result<Vec<u8>, EngineError> {
    serde_json::to_vec(&st.doc_sync(since)).map_err(|e| EngineError::Internal(e.to_string()))
}

/// Header with which a client asks for the document's changes in a mutating answer: `<epoch>:<revision>`.
pub const SYNC_HEADER: &str = "x-openetch-since";

/// A `POST` carrying [`SYNC_HEADER`] whose answer is a JSON object gets a `sync` member: `GET /api/document/sync`'s
/// answer for that revision, taken after the request ran (module `docsync`). Saves the client a request (and a round
/// trip over Wi-Fi) after every edit.
async fn attach_sync(State(st): State<Arc<AppState>>, req: Request<Body>, next: middleware::Next) -> Response {
    let since = (req.method() == Method::POST)
        .then(|| req.headers().get(SYNC_HEADER).and_then(|v| v.to_str().ok()).and_then(crate::docsync::parse_since))
        .flatten();
    let res = next.run(req).await;
    let Some(since) = since else { return res };
    let json = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if !json || res.headers().contains_key(header::CONTENT_ENCODING) {
        return res;
    }
    let (mut parts, body) = res.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "cannot read the answer").into_response();
    };
    let sync = match st.blocking(move |st| sync_json(st, Some(since))).await {
        Ok(s) => s,
        Err(_) => return Response::from_parts(parts, Body::from(bytes)),
    };
    match splice_member(&bytes, "sync", &sync) {
        Some(out) => {
            parts.headers.remove(header::CONTENT_LENGTH);
            Response::from_parts(parts, Body::from(out))
        }
        None => Response::from_parts(parts, Body::from(bytes)),
    }
}

/// `object` (JSON text of an object) with member `name: value` added at the end, or `None` when it is not an object.
pub fn splice_member(object: &[u8], name: &str, value: &[u8]) -> Option<Vec<u8>> {
    let start = object.iter().position(|b| !b.is_ascii_whitespace())?;
    let end = object.iter().rposition(|b| !b.is_ascii_whitespace())?;
    if object[start] != b'{' || object[end] != b'}' || end == start {
        return None;
    }
    let empty = object[start + 1..end].iter().all(u8::is_ascii_whitespace);
    let mut out = Vec::with_capacity(object.len() + value.len() + name.len() + 6);
    out.extend_from_slice(&object[..end]);
    if !empty {
        out.push(b',');
    }
    out.push(b'"');
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\":");
    out.extend_from_slice(value);
    out.push(b'}');
    Some(out)
}

/// Compression of API answers (gzip or brotli, as the client accepts) at a fast level: a large document or preview
/// is mostly repeated keys and numbers, a tenth of its size or less compressed, and Wi-Fi to a Raspberry Pi moves a
/// few MB/s. Small answers, images and event streams are sent as they are; answers that already carry a
/// `Content-Encoding` (the binary preview compresses its own, once per revision) are left alone.
fn compression() -> tower_http::compression::CompressionLayer<impl tower_http::compression::Predicate> {
    use tower_http::compression::predicate::{NotForContentType, Predicate as _, SizeAbove};
    use tower_http::compression::{CompressionLayer, CompressionLevel};
    let predicate = SizeAbove::new(COMPRESS_ABOVE_BYTES)
        .and(NotForContentType::GRPC)
        .and(NotForContentType::IMAGES)
        .and(NotForContentType::SSE);
    CompressionLayer::new().quality(CompressionLevel::Precise(COMPRESSION_LEVEL)).compress_when(predicate)
}

/// Answers smaller than this are sent uncompressed (the headers would cost more than they save).
pub const COMPRESS_ABOVE_BYTES: u16 = 1024;
/// Brotli quality / gzip level of API answers: fast enough for a Raspberry Pi to keep ahead of Wi-Fi.
pub const COMPRESSION_LEVEL: i32 = 3;

async fn history(State(st): State<Arc<AppState>>) -> Json<commands::HistoryView> {
    Json(st.history_view())
}

/// Body of `POST /api/history/goto`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GotoBody {
    /// Index of the last step to apply; 0 rolls back everything.
    marker: usize,
    #[serde(default)]
    origin: Option<Value>,
}

/// Body of `POST /api/history/delete`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteStepBody {
    /// 1-based index of the step to delete.
    index: usize,
    #[serde(default)]
    origin: Option<Value>,
}

/// `POST /api/history/goto` and `/api/history/delete`: a refusal (marker past the end, no such step) is
/// `200 {ok: false, error}`, like a failed command.
fn timeline_response(r: Result<Value, EngineError>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(EngineError::Command(e)) => command_failure(e),
        Err(e) => e.into_response(),
    }
}

async fn history_goto(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: GotoBody = body_required(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    // Blocking thread: restoring a snapshot of a large document copies it.
    Ok(timeline_response(st.blocking(move |st| st.history_goto(b.marker, origin)).await))
}

async fn history_delete(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: DeleteStepBody = body_required(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    // Blocking thread: the later steps are replayed (an import can take seconds).
    Ok(timeline_response(st.blocking(move |st| st.history_delete(b.index, origin)).await))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewQuery {
    /// `?layers=layer-1,layer-3`: only those layers' segments and entries (stats stay the whole job's).
    layers: Option<String>,
}

async fn preview(
    State(st): State<Arc<AppState>>,
    q: Result<Query<PreviewQuery>, QueryRejection>,
) -> Result<Response, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    let result = st.preview().await?;
    if let Err(e) = &result.1 {
        return Err(EngineError::Plan(e.clone()));
    }
    // Blocking thread: a large preview is tens of MB of JSON, seconds of work on a Raspberry Pi that must not hold up
    // the async workers (status polls, Stop).
    let body = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, EngineError> {
        let (doc, preview) = &*result;
        let p = preview.as_ref().map_err(|e| EngineError::Plan(e.clone()))?;
        let mut v = match q.layers {
            None => serde_json::to_value(p),
            Some(list) => {
                let ids: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
                let mut p = p.clone();
                p.retain_layers(&ids);
                serde_json::to_value(&p)
            }
        }
        .map_err(|e| EngineError::Internal(e.to_string()))?;
        // LAY-034: each layer's LightBurn settings that will not be executed (only where there are some).
        crate::preview_bin::add_unsupported(doc, &mut v);
        serde_json::to_vec(&v).map_err(|e| EngineError::Internal(e.to_string()))
    })
    .await??;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

/// Query of `GET /api/preview.bin`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewBinQuery {
    /// `?layers=layer-1,layer-3`, as `GET /api/preview`.
    layers: Option<String>,
    /// `?jumps=false` leaves travel out.
    jumps: Option<bool>,
    /// `?lod=0.2`: level of detail in mm (see `preview_bin::BinOptions::lod_mm`).
    lod: Option<f64>,
}

/// Header naming the document revision a preview answer is of.
pub const REVISION_HEADER: &str = "x-openetch-revision";

/// `GET /api/preview.bin`: the preview in the binary format of module `preview_bin`, compressed once per revision
/// (brotli or gzip, as accepted).
async fn preview_bin(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    q: Result<Query<PreviewBinQuery>, QueryRejection>,
) -> Result<Response, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    let lod_mm = q.lod.unwrap_or(0.0);
    if !(lod_mm.is_finite() && lod_mm >= 0.0) {
        return Err(EngineError::BadRequest("lod must be a number of mm, 0 or more".into()));
    }
    let layers = q.layers.map(|l| l.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect());
    let opts = crate::preview_bin::BinOptions { layers, no_jumps: q.jumps == Some(false), lod_mm };
    let accept = headers.get(header::ACCEPT_ENCODING).and_then(|v| v.to_str().ok());
    let (body, revision) = st.preview_bin(opts, crate::preview_bin::pick_encoding(accept)).await?;
    let mut res = (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (header::VARY, HeaderValue::from_static("accept-encoding")),
        ],
        body.bytes,
    )
        .into_response();
    if let Some(enc) = body.encoding {
        res.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
    }
    if let Ok(v) = HeaderValue::from_str(&revision.to_string()) {
        res.headers_mut().insert(HeaderName::from_static(REVISION_HEADER), v);
    }
    Ok(res)
}

/// `GET /api/preview/stats`: `GET /api/preview` without its segments (stats, passes, layers, warnings), with the
/// segment count and the revision.
async fn preview_stats(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    Ok(Json(st.preview_stats().await?))
}

/// `GET /api/fill_regions`: per fill layer, the even-odd regions its hatch covers, as SVG paths in field mm.
async fn fill_regions(State(st): State<Arc<AppState>>) -> Result<Json<plan::FillRegions>, EngineError> {
    Ok(Json(st.fill_regions().await?))
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

async fn frame_start(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let req: FrameRequest = body_or_default(&body)?;
    if st.machine.is_gantry() && has_mcp_marker(&headers) {
        return Err(EngineError::Forbidden(crate::mcp::GANTRY_FRAME.into()));
    }
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

/// Body of `POST /api/runs`: the origin plus [`crate::runs::RunOptions`] (repeat, continuous, start_input,
/// full_pulse, selection, from_segment) and `revision` (the document revision the app shows: another one now is a
/// 409 and nothing is planned, so a Start Here segment of an older preview never names a segment of a newer
/// document); unknown keys are refused.
async fn request_run(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let mut b: serde_json::Map<String, Value> = body_or_default(&body)?;
    let origin = parse_origin(b.remove("origin").as_ref())?;
    let revision: Option<u64> = match b.remove("revision") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v).map_err(|e| EngineError::BadRequest(format!("bad revision: {e}")))?),
    };
    let options: crate::runs::RunOptions =
        serde_json::from_value(Value::Object(b)).map_err(|e| EngineError::BadRequest(format!("bad run options: {e}")))?;
    let view = st.request_run_at(origin, options, revision).await?;
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

/// A finite number from a query parameter. `parse` accepts "NaN" and "inf", which would become JSON null (the
/// command's default) downstream, so they are refused.
fn finite_param(name: &str, v: &str) -> Result<f64, EngineError> {
    match v.parse::<f64>() {
        Ok(f) if f.is_finite() => Ok(f),
        _ => Err(EngineError::BadRequest(format!("{name} '{v}' is not a finite number"))),
    }
}

/// `POST /api/document/upload?format=openetch|lbrn2|svg|dxf|image[&mode=replace|merge][&field_mm=150][&layer=..]
/// [&name=..][&width_mm=..][&mime=image/png]` with the raw file as the body (at most [`MAX_UPLOAD_BYTES`]). Runs
/// `doc.load`, `doc.import_lbrn2`, `art.import_svg`, `art.import_dxf` or `art.import_image` with origin `ui` (see
/// [`AppState::upload`]); answers like `/api/execute` (a failing command is 200 `{ok: false, error}`).
async fn upload(
    State(st): State<Arc<AppState>>,
    q: FileQuery,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, EngineError> {
    let q = file_query(q, &["format", "mode", "field_mm", "layer", "name", "width_mm", "mime"])?;
    let body = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => {
            EngineError::TooLarge(format!("the file is larger than {} MB", MAX_UPLOAD_BYTES / (1024 * 1024)))
        }
        _ => EngineError::BadRequest(format!("cannot read the upload: {e}")),
    })?;
    let format = UploadFormat::parse(q.get("format").map(String::as_str).ok_or_else(|| {
        EngineError::BadRequest("the format query parameter is required (openetch, lbrn2, svg, dxf or image)".into())
    })?)?;
    let merge = match q.get("mode").map(String::as_str) {
        None => None,
        Some("replace") => Some(false),
        Some("merge") => Some(true),
        Some(other) => return Err(EngineError::BadRequest(format!("unknown mode '{other}' (replace or merge)"))),
    };
    let opts = UploadOptions {
        merge,
        field_mm: q.get("field_mm").map(|v| finite_param("field_mm", v)).transpose()?,
        layer: q.get("layer").cloned(),
        name: q.get("name").cloned(),
        width_mm: q.get("width_mm").map(|v| finite_param("width_mm", v)).transpose()?,
        mime: q.get("mime").cloned(),
    };
    if body.is_empty() {
        return Err(EngineError::BadRequest("the upload is empty".into()));
    }
    // Large files are converted and imported on a blocking thread; the body is moved, not copied (when its buffer
    // is not shared, the usual case).
    let result = st.blocking(move |st| st.upload(format, opts, Vec::from(body), commands::Origin::Ui)).await;
    match result {
        Ok(o) => Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })).into_response()),
        Err(EngineError::Command(e)) => Ok(command_failure(e)),
        Err(e) => Err(e),
    }
}

/// `GET /api/lightburn`: LightBurn's devices (`{available, source, default_device, devices}`).
async fn lightburn(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    // Blocking thread: it may read prefs.ini.
    st.blocking(|st| Ok(Json(st.lightburn.view()))).await
}

/// `POST /api/lightburn/prefs` with LightBurn's `prefs.ini` as the raw body (at most 2 MB): keeps only its device
/// list (see the `lightburn` module). Answers like `GET /api/lightburn`.
async fn lightburn_upload(
    State(st): State<Arc<AppState>>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, EngineError> {
    let body = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => EngineError::TooLarge(format!(
            "prefs.ini is larger than {} MB",
            crate::lightburn::MAX_PREFS_UPLOAD_BYTES / (1024 * 1024)
        )),
        _ => EngineError::BadRequest(format!("cannot read the upload: {e}")),
    })?;
    if body.is_empty() {
        return Err(EngineError::BadRequest("the upload is empty".into()));
    }
    let value = st.blocking(move |st| st.lightburn.upload(&body)).await?;
    st.emit(crate::Event::machine("lightburn", None));
    Ok(Json(value))
}

/// `DELETE /api/lightburn/prefs`: forget the uploaded device list.
async fn lightburn_delete(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let value = st.blocking(|st| st.lightburn.delete_upload()).await?;
    st.emit(crate::Event::machine("lightburn", None));
    Ok(Json(value))
}

/// `GET /api/devices`: `{active, devices: [summary]}`.
async fn devices_list(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(st.devices_view())
}

/// `GET /api/devices/{id}`: the summary plus `settings` (LightBurn's Device Settings, read-only), `profile`, `base`
/// and `diffs`.
async fn device_details(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    Ok(Json(st.device_details(&id)?))
}

/// `POST /api/devices/import[?add=all|<name>][&from=lightburn]` with a LightBurn `prefs.ini` or `.lbdev` as the raw
/// body (at most 2 MB), or, with `from=lightburn` and no body, the devices of LightBurn on the engine's computer (or
/// the uploaded `prefs.ini`). Answers `{ok, added, updated, skipped: [{name, reason}]}` (added/updated are ids).
async fn devices_import(
    State(st): State<Arc<AppState>>,
    q: FileQuery,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, EngineError> {
    let q = file_query(q, &["add", "from"])?;
    let filter = crate::devices::ImportFilter::parse(q.get("add").map(String::as_str));
    let body = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => EngineError::TooLarge(format!(
            "the device file is larger than {} MB",
            crate::lightburn::MAX_PREFS_UPLOAD_BYTES / (1024 * 1024)
        )),
        _ => EngineError::BadRequest(format!("cannot read the upload: {e}")),
    })?;
    let report = match q.get("from").map(String::as_str) {
        Some("lightburn") => {
            if !body.is_empty() {
                return Err(EngineError::BadRequest("from=lightburn reads LightBurn's own prefs.ini; send no body".into()));
            }
            st.blocking(move |st| st.import_devices_from_lightburn(&filter)).await?
        }
        Some(other) => return Err(EngineError::BadRequest(format!("unknown from '{other}' (only lightburn)"))),
        None => {
            if body.is_empty() {
                return Err(EngineError::BadRequest("the upload is empty (send a .lbdev or prefs.ini)".into()));
            }
            st.blocking(move |st| {
                let text = std::str::from_utf8(&body)
                    .map_err(|_| EngineError::BadRequest("the file is not UTF-8 text (expected .lbdev or prefs.ini JSON)".into()))?;
                st.import_devices(text, &filter)
            })
            .await?
        }
    };
    let mut v = json!(report);
    v["ok"] = json!(true);
    Ok(Json(v))
}

/// `DELETE /api/devices/{id}`: not the active device, not the built-in one.
async fn device_remove(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    st.remove_device(&id)?;
    Ok(Json(json!({ "ok": true, "id": id })))
}

/// `POST /api/devices/{id}/activate`: make the device active (the UI's Devices window; it changes the machine, so
/// requests that look like they came from an MCP client or name a non-UI origin are refused).
async fn device_activate(
    State(st): State<Arc<AppState>>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, EngineError> {
    if has_mcp_marker(&headers) {
        return Err(EngineError::Forbidden("devices can only be changed by a person in the OpenEtch UI".into()));
    }
    let b: OriginBody = body_or_default(&body)?;
    if parse_origin(b.origin.as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("devices can only be changed from the UI".into()));
    }
    Ok(Json(st.activate_device(&id).await?))
}

/// Body of `POST /api/runs/{id}/approve`.
#[derive(Debug, Default, Deserialize)]
struct ApproveBody {
    /// The operator PIN (`openetch set-pin`).
    #[serde(default)]
    pin: Option<String>,
    #[serde(default)]
    origin: Option<Value>,
    /// The Job Checklist items ticked, in order (required while the device's checklist is on).
    #[serde(default)]
    checklist: Option<Vec<bool>>,
}

/// Approval by a person. Refuses anything that looks like an MCP client or names a non-UI origin. In the `pin`
/// approval mode the body is `{"pin": "<operator PIN>"}` and PIN refusals are 403 with `code` (`pin_not_set`,
/// `pin_required`, `pin_invalid`, `pin_locked`). In the `click` mode (no PIN) the request must come from the web
/// app's Start button in a browser (`Sec-Fetch-Site: same-origin` and an accepted `Origin`; else 403 with code
/// `browser_required`); a `pin` in the body is ignored.
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
    let approver = match st.approval_mode() {
        ApprovalMode::Pin => approve::Approver::Pin(b.pin.as_deref()),
        ApprovalMode::Click => {
            if let Err(message) = security::same_origin_browser_request(&headers, &st.security) {
                tracing::warn!(run_id = %id, %message, "refused a no-PIN approval that did not come from the app in a browser");
                return Err(EngineError::Approval { code: "browser_required", message });
            }
            approve::Approver::Click
        }
    };
    Ok(Json(approve::approve(&st, &id, approver, b.checklist.as_deref()).await?))
}

/// One-click Start (`--approval click` only), LightBurn's Start: plans the current document with the run options in
/// the body (as `POST /api/runs`, plus `checklist`: the Job Checklist ticks, and `revision`: the document revision the
/// app showed; another revision now is a 409 and nothing starts) and approves it at once (`approve::start_now`).
/// Guarded like a no-PIN approval: MCP client headers or a non-`ui` origin 403, and the request must come from the
/// app in a browser (`Sec-Fetch-Site: same-origin` and an accepted `Origin`, else 403 with code
/// `browser_required`). In the `pin` mode 403 with code `approval_mode` (request the run, then approve it with the PIN).
async fn start_job(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    if has_mcp_marker(&headers) {
        tracing::warn!("refused a one-click Start from an MCP client");
        return Err(EngineError::Forbidden("jobs can only be started by a person in the OpenEtch UI".into()));
    }
    let mut b: serde_json::Map<String, Value> = body_or_default(&body)?;
    if parse_origin(b.remove("origin").as_ref())? != commands::Origin::Ui {
        return Err(EngineError::Forbidden("jobs can only be started from the UI".into()));
    }
    let checklist: Option<Vec<bool>> = match b.remove("checklist") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v).map_err(|e| EngineError::BadRequest(format!("bad checklist: {e}")))?),
    };
    let revision: Option<u64> = match b.remove("revision") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v).map_err(|e| EngineError::BadRequest(format!("bad revision: {e}")))?),
    };
    if st.approval_mode() == ApprovalMode::Click
        && let Err(message) = security::same_origin_browser_request(&headers, &st.security)
    {
        tracing::warn!(%message, "refused a one-click Start that did not come from the app in a browser");
        return Err(EngineError::Approval { code: "browser_required", message });
    }
    let options: crate::runs::RunOptions =
        serde_json::from_value(Value::Object(b)).map_err(|e| EngineError::BadRequest(format!("bad run options: {e}")))?;
    Ok(Json(approve::start_now(&st, options, checklist.as_deref(), revision).await?))
}

#[cfg(test)]
mod web_tests {
    //! The static web UI: navigation-only fallback, cache headers, precompressed files, and the access token kept
    //! across restarts (with its cookie for app windows).

    use super::*;
    use crate::hw::Machine;
    use crate::pin::{self, ApprovalConfig, PinStore};
    use crate::security::{self, SecurityConfig};
    use commands::Session;
    use http_body_util::BodyExt;
    use machine::{MachineProfile, PollConfig};
    use tower::ServiceExt;

    /// A web/dist look-alike: index.html (+ .br/.gz), a hashed bundle (+ .br/.gz), its source map and the manifest.
    fn web_dir() -> PathBuf {
        let dir = pin::temp_dir("webdist");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        let files: [(&str, &[u8]); 8] = [
            ("index.html", b"<!doctype html><title>OpenEtch</title>"),
            ("index.html.br", b"BR-INDEX"),
            ("index.html.gz", b"GZ-INDEX"),
            ("assets/index-abc123.js", b"console.log('plain')"),
            ("assets/index-abc123.js.br", b"BR-JS"),
            ("assets/index-abc123.js.gz", b"GZ-JS"),
            ("assets/index-abc123.js.map", b"{}"),
            ("manifest.webmanifest", b"{\"name\":\"OpenEtch\"}"),
        ];
        for (name, body) in files {
            std::fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    async fn state(security: SecurityConfig) -> Arc<AppState> {
        let machine = Arc::new(Machine::simulated(MachineProfile::generic_lmc_fiber(), None, PollConfig::immediate()));
        machine.connect().await.expect("sim connects");
        let approval = ApprovalConfig { pin: PinStore::new(&pin::temp_dir("webtests")), confirm: None };
        AppState::new(Session::new(doc::Document::default()), machine, security, approval, crate::tests::test_projects())
    }

    fn get(uri: &str, extra: &[(&str, &str)]) -> Request<Body> {
        let mut b = Request::builder().method("GET").uri(uri).header("host", "192.168.1.20:7878");
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    async fn fetch(app: &Router, r: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let res = app.clone().oneshot(r).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        (status, headers, res.into_body().collect().await.unwrap().to_bytes().to_vec())
    }

    fn header(h: &HeaderMap, name: header::HeaderName) -> &str {
        h.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }

    #[test]
    fn navigations_and_cache_policy() {
        for nav in ["/", "/editor/x", "/laser", "/a.b/c"] {
            assert!(is_navigation(&Method::GET, nav), "{nav}");
        }
        assert!(is_navigation(&Method::HEAD, "/editor"));
        for not in ["/assets/nope.js", "/assets/x", "/favicon.ico", "/manifest.webmanifest", "/sw.js"] {
            assert!(!is_navigation(&Method::GET, not), "{not}");
        }
        assert!(!is_navigation(&Method::POST, "/editor"));
        assert_eq!(static_cache_control("/assets/index-abc.js", StatusCode::OK), Some(CACHE_IMMUTABLE));
        assert_eq!(static_cache_control("/assets/index-abc.js", StatusCode::NOT_MODIFIED), Some(CACHE_IMMUTABLE));
        assert_eq!(static_cache_control("/assets/index-abc.js.map", StatusCode::OK), Some(CACHE_REVALIDATE));
        for page in ["/index.html", "/manifest.webmanifest", "/sw.js", "/icon.svg"] {
            assert_eq!(static_cache_control(page, StatusCode::OK), Some(CACHE_REVALIDATE), "{page}");
        }
        assert_eq!(static_cache_control("/assets/nope.js", StatusCode::NOT_FOUND), None);
    }

    #[tokio::test]
    async fn static_routing_cache_headers_and_precompressed_files() {
        let dir = web_dir();
        let app = router(state(SecurityConfig::default()).await, Some(&dir));

        // A page of the app: index.html, revalidated.
        let (s, h, body) = fetch(&app, get("/editor/x", &[])).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body, b"<!doctype html><title>OpenEtch</title>");
        assert_eq!(header(&h, header::CACHE_CONTROL), "no-cache");
        assert!(header(&h, header::CONTENT_TYPE).starts_with("text/html"));
        let (s, h, _) = fetch(&app, get("/", &[])).await;
        assert_eq!((s, header(&h, header::CACHE_CONTROL)), (StatusCode::OK, "no-cache"));

        // Missing files are 404s, not the page.
        for missing in ["/assets/nope.js", "/favicon-missing.ico", "/sw.js"] {
            let (s, h, _) = fetch(&app, get(missing, &[])).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{missing}");
            assert!(h.get(header::CACHE_CONTROL).is_none(), "{missing}: a 404 is never cached");
        }

        // The manifest has its own type and is revalidated.
        let (s, h, _) = fetch(&app, get("/manifest.webmanifest", &[])).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(header(&h, header::CONTENT_TYPE), "application/manifest+json");
        assert_eq!(header(&h, header::CACHE_CONTROL), "no-cache");

        // Hashed bundles: immutable, and the precompressed copy the browser accepts.
        let (s, h, body) = fetch(&app, get("/assets/index-abc123.js", &[])).await;
        assert_eq!((s, body.as_slice()), (StatusCode::OK, &b"console.log('plain')"[..]));
        assert_eq!(header(&h, header::CACHE_CONTROL), CACHE_IMMUTABLE);
        assert!(h.get(header::CONTENT_ENCODING).is_none());
        let (s, h, body) = fetch(&app, get("/assets/index-abc123.js", &[("accept-encoding", "gzip, deflate, br")])).await;
        assert_eq!((s, body.as_slice()), (StatusCode::OK, &b"BR-JS"[..]));
        assert_eq!(header(&h, header::CONTENT_ENCODING), "br");
        assert!(header(&h, header::CONTENT_TYPE).contains("javascript"), "{h:?}");
        assert!(header(&h, header::VARY).contains("accept-encoding"));
        assert_eq!(header(&h, header::CACHE_CONTROL), CACHE_IMMUTABLE);
        let (_, h, body) = fetch(&app, get("/assets/index-abc123.js", &[("accept-encoding", "gzip")])).await;
        assert_eq!((header(&h, header::CONTENT_ENCODING), body.as_slice()), ("gzip", &b"GZ-JS"[..]));
        // The fallback page is precompressed too.
        let (_, h, body) = fetch(&app, get("/editor", &[("accept-encoding", "br")])).await;
        assert_eq!((header(&h, header::CONTENT_ENCODING), body.as_slice()), ("br", &b"BR-INDEX"[..]));

        // Source maps are neither precompressed nor cached for ever.
        let (s, h, _) = fetch(&app, get("/assets/index-abc123.js.map", &[("accept-encoding", "br")])).await;
        assert_eq!(s, StatusCode::OK);
        assert!(h.get(header::CONTENT_ENCODING).is_none());
        assert_eq!(header(&h, header::CACHE_CONTROL), "no-cache");

        // The API is not the web UI: an unknown route stays the API's JSON 404.
        let (s, h, _) = fetch(&app, get("/api/nope", &[])).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert!(h.get(header::CACHE_CONTROL).is_none());
    }

    #[tokio::test]
    async fn the_access_token_survives_a_restart_and_logs_app_windows_in() {
        let config = pin::temp_dir("tokencfg");
        let (token, created) = security::load_or_create_token(&config).unwrap();
        assert!(created && security::valid_token(&token));
        let web = web_dir();
        let first = router(state(SecurityConfig::new(Some(token.clone()), Vec::new())).await, Some(&web));
        let bearer = format!("Bearer {token}");
        assert_eq!(fetch(&first, get("/api/machine", &[])).await.0, StatusCode::UNAUTHORIZED);
        let (s, h, _) = fetch(&first, get("/api/machine", &[("authorization", &bearer)])).await;
        assert_eq!(s, StatusCode::OK);
        let cookie = header(&h, header::SET_COOKIE).to_owned();
        assert!(cookie.starts_with(&format!("{}={token};", security::TOKEN_COOKIE)), "{cookie}");
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && !cookie.contains("Secure"));

        // "Restart": a new AppState from the same config directory keeps the token, so paired devices stay in.
        let (again, created) = security::load_or_create_token(&config).unwrap();
        assert_eq!((again.as_str(), created), (token.as_str(), false));
        let second = router(state(SecurityConfig::new(Some(again), Vec::new())).await, Some(&web));
        assert_eq!(fetch(&second, get("/api/machine", &[("authorization", &bearer)])).await.0, StatusCode::OK);

        // The cookie alone (an app window without the page's localStorage) is accepted for /api, not for /mcp.
        let jar = format!("theme=dark; {}={token}", security::TOKEN_COOKIE);
        let (s, h, _) = fetch(&second, get("/api/machine", &[("cookie", &jar)])).await;
        assert_eq!(s, StatusCode::OK);
        assert!(h.get(header::SET_COOKIE).is_none(), "a good cookie is not set again");
        let same_origin = [("cookie", jar.as_str()), ("origin", "http://192.168.1.20:7878")];
        assert_eq!(fetch(&second, get("/api/machine", &same_origin)).await.0, StatusCode::OK);
        // Another service on the same machine (another port) is same-site for the browser: its requests do not count.
        let other_port = [("cookie", jar.as_str()), ("origin", "http://192.168.1.20:8080")];
        assert_eq!(fetch(&second, get("/api/machine", &other_port)).await.0, StatusCode::UNAUTHORIZED);
        let mcp = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "192.168.1.20:7878")
            .header("cookie", &jar)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(fetch(&second, mcp).await.0, StatusCode::UNAUTHORIZED, "MCP stays bearer-only");
        let wrong = format!("{}=not-the-token-0123456789", security::TOKEN_COOKIE);
        assert_eq!(fetch(&second, get("/api/machine", &[("cookie", &wrong)])).await.0, StatusCode::UNAUTHORIZED);

        // Opening the pairing URL (?token=) sets the cookie on the page itself; a wrong token sets nothing.
        let (s, h, _) = fetch(&second, get(&format!("/?token={token}"), &[("x-forwarded-proto", "https")])).await;
        assert_eq!(s, StatusCode::OK);
        let cookie = header(&h, header::SET_COOKIE);
        assert!(cookie.contains(&token) && cookie.ends_with("; Secure"), "{cookie}");
        let (s, h, _) = fetch(&second, get("/?token=wrong-token-0123456789", &[])).await;
        assert_eq!(s, StatusCode::OK);
        assert!(h.get(header::SET_COOKIE).is_none());

        // Rotating replaces it: the old one is refused by the next engine.
        let rotated = security::rotate_token(&config).unwrap();
        assert_ne!(rotated, token);
        let third = router(state(SecurityConfig::new(security::read_token(&config).unwrap(), Vec::new())).await, None);
        assert_eq!(fetch(&third, get("/api/machine", &[("authorization", &bearer)])).await.0, StatusCode::UNAUTHORIZED);
        let fresh = format!("Bearer {rotated}");
        assert_eq!(fetch(&third, get("/api/machine", &[("authorization", &fresh)])).await.0, StatusCode::OK);
        // STOP never needs it.
        let stop = Request::builder()
            .method("POST")
            .uri("/api/machine/stop")
            .header("host", "192.168.1.20:7878")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(fetch(&third, stop).await.0, StatusCode::OK);
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = pin::temp_dir("tokenmode");
        let (_, _) = security::load_or_create_token(&dir).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&security::token_path(&dir)), 0o600);
        assert_eq!(mode(&dir), 0o700);
        security::rotate_token(&dir).unwrap();
        assert_eq!(mode(&security::token_path(&dir)), 0o600);
        // A hand-made file with loose permissions is tightened when it is read.
        std::fs::set_permissions(security::token_path(&dir), std::fs::Permissions::from_mode(0o644)).unwrap();
        security::read_token(&dir).unwrap();
        assert_eq!(mode(&security::token_path(&dir)), 0o600);
    }

    #[test]
    fn a_damaged_token_file_is_an_error_not_a_silent_new_token() {
        let dir = pin::temp_dir("tokenbad");
        assert_eq!(security::read_token(&dir).unwrap(), None);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(security::token_path(&dir), "short\n").unwrap();
        assert!(security::read_token(&dir).unwrap_err().contains("--rotate"));
        assert!(security::load_or_create_token(&dir).is_err());
        let t = security::rotate_token(&dir).unwrap();
        assert_eq!(security::read_token(&dir).unwrap().as_deref(), Some(t.as_str()));
    }
}
