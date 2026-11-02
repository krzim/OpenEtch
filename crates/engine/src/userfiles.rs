//! Files around the document (ARCHITECTURE.md "Files and image tools"): exports and print as downloads, imports of
//! every further format (also opened as a new project), autosave with rotation, backups on save, project thumbnails,
//! templates, preference export/import with automatic backups, and user bundles. Routes are merged into
//! `api::router`.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/export` | `?format=svg\|dxf\|ai\|pdf&ids=a,b&colors=layers\|black&area=bounds\|workspace&stroke_mm=&tolerance_mm=&keep_circles=&images=` → the file (`doc.export`), named after the project |
//! | GET | `/api/export/gcode` | `?ids=a,b`: the document planned for the active GRBL gantry as a `.gcode` file (`;` comment header, then exactly the lines a run would stream; checked against the profile like a run). 409 unless the active device is a GRBL gantry. Sends nothing to a machine |
//! | GET | `/api/print` | `?format=pdf\|svg&colors=black\|layers&line_width_mm=&paper=w,h&margin_mm=&area=&ids=` → the print-ready file, inline (`doc.print`) |
//! | GET | `/api/images/{id}/file` | the image object's file (`image.export`) |
//! | GET | `/api/images/{id}/processed.png` | `?sub_layer=n`: Save Processed Bitmap (`image.processed`) |
//! | POST | `/api/document/import` | `?format=svg\|dxf\|image\|tiff\|tga\|pdf\|ai\|eps\|ezd\|hpgl\|plt\|gcode\|nc\|gerber\|drill\|model\|lbrn&mode=merge\|replace&name=` + the command's options as query parameters, the raw file as the body (≤ 100 MB) → like `/api/execute`; `mode=replace` opens the file as a new, unsaved project (`doc.new` then the import, one undo step; downloads are named after `name`). `tool_layers=true` (LightBurn's "Allow Importing to Tool Layers", default off) allows `layer=T1`/`T2` (added in the same step when missing); off, those names are refused and an import without `layer` skips a tool layer listed first; a `layer` id is used as given |
//! | GET | `/api/projects/{name}/thumbnail.png` | `?width=32..=1024` (default 256): the saved project rendered |
//! | GET | `/api/backups` | `?project=`: `{autosaves: [{file, project, saved_unix_s, size}], backups: [...]}` newest first |
//! | POST | `/api/backups/restore` | `{kind: "autosave"\|"backup", file, origin?}`: opens it as an unsaved document → like `/api/projects/open` |
//! | GET / PUT | `/api/files/settings` | `{autosave_minutes (0 = off, default 5), autosave_keep (1..=50, default 10), backups_keep (0..=50, default 5)}` (PUT partial, UI only) |
//! | GET | `/api/templates` | `[{name, kind: "openetch"\|"lbt", modified_unix_s, size}]` |
//! | POST | `/api/templates/save` | `{name, overwrite?}`: Save As Template (the document as `<projects-dir>/templates/<name>.openetch`) |
//! | POST | `/api/templates/upload` | `?name=` + a LightBurn `.lbt` (checked) |
//! | POST | `/api/templates/open` | `{name, origin?}`: a new untitled document from the template (it is never overwritten) |
//! | GET | `/api/prefs/export` | the engine's preferences and these settings as `openetch-prefs.oeprefs` (JSON) |
//! | POST | `/api/prefs/import` | an exported file as the body (UI only; the current preferences are backed up first) |
//! | POST | `/api/prefs/import_lightburn` | a LightBurn `.lbprefs` / `prefs.ini` as the body (UI only unless `?preview=1`): the keys OpenEtch has (`crate::lbprefs`); autosave and the active device's Optimization defaults are applied (current preferences backed up first), the browser-side ones returned as `browser` for the UI to apply; `{ok, preview, device, engine, browser, used, rejected, other_keys}` |
//! | GET | `/api/prefs/backups` | `[{file, saved_unix_s, size}]` newest first (at most 50 kept; made at start and before every import) |
//! | POST | `/api/prefs/backups/restore` | `{file, origin?}` (UI only) |
//! | GET | `/api/bundle/export` | `?parts=prefs,devices,libraries` (default all): a user bundle `.oebundle` (zip); `format=lbzip` (parts `devices,libraries,presets,images,art`): a LightBurn bundle (`crate::lbzip`) |
//! | POST | `/api/bundle/import` | `?parts=` + a bundle (UI only) → `{ok, imported: {prefs, devices: [id], libraries: [name]}, skipped: [text]}`; devices are always added as new devices (an identical one is skipped), a library whose name exists with other content is loaded as "<name> (bundle)", as LightBurn keeps what is there. A LightBurn `.lbzip` (`crate::lbzip`; parts `prefs,devices,libraries,presets,images,art`) → `{ok, lightburn: true, imported: {prefs, devices, libraries, presets, images, art}, browser, skipped}` (`browser`: the LightBurn settings for the web UI to apply) |

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use commands::{Invocation, Origin};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::projects::{self, ProjectDir};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError};

/// Largest file `POST /api/document/import` and the bundle import accept.
pub const MAX_IMPORT_BYTES: usize = 100 * 1024 * 1024;
/// The settings file in the config directory.
pub const SETTINGS_FILE: &str = "files.json";
/// Directories inside the projects directory (dot names: never listed as projects).
pub const AUTOSAVE_DIR: &str = ".autosave";
pub const BACKUP_DIR: &str = ".backups";
pub const TEMPLATES_DIR: &str = "templates";
/// Preference backups in the config directory.
pub const PREFS_BACKUP_DIR: &str = "prefs-backups";
pub const MAX_PREFS_BACKUPS: usize = 50;

/// Autosave and backup settings (`<config-dir>/files.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileSettings {
    /// Minutes between autosaves of a changed document (LightBurn's Auto-save interval; 0 turns it off).
    pub autosave_minutes: u32,
    /// Autosaves kept per project (oldest removed first).
    pub autosave_keep: u32,
    /// Backups of a project's previous file kept on every save (0 = none).
    pub backups_keep: u32,
}

impl Default for FileSettings {
    fn default() -> Self {
        FileSettings { autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 }
    }
}

impl FileSettings {
    fn problem(&self) -> Option<String> {
        if self.autosave_minutes > 24 * 60 {
            return Some("autosave_minutes must be 0..=1440".into());
        }
        if !(1..=50).contains(&self.autosave_keep) {
            return Some("autosave_keep must be 1..=50".into());
        }
        if self.backups_keep > 50 {
            return Some("backups_keep must be 0..=50".into());
        }
        None
    }
}

/// State of this module, kept in [`AppState::files`].
#[derive(Debug, Default)]
pub struct UserFiles {
    config_dir: Mutex<Option<PathBuf>>,
    settings: Mutex<FileSettings>,
    /// Revision of the last autosave (or 0).
    autosaved_revision: AtomicU64,
    /// When the last autosave check found the document changed (Unix seconds).
    last_autosave: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn io(what: &str, p: &Path, e: std::io::Error) -> EngineError {
    EngineError::Internal(format!("cannot {what} {}: {e}", p.display()))
}

/// Write through a temporary file in the same directory (creating it).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let dir = path.parent().ok_or_else(|| EngineError::Internal("no directory".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| io("create", dir, e))?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|f| f.to_str()).unwrap_or("file"), std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| io("write", &tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io("write", path, e)
    })
}

