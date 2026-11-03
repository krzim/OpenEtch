//! MOPA materials (merged into `api::router`): the finish library (photo, result colour, notes, lens, focus, lot,
//! surface of each material library entry; `crate::finish`), the library as a colour palette, the hue step nudge, the
//! bracket test and colour separation. The functions here are shared with MCP (`crate::mcp_materials`): design and
//! library edits only, nothing here can frame, run or approve. The hue step model and its calibration change a
//! device setting and are UI only, like the other preferences.
//!
//! | Method | Path | Body / result |
//! |---|---|---|
//! | GET / PUT | `/api/library/{name}/entries/{id}/finish` | the entry's finish / [`FinishPatch`] (`photo_base64` + `photo_mime`: a new photo; `remove_photo`) → `{ok, path, finish}` |
//! | GET | `/api/library/{name}/entries/{id}/photo` | the result photo |
//! | GET | `/api/library/{name}/finish/download` | the sidecar (`<name>.oefinish.json`) |
//! | POST | `/api/library/{name}/finish/import` | a sidecar as the body → `{ok, imported, skipped}` |
//! | GET | `/api/materials/palette` | `?library=&surface=&material=` → `{library, swatches: [...]}` (entries with a result colour) |
//! | GET / PUT | `/api/materials/hue-model` | `{device, model, default}` / `{model \| null, origin}` (UI only) |
//! | POST | `/api/materials/hue-model/calibrate` | `{best, other, hues?, origin}` (UI only): two bracket boxes a known number of hues apart set that setting's step |
//! | POST | `/api/materials/nudge` | [`NudgeArgs`] → `{settings, param, delta, warnings, applied, ...}` |
//! | POST | `/api/materials/bracket` | [`BracketArgs`] → `{ok, document, result}` |
//! | POST | `/api/materials/bracket/pick` | `{layer}`: write the box's settings back to its library entry → `{ok, library, id, path, cell}` |
//! | POST | `/api/materials/separate` | [`SeparateArgs`] → `image.color_separate`'s result (or the preview's) |

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::bracket::{self, BracketAxis};
use commands::finish::{HueParam, SurfaceFinish};
use commands::{Invocation, Origin};
use doc::PresetLink;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api_libs::{apply_invocation, attachment, body, origin_of, sync_linked, ui_only};
use crate::finish::{self, FinishPatch, now_unix_s};
use crate::libs::lock;
use crate::library::{self, EntryId, entry_id, parse_entry_id, preset_of};
use crate::{AppState, EngineError, Event};

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/library/{name}/entries/{id}/finish",
            get(finish_get).put(finish_put).layer(DefaultBodyLimit::max(finish::MAX_PHOTO_BYTES / 3 * 4 + 64 * 1024)),
        )
        .route("/library/{name}/entries/{id}/photo", get(photo_get))
        .route("/library/{name}/finish/download", get(sidecar_download))
        .route("/library/{name}/finish/import", post(sidecar_import).layer(DefaultBodyLimit::max(finish::MAX_SIDECAR_BYTES)))
        .route("/materials/palette", get(palette_get))
        .route("/materials/hue-model", get(hue_model_get).put(hue_model_put))
        .route("/materials/hue-model/calibrate", post(hue_calibrate))
        .route("/materials/nudge", post(nudge_post))
        .route("/materials/bracket", post(bracket_post))
        .route("/materials/bracket/pick", post(pick_post))
        .route("/materials/separate", post(separate_post))
}

/// Tell clients the libraries changed (they refetch `GET /api/library`; machine events also refetch the status).
pub(crate) fn library_changed(st: &AppState) {
    st.emit(Event::machine("library", None));
}

/// The library `name`, else the selected one.
pub(crate) fn library_name(st: &AppState, name: Option<&str>) -> Result<String, EngineError> {
    let s = lock(&st.libs.library);
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => Ok(s.get(n)?.name.clone()),
        None => s.selected.clone().ok_or_else(|| EngineError::NotFound("no material library is loaded".into())),
    }
}

