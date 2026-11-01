//! Routes for material libraries, layer and optimization defaults, preferences, the job log, diagnostics, the
//! update check, the Find My Laser wizard and the camera (merged into `api::router`). Changes to the machine's
//! behaviour (preferences, device wizard, camera calibration) are UI-only like the other device settings; reads
//! are open to every client.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/library` | `{selected, libraries: [{name, entries, materials, dirty}], tree}` |
//! | POST | `/api/library/new` | `{name}`: Create New Library (selected) |
//! | POST | `/api/library/load` | `?name=`, a `.clb` as the body: Load Library (stored in `<config-dir>/libraries/`, selected) |
//! | POST | `/api/library/select` | `{name}`: Select Library; linked layers relink to it where it has their preset |
//! | POST | `/api/library/unload` | `{name, discard?}` |
//! | POST | `/api/library/save` | `{name, as?}`: Save Library (As) |
//! | POST | `/api/library/rename` | `{name, new_name}`: linked layers follow |
//! | POST | `/api/library/merge` | `{name, from}`: Merge Library With → `{added}` |
//! | GET | `/api/library/{name}` | the tree: materials → groups (thickness or title) → entries `{id, path, summary, ...}` |
//! | GET | `/api/library/{name}/download` | the `.clb` |
//! | POST | `/api/library/{name}/entries` | `{layer, material, thickness_mm?, title?, description}`: Create new from layer → `{id}` |
//! | PUT | `/api/library/{name}/entries/{id}` | `{from_layer?}` (Update), `{settings?, sub_layers?}` (Edit Cut), `{meta?: {material, thickness_mm?, title?, description}}` (Edit Description); linked layers follow |
//! | POST | `/api/library/{name}/entries/{id}/duplicate` | Duplicate → `{id}` |
//! | DELETE | `/api/library/{name}/entries/{id}` | Delete (layers linked to it are unlinked) |
//! | POST | `/api/library/{name}/entries/{id}/apply` | `{layer, link?: bool}`: Assign (default) or Link |
//! | GET / PUT | `/api/prefs` | the active device's preferences and the global ones; PUT `{require_framing?, play_sound?, checklist?, load_defaults_on_new?, debug_log?, check_for_updates?}` (UI only) |
//! | POST | `/api/layers/{id}/default` | `{all?}`: Make Default (for the layer's colour) / Make Default for All |
//! | POST | `/api/layers/{id}/reset` | `{builtin?}`: Reset to Default (`builtin`: OpenEtch's defaults, LightBurn's Shift+Reset) |
//! | POST / DELETE | `/api/optimization/default` | Set as Defaults / forget them |
//! | POST | `/api/optimization/reset` | `{builtin?}`: the saved defaults (or OpenEtch's) applied to the document |
//! | GET | `/api/job-log` | `?limit=` (default 200), `?format=csv`: the newest job log entries |
//! | GET | `/api/diagnostics/report` | the bug report bundle (download) |
//! | GET | `/api/diagnostics/logs` | `{debug, lines}` |
//! | GET / POST | `/api/updates`, `/api/updates/check` | the last check / check now (only with `check_for_updates` on) |
//! | GET | `/api/devices/scan` | Find My Laser: `{usb_support, boards: [{vid, pid, location, serial, supported, note}]}` |
//! | POST | `/api/devices/wizard` | `{name?, markcfg_base64, cor_name?, cor_base64?, activate?}`: Add Galvo in one step (UI only) |
//! | GET | `/api/camera` | `{capture_support, frame, calibration}` |
//! | POST | `/api/camera/frame` | a PNG/JPEG frame as the body (from a browser camera) |
//! | POST | `/api/camera/capture` | `{index?}`: grab a frame from a UVC camera (feature `camera`) |
//! | GET | `/api/camera/frame.png`, `/api/camera/overlay.png` | the frame / the frame in field coordinates (`?px_per_mm=`) |
//! | POST | `/api/camera/detect` | `{cols, rows}`: checkerboard corners in the frame |
//! | POST | `/api/camera/lens` | `{width, height, views: [{cols, rows, corners}]}`: lens calibration (UI only) |
//! | POST | `/api/camera/align` | `{image: [[u, v]...], field: [[x, y]...]}`: field alignment (UI only) |
//! | POST | `/api/camera/markers` | the camera markers (`generate.camera_markers`) found in the frame: `{markers: [{id, corners, center, inverted, errors}]}` |
//! | POST | `/api/camera/align_markers` | `{origin?}` (UI only): align with the markers found in the frame, each matched by id to the document's marker object (its centre is the field position); at least four → `{ok, alignment, used: [id], missing: [id], unknown: [id]}` |
//! | DELETE | `/api/camera/calibration` | forget the active device's camera calibration (UI only) |

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use commands::{Invocation, Origin};
use doc::PresetLink;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::camera::{self, View};
use crate::libs::lock;
use crate::library::{self, EntryMeta, Preset, entry_id, parse_entry_id, preset_of};
use crate::prefs::{ALL_COLORS, Checklist, LayerDefault};
use crate::security::has_mcp_marker;
use crate::state::parse_origin;
use crate::{AppState, EngineError, Event};