/// A file name inside one of our directories: no separators, no leading dot, ends with `ext`.
fn plain_file(name: &str, ext: &str) -> Result<(), EngineError> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':'])
        && !name.contains("..")
        && name.ends_with(ext);
    if ok { Ok(()) } else { Err(EngineError::BadRequest(format!("bad file name {name:?}"))) }
}

/// One stored copy (autosave, backup, preference backup).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stored {
    pub file: String,
    /// The project it belongs to (autosaves and backups).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub saved_unix_s: u64,
    pub size: u64,
}

/// `<stem>.<unix>.<ext>` files in `dir`, newest first; `project` keeps only that stem.
fn stored(dir: &Path, ext: &str, project: Option<&str>) -> Vec<Stored> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(rest) = name.strip_suffix(&format!(".{ext}")) else { continue };
        let Some((stem, ts)) = rest.rsplit_once('.') else { continue };
        let Ok(ts) = ts.parse::<u64>() else { continue };
        if project.is_some_and(|p| !projects::same_name_ignoring_case(p, stem)) {
            continue;
        }
        let size = e.metadata().map_or(0, |m| m.len());
        out.push(Stored { file: name.clone(), project: Some(stem.to_string()), saved_unix_s: ts, size });
    }
    out.sort_by(|a, b| b.saved_unix_s.cmp(&a.saved_unix_s).then_with(|| b.file.cmp(&a.file)));
    out
}

/// Remove all but the newest `keep` copies of `stem` in `dir`.
fn rotate(dir: &Path, ext: &str, stem: Option<&str>, keep: usize) {
    for old in stored(dir, ext, stem).into_iter().skip(keep) {
        let _ = std::fs::remove_file(dir.join(&old.file));
    }
}

/// A unique `<stem>.<unix>.<ext>` path in `dir`.
fn stamped(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    // Never older than the newest copy, so a copy removed by rotation never frees a name that sorts first.
    let newest = stored(dir, ext, Some(stem)).first().map_or(0, |s| s.saved_unix_s + 1);
    let mut t = now_unix().max(newest);
    loop {
        let p = dir.join(format!("{stem}.{t}.{ext}"));
        if !p.exists() {
            return p;
        }
        t += 1;
    }
}

impl UserFiles {
    /// Use `<config-dir>`: read the settings and back up the preferences (if they changed since the last backup).
    pub fn open(&self, config_dir: &Path) -> Vec<String> {
        let mut notes = Vec::new();
        *lock(&self.config_dir) = Some(config_dir.to_path_buf());
        let path = config_dir.join(SETTINGS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(t) => match serde_json::from_str::<FileSettings>(&t) {
                Ok(s) if s.problem().is_none() => *lock(&self.settings) = s,
                _ => notes.push(format!("{}: not usable; using the defaults", path.display())),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => notes.push(format!("{}: {e}", path.display())),
        }
        let current = std::fs::read(config_dir.join(crate::prefs::PREFS_FILE)).ok();
        if let Err(e) = self.backup_prefs(current, false) {
            notes.push(format!("preferences backup: {e}"));
        }
        notes
    }

    pub fn settings(&self) -> FileSettings {
        lock(&self.settings).clone()
    }

    pub(crate) fn config_dir(&self) -> Result<PathBuf, EngineError> {
        lock(&self.config_dir).clone().ok_or_else(|| EngineError::Conflict("the engine has no config directory".into()))
    }

    fn set_settings(&self, s: FileSettings) -> Result<(), EngineError> {
        if let Some(p) = s.problem() {
            return Err(EngineError::BadRequest(p));
        }
        if let Ok(dir) = self.config_dir() {
            let text = serde_json::to_string_pretty(&s).map_err(|e| EngineError::Internal(e.to_string()))?;
            write_atomic(&dir.join(SETTINGS_FILE), text.as_bytes())?;
        }
        *lock(&self.settings) = s;
        Ok(())
    }

    /// Keep `current` (the preferences as JSON) in the backups (always with `force`, else only when it differs from
    /// the newest backup); keeps [`MAX_PREFS_BACKUPS`].
    fn backup_prefs(&self, current: Option<Vec<u8>>, force: bool) -> Result<(), EngineError> {
        let dir = self.config_dir()?;
        let Some(bytes) = current else { return Ok(()) };
        let bdir = dir.join(PREFS_BACKUP_DIR);
        if !force
            && let Some(newest) = stored(&bdir, "json", None).first()
            && std::fs::read(bdir.join(&newest.file)).ok().as_deref() == Some(bytes.as_slice())
        {
            return Ok(());
        }
        write_atomic(&stamped(&bdir, "preferences", "json"), &bytes)?;
        rotate(&bdir, "json", None, MAX_PREFS_BACKUPS);
        Ok(())
    }
}

/// Before a project file is replaced: copy the old one to `<projects-dir>/.backups/<name>.<unix>.openetch` and keep
/// the newest `keep`. A missing old file (first save) or `keep` 0 does nothing; failures are logged, never fatal.
pub fn backup_before_save(dir: &ProjectDir, name: &str, keep: u32) {
    if keep == 0 {
        return;
    }
    let Ok((on_disk, text)) = dir.read(name) else { return };
    let bdir = dir.root().join(BACKUP_DIR);
    // A project in folders is backed up under its flat name (`Clients__Acme__badge`).
    let stem = projects::flat_name(&on_disk);
    let path = stamped(&bdir, &stem, projects::PROJECT_EXT);
    match write_atomic(&path, text.as_bytes()) {
        Ok(()) => rotate(&bdir, projects::PROJECT_EXT, Some(&stem), keep as usize),
        Err(e) => tracing::warn!(error = %e, "project backup failed"),
    }
}

/// Write an autosave of the document when it changed since the last one and the interval has passed. Returns the
/// path written.
pub fn autosave_now(st: &AppState, force: bool) -> Result<Option<PathBuf>, EngineError> {
    let s = st.files.settings();
    if s.autosave_minutes == 0 && !force {
        return Ok(None);
    }
    // The document and its project status, read together: another open document may become the active one between
    // two reads, and its content must not be autosaved under this one's name (module `documents`).
    let mut read = None;
    for _ in 0..8 {
        let doc = st.document();
        let status = st.project_status();
        if status["revision"].as_u64() == Some(doc.revision) {
            read = Some((status, doc));
            break;
        }
    }
    // Still changing under us: the next tick tries again.
    let Some((status, doc)) = read else { return Ok(None) };
    let revision = doc.revision;
    let dirty = status["dirty"].as_bool().unwrap_or(false);
    if !force && (!dirty || st.files.autosaved_revision.load(Ordering::SeqCst) == revision) {
        return Ok(None);
    }
    let text = doc::file::try_to_openetch_json(&doc).map_err(|e| EngineError::Internal(e.to_string()))?;
    let stem = status["name"]
        .as_str()
        .filter(|n| projects::validate_project_name(n).is_ok())
        .map(projects::flat_name)
        .or_else(|| status["display_name"].as_str().filter(|n| projects::validate_name(n).is_ok()).map(str::to_owned))
        .unwrap_or_else(|| "Untitled".into());
    let dir = st.projects.root().join(AUTOSAVE_DIR);
    let path = stamped(&dir, &stem, projects::PROJECT_EXT);
    write_atomic(&path, text.as_bytes())?;
    rotate(&dir, projects::PROJECT_EXT, Some(&stem), s.autosave_keep as usize);
    st.files.autosaved_revision.store(revision, Ordering::SeqCst);
    st.files.last_autosave.store(now_unix(), Ordering::SeqCst);
    tracing::info!(path = %path.display(), revision, "autosaved");
    Ok(Some(path))
}

/// Autosave every `autosave_minutes` (checked every 30 s) while the document has unsaved changes.
pub fn spawn_autosave(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    st.files.last_autosave.store(now_unix(), Ordering::SeqCst);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            let minutes = u64::from(st.files.settings().autosave_minutes);
            if minutes == 0 || now_unix().saturating_sub(st.files.last_autosave.load(Ordering::SeqCst)) < minutes * 60 {
                continue;
            }
            let s = st.clone();
            match tokio::task::spawn_blocking(move || autosave_now(&s, false)).await {
                Ok(Err(e)) => tracing::warn!(error = %e, "autosave failed"),
                Err(e) => tracing::warn!(error = %e, "autosave task failed"),
                Ok(Ok(_)) => st.files.last_autosave.store(now_unix(), Ordering::SeqCst),
            }
        }
    })
}