/// An entry by id (`m<i>.e<j>`) or by LinkPath (`material/title/description`): its id and path.
pub(crate) fn resolve_entry(st: &AppState, lib: &str, entry: &str) -> Result<(EntryId, String), EngineError> {
    let s = lock(&st.libs.library);
    let l = s.get(lib)?;
    let id = match parse_entry_id(entry) {
        Ok(id) => id,
        Err(_) => l.lib.find_path(entry).ok_or_else(|| EngineError::NotFound(format!("library \"{}\" has no preset \"{entry}\"", l.name)))?,
    };
    let (m, e) = s.entry(lib, id)?;
    Ok((id, e.link_path(m)))
}

/// The libraries, the chosen one's tree with every entry's finish, and the active device's hue step model.
pub(crate) fn library_json(st: &AppState, name: Option<&str>) -> Result<Value, EngineError> {
    let device = st.active_device().id;
    let s = lock(&st.libs.library);
    let mut v = s.view();
    let chosen = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => Some(s.get(n)?),
        None => s.selected.as_deref().and_then(|n| s.find(n)),
    };
    if let Some(l) = chosen {
        v["library"] = json!(l.name);
        v["tree"] = library::tree(&l.lib, &l.finish);
    }
    v["hue_model"] = json!({ "device": device, "model": s.hue_model(&device), "calibrated": s.hue.contains_key(&device) });
    Ok(v)
}

/// The palette: entries with a result colour, filtered by surface and material.
pub(crate) fn palette_json(st: &AppState, name: Option<&str>, surface: Option<SurfaceFinish>, material: Option<&str>) -> Result<Value, EngineError> {
    let lib = library_name(st, name)?;
    let s = lock(&st.libs.library);
    let l = s.get(&lib)?;
    let mut swatches = Vec::new();
    for (mi, m) in l.lib.materials.iter().enumerate() {
        if material.is_some_and(|x| !x.eq_ignore_ascii_case(m.name())) {
            continue;
        }
        for (ei, e) in m.entries.iter().enumerate() {
            let path = e.link_path(m.name());
            let Some(f) = l.finish.get(&path) else { continue };
            let Some(color) = &f.color else { continue };
            if surface.is_some() && f.surface != surface {
                continue;
            }
            let summary = preset_of(e).ok().map(|p| {
                json!({ "mode": p.settings.mode, "power_pct": p.settings.power_pct, "speed_mm_s": p.settings.speed_mm_s,
                        "frequency_khz": p.settings.frequency_khz, "pulse_width_ns": p.settings.pulse_width_ns,
                        "interval_mm": p.settings.hatch.interval_mm, "passes": p.settings.passes })
            });
            let fs = f.summary();
            swatches.push(json!({
                "id": entry_id((mi, ei)), "path": path, "material": m.name(), "group": e.group_title(),
                "description": e.description(), "color": color, "surface": f.surface, "photo": fs.get("photo"),
                "photo_stale": fs.get("photo_stale"), "lot": f.lot, "date": f.date, "lens": f.lens, "focus_mm": f.focus_mm,
                "summary": summary,
            }));
        }
    }
    Ok(json!({ "library": l.name, "swatches": swatches }))
}

/// Change an entry's finish. Returns `{ok, library, id, path, finish}`.
pub(crate) fn finish_update(st: &AppState, lib: &str, entry: &str, patch: FinishPatch) -> Result<Value, EngineError> {
    let lib = library_name(st, Some(lib))?;
    let (id, path) = resolve_entry(st, &lib, entry)?;
    let f = lock(&st.libs.library).edit_finish(&lib, &path, |f| {
        finish::apply_patch(f, patch)?;
        Ok(f.clone())
    })?;
    library_changed(st);
    Ok(json!({ "ok": true, "library": lib, "id": entry_id(id), "path": path, "finish": f.summary() }))
}