/// Largest camera frame upload.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/library", get(library_view))
        .route("/library/new", post(library_new))
        .route("/library/load", post(library_load).layer(DefaultBodyLimit::max(library::MAX_LIBRARY_BYTES)))
        .route("/library/select", post(library_select))
        .route("/library/unload", post(library_unload))
        .route("/library/save", post(library_save))
        .route("/library/rename", post(library_rename))
        .route("/library/merge", post(library_merge))
        .route("/library/{name}", get(library_tree))
        .route("/library/{name}/download", get(library_download))
        .route("/library/{name}/entries", post(entry_create))
        .route("/library/{name}/entries/{id}", put(entry_edit).delete(entry_delete))
        .route("/library/{name}/entries/{id}/duplicate", post(entry_duplicate))
        .route("/library/{name}/entries/{id}/apply", post(entry_apply))
        .route("/prefs", get(prefs_view).put(prefs_edit))
        .route("/layers/{id}/default", post(layer_make_default))
        .route("/layers/{id}/reset", post(layer_reset))
        .route("/optimization/default", post(optimization_make_default).delete(optimization_forget_default))
        .route("/optimization/reset", post(optimization_reset))
        .route("/job-log", get(job_log))
        .route("/diagnostics/report", get(diagnostics_report))
        .route("/diagnostics/logs", get(diagnostics_logs))
        .route("/updates", get(updates_view))
        .route("/updates/check", post(updates_check))
        .route("/devices/scan", get(devices_scan))
        .route("/devices/wizard", post(devices_wizard).layer(DefaultBodyLimit::max(16 * 1024 * 1024)))
        .route("/camera", get(camera_view))
        .route("/camera/frame", post(camera_frame).layer(DefaultBodyLimit::max(MAX_FRAME_BYTES)))
        .route("/camera/capture", post(camera_capture))
        .route("/camera/frame.png", get(camera_frame_png))
        .route("/camera/overlay.png", get(camera_overlay_png))
        .route("/camera/detect", post(camera_detect))
        .route("/camera/lens", post(camera_lens))
        .route("/camera/align", post(camera_align))
        .route("/camera/markers", post(camera_markers))
        .route("/camera/align_markers", post(camera_align_markers))
        .route("/camera/calibration", axum::routing::delete(camera_forget))
}

fn body<T: DeserializeOwned>(b: &Bytes) -> Result<T, EngineError> {
    let b: &[u8] = if b.iter().all(u8::is_ascii_whitespace) { b"{}" } else { b };
    serde_json::from_slice(b).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
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

fn origin_of(headers: &HeaderMap, v: Option<&Value>) -> Result<Origin, EngineError> {
    let o = parse_origin(v)?;
    if has_mcp_marker(headers) && o == Origin::Ui {
        return Ok(Origin::Agent("mcp".into()));
    }
    Ok(o)
}

fn attachment(name: &str, content_type: &str, body: Vec<u8>) -> Response {
    let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect();
    (
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{ascii}\"")),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        body,
    )
        .into_response()
}

// ---------------------------------------------------------------------------------------------------------------
// Libraries
// ---------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: String,
    #[serde(default)]
    discard: bool,
    #[serde(default, rename = "as")]
    save_as: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
    #[serde(default)]
    from: Option<String>,
}

async fn library_view(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(lock(&st.libs.library).view())
}

async fn library_tree(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    let store = lock(&st.libs.library);
    let l = store.get(&name)?;
    let mut t = library::tree(&l.lib);
    t["name"] = json!(l.name);
    t["dirty"] = json!(l.dirty);
    Ok(Json(t))
}

async fn library_new(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    lock(&st.libs.library).create(&b.name)?;
    remember_device_library(&st, &b.name);
    Ok(Json(lock(&st.libs.library).view()))
}

#[derive(Deserialize)]
struct LoadQuery {
    name: String,
}

async fn library_load(State(st): State<Arc<AppState>>, Query(q): Query<LoadQuery>, b: Bytes) -> Result<Json<Value>, EngineError> {
    lock(&st.libs.library).load(&q.name, &b)?;
    remember_device_library(&st, &q.name);
    relink_all(&st, &q.name).await?;
    Ok(Json(lock(&st.libs.library).view()))
}

/// LightBurn: a library saved or loaded while a device is active loads with that device.
fn remember_device_library(st: &AppState, name: &str) {
    let device = st.active_device().id;
    let mut p = lock(&st.libs.prefs);
    p.device_mut(&device).library = Some(name.to_string());
    if let Err(e) = p.save() {
        tracing::warn!(error = %e, "preferences: cannot save");
    }
}

