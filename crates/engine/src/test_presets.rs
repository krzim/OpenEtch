//! Material test presets (LightBurn's Material Test Preset list, LAY-070): named `generate.material_test`
//! parameters kept in `<config-dir>/material-test-presets.json`, shared by every client, plus LightBurn's
//! `material_test_presets.lbmt` import and export (`commands::testgen_presets` has the format). Routes are merged
//! into `api::router`.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/material-test/presets` | `{presets: [{name, params}]}` sorted by name |
//! | PUT | `/api/material-test/presets/{name}` | `{params}` (a `generate.material_test` input, checked by running it on an empty document) → `{ok, name, replaced}`; replaces a preset of that name |
//! | DELETE | `/api/material-test/presets/{name}` | → `{ok}`; 404 when there is none |
//! | POST | `/api/material-test/presets/import` | a LightBurn `.lbmt` file as the body, `?replace=true` to replace presets of the same name (default: the import is renamed "<name> (2)") → `{ok, imported: [name], warnings}` |
//! | GET | `/api/material-test/presets/export` | `?names=a,b` (default all): LightBurn's `material_test_presets.lbmt` as a download; `X-OpenEtch-Warnings` lists what LightBurn's file cannot hold |
//!
//! Positions convert at the active device's LightBurn field (`XCenter` counts from the field's lower-left corner).
//!
//! **Adjust Image presets** (LightBurn's Adjust Image Presets list, OTH-022), in `<config-dir>/image-presets.json`:
//! a preset holds the image adjustments (`adjust`, `doc::ImageAdjust`) and/or the layer's image mode settings
//! (`image`, a partial `layer.update` `settings.image`), as LightBurn's Save Preset keeps a chosen subset. Applying
//! one is `image.adjust` plus `layer.update` from the client. LightBurn's built-in "Basic" and "Black Paint on White"
//! presets are not included (their values are not documented).
//!
//! **LightBurn's `.lbimg`** (Adjust Image > Presets > Export, LightBurn 2.2; the owner's sample): plain JSON, one
//! object per preset name, written by Qt (4-space indent, sorted keys, float32 values as doubles):
//! `{"<name>": {"Brightness": 0.1, "Contrast": 0.25, "EnhanceAmount": 0.5, "EnhanceRadius": 3, "Gamma": 1.2,
//! "HalftoneAngle": 22.5, "HalftoneCells": 50, "ImgInterval": 0.1, "ImgMode": "Stucki", "Negative": false}}`.
//! Brightness, Contrast and EnhanceAmount are fractions of OpenEtch's % (as in `.lbrn2`), EnhanceRadius pixels,
//! ImgInterval mm, HalftoneCells cells per inch, ImgMode the dialog's mode name. Import maps a preset to `adjust` plus
//! `image` (`dither`, `interval_mm`, `negative`, `halftone_angle_deg`, `halftone_cells_per_inch`), the float32 noise
//! rounded off (1.2000000476837158 → 1.2); export writes every key, filling what a preset lacks with OpenEtch's
//! defaults (LightBurn's 0.1 mm interval).
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET | `/api/image-adjust/presets` | `{presets: [{name, adjust?, image?}]}` (also the export: the same JSON) |
//! | PUT | `/api/image-adjust/presets/{name}` | `{adjust?, image?}` (at least one; checked) → `{ok, name, replaced}` |
//! | DELETE | `/api/image-adjust/presets/{name}` | → `{ok}` |
//! | POST | `/api/image-adjust/presets/import` | `{presets: [...]}` as GET gives them, or LightBurn's `.lbimg` → `{ok, imported: [name], warnings}` (names taken get " (2)") |
//! | GET | `/api/image-adjust/presets/export` | `?format=lbimg`: every preset as LightBurn's `.lbimg` (a download) |
//!
//! LightBurn `.lbzip` bundles carry the same `.lbimg` as `image_presets.lbimg` (`import_lbimg`, `export_lbimg`;
//! `crate::lbzip`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Bytes;
use axum::extract::rejection::QueryRejection;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use commands::testgen_presets::{self, MAX_LBMT_BYTES, MAX_PRESETS, TestPreset};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{AppState, EngineError};

/// The presets file in the config directory.
pub const PRESETS_FILE: &str = "material-test-presets.json";
// The preset types, checks and LightBurn .lbimg conversion (shared with the browser core).
pub use appcore::presets::{ImagePreset, MAX_NAME_CHARS, lbimg_to_presets, presets_to_lbimg};
use appcore::presets::numbered;