/// `POST /api/materials/nudge`, MCP `material_hue_nudge`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NudgeArgs {
    /// Library (default the selected one), with `entry`.
    pub library: Option<String>,
    /// The library entry: its id (m<i>.e<j>) or LinkPath (material/title/description).
    pub entry: Option<String>,
    /// Or a document layer id (instead of an entry).
    pub layer: Option<String>,
    /// Hues warmer (positive: less energy) or cooler (negative). Default 0.
    #[serde(default)]
    pub steps: i32,
    /// auto (default), power, speed or frequency.
    #[serde(default)]
    pub via: HueParam,
    /// Adapt the setting to this surface (mill, brushed, polished, mirror): adds the model's power offset from the
    /// surface it was made for (an entry's recorded surface, else `from_surface`, else mill).
    pub surface: Option<SurfaceFinish>,
    /// The surface a layer's (or an unrecorded entry's) setting was made for. Default mill.
    pub from_surface: Option<SurfaceFinish>,
    /// Write the result (to the entry, unsaved until Save Library, with linked layers following; or to the layer as
    /// an undoable edit). Default false: only compute.
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    #[schemars(skip)]
    pub origin: Option<Value>,
}

/// The hue step for an entry or a layer with the active device's model (see [`NudgeArgs`]).
pub(crate) async fn hue_nudge(st: &Arc<AppState>, a: NudgeArgs, origin: Origin) -> Result<Value, EngineError> {
    let device = st.active_device().id;
    let model = lock(&st.libs.library).hue_model(&device);
    match (&a.entry, &a.layer) {
        (Some(entry), None) => {
            let lib = library_name(st, a.library.as_deref())?;
            let (id, path) = resolve_entry(st, &lib, entry)?;
            let (current, recorded) = {
                let s = lock(&st.libs.library);
                (s.entry(&lib, id)?.1.clone(), s.finish_of(&lib, &path)?.surface)
            };
            let preset = preset_of(&current)?;
            let mut warnings = Vec::new();
            let change = match a.surface {
                Some(to) => {
                    let from = recorded.or(a.from_surface).unwrap_or_else(|| {
                        warnings.push("the entry records no surface; mill was assumed".to_string());
                        SurfaceFinish::Mill
                    });
                    Some((from, to))
                }
                None => None,
            };
            let n = commands::finish::nudge(&preset.settings, a.steps, a.via, change, &model).map_err(EngineError::BadRequest)?;
            warnings.extend(n.warnings.iter().cloned());
            let mut out = json!({
                "target": "entry", "library": lib, "id": entry_id(id), "path": path, "settings": n.settings, "param": n.param,
                "delta": n.delta, "warnings": warnings, "applied": false, "model_source": model.source,
            });
            if a.apply {
                if n.param.is_none() && a.surface.is_none() {
                    return Err(EngineError::BadRequest("give steps or a surface".into()));
                }
                let new_preset = library::Preset { settings: n.settings.clone(), ..preset };
                // Checked as layer.apply_preset checks a preset, on a scratch document (as Edit Cut does).
                let mut scratch = commands::Session::new(doc::Document::default());
                let layer_id = scratch.document().layers[0].id.clone();
                scratch.execute(apply_invocation(&layer_id, &new_preset, None), Origin::Script)?;
                let l = scratch.document().layers[0].clone();
                let entry = library::entry_from_layer(&l, current.thickness(), current.title(), current.description())?;
                {
                    let mut s = lock(&st.libs.library);
                    s.replace_settings(&lib, id, entry)?;
                    let to = a.surface;
                    s.edit_finish(&lib, &path, |f| {
                        if let Some(t) = to {
                            f.surface = Some(t);
                        }
                        f.settings_unix_s = Some(now_unix_s());
                        Ok(())
                    })?;
                }
                let synced = sync_linked(st, &lib, &path, Some(&path)).await?;
                library_changed(st);
                out["applied"] = json!(true);
                out["updated_layers"] = json!(synced);
            }
            Ok(out)
        }
        (None, Some(layer)) => {
            let finish = a.surface.map(|to| json!({ "from": a.from_surface.unwrap_or(SurfaceFinish::Mill), "to": to }));
            let id = if a.apply { "layer.hue_nudge" } else { "material.hue_step" };
            let params = if a.apply {
                json!({ "layer": layer, "steps": a.steps, "via": a.via, "finish": finish, "model": model })
            } else {
                let doc = st.document();
                let l = doc.layer(layer).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{layer}'")))?;
                json!({ "settings": l.settings, "steps": a.steps, "via": a.via, "finish": finish, "model": model })
            };
            let inv = Invocation { id: id.into(), params };
            let o = st.blocking(move |st| st.execute(inv, origin)).await?;
            let mut r = o.result;
            r["target"] = json!("layer");
            r["applied"] = json!(a.apply);
            r["model_source"] = json!(model.source);
            if a.apply {
                r["revision"] = json!(o.revision);
            }
            Ok(r)
        }
        _ => Err(EngineError::BadRequest("give either entry (with library) or layer".into())),
    }
}