// ---------------------------------------------------------------------------------------------------------------------
// Routes.

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/export", get(export))
        .route("/export/gcode", get(export_gcode))
        .route("/print", get(print))
        .route("/images/{id}/file", get(image_file))
        .route("/images/{id}/processed.png", get(image_processed))
        .route("/document/import", post(import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)))
        .route("/projects/{name}/thumbnail.png", get(thumbnail))
        .route("/backups", get(backups))
        .route("/backups/restore", post(backup_restore))
        .route("/files/settings", get(settings_get).put(settings_put))
        .route("/templates", get(templates))
        .route("/templates/save", post(template_save))
        .route("/templates/upload", post(template_upload).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)))
        .route("/templates/open", post(template_open))
        .route("/prefs/export", get(prefs_export))
        .route("/prefs/import", post(prefs_import))
        .route("/prefs/import_lightburn", post(prefs_import_lightburn))
        .route("/prefs/backups", get(prefs_backups))
        .route("/prefs/backups/restore", post(prefs_restore))
        .route("/bundle/export", get(bundle_export))
        .route("/bundle/import", post(bundle_import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)))
}

type Q = Result<Query<HashMap<String, String>>, QueryRejection>;

fn query(q: Q, allowed: &[&str]) -> Result<HashMap<String, String>, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    if let Some(k) = q.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: {})", allowed.join(", "))));
    }
    Ok(q)
}

fn ui_only(headers: &HeaderMap, origin: Option<&Value>, what: &str) -> Result<(), EngineError> {
    if has_mcp_marker(headers) {
        return Err(EngineError::Forbidden(format!("{what} only by a person in the OpenEtch UI")));
    }
    if parse_origin(origin)? != Origin::Ui {
        return Err(EngineError::Forbidden(format!("{what} only from the UI")));
    }
    Ok(())
}

/// `Content-Disposition` for `<stem>.<ext>` (ASCII `filename` plus UTF-8 `filename*`).
fn disposition(kind: &str, stem: &str, ext: &str) -> String {
    let stem: String = stem.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\')).collect();
    let stem = if stem.trim().is_empty() { "Untitled".to_string() } else { stem.trim().to_string() };
    let file = format!("{stem}.{ext}");
    let ascii: String = file.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    let mut enc = String::new();
    for b in file.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            enc.push(char::from(b));
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

fn file_response(kind: &str, stem: &str, ext: &str, mime: &str, body: Vec<u8>, warnings: usize) -> Response {
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition(kind, stem, ext)),
            (header::CACHE_CONTROL, "no-store".to_string()),
            (header::HeaderName::from_static("x-openetch-warnings"), warnings.to_string()),
        ],
        body,
    )
        .into_response()
}

/// The GRBL layer options a G-code job uses, for the export header ("" when none).
fn gantry_options_note(job: &plan::JobPlan) -> String {
    let g = |f: fn(&plan::GantryPass) -> bool| job.passes.iter().any(|p| f(&p.gantry));
    let notes: Vec<&str> = [
        (g(|p| p.constant_power), "M3 constant power on some layers"),
        (g(|p| p.air_assist), "air assist M8/M9"),
        (g(|p| p.overscan_mm > 0.0), "overscan"),
        (g(|p| p.overcut_mm > 0.0), "overcut"),
    ]
    .into_iter()
    .filter_map(|(on, n)| on.then_some(n))
    .collect();
    if notes.is_empty() { String::new() } else { format!("; layer options: {}", notes.join(", ")) }
}

/// The name downloads get: the project, else the opened file, else the document.
fn stem(st: &AppState) -> String {
    let p = st.project_status();
    p["name"].as_str().or(p["display_name"].as_str()).map(str::to_owned).unwrap_or_else(|| st.document().name)
}

/// A query value as JSON: numbers, booleans, `a,b` lists for `ids` / `paper`, else text.
fn qvalue(key: &str, v: &str) -> Result<Value, EngineError> {
    if matches!(key, "ids") {
        return Ok(json!(v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>()));
    }
    if matches!(key, "paper" | "rotation_deg") {
        let n: Result<Vec<f64>, _> = v.split(',').map(|s| s.trim().parse::<f64>()).collect();
        return match n {
            Ok(n) if n.iter().all(|x| x.is_finite()) => Ok(json!(n)),
            _ => Err(EngineError::BadRequest(format!("{key} must be numbers separated by commas"))),
        };
    }
    if v == "true" || v == "false" {
        return Ok(json!(v == "true"));
    }
    if let Ok(n) = v.parse::<f64>()
        && !matches!(key, "name" | "layer" | "kind" | "format" | "colors" | "area" | "mime" | "render")
    {
        if !n.is_finite() {
            return Err(EngineError::BadRequest(format!("{key} '{v}' is not a finite number")));
        }
        // Whole numbers stay integers, so integer options (a PDF `page`, a `dpi`) read; float options take them too.
        if let Ok(i) = v.trim().parse::<i64>() {
            return Ok(json!(i));
        }
        return Ok(json!(n));
    }
    Ok(json!(v))
}

/// Run a query command and turn a failure into `EngineError`.
fn query_cmd(st: &AppState, id: &str, params: Value) -> Result<Value, EngineError> {
    Ok(st.execute(Invocation { id: id.into(), params }, Origin::Ui)?.result)
}

fn decoded_content(r: &Value) -> Result<Vec<u8>, EngineError> {
    if let Some(t) = r["content"].as_str() {
        return Ok(t.as_bytes().to_vec());
    }
    let b = r["content_base64"].as_str().unwrap_or_default();
    STANDARD.decode(b).map_err(|e| EngineError::Internal(e.to_string()))
}

async fn export(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["format", "ids", "colors", "stroke_mm", "images", "tolerance_mm", "keep_circles", "area"])?;
    let mut params = serde_json::Map::new();
    for (k, v) in &q {
        params.insert(k.clone(), qvalue(k, v)?);
    }
    if !params.contains_key("format") {
        return Err(EngineError::BadRequest("format is required (svg, dxf, ai or pdf)".into()));
    }
    st.blocking(move |st| {
        let r = query_cmd(st, "doc.export", Value::Object(params))?;
        let body = decoded_content(&r)?;
        let n = r["warnings"].as_array().map_or(0, Vec::len);
        Ok(file_response("attachment", &stem(st), r["extension"].as_str().unwrap_or("bin"), r["mime"].as_str().unwrap_or("application/octet-stream"), body, n))
    })
    .await
}