async fn library_select(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    let name = lock(&st.libs.library).select(&b.name)?;
    remember_device_library(&st, &name);
    let relinked = relink_all(&st, &name).await?;
    let mut v = lock(&st.libs.library).view();
    v["relinked_layers"] = json!(relinked);
    Ok(Json(v))
}

async fn library_unload(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    lock(&st.libs.library).unload(&b.name, b.discard)?;
    Ok(Json(lock(&st.libs.library).view()))
}

async fn library_save(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    let saved = lock(&st.libs.library).save(&b.name, b.save_as.as_deref())?;
    remember_device_library(&st, &saved);
    let mut v = lock(&st.libs.library).view();
    v["saved"] = json!(saved);
    Ok(Json(v))
}

async fn library_rename(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    let new_name = b.new_name.ok_or_else(|| EngineError::BadRequest("give new_name".into()))?;
    let old = lock(&st.libs.library).rename(&b.name, &new_name)?;
    // Linked layers follow the library's new name.
    let doc = st.document();
    let plan: Vec<(String, Option<(PresetLink, Preset)>)> = doc
        .layers
        .iter()
        .filter_map(|l| l.link.as_ref().filter(|k| k.library == old).map(|k| (l.id.clone(), k.path.clone())))
        .filter_map(|(id, path)| {
            let preset = preset_for(&st, &new_name, &path).ok()?;
            Some((id, Some((PresetLink { library: new_name.clone(), path }, preset))))
        })
        .collect();
    apply_links(&st, plan).await?;
    Ok(Json(lock(&st.libs.library).view()))
}

async fn library_merge(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: NameBody = body(&b)?;
    let from = b.from.ok_or_else(|| EngineError::BadRequest("give from".into()))?;
    let added = lock(&st.libs.library).merge(&b.name, &from)?;
    let mut v = lock(&st.libs.library).view();
    v["added"] = json!(added);
    Ok(Json(v))
}