/// `POST /api/materials/bracket`, MCP `material_bracket_test`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BracketArgs {
    /// Library (default the selected one).
    pub library: Option<String>,
    /// The entry: id (m<i>.e<j>) or LinkPath.
    pub entry: String,
    /// Across the columns: {param, spread_pct (default 10), count (default 5)}. Default power.
    pub columns: Option<BracketAxis>,
    /// Down the rows. Default speed.
    pub rows: Option<BracketAxis>,
    /// Box size, mm. Default 5.
    pub cell_mm: Option<f64>,
    /// Make it in a new document (default true; it becomes the active document, which the person sees), else add it
    /// to the active one at its centre.
    pub new_document: Option<bool>,
    #[serde(default)]
    #[schemars(skip)]
    pub origin: Option<Value>,
}

pub(crate) async fn bracket_test(st: &Arc<AppState>, a: BracketArgs, origin: Origin) -> Result<Value, EngineError> {
    let lib = library_name(st, a.library.as_deref())?;
    let (id, path) = resolve_entry(st, &lib, &a.entry)?;
    let (preset, description) = {
        let s = lock(&st.libs.library);
        let e = s.entry(&lib, id)?.1;
        (preset_of(e)?, e.description().to_string())
    };
    let extras: serde_json::Map<String, Value> = preset.extras.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let mut params = json!({
        "settings": preset.settings, "sub_layers": preset.sub_layers, "extras": extras,
        "source": PresetLink { library: lib.clone(), path: path.clone() }, "title": format!("Bracket {description}"),
    });
    if let Some(c) = a.columns {
        params["columns"] = json!({ "param": bracket_param_name(&c), "spread_pct": c.spread_pct, "count": c.count });
    }
    if let Some(r) = a.rows {
        params["rows"] = json!({ "param": bracket_param_name(&r), "spread_pct": r.spread_pct, "count": r.count });
    }
    if let Some(c) = a.cell_mm {
        params["cell_mm"] = json!(c);
    }
    // Checked before a document is opened for it.
    let mut scratch = commands::Session::new(doc::Document::default());
    scratch.execute(Invocation { id: "generate.bracket_test".into(), params: params.clone() }, Origin::Script)?;
    let mut document = None;
    if a.new_document.unwrap_or(true) {
        let doc_id = st.new_document()?;
        st.activate_document(&doc_id, origin.clone())?;
        document = Some(doc_id);
    }
    let name = format!("Bracket {description}");
    let mut invs = vec![Invocation { id: "generate.bracket_test".into(), params }];
    if document.is_some() {
        invs.push(Invocation { id: "doc.rename".into(), params: json!({ "name": name }) });
    }
    let outs = st.blocking(move |st| st.batch(invs, origin)).await?;
    let out = outs.into_iter().next().ok_or_else(|| EngineError::Internal("the bracket test made nothing".into()))?;
    Ok(json!({ "ok": true, "document": document, "library": lib, "path": path, "result": out.result, "revision": out.revision }))
}

fn bracket_param_name(a: &BracketAxis) -> &'static str {
    bracket::param_name(a.param)
}