/// `GET /api/export/gcode`: the document as G-code for the active GRBL gantry (Save G-code, LightBurn UX-007). The
/// same planning and checks as a run (`runs::prepare_gcode_job`'s), written to a file instead of a serial port.
async fn export_gcode(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["ids"])?;
    let selection = q.get("ids").map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect::<Vec<_>>());
    let config = st.machine.config();
    let Some(grbl) = config.grbl.clone() else {
        return Err(EngineError::Conflict("G-code export needs a GRBL gantry as the active device".into()));
    };
    let device = st.active_device().display_name;
    st.blocking(move |st| {
        let doc = commands::prepare_output(&st.document()).map_err(|e| EngineError::BadRequest(format!("text: {e}")))?;
        let options = plan::JobOptions { selection, run_index: 0, preview: false };
        let job = plan::plan_job_with(&doc, &config.target, &options)?;
        let settings = crate::hw::gcode_settings(&grbl);
        let lines = plan::gcode_lines(&job, &settings)?;
        machine::grbl::check_program(&grbl, &machine::grbl::GcodeProgram::new(lines))
            .map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let header = [
            format!("OpenEtch G-code for {device} ({} x {} mm bed)", grbl.work_area_mm[0], grbl.work_area_mm[1]),
            format!(
                "GRBL 1.1 laser mode ($32=1), S 0..{} ($30), {}{}",
                plan::gcode_number(grbl.s_max),
                if grbl.dynamic_power { "M4 dynamic power" } else { "M3 constant power" },
                gantry_options_note(&job)
            ),
            format!("{} polylines, {:.1} mm marked, about {:.0} s", job.stats.polyline_count, job.stats.mark_length_mm, job.stats.estimated_time_s),
        ];
        let text = plan::gcode_file(&job, &settings, &header)?;
        Ok(file_response("attachment", &stem(st), "gcode", "text/x-gcode", text.into_bytes(), 0))
    })
    .await
}

async fn print(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["format", "colors", "line_width_mm", "paper", "margin_mm", "area", "ids"])?;
    let mut params = serde_json::Map::new();
    for (k, v) in &q {
        let key = if k == "paper" { "paper_mm".to_string() } else { k.clone() };
        params.insert(key, qvalue(k, v)?);
    }
    st.blocking(move |st| {
        let r = query_cmd(st, "doc.print", Value::Object(params))?;
        let body = decoded_content(&r)?;
        let n = r["warnings"].as_array().map_or(0, Vec::len);
        Ok(file_response("inline", &stem(st), r["extension"].as_str().unwrap_or("pdf"), r["mime"].as_str().unwrap_or("application/pdf"), body, n))
    })
    .await
}

async fn image_file(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>) -> Result<Response, EngineError> {
    st.blocking(move |st| {
        let r = query_cmd(st, "image.export", json!({ "id": id }))?;
        let body = STANDARD.decode(r["data_base64"].as_str().unwrap_or_default()).map_err(|e| EngineError::Internal(e.to_string()))?;
        let name = st.document().object(&id).map(|o| o.name.clone()).unwrap_or(id);
        Ok(file_response("attachment", &name, r["extension"].as_str().unwrap_or("png"), r["mime"].as_str().unwrap_or("image/png"), body, 0))
    })
    .await
}

async fn image_processed(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["sub_layer"])?;
    let sub = q.get("sub_layer").map(|v| v.parse::<usize>().map_err(|_| EngineError::BadRequest("sub_layer must be a whole number".into()))).transpose()?;
    st.blocking(move |st| {
        let r = query_cmd(st, "image.processed", json!({ "id": id, "sub_layer": sub }))?;
        let body = STANDARD.decode(r["png_base64"].as_str().unwrap_or_default()).map_err(|e| EngineError::Internal(e.to_string()))?;
        let name = st.document().object(&id).map(|o| format!("{} processed", o.name)).unwrap_or(id);
        Ok(file_response("attachment", &name, "png", "image/png", body, 0))
    })
    .await
}

/// The import command and its parameters for a format (the file in `content` or `data_base64`).
pub(crate) fn import_invocation(format: &str, bytes: Vec<u8>, q: &HashMap<String, String>) -> Result<Invocation, EngineError> {
    let text = |bytes: Vec<u8>| -> Result<String, EngineError> {
        let mut t = String::from_utf8(bytes).map_err(|_| EngineError::BadRequest(format!("a {format} file must be UTF-8 text")))?;
        if t.starts_with('\u{feff}') {
            t.drain(..3);
        }
        Ok(t)
    };
    let mut params = serde_json::Map::new();
    let id = match format {
        "svg" => {
            params.insert("svg".into(), json!(text(bytes)?));
            "art.import_svg"
        }
        "dxf" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_dxf"
        }
        "hpgl" | "plt" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_hpgl"
        }
        "gcode" | "nc" | "gc" | "ngc" | "tap" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_gcode"
        }
        "gerber" | "drill" => {
            params.insert("content".into(), json!(text(bytes)?));
            if format == "drill" {
                params.insert("kind".into(), json!("excellon"));
            }
            "art.import_gerber"
        }
        "lbrn" | "lbrn2" | "lbt" => {
            params.insert("content".into(), json!(text(bytes)?));
            "doc.import_lbrn2"
        }
        "image" | "tiff" | "tga" => {
            let mime = match q.get("mime") {
                Some(m) => m.clone(),
                None => crate::state::sniff_image(&bytes)
                    .map(str::to_owned)
                    .or_else(|| (format == "tga").then(|| "image/x-tga".to_string()))
                    .ok_or_else(|| EngineError::BadRequest("not an image the engine recognises; pass mime=".into()))?,
            };
            params.insert("mime".into(), json!(mime));
            params.insert("data_base64".into(), json!(STANDARD.encode(&bytes)));
            "art.import_image"
        }
        "pdf" | "ai" | "eps" | "ezd" | "model" => {
            params.insert("data_base64".into(), json!(STANDARD.encode(&bytes)));
            match format {
                "pdf" => "art.import_pdf",
                "ai" => "art.import_ai",
                "eps" => "art.import_eps",
                "ezd" => "art.import_ezd",
                _ => "art.import_model",
            }
        }
        other => {
            return Err(EngineError::BadRequest(format!(
                "unknown format '{other}' (svg, dxf, image, tiff, tga, pdf, ai, eps, ezd, hpgl, plt, gcode, nc, gerber, drill, model, lbrn)"
            )));
        }
    };
    for (k, v) in q {
        match k.as_str() {
            "format" | "mode" | "mime" => {}
            "name" if id == "doc.import_lbrn2" => {
                params.insert("name".into(), json!(v));
            }
            // Art imports name their group (or object) after the file, without its extension.
            "name" => {
                let stem = v.rsplit_once('.').map_or(v.as_str(), |(s, _)| s);
                params.insert("name".into(), json!(stem));
            }
            "model_format" => {
                params.insert("format".into(), json!(v));
            }
            _ => {
                params.insert(k.clone(), qvalue(k, v)?);
            }
        }
    }
    Ok(Invocation { id: id.into(), params: Value::Object(params) })
}