async fn library_download(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<Response, EngineError> {
    let (name, text) = lock(&st.libs.library).text(&name)?;
    Ok(attachment(&format!("{name}.clb"), "application/xml", text.into_bytes()))
}

/// The preset at `path` in library `name`.
fn preset_for(st: &AppState, name: &str, path: &str) -> Result<Preset, EngineError> {
    let store = lock(&st.libs.library);
    let l = store.get(name)?;
    let (m, e) = l.lib.find_path(path).ok_or_else(|| EngineError::NotFound(format!("library \"{name}\" has no preset \"{path}\"")))?;
    preset_of(&l.lib.materials[m].entries[e])
}

fn apply_invocation(layer: &str, preset: &Preset, link: Option<&PresetLink>) -> Invocation {
    Invocation {
        id: "layer.apply_preset".into(),
        params: json!({
            "layer": layer, "settings": preset.settings, "sub_layers": preset.sub_layers,
            "global_passes": preset.global_passes, "extras": preset.extras, "link": link,
        }),
    }
}

/// Apply or unlink presets on layers as one undo step (origin `script`): `Some((link, preset))` links the layer
/// to the preset, `None` unlinks it. Returns the layer ids changed.
async fn apply_links(st: &Arc<AppState>, plan: Vec<(String, Option<(PresetLink, Preset)>)>) -> Result<Vec<String>, EngineError> {
    if plan.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = plan.iter().map(|(id, _)| id.clone()).collect();
    let invs: Vec<Invocation> = plan
        .into_iter()
        .map(|(layer, p)| match p {
            Some((link, preset)) => apply_invocation(&layer, &preset, Some(&link)),
            None => Invocation { id: "layer.unlink_preset".into(), params: json!({ "layer": layer }) },
        })
        .collect();
    st.blocking(move |st| st.batch(invs, Origin::Script)).await?;
    Ok(ids)
}

/// Layers linked to `(library, path)` brought up to date with the entry now at `new_path` (or unlinked when it is
/// gone).
async fn sync_linked(st: &Arc<AppState>, name: &str, old_path: &str, new_path: Option<&str>) -> Result<Vec<String>, EngineError> {
    let lib_name = lock(&st.libs.library).get(name)?.name.clone();
    let doc = st.document();
    let linked: Vec<String> = doc
        .layers
        .iter()
        .filter(|l| l.link.as_ref().is_some_and(|k| crate::projects::same_name_ignoring_case(&k.library, &lib_name) && k.path == old_path))
        .map(|l| l.id.clone())
        .collect();
    if linked.is_empty() {
        return Ok(Vec::new());
    }
    let plan = match new_path {
        Some(p) => {
            let preset = preset_for(st, &lib_name, p)?;
            let link = PresetLink { library: lib_name.clone(), path: p.to_string() };
            linked.into_iter().map(|id| (id, Some((link.clone(), preset.clone())))).collect()
        }
        None => linked.into_iter().map(|id| (id, None)).collect(),
    };
    apply_links(st, plan).await
}

/// LightBurn: on a library switch, a linked layer relinks to the new library when it has a preset of the same
/// material, thickness/title and description, and takes its settings.
async fn relink_all(st: &Arc<AppState>, name: &str) -> Result<Vec<String>, EngineError> {
    let lib_name = lock(&st.libs.library).get(name)?.name.clone();
    let doc = st.document();
    let plan: Vec<(String, Option<(PresetLink, Preset)>)> = doc
        .layers
        .iter()
        .filter_map(|l| l.link.as_ref().map(|k| (l.id.clone(), k.path.clone())))
        .filter_map(|(id, path)| {
            let preset = preset_for(st, &lib_name, &path).ok()?;
            Some((id, Some((PresetLink { library: lib_name.clone(), path }, preset))))
        })
        .collect();
    apply_links(st, plan).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    layer: String,
    material: String,
    #[serde(default)]
    thickness_mm: Option<f64>,
    #[serde(default)]
    title: String,
    description: String,
}

async fn entry_create(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: CreateBody = body(&b)?;
    let meta = EntryMeta { material: b.material, thickness_mm: b.thickness_mm, title: b.title, description: b.description };
    meta.check()?;
    let doc = st.document();
    let layer = doc.layer(&b.layer).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{}'", b.layer)))?;
    let entry = library::entry_from_layer(layer, meta.thickness_mm, &meta.title, &meta.description)?;
    let id = lock(&st.libs.library).add_entry(&name, &meta.material, entry)?;
    let (material, e) = { let s = lock(&st.libs.library); let (m, e) = s.entry(&name, id)?; (m.to_string(), e.link_path(m)) };
    Ok(Json(json!({ "ok": true, "id": entry_id(id), "material": material, "path": e })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    /// Update: the entry takes this layer's settings.
    #[serde(default)]
    from_layer: Option<String>,
    /// Edit Cut: the entry's new settings (complete CutSettings) ...
    #[serde(default)]
    settings: Option<doc::CutSettings>,
    /// ... and sub-layers.
    #[serde(default)]
    sub_layers: Option<Vec<doc::CutSettings>>,
    /// Edit Description.
    #[serde(default)]
    meta: Option<EntryMeta>,
}

async fn entry_edit(
    State(st): State<Arc<AppState>>,
    UrlPath((name, id)): UrlPath<(String, String)>,
    b: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: EditBody = body(&b)?;
    let mut id = parse_entry_id(&id)?;
    let old_path = { let s = lock(&st.libs.library); let (m, e) = s.entry(&name, id)?; e.link_path(m) };
    let current = lock(&st.libs.library).entry(&name, id)?.1.clone();
    let new_settings = if let Some(layer_id) = &b.from_layer {
        let doc = st.document();
        let layer = doc.layer(layer_id).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{layer_id}'")))?;
        Some(library::entry_from_layer(layer, current.thickness(), current.title(), current.description())?)
    } else if b.settings.is_some() || b.sub_layers.is_some() {
        let p = preset_of(&current)?;
        let preset = Preset {
            settings: b.settings.unwrap_or(p.settings),
            sub_layers: b.sub_layers.unwrap_or(p.sub_layers),
            global_passes: p.global_passes,
            extras: p.extras,
        };
        // Checked as layer.apply_preset checks a preset, on a scratch document.
        let mut scratch = commands::Session::new(doc::Document::default());
        let layer_id = scratch.document().layers[0].id.clone();
        scratch.execute(apply_invocation(&layer_id, &preset, None), Origin::Script)?;
        let l = scratch.document().layers[0].clone();
        Some(library::entry_from_layer(&l, current.thickness(), current.title(), current.description())?)
    } else {
        None
    };
    if new_settings.is_none() && b.meta.is_none() {
        return Err(EngineError::BadRequest("give from_layer, settings / sub_layers, or meta".into()));
    }
    if let Some(e) = new_settings {
        lock(&st.libs.library).replace_settings(&name, id, e)?;
    }
    if let Some(m) = &b.meta {
        id = lock(&st.libs.library).edit_meta(&name, id, m)?;
    }
    let new_path = { let s = lock(&st.libs.library); let (m, e) = s.entry(&name, id)?; e.link_path(m) };
    let synced = sync_linked(&st, &name, &old_path, Some(&new_path)).await?;
    Ok(Json(json!({ "ok": true, "id": entry_id(id), "path": new_path, "updated_layers": synced })))
}

async fn entry_duplicate(State(st): State<Arc<AppState>>, UrlPath((name, id)): UrlPath<(String, String)>) -> Result<Json<Value>, EngineError> {
    let id = lock(&st.libs.library).duplicate(&name, parse_entry_id(&id)?)?;
    Ok(Json(json!({ "ok": true, "id": entry_id(id) })))
}

async fn entry_delete(State(st): State<Arc<AppState>>, UrlPath((name, id)): UrlPath<(String, String)>) -> Result<Json<Value>, EngineError> {
    let path = lock(&st.libs.library).delete(&name, parse_entry_id(&id)?)?;
    let unlinked = sync_linked(&st, &name, &path, None).await?;
    Ok(Json(json!({ "ok": true, "path": path, "unlinked_layers": unlinked })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyBody {
    layer: String,
    #[serde(default)]
    link: bool,
    #[serde(default)]
    origin: Option<Value>,
}

async fn entry_apply(
    State(st): State<Arc<AppState>>,
    UrlPath((name, id)): UrlPath<(String, String)>,
    headers: HeaderMap,
    b: Bytes,
) -> Result<Json<Value>, EngineError> {
    let b: ApplyBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    let (lib_name, path, preset) = {
        let s = lock(&st.libs.library);
        let lib_name = s.get(&name)?.name.clone();
        let (m, e) = s.entry(&name, parse_entry_id(&id)?)?;
        (lib_name, e.link_path(m), preset_of(e)?)
    };
    let link = b.link.then_some(PresetLink { library: lib_name, path });
    let inv = apply_invocation(&b.layer, &preset, link.as_ref());
    let out = st.blocking(move |st| st.execute(inv, origin)).await?;
    Ok(Json(json!({ "ok": true, "result": out.result, "revision": out.revision })))
}

// ---------------------------------------------------------------------------------------------------------------
// Preferences and defaults
// ---------------------------------------------------------------------------------------------------------------

fn prefs_json(st: &AppState) -> Value {
    let device = st.active_device().id;
    let p = lock(&st.libs.prefs);
    json!({
        "device": device,
        "device_prefs": p.device(&device),
        "debug_log": p.prefs.debug_log,
        "check_for_updates": p.prefs.check_for_updates,
    })
}

async fn prefs_view(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(prefs_json(&st))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrefsBody {
    #[serde(default)]
    require_framing: Option<bool>,
    #[serde(default)]
    play_sound: Option<bool>,
    #[serde(default)]
    checklist: Option<Checklist>,
    #[serde(default)]
    load_defaults_on_new: Option<bool>,
    #[serde(default)]
    debug_log: Option<bool>,
    #[serde(default)]
    check_for_updates: Option<bool>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn prefs_edit(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: PrefsBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "preferences can be changed")?;
    if let Some(c) = &b.checklist
        && let Some(p) = c.problem()
    {
        return Err(EngineError::BadRequest(p));
    }
    let device = st.active_device().id;
    {
        let mut p = lock(&st.libs.prefs);
        let d = p.device_mut(&device);
        if let Some(v) = b.require_framing {
            d.require_framing = v;
        }
        if let Some(v) = b.play_sound {
            d.play_sound = v;
        }
        if let Some(c) = b.checklist {
            d.checklist = c;
        }
        if let Some(v) = b.load_defaults_on_new {
            d.load_defaults_on_new = v;
        }
        if let Some(v) = b.debug_log {
            p.prefs.debug_log = v;
            crate::diagnostics::set_debug(v);
        }
        if let Some(v) = b.check_for_updates {
            p.prefs.check_for_updates = v;
        }
        p.save()?;
    }
    st.emit(Event::machine("prefs", None));
    Ok(Json(prefs_json(&st)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct DefaultBody {
    #[serde(default)]
    all: bool,
    #[serde(default)]
    builtin: bool,
    #[serde(default)]
    origin: Option<Value>,
}

async fn layer_make_default(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: DefaultBody = body(&b)?;
    let doc = st.document();
    let l = doc.layer(&id).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{id}'")))?;
    let key = if b.all { ALL_COLORS.to_string() } else { l.color.to_ascii_lowercase() };
    let device = st.active_device().id;
    {
        let mut p = lock(&st.libs.prefs);
        let d = p.device_mut(&device);
        if b.all {
            // Make Default for All: one default for every colour.
            d.layer_defaults.clear();
        }
        d.layer_defaults.insert(key.clone(), LayerDefault { settings: l.settings.clone(), sub_layers: l.sub_layers.clone() });
        p.save()?;
    }
    Ok(Json(json!({ "ok": true, "color": key, "device": device })))
}

async fn layer_reset(State(st): State<Arc<AppState>>, UrlPath(id): UrlPath<String>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: DefaultBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    let doc = st.document();
    let l = doc.layer(&id).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{id}'")))?;
    let device = st.active_device().id;
    let saved = lock(&st.libs.prefs).device(&device).layer_default(&l.color).cloned();
    let d = match (b.builtin, saved) {
        (false, Some(d)) => d,
        _ => LayerDefault { settings: doc::CutSettings::default(), sub_layers: Vec::new() },
    };
    let preset = Preset { settings: d.settings, sub_layers: d.sub_layers, global_passes: 1, extras: Default::default() };
    let inv = apply_invocation(&id, &preset, None);
    let out = st.blocking(move |st| st.execute(inv, origin)).await?;
    Ok(Json(json!({ "ok": true, "result": out.result, "revision": out.revision })))
}

async fn optimization_make_default(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let o = st.document().optimization;
    let device = st.active_device().id;
    let mut p = lock(&st.libs.prefs);
    p.device_mut(&device).optimization_defaults = Some(o.clone());
    p.save()?;
    Ok(Json(json!({ "ok": true, "optimization": o })))
}

async fn optimization_forget_default(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let device = st.active_device().id;
    let mut p = lock(&st.libs.prefs);
    p.device_mut(&device).optimization_defaults = None;
    p.save()?;
    Ok(Json(json!({ "ok": true })))
}

async fn optimization_reset(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: DefaultBody = body(&b)?;
    let origin = origin_of(&headers, b.origin.as_ref())?;
    let device = st.active_device().id;
    let saved = lock(&st.libs.prefs).device(&device).optimization_defaults;
    let o = if b.builtin { doc::Optimization::default() } else { saved.unwrap_or_default() };
    let inv = Invocation { id: "job.set_optimization".into(), params: json!({ "optimization": o }) };
    let out = st.blocking(move |st| st.execute(inv, origin)).await?;
    Ok(Json(json!({ "ok": true, "result": out.result, "revision": out.revision })))
}

// ---------------------------------------------------------------------------------------------------------------
// Job log, diagnostics, updates
// ---------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct LogQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    format: Option<String>,
}

async fn job_log(State(st): State<Arc<AppState>>, Query(q): Query<LogQuery>) -> Result<Response, EngineError> {
    let entries = st.libs.read_job_log(q.limit.unwrap_or(200))?;
    match q.format.as_deref() {
        None | Some("json") => Ok(Json(json!({ "entries": entries })).into_response()),
        Some("csv") => Ok(attachment("OpenEtchJobs.csv", "text/csv", crate::joblog::to_csv(&entries).into_bytes())),
        Some(f) => Err(EngineError::BadRequest(format!("unknown format '{f}' (json or csv)"))),
    }
}

async fn diagnostics_report(State(st): State<Arc<AppState>>) -> Response {
    let r = crate::diagnostics::report(&st);
    let text = serde_json::to_vec_pretty(&r).unwrap_or_default();
    attachment("openetch-bug-report.json", "application/json", text)
}

async fn diagnostics_logs() -> Json<Value> {
    Json(json!({ "debug": crate::diagnostics::debug_enabled(), "lines": crate::diagnostics::lines() }))
}

async fn updates_view(State(st): State<Arc<AppState>>) -> Json<Value> {
    let enabled = lock(&st.libs.prefs).prefs.check_for_updates;
    Json(json!({ "enabled": enabled, "current": env!("CARGO_PKG_VERSION"), "repository": crate::updates::REPOSITORY, "last": *lock(&st.libs.updates) }))
}

async fn updates_check(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    if !lock(&st.libs.prefs).prefs.check_for_updates {
        return Err(EngineError::Conflict("the update check is off; turn on check_for_updates in the preferences first".into()));
    }
    let r = tokio::task::spawn_blocking(crate::updates::check_now).await?;
    *lock(&st.libs.updates) = Some(r.clone());
    Ok(Json(json!(r)))
}

// ---------------------------------------------------------------------------------------------------------------
// Find My Laser / Add Galvo
// ---------------------------------------------------------------------------------------------------------------

async fn devices_scan() -> Result<Json<Value>, EngineError> {
    #[cfg(feature = "usb")]
    {
        let boards = tokio::task::spawn_blocking(machine::detect).await?.map_err(|e| EngineError::Internal(e.to_string()))?;
        Ok(Json(json!({ "usb_support": true, "boards": boards })))
    }
    #[cfg(not(feature = "usb"))]
    {
        Ok(Json(json!({ "usb_support": false, "boards": [], "note": "this engine was built without USB support (feature usb)" })))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WizardBody {
    #[serde(default)]
    name: Option<String>,
    /// The EzCad `markcfg7` file, standard base64 (EzCad writes it in the Windows code page, not UTF-8).
    markcfg_base64: String,
    #[serde(default)]
    cor_name: Option<String>,
    /// The `.cor` file, standard base64.
    #[serde(default)]
    cor_base64: Option<String>,
    #[serde(default)]
    activate: bool,
    #[serde(default)]
    origin: Option<Value>,
}

/// Add Galvo in one pass: a device from the EzCad `markcfg7`, its correction file, and (optionally) made active.
async fn devices_wizard(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Response, EngineError> {
    use base64::Engine as _;
    let b: WizardBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "devices can be added")?;
    let cor = match (&b.cor_name, &b.cor_base64) {
        (Some(n), Some(data)) => Some((
            n.clone(),
            base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| EngineError::BadRequest(format!("cor_base64: {e}")))?,
        )),
        (None, None) => None,
        _ => return Err(EngineError::BadRequest("give cor_name and cor_base64 together".into())),
    };
    let name = b.name.clone().unwrap_or_default();
    let markcfg = base64::engine::general_purpose::STANDARD
        .decode(b.markcfg_base64.trim())
        .map_err(|e| EngineError::BadRequest(format!("markcfg_base64: {e}")))?;
    let device = st
        .blocking(move |st| {
            let mut reg = st.devices();
            let d = reg.import_markcfg(&markcfg, &name)?;
            match cor {
                Some((n, bytes)) => match reg.set_cor(&d.id, Some((&n, &bytes))) {
                    Ok(d) => Ok(d),
                    Err(e) => {
                        let _ = reg.remove(&d.id);
                        Err(e)
                    }
                },
                None => Ok(d),
            }
        })
        .await?;
    st.emit(Event::DevicesChanged);
    let mut out = json!({ "ok": true, "device": device.summary() });
    if b.activate {
        out["activated"] = st.activate_device(&device.id).await?;
    }
    Ok((StatusCode::CREATED, Json(out)).into_response())
}

// ---------------------------------------------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------------------------------------------

fn png(img: &image::DynamicImage) -> Result<Vec<u8>, EngineError> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| EngineError::Internal(e.to_string()))?;
    Ok(out.into_inner())
}

async fn camera_view(State(st): State<Arc<AppState>>) -> Json<Value> {
    let device = st.active_device().id;
    let c = lock(&st.libs.camera);
    let frame = c.frame.as_ref().map(|f| json!({ "width": f.width(), "height": f.height(), "unix_s": c.frame_unix_s }));
    Json(json!({
        "capture_support": cfg!(feature = "camera"),
        "frame": frame,
        "calibration": c.calibrations.get(&device),
    }))
}

fn store_frame(st: &AppState, img: image::RgbImage) -> Value {
    let mut c = lock(&st.libs.camera);
    let v = json!({ "ok": true, "width": img.width(), "height": img.height() });
    c.frame = Some(img);
    c.frame_unix_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    v
}

async fn camera_frame(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let img = tokio::task::spawn_blocking(move || camera::decode_frame(&b))
        .await?
        .map_err(|e| EngineError::BadRequest(format!("not a usable PNG or JPEG frame: {e}")))?;
    Ok(Json(store_frame(&st, img)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CaptureBody {
    #[serde(default)]
    index: u32,
}

async fn camera_capture(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: CaptureBody = body(&b)?;
    let img = tokio::task::spawn_blocking(move || crate::camera_capture::grab(b.index)).await?.map_err(EngineError::BadRequest)?;
    Ok(Json(store_frame(&st, img)))
}

async fn camera_frame_png(State(st): State<Arc<AppState>>) -> Result<Response, EngineError> {
    let f = lock(&st.libs.camera).frame.clone().ok_or_else(|| EngineError::NotFound("no camera frame yet".into()))?;
    let bytes = tokio::task::spawn_blocking(move || png(&image::DynamicImage::ImageRgb8(f))).await??;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], bytes).into_response())
}

#[derive(Deserialize)]
struct OverlayQuery {
    #[serde(default)]
    px_per_mm: Option<f64>,
}

async fn camera_overlay_png(State(st): State<Arc<AppState>>, Query(q): Query<OverlayQuery>) -> Result<Response, EngineError> {
    let device = st.active_device().id;
    let (frame, cal) = {
        let c = lock(&st.libs.camera);
        (c.frame.clone(), c.calibrations.get(&device).cloned().unwrap_or_default())
    };
    let frame = frame.ok_or_else(|| EngineError::NotFound("no camera frame yet".into()))?;
    let align = cal.alignment.ok_or_else(|| EngineError::Conflict("the camera is not aligned to the field yet (POST /api/camera/align)".into()))?;
    let lens = cal.lens.unwrap_or(camera::Lens::identity(frame.width(), frame.height()));
    let field = st.document().field_mm.unwrap_or(st.machine.profile().field_mm);
    let ppm = q.px_per_mm.unwrap_or(4.0);
    let bytes = tokio::task::spawn_blocking(move || {
        let o = camera::overlay(&frame, &lens, &align, field, ppm).map_err(EngineError::BadRequest)?;
        png(&image::DynamicImage::ImageRgba8(o))
    })
    .await??;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-store")], bytes).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetectBody {
    cols: usize,
    rows: usize,
}

async fn camera_detect(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: DetectBody = body(&b)?;
    if !camera::board_size_ok(b.cols, b.rows) {
        return Err(EngineError::BadRequest(format!("cols and rows must be within 3..={}", camera::MAX_BOARD_SIDE)));
    }
    let f = lock(&st.libs.camera).frame.clone().ok_or_else(|| EngineError::NotFound("no camera frame yet".into()))?;
    let corners = tokio::task::spawn_blocking(move || {
        let g = camera::Gray::from_luma8(&image::DynamicImage::ImageRgb8(f).to_luma8());
        camera::detect_checkerboard(&g, b.cols, b.rows)
    })
    .await?;
    Ok(Json(json!({ "found": corners.is_some(), "corners": corners, "cols": b.cols, "rows": b.rows })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LensBody {
    width: u32,
    height: u32,
    views: Vec<View>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn camera_lens(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: LensBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the camera can be calibrated")?;
    if b.views.len() > 50 || b.views.iter().any(|v| v.corners.len() > 2000) {
        return Err(EngineError::BadRequest("at most 50 views of at most 2000 corners".into()));
    }
    let (w, h, views) = (b.width, b.height, b.views);
    let lens = tokio::task::spawn_blocking(move || camera::calibrate_lens(w, h, &views)).await?.map_err(EngineError::BadRequest)?;
    let device = st.active_device().id;
    let mut c = lock(&st.libs.camera);
    let cal = c.calibrations.entry(device).or_default();
    cal.lens = Some(lens);
    // A new lens model invalidates the alignment made through the old one.
    cal.alignment = None;
    c.save()?;
    Ok(Json(json!({ "ok": true, "lens": lens })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AlignBody {
    image: Vec<[f64; 2]>,
    field: Vec<[f64; 2]>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn camera_align(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: AlignBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the camera can be aligned")?;
    if b.image.len() > 100 {
        return Err(EngineError::BadRequest("at most 100 alignment points".into()));
    }
    let device = st.active_device().id;
    let mut c = lock(&st.libs.camera);
    let (fw, fh) = c.frame.as_ref().map_or((0, 0), |f| (f.width(), f.height()));
    let cal = c.calibrations.entry(device).or_default();
    let lens = cal.lens.unwrap_or(camera::Lens::identity(fw, fh));
    let a = camera::align(&lens, &b.image, &b.field).map_err(EngineError::BadRequest)?;
    cal.alignment = Some(a.clone());
    c.save()?;
    Ok(Json(json!({ "ok": true, "alignment": a })))
}

/// The markers in the latest frame.
async fn find_markers(st: &AppState) -> Result<Vec<crate::markers::Marker>, EngineError> {
    let f = lock(&st.libs.camera).frame.clone().ok_or_else(|| EngineError::NotFound("no camera frame yet".into()))?;
    Ok(tokio::task::spawn_blocking(move || {
        let g = camera::Gray::from_luma8(&image::DynamicImage::ImageRgb8(f).to_luma8());
        crate::markers::detect_markers(&g)
    })
    .await?)
}

async fn camera_markers(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let markers = find_markers(&st).await?;
    Ok(Json(json!({ "markers": markers })))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AlignMarkersBody {
    #[serde(default)]
    origin: Option<Value>,
}

async fn camera_align_markers(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: AlignMarkersBody = if b.iter().all(u8::is_ascii_whitespace) { AlignMarkersBody::default() } else { body(&b)? };
    ui_only(&headers, b.origin.as_ref(), "the camera can be aligned")?;
    let found = find_markers(&st).await?;
    let placed = commands::camera_markers::document_markers(&st.document());
    let (mut image, mut field, mut used, mut unknown) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for m in &found {
        match placed.iter().find(|(id, _)| *id == m.id) {
            Some((_, f)) => {
                image.push(m.center);
                field.push(*f);
                used.push(m.id);
            }
            None => unknown.push(m.id),
        }
    }
    let missing: Vec<usize> = placed.iter().map(|(id, _)| *id).filter(|id| !used.contains(id)).collect();
    if image.len() < 4 {
        return Err(EngineError::Conflict(format!(
            "{} of the document's camera markers were found in the frame (ids {used:?}; missing {missing:?}); at least four are needed",
            image.len()
        )));
    }
    let device = st.active_device().id;
    let mut c = lock(&st.libs.camera);
    let (fw, fh) = c.frame.as_ref().map_or((0, 0), |f| (f.width(), f.height()));
    let cal = c.calibrations.entry(device).or_default();
    let lens = cal.lens.unwrap_or(camera::Lens::identity(fw, fh));
    let a = camera::align(&lens, &image, &field).map_err(EngineError::BadRequest)?;
    cal.alignment = Some(a.clone());
    c.save()?;
    Ok(Json(json!({ "ok": true, "alignment": a, "used": used, "missing": missing, "unknown": unknown })))
}

async fn camera_forget(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Result<Json<Value>, EngineError> {
    ui_only(&headers, None, "the camera calibration can be cleared")?;
    let device = st.active_device().id;
    let mut c = lock(&st.libs.camera);
    c.calibrations.remove(&device);
    c.save()?;
    Ok(Json(json!({ "ok": true })))
}