/// Check an image preset (`appcore::presets::check_image_preset`).
fn check_image_preset(p: &ImagePreset) -> Result<(), EngineError> {
    Ok(appcore::presets::check_image_preset(p)?)
}

/// Check a preset name (`appcore::presets::check_name`).
fn check_name(name: &str) -> Result<String, EngineError> {
    Ok(appcore::presets::check_name(name)?)
}

/// Serialises read-modify-write of the presets file.
static WRITE: Mutex<()> = Mutex::new(());

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/material-test/presets", get(list))
        .route("/material-test/presets/import", post(import).layer(DefaultBodyLimit::max(MAX_LBMT_BYTES)))
        .route("/material-test/presets/export", get(export))
        .route("/material-test/presets/{name}", axum::routing::put(save).delete(remove))
        .route("/image-adjust/presets", get(image_list))
        .route("/image-adjust/presets/import", post(image_import))
        .route("/image-adjust/presets/export", get(image_export))
        .route("/image-adjust/presets/{name}", axum::routing::put(image_save).delete(image_remove))
}

/// The Adjust Image presets file in the config directory.
pub const IMAGE_PRESETS_FILE: &str = "image-presets.json";

fn image_path(st: &AppState) -> Result<PathBuf, EngineError> {
    Ok(st.files.config_dir()?.join(IMAGE_PRESETS_FILE))
}