async fn import(State(st): State<Arc<AppState>>, q: Q, body: Result<Bytes, BytesRejection>) -> Result<Response, EngineError> {
    let q = query(
        q,
        &[
            "format", "mode", "name", "layer", "width_mm", "keep_position", "split", "page", "hidden_layers", "units_per_mm",
            "invert", "kind", "model_format", "dpi", "outline", "sixteen_bit", "transparent_background", "depth_cutoff_mm",
            "equalize", "rotation_deg", "mime", "field_mm", "tool_layers", "pen_layers",
        ],
    )?;
    let mut q = q;
    // LightBurn's "Allow Importing to Tool Layers" (off by default): only then may art land on T1 / T2.
    let tool_layers = match q.remove("tool_layers").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(v) => return Err(EngineError::BadRequest(format!("tool_layers must be true or false (got '{v}')"))),
    };
    let body = body.map_err(|e| match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => EngineError::TooLarge(format!("the file is larger than {} MB", MAX_IMPORT_BYTES / (1024 * 1024))),
        _ => EngineError::BadRequest(format!("cannot read the upload: {e}")),
    })?;
    if body.is_empty() {
        return Err(EngineError::BadRequest("the upload is empty".into()));
    }
    let format = q.get("format").cloned().ok_or_else(|| EngineError::BadRequest("format is required".into()))?;
    let replace = match q.get("mode").map(String::as_str) {
        None | Some("merge") => false,
        Some("replace") => true,
        Some(m) => return Err(EngineError::BadRequest(format!("unknown mode '{m}' (merge or replace)"))),
    };
    let lightburn = matches!(format.as_str(), "lbrn" | "lbrn2" | "lbt");
    let result = st
        .blocking(move |st| {
            // The file's name without its extension names the document (and the import's group).
            let display = q.get("name").and_then(|n| projects::display_name(n.rsplit_once('.').map_or(n.as_str(), |(stem, _)| stem)));
            let mut q = q;
            let before = if lightburn { Vec::new() } else { import_layer(st, &mut q, tool_layers, replace)? };
            let mut inv = import_invocation(&format, Vec::from(body), &q)?;
            if lightburn {
                inv.params["mode"] = json!(if replace { "replace" } else { "merge" });
                return st.batch_opening(vec![inv], Origin::Ui, None, display).map(|mut v| v.remove(0));
            }
            if !replace {
                let mut invs = before;
                invs.push(inv);
                return st.batch(invs, Origin::Ui).map(|mut v| v.pop().expect("the import ran"));
            }
            let new = Invocation { id: "doc.new".into(), params: json!({ "name": display.clone().unwrap_or_else(|| "Untitled".into()) }) };
            let mut invs = vec![new];
            invs.extend(before);
            invs.push(inv);
            st.batch_opening(invs, Origin::Ui, None, display).map(|mut v| v.pop().expect("the import ran"))
        })
        .await;
    match result {
        Ok(o) => Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })).into_response()),
        Err(EngineError::Command(e)) => Ok(Json(EngineError::Command(e).body()).into_response()),
        Err(e) => Err(e),
    }
}

/// The import's target layer under LightBurn's "Allow Importing to Tool Layers" (`tool_layers`): with it, `layer`
/// may be `T1` / `T2` (the tool layer, added in the same step when the document has none; returned as the commands
/// to run first); without it, those names are refused and an import without `layer` goes to the first ordinary
/// layer. A layer given by id is used as given.
fn import_layer(st: &AppState, q: &mut HashMap<String, String>, tool_layers: bool, replace: bool) -> Result<Vec<Invocation>, EngineError> {
    // A replacing import starts from an empty document (one ordinary layer).
    let doc = if replace { doc::Document::default() } else { st.document() };
    let off = || EngineError::BadRequest("importing to tool layers is off: pass tool_layers=true (LightBurn's \"Allow Importing to Tool Layers\")".into());
    match q.get("layer").cloned() {
        Some(l) if doc.layer(&l).is_none() && (l.eq_ignore_ascii_case("t1") || l.eq_ignore_ascii_case("t2")) => {
            if !tool_layers {
                return Err(off());
            }
            let t = if l.eq_ignore_ascii_case("t1") { doc::ToolLayer::T1 } else { doc::ToolLayer::T2 };
            q.insert("layer".into(), t.name().into());
            if doc.layers.iter().any(|x| x.tool == Some(t)) {
                Ok(Vec::new())
            } else {
                Ok(vec![Invocation { id: "layer.tool".into(), params: json!({ "tool": t.name() }) }])
            }
        }
        // An existing layer named by id is the caller's choice (the web UI applies the preference itself).
        Some(_) => Ok(Vec::new()),
        None => {
            if !tool_layers && doc.layers.first().is_some_and(|x| x.tool.is_some()) {
                let plain = doc.layers.iter().find(|x| x.tool.is_none()).ok_or_else(|| {
                    EngineError::BadRequest("the document has only tool layers: add a layer, or pass tool_layers=true".into())
                })?;
                q.insert("layer".into(), plain.id.clone());
            }
            Ok(Vec::new())
        }
    }
}

async fn thumbnail(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["width"])?;
    let width = match q.get("width") {
        Some(w) => w.parse::<u32>().ok().filter(|w| (32..=1024).contains(w)).ok_or_else(|| EngineError::BadRequest("width must be 32..=1024".into()))?,
        None => 256,
    };
    projects::validate_project_name(&name)?;
    let png = st
        .blocking(move |st| {
            let (_, text) = st.projects.read(&name)?;
            let doc = doc::file::from_openetch_json(&text).map_err(|e| EngineError::BadRequest(format!("project \"{name}\": {e}")))?;
            let field = doc.field_mm.unwrap_or_else(|| st.machine.profile().field_mm);
            let svg = crate::render::preview_svg(&doc, field, None, crate::render::RenderOptions { show_jumps: false });
            crate::render::svg_to_png(&svg, width)
        })
        .await?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-cache")], png).into_response())
}

async fn backups(State(st): State<Arc<AppState>>, q: Q) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["project"])?;
    let root = st.projects.root().to_path_buf();
    let p = q.get("project").map(|p| projects::flat_name(p));
    let a = stored(&root.join(AUTOSAVE_DIR), projects::PROJECT_EXT, p.as_deref());
    let b = stored(&root.join(BACKUP_DIR), projects::PROJECT_EXT, p.as_deref());
    Ok(Json(json!({ "autosaves": a, "backups": b })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreBody {
    kind: String,
    file: String,
    #[serde(default)]
    origin: Option<Value>,
}

async fn backup_restore(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: RestoreBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let origin = parse_origin(b.origin.as_ref())?;
    let dir = match b.kind.as_str() {
        "autosave" => AUTOSAVE_DIR,
        "backup" => BACKUP_DIR,
        k => return Err(EngineError::BadRequest(format!("unknown kind '{k}' (autosave or backup)"))),
    };
    plain_file(&b.file, &format!(".{}", projects::PROJECT_EXT))?;
    let path = st.projects.root().join(dir).join(&b.file);
    let o = st
        .blocking(move |st| {
            let text = std::fs::read_to_string(&path).map_err(|_| EngineError::NotFound(format!("no {} {}", b.kind, b.file)))?;
            let project = b.file.rsplitn(3, '.').nth(2).map(str::to_owned);
            let inv = Invocation { id: "doc.load".into(), params: json!({ "content": text }) };
            st.batch_opening(vec![inv], origin, None, project).map(|mut v| v.remove(0))
        })
        .await?;
    Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })))
}

async fn settings_get(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!(st.files.settings()))
}

async fn settings_put(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let mut v: Value = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let origin = v.as_object_mut().and_then(|m| m.remove("origin"));
    ui_only(&headers, origin.as_ref(), "file settings can be changed")?;
    let mut cur = serde_json::to_value(st.files.settings()).map_err(|e| EngineError::Internal(e.to_string()))?;
    let Value::Object(patch) = v else { return Err(EngineError::BadRequest("expected a JSON object".into())) };
    for (k, val) in patch {
        cur[k] = val;
    }
    let s: FileSettings = serde_json::from_value(cur).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    st.files.set_settings(s.clone())?;
    Ok(Json(json!(s)))
}