/// The bracket test box on layer `layer` of the active document: `(library, path, cell, the layer)`.
fn bracket_box(st: &AppState, layer: &str) -> Result<(String, String, String, doc::Layer, doc::Group), EngineError> {
    let doc = st.document();
    let l = doc.layer(layer).ok_or_else(|| EngineError::NotFound(format!("no layer with id '{layer}'")))?;
    let cell = l.extras.get(bracket::CELL_KEY).cloned().ok_or_else(|| EngineError::BadRequest(format!("layer \"{}\" is not a bracket test box", l.name)))?;
    let group = doc
        .objects
        .iter()
        .filter(|o| o.layer == l.id)
        .filter_map(|o| o.group.as_ref())
        .find_map(|g| doc.groups.iter().find(|x| &x.id == g && x.extras.contains_key(bracket::PATH_KEY)))
        .cloned()
        .ok_or_else(|| EngineError::BadRequest("this box's bracket test does not say which library preset it tests".into()))?;
    let lib = group.extras.get(bracket::LIBRARY_KEY).cloned().unwrap_or_default();
    let path = group.extras[bracket::PATH_KEY].clone();
    Ok((lib, path, cell, l.clone(), group))
}

/// Write a bracket box's settings back to the entry it tests (`POST /api/materials/bracket/pick`, MCP
/// `material_bracket_pick`).
pub(crate) async fn bracket_pick(st: &Arc<AppState>, layer: &str) -> Result<Value, EngineError> {
    let (lib, path, cell, l, _) = bracket_box(st, layer)?;
    let lib = library_name(st, Some(&lib))?;
    let (id, path) = resolve_entry(st, &lib, &path)?;
    {
        let mut s = lock(&st.libs.library);
        let current = s.entry(&lib, id)?.1.clone();
        let entry = library::entry_from_layer(&l, current.thickness(), current.title(), current.description())?;
        s.replace_settings(&lib, id, entry)?;
        s.edit_finish(&lib, &path, |f| {
            f.settings_unix_s = Some(now_unix_s());
            Ok(())
        })?;
    }
    let synced = sync_linked(st, &lib, &path, Some(&path)).await?;
    library_changed(st);
    Ok(json!({ "ok": true, "library": lib, "id": entry_id(id), "path": path, "cell": cell, "settings": l.settings, "updated_layers": synced }))
}

/// `POST /api/materials/separate`, MCP `material_color_separate`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeparateArgs {
    /// The image object id.
    pub image: String,
    /// Colours, 1..=16. Default 4.
    pub colors: Option<u32>,
    /// auto (default), none or #rrggbb.
    pub background: Option<String>,
    pub min_speck_px: Option<u32>,
    pub smoothness: Option<f64>,
    pub optimize: Option<f64>,
    pub max_side_px: Option<u32>,
    /// Largest colour difference (ΔE) for a preset match. Default 30.
    pub max_distance: Option<f64>,
    /// Colour indices (as the preview lists them) not to trace.
    #[serde(default)]
    pub skip: Vec<usize>,
    #[serde(default)]
    pub delete_image: bool,
    /// The library whose entries with a result colour are the swatches (default the selected one; "" for none).
    pub library: Option<String>,
    /// Link the layers to their presets (default true), else copy the settings.
    pub link: Option<bool>,
    /// Only preview (nothing changes).
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    #[schemars(skip)]
    pub origin: Option<Value>,
}

pub(crate) async fn color_separate(st: &Arc<AppState>, a: SeparateArgs, origin: Origin) -> Result<Value, EngineError> {
    let swatches: Vec<Value> = match a.library.as_deref().map(str::trim) {
        Some("") => Vec::new(),
        other => {
            let lib = match other {
                Some(n) => Some(library_name(st, Some(n))?),
                None => lock(&st.libs.library).selected.clone(),
            };
            match lib {
                Some(lib) => swatches_of(st, &lib, a.link.unwrap_or(true))?,
                None => Vec::new(),
            }
        }
    };
    let mut params = json!({ "id": a.image, "swatches": swatches, "skip": a.skip });
    for (k, v) in [
        ("colors", json!(a.colors)),
        ("background", json!(a.background)),
        ("min_speck_px", json!(a.min_speck_px)),
        ("smoothness", json!(a.smoothness)),
        ("optimize", json!(a.optimize)),
        ("max_side_px", json!(a.max_side_px)),
        ("max_distance", json!(a.max_distance)),
    ] {
        if !v.is_null() {
            params[k] = v;
        }
    }
    if !a.preview && a.delete_image {
        params["delete_image"] = json!(true);
    }
    let id = if a.preview { "image.color_separate_preview" } else { "image.color_separate" };
    let inv = Invocation { id: id.into(), params };
    let o = st.blocking(move |st| st.execute(inv, origin)).await?;
    let mut r = o.result;
    if !a.preview {
        r["revision"] = json!(o.revision);
    }
    Ok(r)
}