fn image_load(st: &AppState) -> Result<Vec<ImagePreset>, EngineError> {
    let p = image_path(st)?;
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| EngineError::Internal(format!("{} is not usable ({e}); fix or remove it", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(EngineError::Internal(format!("cannot read {}: {e}", p.display()))),
    }
}

fn image_store(st: &AppState, mut presets: Vec<ImagePreset>) -> Result<(), EngineError> {
    if presets.len() > MAX_PRESETS {
        return Err(EngineError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    presets.sort_by_key(|p| p.name.to_lowercase());
    let text = serde_json::to_string_pretty(&presets).map_err(|e| EngineError::Internal(e.to_string()))?;
    crate::devices::write_atomic(&st.files.config_dir()?, IMAGE_PRESETS_FILE, &text).map_err(EngineError::Internal)
}

async fn image_list(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let presets = st.blocking(image_load).await?;
    Ok(Json(json!({ "presets": presets })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageSaveBody {
    #[serde(default)]
    adjust: Option<doc::ImageAdjust>,
    #[serde(default)]
    image: Option<Value>,
    #[serde(default)]
    #[allow(dead_code)]
    origin: Option<Value>,
}

async fn image_save(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let name = check_name(&name)?;
    let b: ImageSaveBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let preset = ImagePreset { name: name.clone(), adjust: b.adjust, image: b.image };
    check_image_preset(&preset)?;
    st.blocking(move |st| {
        let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all = image_load(st)?;
        let before = all.len();
        all.retain(|p| p.name != name);
        let replaced = all.len() != before;
        all.push(preset);
        image_store(st, all)?;
        Ok(Json(json!({ "ok": true, "name": name, "replaced": replaced })))
    })
    .await
}

async fn image_remove(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    st.blocking(move |st| {
        let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all = image_load(st)?;
        let before = all.len();
        all.retain(|p| p.name != name);
        if all.len() == before {
            return Err(EngineError::NotFound(format!("no image preset named \"{name}\"")));
        }
        image_store(st, all)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageImportBody {
    presets: Vec<ImagePreset>,
}

async fn image_export(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["format"])?;
    if q.get("format").map(String::as_str) != Some("lbimg") {
        return Err(EngineError::BadRequest("format must be lbimg (GET /api/image-adjust/presets is OpenEtch's own export)".into()));
    }
    let text = st.blocking(|st| image_load(st).map(|all| presets_to_lbimg(&all))).await?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"openetch-presets.lbimg\"".to_owned()),
        ],
        text,
    )
        .into_response())
}

async fn image_import(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let v: Value = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("not a preset file (not JSON): {e}")))?;
    let (b, warnings) = if v.get("presets").is_some_and(Value::is_array) {
        let b: ImageImportBody = serde_json::from_value(v).map_err(|e| EngineError::BadRequest(format!("not an OpenEtch image preset export: {e}")))?;
        (b, Vec::new())
    } else {
        let (presets, warnings) = lbimg_to_presets(&v)?;
        (ImageImportBody { presets }, warnings)
    };
    if b.presets.len() > MAX_PRESETS {
        return Err(EngineError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    for p in &b.presets {
        check_name(&p.name)?;
        check_image_preset(p)?;
    }
    st.blocking(move |st| image_add(st, b.presets, warnings).map(Json)).await
}

/// Add checked image presets (a name taken gets " (2)", ...). Returns `{ok, imported: [name], warnings}`.
fn image_add(st: &AppState, presets: Vec<ImagePreset>, warnings: Vec<String>) -> Result<Value, EngineError> {
    let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut all = image_load(st)?;
    let mut imported = Vec::new();
    for mut p in presets {
        p.name = p.name.trim().to_owned();
        if all.iter().any(|o| o.name == p.name) {
            let base = p.name.clone();
            p.name = (2..).map(|n| numbered(&base, n)).find(|c| !all.iter().any(|o| &o.name == c)).unwrap_or(base);
        }
        imported.push(p.name.clone());
        all.push(p);
    }
    image_store(st, all)?;
    Ok(json!({ "ok": true, "imported": imported, "warnings": warnings }))
}

/// Largest `.lbimg` read from a `.lbzip` bundle (the HTTP import takes axum's default 2 MB body).
pub const MAX_LBIMG_BYTES: usize = 2 * 1024 * 1024;

/// A LightBurn `.lbimg` file (`image_presets.lbimg` of a `.lbzip` bundle) added to the image presets, as
/// `POST /api/image-adjust/presets/import` adds one. Returns `{ok, imported: [name], warnings}`.
pub fn import_lbimg(st: &AppState, bytes: &[u8]) -> Result<Value, EngineError> {
    if bytes.len() > MAX_LBIMG_BYTES {
        return Err(EngineError::TooLarge(format!("an image preset file may be at most {} MB", MAX_LBIMG_BYTES / (1024 * 1024))));
    }
    let v: Value = serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))
        .map_err(|e| EngineError::BadRequest(format!("not a LightBurn .lbimg preset file (not JSON): {e}")))?;
    let (presets, warnings) = lbimg_to_presets(&v)?;
    image_add(st, presets, warnings)
}

/// Every image preset as LightBurn's `.lbimg` text, `None` when there are none.
pub fn export_lbimg(st: &AppState) -> Result<Option<String>, EngineError> {
    let all = image_load(st)?;
    Ok((!all.is_empty()).then(|| presets_to_lbimg(&all)))
}

fn path(st: &AppState) -> Result<PathBuf, EngineError> {
    Ok(st.files.config_dir()?.join(PRESETS_FILE))
}

/// The saved presets; a missing file is none, an unreadable one an error (never silently replaced).
fn load(st: &AppState) -> Result<Vec<TestPreset>, EngineError> {
    let p = path(st)?;
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_json::from_str::<Vec<TestPreset>>(&t)
            .map_err(|e| EngineError::Internal(format!("{} is not usable ({e}); fix or remove it", p.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(EngineError::Internal(format!("cannot read {}: {e}", p.display()))),
    }
}

fn store(st: &AppState, mut presets: Vec<TestPreset>) -> Result<(), EngineError> {
    if presets.len() > MAX_PRESETS {
        return Err(EngineError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    presets.sort_by_key(|p| p.name.to_lowercase());
    let text = serde_json::to_string_pretty(&presets).map_err(|e| EngineError::Internal(e.to_string()))?;
    crate::devices::write_atomic(&st.files.config_dir()?, PRESETS_FILE, &text).map_err(EngineError::Internal)
}

/// The field LightBurn files of the active device are laid out for.
fn field(st: &AppState) -> f64 {
    st.active_device().lightburn_field_mm().unwrap_or_else(|| st.machine.profile().field_mm)
}

fn command_error(e: commands::CommandError) -> EngineError {
    EngineError::BadRequest(e.to_string())
}

async fn list(State(st): State<Arc<AppState>>) -> Result<Json<Value>, EngineError> {
    let mut presets = st.blocking(load).await?;
    presets.sort_by_key(|p| p.name.to_lowercase());
    Ok(Json(json!({ "presets": presets })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveBody {
    params: Value,
    #[serde(default)]
    #[allow(dead_code)]
    origin: Option<Value>,
}

async fn save(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, body: Bytes) -> Result<Json<Value>, EngineError> {
    let name = check_name(&name)?;
    let b: SaveBody = serde_json::from_slice(&body).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))?;
    let replaced = st
        .blocking(move |st| {
            testgen_presets::check_material_test_params(&b.params).map_err(command_error)?;
            let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
            let mut all = load(st)?;
            let before = all.len();
            all.retain(|p| p.name != name);
            let replaced = all.len() != before;
            all.push(TestPreset { name: name.clone(), params: b.params });
            store(st, all)?;
            Ok(json!({ "ok": true, "name": name, "replaced": replaced }))
        })
        .await?;
    Ok(Json(replaced))
}

async fn remove(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<Json<Value>, EngineError> {
    st.blocking(move |st| {
        let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all = load(st)?;
        let before = all.len();
        all.retain(|p| p.name != name);
        if all.len() == before {
            return Err(EngineError::NotFound(format!("no material test preset named \"{name}\"")));
        }
        store(st, all)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

type Q = Result<Query<HashMap<String, String>>, QueryRejection>;

fn query(q: Q, allowed: &[&str]) -> Result<HashMap<String, String>, EngineError> {
    let Query(q) = q.map_err(|e| EngineError::BadRequest(format!("bad query: {e}")))?;
    if let Some(k) = q.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: {})", allowed.join(", "))));
    }
    Ok(q)
}

async fn import(State(st): State<Arc<AppState>>, q: Q, body: Bytes) -> Result<Json<Value>, EngineError> {
    let q = query(q, &["replace"])?;
    let replace = match q.get("replace").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(v) => return Err(EngineError::BadRequest(format!("replace must be true or false (got '{v}')"))),
    };
    let text = String::from_utf8(body.to_vec()).map_err(|_| EngineError::BadRequest("the preset file is not UTF-8 text".into()))?;
    st.blocking(move |st| import_lbmt(st, &text, replace).map(Json)).await
}

/// Import a LightBurn `material_test_presets.lbmt` (also from a `.lbzip` bundle): `replace` replaces presets of the
/// same name, else the import is renamed "<name> (2)". Returns `{ok, imported: [name], warnings}`.
pub fn import_lbmt(st: &AppState, text: &str, replace: bool) -> Result<Value, EngineError> {
    let (presets, warnings) = testgen_presets::lbmt_to_presets(text, field(st)).map_err(command_error)?;
    let _w = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut all = load(st)?;
    let mut imported = Vec::new();
    for mut p in presets {
        if replace {
            all.retain(|o| o.name != p.name);
        } else if all.iter().any(|o| o.name == p.name) {
            let base = p.name.clone();
            p.name = (2..).map(|n| numbered(&base, n)).find(|c| !all.iter().any(|o| &o.name == c)).unwrap_or(base);
        }
        imported.push(p.name.clone());
        all.push(p);
    }
    store(st, all)?;
    Ok(json!({ "ok": true, "imported": imported, "warnings": warnings }))
}

/// Every material test preset as LightBurn's `material_test_presets.lbmt` text, `None` when there are none, and what
/// LightBurn's file cannot hold.
pub fn export_lbmt(st: &AppState) -> Result<Option<(String, Vec<String>)>, EngineError> {
    let all = load(st)?;
    Ok((!all.is_empty()).then(|| testgen_presets::presets_to_lbmt(&all, field(st))))
}

async fn export(State(st): State<Arc<AppState>>, q: Q) -> Result<Response, EngineError> {
    let q = query(q, &["names"])?;
    let names: Option<Vec<String>> = q.get("names").map(|n| n.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect());
    let (text, warnings) = st
        .blocking(move |st| {
            let mut all = load(st)?;
            if let Some(n) = &names {
                if let Some(missing) = n.iter().find(|n| !all.iter().any(|p| &&p.name == n)) {
                    return Err(EngineError::NotFound(format!("no material test preset named \"{missing}\"")));
                }
                all.retain(|p| n.contains(&p.name));
            }
            Ok(testgen_presets::presets_to_lbmt(&all, field(st)))
        })
        .await?;
    // Header values must be visible ASCII: anything else becomes '?'.
    let header_text: String = warnings.join(" | ").chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' }).take(4000).collect();
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"material_test_presets.lbmt\"".to_owned()),
            (header::HeaderName::from_static("x-openetch-warnings"), header_text),
        ],
        text,
    )
        .into_response())
}