fn template_dir(st: &AppState) -> ProjectDir {
    ProjectDir::new(st.projects.root().join(TEMPLATES_DIR))
}

async fn templates(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let dir = template_dir(&st);
    let mut out: Vec<Value> = dir.list()?.into_iter().map(|p| json!({ "name": p.name, "kind": "openetch", "modified_unix_s": p.modified_unix_s, "size": p.size })).collect();
    for e in std::fs::read_dir(dir.root()).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if let Some(stem) = n.strip_suffix(".lbt").filter(|s| projects::validate_name(s).is_ok()) {
            let m = e.metadata().ok();
            let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            out.push(json!({ "name": stem, "kind": "lbt", "modified_unix_s": t, "size": m.map_or(0, |m| m.len()) }));
        }
    }
    out.sort_by_key(|v| v["name"].as_str().unwrap_or_default().to_lowercase());
    Ok(Json(json!(out)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateBody {
    name: String,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    origin: Option<Value>,
}

async fn template_save(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: TemplateBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    projects::validate_name(&b.name)?;
    let text = doc::file::try_to_openetch_json(&st.document()).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let dir = template_dir(&st);
    let name = b.name.clone();
    let path = tokio::task::spawn_blocking(move || {
        if !b.overwrite && dir.list()?.iter().any(|p| projects::same_name_ignoring_case(&p.name, &name)) {
            return Err(EngineError::Conflict(format!("a template named \"{name}\" exists; pass overwrite: true")));
        }
        dir.write(&name, projects::PROJECT_EXT, &text)
    })
    .await??;
    Ok(Json(json!({ "ok": true, "name": b.name, "path": path.display().to_string() })))
}

async fn template_upload(State(st): State<Arc<AppState>>, q: Q, body: Result<Bytes, BytesRejection>) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["name"])?;
    let name = q.get("name").cloned().ok_or_else(|| EngineError::BadRequest("name is required".into()))?;
    projects::validate_name(&name)?;
    let body = body.map_err(|e| EngineError::BadRequest(format!("cannot read the upload: {e}")))?;
    let text = String::from_utf8(body.to_vec()).map_err(|_| EngineError::BadRequest("a .lbt template must be UTF-8 XML".into()))?;
    let lb = fileio::lbrn_legacy::to_lbrn2(&text).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    lbrn::parse(&lb).map_err(|e| EngineError::BadRequest(format!("not a LightBurn template: {e}")))?;
    let dir = template_dir(&st);
    let path = tokio::task::spawn_blocking(move || dir.write(&name, "lbt", &text)).await??;
    Ok(Json(json!({ "ok": true, "path": path.display().to_string() })))
}

async fn template_open(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: TemplateBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let origin = parse_origin(b.origin.as_ref())?;
    projects::validate_name(&b.name)?;
    let o = st
        .blocking(move |st| {
            let dir = template_dir(st);
            let inv = match dir.read(&b.name) {
                Ok((_, text)) => Invocation { id: "doc.load".into(), params: json!({ "content": text }) },
                Err(EngineError::NotFound(_)) => {
                    let path = dir.root().join(format!("{}.lbt", b.name));
                    let text = std::fs::read_to_string(&path).map_err(|_| EngineError::NotFound(format!("no template named \"{}\"", b.name)))?;
                    Invocation { id: "doc.import_lbrn2".into(), params: json!({ "content": text, "mode": "replace" }) }
                }
                Err(e) => return Err(e),
            };
            // A new, untitled document: the template is never the current project.
            st.batch_opening(vec![inv], origin, None, None).map(|mut v| v.remove(0))
        })
        .await?;
    Ok(Json(json!({ "ok": true, "result": o.result, "revision": o.revision })))
}

/// The exported preferences file.
#[derive(Debug, Serialize, Deserialize)]
struct PrefsFile {
    format: String,
    version: u32,
    preferences: Value,
    #[serde(default)]
    files: Option<FileSettings>,
}

const PREFS_FORMAT: &str = "openetch-prefs";

fn prefs_value(st: &AppState) -> Result<Value, EngineError> {
    serde_json::to_value(&crate::libs::lock(&st.libs.prefs).prefs).map_err(|e| EngineError::Internal(e.to_string()))
}

/// Replace the engine's preferences (backing up the current ones first).
fn set_prefs(st: &AppState, prefs: Value, files: Option<FileSettings>) -> Result<(), EngineError> {
    let p: crate::prefs::Preferences = serde_json::from_value(prefs).map_err(|e| EngineError::BadRequest(format!("bad preferences: {e}")))?;
    // Everything is checked before anything changes: the same rules as `PUT /api/prefs` (a Job Checklist that can
    // be ticked) and `PUT /api/files/settings`.
    for (device, d) in &p.devices {
        if let Some(problem) = d.checklist.problem() {
            return Err(EngineError::BadRequest(format!("bad preferences: device {device}: {problem}")));
        }
    }
    if let Some(problem) = files.as_ref().and_then(FileSettings::problem) {
        return Err(EngineError::BadRequest(format!("bad file settings: {problem}")));
    }
    let current = serde_json::to_vec_pretty(&crate::libs::lock(&st.libs.prefs).prefs).ok();
    if let Err(e) = st.files.backup_prefs(current, true) {
        tracing::warn!(error = %e, "preferences backup failed");
    }
    let debug = p.debug_log;
    {
        let mut store = crate::libs::lock(&st.libs.prefs);
        store.prefs = p;
        store.save()?;
    }
    // As `PUT /api/prefs` does: the debug log follows the setting, and the UI hears of the change.
    crate::diagnostics::set_debug(debug);
    if let Some(f) = files {
        st.files.set_settings(f)?;
    }
    st.emit(crate::Event::machine("prefs", None));
    Ok(())
}

async fn prefs_export(State(st): State<Arc<AppState>>) -> Result<Response, EngineError> {
    let f = PrefsFile { format: PREFS_FORMAT.into(), version: 1, preferences: prefs_value(&st)?, files: Some(st.files.settings()) };
    let text = serde_json::to_string_pretty(&f).map_err(|e| EngineError::Internal(e.to_string()))?;
    Ok(file_response("attachment", "openetch-prefs", "oeprefs", "application/json", text.into_bytes(), 0))
}

async fn prefs_import(State(st): State<Arc<AppState>>, headers: HeaderMap, q: Q, body: Bytes) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["origin"])?;
    ui_only(&headers, q.get("origin").map(|o| json!(o)).as_ref(), "preferences can be imported")?;
    let f: PrefsFile = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("not an OpenEtch preferences file: {e}")))?;
    if f.format != PREFS_FORMAT || f.version > 1 {
        return Err(EngineError::BadRequest("not an OpenEtch preferences file (or a newer version)".into()));
    }
    st.blocking(move |st| set_prefs(st, f.preferences, f.files)).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn prefs_import_lightburn(State(st): State<Arc<AppState>>, headers: HeaderMap, q: Q, body: Bytes) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["origin", "preview"])?;
    let preview = matches!(q.get("preview").map(String::as_str), Some("1" | "true"));
    if !preview {
        ui_only(&headers, q.get("origin").map(|o| json!(o)).as_ref(), "LightBurn preferences can be imported")?;
    }
    Ok(Json(st.blocking(move |st| import_lightburn_prefs(st, &body, preview)).await?))
}