/// `doc.new {example}` without `finishes`: the selected library's entries with a result colour become the colour
/// examples' choices (`commands` module `examples`), so an example coin starts on the person's own finishes.
pub(crate) fn fill_example_finishes(st: &AppState, invs: &mut [Invocation]) {
    if !invs.iter().any(|i| i.id == "doc.new" && i.params.get("example").is_some_and(|e| !e.is_null()) && i.params.get("finishes").is_none()) {
        return;
    }
    let Some(lib) = lock(&st.libs.library).selected.clone() else { return };
    let Ok(swatches) = swatches_of(st, &lib, false) else { return };
    let finishes: Vec<Value> = swatches
        .into_iter()
        .map(|s| json!({ "label": s["name"], "color": s["color"], "settings": s["settings"], "sub_layers": s["sub_layers"] }))
        .collect();
    if finishes.is_empty() {
        return;
    }
    for inv in invs.iter_mut().filter(|i| i.id == "doc.new") {
        if let Some(p) = inv.params.as_object_mut()
            && p.get("example").is_some_and(|e| !e.is_null())
            && !p.contains_key("finishes")
        {
            p.insert("finishes".into(), json!(finishes));
        }
    }
}

/// The swatches of library `lib` for `image.color_separate`: its entries with a result colour.
fn swatches_of(st: &AppState, lib: &str, link: bool) -> Result<Vec<Value>, EngineError> {
    let s = lock(&st.libs.library);
    let l = s.get(lib)?;
    let mut out = Vec::new();
    for m in &l.lib.materials {
        for e in &m.entries {
            let path = e.link_path(m.name());
            let Some(color) = l.finish.get(&path).and_then(|f| f.color.clone()) else { continue };
            let Ok(p) = preset_of(e).map(|p| p.with_pauses(l.finish.get(&path))) else { continue };
            let mut v = json!({
                "color": color, "name": e.description(), "settings": p.settings, "sub_layers": p.sub_layers,
                "global_passes": p.global_passes, "extras": p.extras,
            });
            if link {
                v["link"] = json!(PresetLink { library: l.name.clone(), path });
            }
            out.push(v);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------------------------------------------

async fn finish_get(State(st): State<Arc<AppState>>, UrlPath((name, id)): UrlPath<(String, String)>) -> Result<Json<Value>, EngineError> {
    let lib = library_name(&st, Some(&name))?;
    let (id, path) = resolve_entry(&st, &lib, &id)?;
    let f = lock(&st.libs.library).finish_of(&lib, &path)?;
    Ok(Json(json!({ "library": lib, "id": entry_id(id), "path": path, "finish": f.summary() })))
}

async fn finish_put(State(st): State<Arc<AppState>>, UrlPath((name, id)): UrlPath<(String, String)>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let p: FinishPatch = body(&b)?;
    Ok(Json(st.blocking(move |st| finish_update(st, &name, &id, p)).await?))
}

async fn photo_get(State(st): State<Arc<AppState>>, UrlPath((name, id)): UrlPath<(String, String)>) -> Result<Response, EngineError> {
    let lib = library_name(&st, Some(&name))?;
    let (_, path) = resolve_entry(&st, &lib, &id)?;
    let f = lock(&st.libs.library).finish_of(&lib, &path)?;
    let p = f.photo.ok_or_else(|| EngineError::NotFound(format!("\"{path}\" has no photo")))?;
    let bytes = STANDARD.decode(p.data_base64.as_bytes()).map_err(|_| EngineError::Internal("the stored photo is not base64".into()))?;
    // Only an image type goes out, whatever the stored one says (`Finish::problem` checks it too).
    let mime = finish::PHOTO_MIMES.into_iter().find(|m| *m == p.mime).unwrap_or("application/octet-stream");
    Ok((
        [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-store"), (header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        bytes,
    )
        .into_response())
}

async fn sidecar_download(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<Response, EngineError> {
    let (name, text) = {
        let s = lock(&st.libs.library);
        let l = s.get(&name)?;
        (l.name.clone(), finish::sidecar_text(&l.finish))
    };
    Ok(attachment(&format!("{name}{}", finish::SIDECAR_SUFFIX), "application/json", text.into_bytes()))
}

async fn sidecar_import(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let (imported, skipped) = st.blocking(move |st| lock(&st.libs.library).import_finish(&name, &b)).await?;
    library_changed(&st);
    Ok(Json(json!({ "ok": true, "imported": imported, "skipped": skipped })))
}

#[derive(Deserialize)]
struct PaletteQuery {
    library: Option<String>,
    surface: Option<SurfaceFinish>,
    material: Option<String>,
}

async fn palette_get(State(st): State<Arc<AppState>>, Query(q): Query<PaletteQuery>) -> Result<Json<Value>, EngineError> {
    Ok(Json(palette_json(&st, q.library.as_deref(), q.surface, q.material.as_deref())?))
}

fn hue_model_json(st: &AppState) -> Value {
    let device = st.active_device().id;
    let s = lock(&st.libs.library);
    json!({ "device": device, "model": s.hue_model(&device), "default": commands::finish::HueModel::default(), "calibrated": s.hue.contains_key(&device) })
}

async fn hue_model_get(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(hue_model_json(&st))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HueModelBody {
    /// `null` resets to xTool's model.
    model: Option<commands::finish::HueModel>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn hue_model_put(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: HueModelBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the hue step model is changed")?;
    let device = st.active_device().id;
    lock(&st.libs.library).set_hue_model(&device, b.model)?;
    Ok(Json(hue_model_json(&st)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CalibrateBody {
    /// The best bracket box's layer.
    best: String,
    /// A box in the same row or column, `hues` hues from the best one.
    other: String,
    /// How many hues warmer (positive) or cooler (negative) the other box came out. Default 1.
    #[serde(default)]
    hues: Option<i32>,
    #[serde(default)]
    origin: Option<Value>,
}

async fn hue_calibrate(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let b: CalibrateBody = body(&b)?;
    ui_only(&headers, b.origin.as_ref(), "the hue step model is changed")?;
    let hues = b.hues.unwrap_or(1);
    if hues == 0 || hues.abs() > 20 {
        return Err(EngineError::BadRequest("hues must be within -20..=20 and not 0".into()));
    }
    let (_, best_path, _, best, group) = bracket_box(&st, &b.best)?;
    let (_, other_path, _, other, other_group) = bracket_box(&st, &b.other)?;
    if best_path != other_path || group.id != other_group.id {
        return Err(EngineError::BadRequest("pick two boxes of the same bracket test".into()));
    }
    let device = st.active_device().id;
    let model = calibrated(lock(&st.libs.library).hue_model(&device), &best.settings, &other.settings, hues)?;
    lock(&st.libs.library).set_hue_model(&device, Some(model))?;
    Ok(Json(hue_model_json(&st)))
}

/// `m` with the step of the one setting that differs between `best` and `other` (`other` came out `hues` hues
/// warmer, or cooler when negative) set from that difference.
pub(crate) fn calibrated(
    mut m: commands::finish::HueModel,
    best: &doc::CutSettings,
    other: &doc::CutSettings,
    hues: i32,
) -> Result<commands::finish::HueModel, EngineError> {
    let diff = |a: f64, b: f64| (b - a).abs() > 1e-9 * a.abs().max(1.0);
    let changed: Vec<(&str, f64)> = [
        ("power", best.power_pct, other.power_pct),
        ("speed", best.speed_mm_s, other.speed_mm_s),
        ("frequency", best.frequency_khz, other.frequency_khz),
    ]
    .into_iter()
    .filter(|(_, a, b)| diff(*a, *b))
    .map(|(k, a, b)| (k, b - a))
    .collect();
    let others_differ = best.pulse_width_ns != other.pulse_width_ns || diff(best.hatch.interval_mm, other.hatch.interval_mm) || best.passes != other.passes;
    let [(param, delta)] = changed.as_slice() else {
        return Err(EngineError::BadRequest("pick a box in the same row or column as the best one (only power, speed or frequency may differ)".into()));
    };
    if others_differ {
        return Err(EngineError::BadRequest("the boxes differ in pulse width, interval or passes; pick boxes that differ only in power, speed or frequency".into()));
    }
    // Warmer is less energy: less power, more speed, higher frequency.
    let per_hue = delta / f64::from(hues);
    let step = match *param {
        "power" => -per_hue,
        _ => per_hue,
    };
    if step <= 0.0 {
        return Err(EngineError::BadRequest(format!(
            "a box that came out {} should have {} {param}; this pair runs against the model",
            if hues > 0 { "warmer" } else { "cooler" },
            if (hues > 0) == (*param == "power") { "less" } else { "more" },
        )));
    }
    let round = |v: f64| (v * 100.0).round() / 100.0;
    match *param {
        "power" => m.power_pct = round(step),
        "speed" => m.speed_mm_s = round(step),
        _ => m.frequency_khz = round(step),
    }
    m.source = "bracket".into();
    if let Some(p) = m.problem() {
        return Err(EngineError::BadRequest(format!("the step would be out of range: {p}")));
    }
    Ok(m)
}

async fn nudge_post(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let a: NudgeArgs = body(&b)?;
    let origin = origin_of(&headers, a.origin.as_ref())?;
    Ok(Json(hue_nudge(&st, a, origin).await?))
}

async fn bracket_post(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let a: BracketArgs = body(&b)?;
    let origin = origin_of(&headers, a.origin.as_ref())?;
    Ok(Json(bracket_test(&st, a, origin).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PickBody {
    layer: String,
    #[serde(default)]
    origin: Option<Value>,
}

async fn pick_post(State(st): State<Arc<AppState>>, b: Bytes) -> Result<Json<Value>, EngineError> {
    let a: PickBody = body(&b)?;
    let _ = a.origin;
    Ok(Json(bracket_pick(&st, &a.layer).await?))
}

async fn separate_post(State(st): State<Arc<AppState>>, headers: HeaderMap, b: Bytes) -> Result<Json<Value>, EngineError> {
    let a: SeparateArgs = body(&b)?;
    let origin = origin_of(&headers, a.origin.as_ref())?;
    Ok(Json(color_separate(&st, a, origin).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::CutSettings;

    #[test]
    fn calibration_takes_the_step_of_the_one_setting_that_differs() {
        let m = commands::finish::HueModel::default();
        let best = CutSettings { power_pct: 40.0, speed_mm_s: 1000.0, frequency_khz: 600.0, ..CutSettings::default() };
        // A box with 2 % less power came out one hue warmer: 2 % per hue.
        let other = CutSettings { power_pct: 38.0, ..best.clone() };
        let c = calibrated(m.clone(), &best, &other, 1).unwrap();
        assert_eq!((c.power_pct, c.source.as_str()), (2.0, "bracket"));
        // 200 mm/s faster came out two hues warmer: 100 per hue.
        let other = CutSettings { speed_mm_s: 1200.0, ..best.clone() };
        assert_eq!(calibrated(m.clone(), &best, &other, 2).unwrap().speed_mm_s, 100.0);
        // More power that came out warmer runs against the model.
        let other = CutSettings { power_pct: 42.0, ..best.clone() };
        assert!(calibrated(m.clone(), &best, &other, 1).is_err());
        // ... but cooler is fine.
        assert_eq!(calibrated(m.clone(), &best, &other, -1).unwrap().power_pct, 2.0);
        // Two settings differ: refused.
        let other = CutSettings { power_pct: 42.0, speed_mm_s: 900.0, ..best.clone() };
        assert!(calibrated(m.clone(), &best, &other, -1).is_err());
        let other = CutSettings { passes: 2, power_pct: 39.0, ..best.clone() };
        assert!(calibrated(m, &best, &other, 1).is_err());
    }
}