/// Map a LightBurn `.lbprefs` / `prefs.ini` (`crate::lbprefs`; also from a `.lbzip` bundle) and, unless `preview`,
/// apply its engine-side values (current preferences backed up first). Returns `{ok, preview, device, engine,
/// browser, used, rejected, other_keys}`; `browser` is for the web UI to apply.
pub fn import_lightburn_prefs(st: &AppState, body: &[u8], preview: bool) -> Result<Value, EngineError> {
    let device = st.active_device().id;
    let base = crate::libs::lock(&st.libs.prefs).device(&device).optimization_defaults.unwrap_or_default();
    let m = crate::lbprefs::map(body, &base).map_err(EngineError::BadRequest)?;
    if !preview && (m.optimization.is_some() || m.autosave_minutes.is_some()) {
        let mut prefs = crate::libs::lock(&st.libs.prefs).prefs.clone();
        if let Some(o) = &m.optimization {
            prefs.devices.entry(device.clone()).or_default().optimization_defaults = Some(o.clone());
        }
        let files = m.autosave_minutes.map(|minutes| FileSettings { autosave_minutes: minutes, ..st.files.settings() });
        let v = serde_json::to_value(&prefs).map_err(|e| EngineError::Internal(e.to_string()))?;
        set_prefs(st, v, files)?;
    }
    let engine = json!({ "autosave_minutes": m.autosave_minutes, "optimization": m.optimization });
    Ok(json!({
        "ok": true,
        "preview": preview,
        "device": device,
        "engine": engine,
        "browser": m.browser,
        "used": m.used,
        "rejected": m.rejected,
        "other_keys": m.other_keys,
    }))
}

async fn prefs_backups(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let dir = st.files.config_dir()?.join(PREFS_BACKUP_DIR);
    let list: Vec<Value> = stored(&dir, "json", None).into_iter().map(|s| json!({ "file": s.file, "saved_unix_s": s.saved_unix_s, "size": s.size })).collect();
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBody {
    file: String,
    #[serde(default)]
    origin: Option<Value>,
}

async fn prefs_restore(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> Result<Json<Value>, EngineError> {
    let b: FileBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    ui_only(&headers, b.origin.as_ref(), "preferences can be restored")?;
    plain_file(&b.file, ".json")?;
    let path = st.files.config_dir()?.join(PREFS_BACKUP_DIR).join(&b.file);
    let text = std::fs::read_to_string(&path).map_err(|_| EngineError::NotFound(format!("no backup {}", b.file)))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    st.blocking(move |st| set_prefs(st, v, None)).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Bundle parts. `presets` (material test presets), `images` (Adjust Image presets) and `art` (Art Libraries) are
/// read from and written to LightBurn `.lbzip` bundles only.
const PARTS: [&str; 6] = ["prefs", "devices", "libraries", "presets", "images", "art"];

fn parts(q: &HashMap<String, String>) -> Result<Vec<String>, EngineError> {
    let Some(p) = q.get("parts") else { return Ok(PARTS.iter().map(|s| s.to_string()).collect()) };
    let list: Vec<String> = p.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if let Some(bad) = list.iter().find(|s| !PARTS.contains(&s.as_str())) {
        return Err(EngineError::BadRequest(format!("unknown part '{bad}' (prefs, devices, libraries, presets, images, art)")));
    }
    Ok(list)
}

const BUNDLE_FORMAT: &str = "openetch-bundle";

/// Build a bundle of `parts`.
pub fn bundle(st: &AppState, parts: &[String]) -> Result<Vec<u8>, EngineError> {
    let mut entries = Vec::new();
    let mut manifest = json!({ "format": BUNDLE_FORMAT, "version": 1, "created_unix_s": now_unix(), "parts": parts });
    if parts.iter().any(|p| p == "prefs") {
        let f = PrefsFile { format: PREFS_FORMAT.into(), version: 1, preferences: prefs_value(st)?, files: Some(st.files.settings()) };
        entries.push(fileio::zip::Entry { name: "prefs.oeprefs".into(), data: serde_json::to_vec_pretty(&f).map_err(|e| EngineError::Internal(e.to_string()))? });
    }
    if parts.iter().any(|p| p == "devices") {
        let mut ids = Vec::new();
        let reg = st.devices();
        let builtin = reg.list().first().map(|d| d.id.clone());
        let dir = st.files.config_dir().ok().map(|d| d.join(crate::devices::DEVICES_DIR));
        let mut cors = std::collections::BTreeSet::new();
        for d in reg.list() {
            if Some(&d.id) == builtin.as_ref() {
                continue;
            }
            let text = d.to_toml().map_err(EngineError::Internal)?;
            entries.push(fileio::zip::Entry { name: format!("devices/{}.toml", d.id), data: text.into_bytes() });
            ids.push(d.id.clone());
            if let Some(cor) = d.profile.as_ref().and_then(|p| p.cor_file.clone()) {
                cors.insert(cor);
            }
        }
        drop(reg);
        if let Some(dir) = dir {
            for cor in cors {
                // Correction files are kept by name in the devices directory.
                let Some(file) = cor.file_name().and_then(|f| f.to_str()).map(str::to_owned) else { continue };
                if let Ok(bytes) = std::fs::read(dir.join(&file)) {
                    entries.push(fileio::zip::Entry { name: format!("devices/{file}"), data: bytes });
                }
            }
        }
        manifest["devices"] = json!(ids);
    }
    if parts.iter().any(|p| p == "libraries") {
        let lib = crate::libs::lock(&st.libs.library);
        let names: Vec<String> = lib.view()["libraries"].as_array().into_iter().flatten().filter_map(|l| l["name"].as_str().map(str::to_owned)).collect();
        for n in &names {
            if let Ok((file, text)) = lib.text(n) {
                let _ = file;
                entries.push(fileio::zip::Entry { name: format!("libraries/{n}.clb"), data: text.into_bytes() });
            }
        }
        manifest["libraries"] = json!(names);
    }
    entries.insert(0, fileio::zip::Entry { name: "bundle.json".into(), data: serde_json::to_vec_pretty(&manifest).map_err(|e| EngineError::Internal(e.to_string()))? });
    fileio::zip::write(&entries).map_err(|e| EngineError::Internal(e.to_string()))
}

/// Whether a bundle entry's file name is safe to create in one of our directories: no folders, no drive (`C:x.cor`
/// on Windows is relative to drive C's current folder, so `dir.join` would leave `dir`), no alternate data stream,
/// no leading dot, no trailing dot or space, no control characters and no Windows device name.
pub(crate) fn bundle_file_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && !name.ends_with(['.', ' '])
        && !name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        && !name.chars().any(char::is_control)
        && !device
}

/// Why a bundle's material libraries past [`crate::library::MAX_LOADED`] are skipped: no more can be loaded, and
/// each one tried is compared with the loaded library of its name (a hostile bundle repeating a name thousands of
/// times would otherwise hold the library lock for minutes).
pub(crate) fn too_many_libraries() -> String {
    format!("a bundle may hold at most {} material libraries (as many as can be loaded)", crate::library::MAX_LOADED)
}

/// Load a bundle's material library (`.clb` bytes) as `name`: one with the same content is skipped ("already here");
/// a name taken, by a loaded library or by a file in the libraries folder that is not loaded (`load` would overwrite
/// it), loads as "<name> (bundle)", as LightBurn keeps what is there. Returns the name it loaded as, or why not.
pub(crate) fn import_library(st: &AppState, name: &str, data: &[u8]) -> Result<String, String> {
    crate::projects::validate_name(name).map_err(|e| e.to_string())?;
    let mut lib = crate::libs::lock(&st.libs.library);
    if lib.text(name).ok().map(|(_, t)| t.into_bytes()).as_deref() == Some(data) {
        return Err("already here".into());
    }
    let lib_dir = st.files.config_dir().ok().map(|d| d.join(crate::library::LIBRARIES_DIR));
    let taken = |n: &str| lib.find(n).is_some() || lib_dir.as_ref().is_some_and(|d| d.join(format!("{n}.clb")).exists());
    let target = if taken(name) {
        // Shortened so the name stays valid: a library named at the length limit gets " (bundle)" too.
        let keep = crate::projects::MAX_NAME_CHARS - " (bundle)".len();
        format!("{} (bundle)", name.chars().take(keep).collect::<String>().trim_end())
    } else {
        name.to_owned()
    };
    if taken(&target) {
        return Err(format!("\"{target}\" exists too"));
    }
    lib.load(&target, data).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Import `parts` of a bundle. Returns `{prefs, devices, libraries}` and what was skipped.
pub fn unbundle(st: &AppState, bytes: &[u8], parts: &[String]) -> Result<Value, EngineError> {
    let entries = fileio::zip::read(bytes).map_err(|e| EngineError::BadRequest(format!("not a bundle: {e}")))?;
    if crate::lbzip::is_lbzip(&entries) {
        return crate::lbzip::import(st, &entries, parts);
    }
    let manifest: Value = entries
        .iter()
        .find(|e| e.name == "bundle.json")
        .and_then(|e| serde_json::from_slice(&e.data).ok())
        .ok_or_else(|| {
            EngineError::BadRequest("not a user bundle (no bundle.json, and no LightBurnSoftware.json of a LightBurn .lbzip)".into())
        })?;
    if manifest["format"] != BUNDLE_FORMAT || manifest["version"].as_u64().unwrap_or(99) > 1 {
        return Err(EngineError::BadRequest("not an OpenEtch bundle (or a newer version)".into()));
    }
    let mut skipped: Vec<String> = Vec::new();
    let mut prefs = false;
    if parts.iter().any(|p| p == "prefs")
        && let Some(e) = entries.iter().find(|e| e.name == "prefs.oeprefs")
    {
        let f: PrefsFile = serde_json::from_slice(&e.data).map_err(|e| EngineError::BadRequest(format!("bundle preferences: {e}")))?;
        set_prefs(st, f.preferences, f.files)?;
        prefs = true;
    }
    let mut devices = Vec::new();
    if parts.iter().any(|p| p == "devices") {
        let dir = st.files.config_dir().ok().map(|d| d.join(crate::devices::DEVICES_DIR));
        for e in entries.iter().filter(|e| e.name.starts_with("devices/")) {
            let file = &e.name["devices/".len()..];
            if !bundle_file_name(file) {
                if !file.is_empty() {
                    skipped.push(format!("{}: not a plain file name", e.name));
                }
                continue;
            }
            if file.to_ascii_lowercase().ends_with(".cor") {
                let Some(dir) = &dir else { continue };
                let path = dir.join(file);
                match std::fs::read(&path) {
                    Ok(existing) if existing != e.data => skipped.push(format!("correction file {file}: a different file of that name exists")),
                    Ok(_) => {}
                    Err(_) => {
                        if corfile::CorTable::parse(&e.data).is_err() {
                            skipped.push(format!("correction file {file}: not a correction file"));
                        } else {
                            write_atomic(&path, &e.data)?;
                        }
                    }
                }
            }
        }
        for e in entries.iter().filter(|e| e.name.starts_with("devices/") && e.name.ends_with(".toml")) {
            let text = String::from_utf8_lossy(&e.data);
            let d = match machine::Device::from_toml(&text) {
                Ok(d) => d,
                Err(err) => {
                    skipped.push(format!("{}: {err}", e.name));
                    continue;
                }
            };
            let mut reg = st.devices();
            // A bundle is a file from elsewhere: its correction file must be one in the devices directory (a path
            // could name any file, or a network share that Windows would sign in to), and its limits must stay
            // within this machine's base profile, as an edit's must (`machine::edit_profile`).
            if let Some(p) = &d.profile {
                if let Some(cor) = &p.cor_file
                    && !cor.to_str().is_some_and(bundle_file_name)
                {
                    skipped.push(format!("device \"{}\": correction file {} is not a plain file name", d.display_name, cor.display()));
                    continue;
                }
                let limits = reg.import_limits(&d);
                let patch = serde_json::to_value(p).map_err(|e| EngineError::Internal(e.to_string()))?;
                if let Err(err) = machine::edit_profile(&limits, &patch, &limits) {
                    skipped.push(format!("device \"{}\": {err}", d.display_name));
                    continue;
                }
            }
            let same = reg.list().iter().any(|x| x.display_name == d.display_name && x.profile == d.profile && x.workspace_mm == d.workspace_mm);
            if same {
                skipped.push(format!("device \"{}\": already here", d.display_name));
                continue;
            }
            match reg.add_manual(d) {
                Ok(added) => devices.push(added.id),
                Err(err) => skipped.push(format!("{}: {err}", e.name)),
            }
        }
        if !devices.is_empty() {
            st.emit(crate::Event::DevicesChanged);
        }
    }
    let mut libraries = Vec::new();
    if parts.iter().any(|p| p == "libraries") {
        for (i, e) in entries.iter().filter(|e| e.name.starts_with("libraries/") && e.name.ends_with(".clb")).enumerate() {
            let name = &e.name["libraries/".len()..e.name.len() - 4];
            if i >= crate::library::MAX_LOADED {
                skipped.push(format!("library \"{name}\": {}", too_many_libraries()));
                continue;
            }
            match import_library(st, name, &e.data) {
                Ok(target) => libraries.push(target),
                Err(why) => skipped.push(format!("library \"{name}\": {why}")),
            }
        }
    }
    Ok(json!({ "ok": true, "imported": { "prefs": prefs, "devices": devices, "libraries": libraries }, "skipped": skipped }))
}

async fn bundle_export(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["parts", "format"])?;
    let parts = parts(&q)?;
    match q.get("format").map(String::as_str) {
        None | Some("oebundle") => {}
        Some("lbzip") => {
            let (bytes, skipped) = st.blocking(move |st| crate::lbzip::export(st, &parts)).await?;
            return Ok(file_response("attachment", "openetch-bundle", "lbzip", "application/zip", bytes, skipped));
        }
        Some(f) => return Err(EngineError::BadRequest(format!("format must be oebundle or lbzip (got '{f}')"))),
    }
    let bytes = st.blocking(move |st| bundle(st, &parts)).await?;
    Ok(file_response("attachment", "openetch-bundle", "oebundle", "application/zip", bytes, 0))
}

async fn bundle_import(State(st): State<Arc<AppState>>, headers: HeaderMap, q: Q, body: Result<Bytes, BytesRejection>) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["parts", "origin"])?;
    ui_only(&headers, q.get("origin").map(|o| json!(o)).as_ref(), "bundles can be imported")?;
    let parts = parts(&q)?;
    let body = body.map_err(|e| EngineError::BadRequest(format!("cannot read the upload: {e}")))?;
    let v = st.blocking(move |st| unbundle(st, &body, &parts)).await?;
    Ok(Json(v))
}
