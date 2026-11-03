//! The store routes, as the engine answers them: preferences and layer / optimization defaults, preference export
//! and import, autosave settings, font favourites, material test and Adjust Image presets, devices and LightBurn's
//! device list. Every change records its [`StoreWrite`]s.

use std::collections::HashMap;

use appcore::AppError;
use appcore::http::{body_or_default, body_required, check_query, parse_origin, ui_origin_only};
use appcore::prefs::{ALL_COLORS, Checklist, LayerDefault};
use appcore::presets::{ImagePreset, check_image_preset, check_name, numbered};
use appcore::stores::{FileSettings, StoreWrite, names};
use commands::testgen_presets::{self, MAX_PRESETS, TestPreset};
use commands::{Invocation, Origin};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Backend;
use super::reply::{Answer, Routed};

/// A route of this module, or `None` when the path is not one.
pub fn route(b: &mut Backend, method: &str, segs: &[&str], q: &HashMap<String, String>, body: &[u8]) -> Result<Option<Answer>, AppError> {
    let a = match (method, segs) {
        ("GET", ["prefs"]) => Answer::json(&prefs_json(b)),
        ("PUT", ["prefs"]) => prefs_edit(b, body)?,
        ("POST", ["layers", id, "default"]) => layer_make_default(b, id, body)?,
        ("POST", ["layers", id, "reset"]) => layer_reset(b, id, body)?,
        ("POST", ["optimization", "default"]) => {
            let o = b.session.document().optimization.clone();
            let id = b.devices.active().id.clone();
            b.prefs.devices.entry(id).or_default().optimization_defaults = Some(o.clone());
            b.save_prefs();
            Answer::json(&json!({ "ok": true, "optimization": o }))
        }
        ("DELETE", ["optimization", "default"]) => {
            let id = b.devices.active().id.clone();
            b.prefs.devices.entry(id).or_default().optimization_defaults = None;
            b.save_prefs();
            Answer::json(&json!({ "ok": true }))
        }
        ("POST", ["optimization", "reset"]) => optimization_reset(b, body)?,
        ("GET", ["prefs", "export"]) => prefs_export(b)?,
        ("POST", ["prefs", "import"]) => prefs_import(b, q, body)?,
        ("GET", ["files", "settings"]) => Answer::json(&json!(b.files)),
        ("PUT", ["files", "settings"]) => files_put(b, body)?,
        ("GET", ["fonts", "prefs"]) => Answer::json(&json!(b.fonts)),
        ("PUT", ["fonts", "prefs"]) => fonts_put(b, body)?,
        ("GET", ["material-test", "presets"]) => {
            let mut presets = b.test_presets.clone();
            presets.sort_by_key(|p| p.name.to_lowercase());
            Answer::json(&json!({ "presets": presets }))
        }
        ("POST", ["material-test", "presets", "import"]) => test_import(b, q, body)?,
        ("GET", ["material-test", "presets", "export"]) => test_export(b, q)?,
        ("PUT", ["material-test", "presets", name]) => test_save(b, name, body)?,
        ("DELETE", ["material-test", "presets", name]) => {
            let mut all = b.test_presets.clone();
            let before = all.len();
            all.retain(|p| p.name != *name);
            if all.len() == before {
                return Err(AppError::NotFound(format!("no material test preset named \"{name}\"")));
            }
            store_test(b, all)?;
            Answer::json(&json!({ "ok": true }))
        }
        ("GET", ["image-adjust", "presets"]) => Answer::json(&json!({ "presets": b.image_presets })),
        ("POST", ["image-adjust", "presets", "import"]) => image_import(b, body)?,
        ("GET", ["image-adjust", "presets", "export"]) => {
            check_query(q, &["format"])?;
            if q.get("format").map(String::as_str) != Some("lbimg") {
                return Err(AppError::BadRequest("format must be lbimg (GET /api/image-adjust/presets is OpenEtch's own export)".into()));
            }
            let text = appcore::presets::presets_to_lbimg(&b.image_presets);
            Answer::bytes("application/json", text.into_bytes()).header("content-disposition", "attachment; filename=\"openetch-presets.lbimg\"")
        }
        ("PUT", ["image-adjust", "presets", name]) => image_save(b, name, body)?,
        ("DELETE", ["image-adjust", "presets", name]) => {
            let mut all = b.image_presets.clone();
            let before = all.len();
            all.retain(|p| p.name != *name);
            if all.len() == before {
                return Err(AppError::NotFound(format!("no image preset named \"{name}\"")));
            }
            store_images(b, all)?;
            Answer::json(&json!({ "ok": true }))
        }
        ("GET", ["devices"]) => Answer::json(&b.devices.view()),
        ("GET", ["devices", id]) => Answer::json(&b.devices.details(id)?),
        ("GET", ["devices", id, "lbdev"]) => {
            let (file, text) = b.devices.lbdev(id)?;
            Answer::bytes("application/json", text.into_bytes()).header("content-disposition", format!("attachment; filename=\"{file}\""))
        }
        ("DELETE", ["devices", id]) => {
            let w = b.devices.remove(id)?;
            b.writes.extend(w);
            if let Some(p) = b.prefs.devices.remove(*id) {
                drop(p);
                b.save_prefs();
            }
            b.emit(appcore::Event::DevicesChanged);
            Answer::json(&json!({ "ok": true, "id": id }))
        }
        ("POST", ["devices", "import"]) => devices_import(b, q, body)?,
        ("POST", ["devices", id, "activate"]) => device_activate(b, id, body)?,
        ("GET", ["lightburn"]) => Answer::json(&json!({ "available": false, "source": null, "default_device": null, "devices": [] })),
        _ => return Ok(None),
    };
    Ok(Some(a))
}

fn prefs_json(b: &Backend) -> Value {
    json!({
        "device": b.devices.active().id,
        "device_prefs": b.device_prefs(),
        "debug_log": b.prefs.debug_log,
        "check_for_updates": b.prefs.check_for_updates,
        "user_tools": b.prefs.user_tools,
    })
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
    console_macros: Option<Vec<appcore::prefs::ConsoleMacro>>,
    #[serde(default)]
    move_positions: Option<Vec<appcore::prefs::MovePosition>>,
    #[serde(default)]
    debug_log: Option<bool>,
    #[serde(default)]
    check_for_updates: Option<bool>,
    #[serde(default)]
    user_tools: Option<Vec<appcore::user_tools::UserTool>>,
    #[serde(default)]
    keep_out: Option<Vec<appcore::zones::DeviceZone>>,
    #[serde(default)]
    origin: Option<Value>,
}

/// A JSON body of the preference routes ("bad JSON body" as the engine's `api_libs`).
fn body<T: for<'de> Deserialize<'de> + Default>(bytes: &[u8]) -> Result<T, AppError> {
    body_or_default(bytes)
}

fn prefs_edit(b: &mut Backend, bytes: &[u8]) -> Routed {
    let p: PrefsBody = body_required(bytes)?;
    ui_origin_only(p.origin.as_ref(), "preferences can be changed")?;
    if let Some(c) = &p.checklist
        && let Some(why) = c.problem()
    {
        return Err(AppError::BadRequest(why));
    }
    if let Some(m) = &p.console_macros
        && let Some(why) = appcore::prefs::macros_problem(m)
    {
        return Err(AppError::BadRequest(why));
    }
    if let Some(m) = &p.move_positions {
        let Some(bed) = b.devices.active().grbl.as_ref().map(|g| g.work_area_mm) else {
            return Err(AppError::Conflict("saved Move positions need a GRBL gantry as the active device".into()));
        };
        if let Some(why) = appcore::prefs::move_positions_problem(m, bed) {
            return Err(AppError::BadRequest(why));
        }
    }
    if let Some(t) = &p.user_tools {
        let specs: Vec<_> = b.session.registry().specs().map(|s| (s.id, s.risk)).collect();
        if let Some(why) = appcore::user_tools::problem(t, |id| specs.iter().find(|(s, _)| *s == id).map(|(_, r)| *r)) {
            return Err(AppError::BadRequest(why));
        }
    }
    if let Some(z) = &p.keep_out
        && let Some(why) = appcore::zones::device_zones_problem(z)
    {
        return Err(AppError::BadRequest(why));
    }
    let device = b.devices.active().id.clone();
    let d = b.prefs.devices.entry(device).or_default();
    if let Some(v) = p.require_framing {
        d.require_framing = v;
    }
    if let Some(v) = p.play_sound {
        d.play_sound = v;
    }
    if let Some(c) = p.checklist {
        d.checklist = c;
    }
    if let Some(v) = p.load_defaults_on_new {
        d.load_defaults_on_new = v;
    }
    if let Some(m) = p.console_macros {
        d.console_macros = m;
    }
    if let Some(m) = p.move_positions {
        d.move_positions = m;
    }
    if let Some(z) = p.keep_out {
        d.keep_out = z;
    }
    if let Some(v) = p.debug_log {
        b.prefs.debug_log = v;
    }
    if let Some(v) = p.check_for_updates {
        b.prefs.check_for_updates = v;
    }
    if let Some(t) = p.user_tools {
        b.prefs.user_tools = t;
    }
    b.save_prefs();
    b.emit(appcore::Event::machine("prefs", None));
    Ok(Answer::json(&prefs_json(b)))
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

fn layer_make_default(b: &mut Backend, id: &str, bytes: &[u8]) -> Routed {
    let d: DefaultBody = body(bytes)?;
    let l = b.session.document().layer(id).cloned().ok_or_else(|| AppError::NotFound(format!("no layer with id '{id}'")))?;
    let key = if d.all { ALL_COLORS.to_string() } else { l.color.to_ascii_lowercase() };
    let device = b.devices.active().id.clone();
    let p = b.prefs.devices.entry(device.clone()).or_default();
    if d.all {
        p.layer_defaults.clear();
    }
    p.layer_defaults.insert(key.clone(), LayerDefault { settings: l.settings.clone(), sub_layers: l.sub_layers.clone() });
    b.save_prefs();
    Ok(Answer::json(&json!({ "ok": true, "color": key, "device": device })))
}

fn layer_reset(b: &mut Backend, id: &str, bytes: &[u8]) -> Routed {
    let d: DefaultBody = body(bytes)?;
    let origin = parse_origin(d.origin.as_ref())?;
    let l = b.session.document().layer(id).cloned().ok_or_else(|| AppError::NotFound(format!("no layer with id '{id}'")))?;
    let saved = b.device_prefs().layer_default(&l.color).cloned();
    let def = match (d.builtin, saved) {
        (false, Some(def)) => def,
        _ => LayerDefault { settings: doc::CutSettings::default(), sub_layers: Vec::new() },
    };
    let inv = apply_invocation(id, &def);
    let out = b.execute(inv, origin)?;
    Ok(Answer::json(&json!({ "ok": true, "result": out.result, "revision": out.revision })))
}

/// `layer.apply_preset` setting a layer to a saved default (the engine's `api_libs::apply_invocation` of a preset
/// with one global pass, no extras and no library link).
fn apply_invocation(layer: &str, d: &LayerDefault) -> Invocation {
    Invocation {
        id: "layer.apply_preset".into(),
        params: json!({
            "layer": layer, "settings": d.settings, "sub_layers": d.sub_layers,
            "global_passes": 1, "extras": {}, "link": null,
        }),
    }
}

fn optimization_reset(b: &mut Backend, bytes: &[u8]) -> Routed {
    let d: DefaultBody = body(bytes)?;
    let origin = parse_origin(d.origin.as_ref())?;
    let saved = b.device_prefs().optimization_defaults;
    let o = if d.builtin { doc::Optimization::default() } else { saved.unwrap_or_default() };
    let inv = Invocation { id: "job.set_optimization".into(), params: json!({ "optimization": o }) };
    let out = b.execute(inv, origin)?;
    Ok(Answer::json(&json!({ "ok": true, "result": out.result, "revision": out.revision })))
}

/// The exported preferences file (the engine's `userfiles::PrefsFile`).
#[derive(Debug, Serialize, Deserialize)]
struct PrefsFile {
    format: String,
    version: u32,
    preferences: Value,
    #[serde(default)]
    files: Option<FileSettings>,
}

const PREFS_FORMAT: &str = "openetch-prefs";

fn prefs_export(b: &Backend) -> Routed {
    let file = PrefsFile { format: PREFS_FORMAT.into(), version: 1, preferences: json!(b.prefs), files: Some(b.files.clone()) };
    let text = serde_json::to_string_pretty(&file).map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Answer::file("attachment", "openetch-prefs", "oeprefs", "application/json", text.into_bytes(), 0))
}

fn prefs_import(b: &mut Backend, q: &HashMap<String, String>, bytes: &[u8]) -> Routed {
    check_query(q, &["origin"])?;
    ui_origin_only(q.get("origin").map(|o| json!(o)).as_ref(), "preferences can be imported")?;
    let f: PrefsFile = serde_json::from_slice(bytes).map_err(|e| AppError::BadRequest(format!("not an OpenEtch preferences file: {e}")))?;
    if f.format != PREFS_FORMAT || f.version > 1 {
        return Err(AppError::BadRequest("not an OpenEtch preferences file (or a newer version)".into()));
    }
    let p: appcore::prefs::Preferences = serde_json::from_value(f.preferences).map_err(|e| AppError::BadRequest(format!("bad preferences: {e}")))?;
    // The same rules as `PUT /api/prefs` and `PUT /api/files/settings`, checked before anything changes.
    for (device, d) in &p.devices {
        let bed = b.devices.get(device).map(|dev| dev.grbl.as_ref().map(|g| g.work_area_mm));
        let problem = d.checklist.problem().or_else(|| appcore::prefs::macros_problem(&d.console_macros)).or_else(|| match bed {
            Some(None) if !d.move_positions.is_empty() => Some("saved Move positions need a GRBL gantry".to_owned()),
            Some(Some(bed)) => appcore::prefs::move_positions_problem(&d.move_positions, bed),
            _ => appcore::prefs::move_positions_problem(&d.move_positions, [f64::MAX, f64::MAX]),
        });
        if let Some(problem) = problem {
            return Err(AppError::BadRequest(format!("bad preferences: device {device}: {problem}")));
        }
    }
    let specs: Vec<_> = b.session.registry().specs().map(|s| (s.id, s.risk)).collect();
    if let Some(problem) = appcore::user_tools::problem(&p.user_tools, |id| specs.iter().find(|(s, _)| *s == id).map(|(_, r)| *r)) {
        return Err(AppError::BadRequest(format!("bad preferences: user tools: {problem}")));
    }
    if let Some(problem) = f.files.as_ref().and_then(FileSettings::problem) {
        return Err(AppError::BadRequest(format!("bad file settings: {problem}")));
    }
    b.prefs = p;
    b.save_prefs();
    if let Some(files) = f.files {
        b.files = files;
        let w = StoreWrite::json(names::FILES, "settings", &b.files);
        b.write(w);
    }
    b.emit(appcore::Event::machine("prefs", None));
    Ok(Answer::json(&json!({ "ok": true })))
}

fn files_put(b: &mut Backend, bytes: &[u8]) -> Routed {
    let mut v: Value = body_required(bytes)?;
    let origin = v.as_object_mut().and_then(|m| m.remove("origin"));
    ui_origin_only(origin.as_ref(), "file settings can be changed")?;
    let mut cur = json!(b.files);
    let Value::Object(patch) = v else { return Err(AppError::BadRequest("expected a JSON object".into())) };
    for (k, val) in patch {
        cur[k] = val;
    }
    let next: FileSettings = serde_json::from_value(cur).map_err(|e| AppError::BadRequest(e.to_string()))?;
    if let Some(why) = next.problem() {
        return Err(AppError::BadRequest(why));
    }
    b.files = next;
    let w = StoreWrite::json(names::FILES, "settings", &b.files);
    b.write(w);
    Ok(Answer::json(&json!(b.files)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FontBody {
    favourites: Option<Vec<appcore::fonts::FontRef>>,
    hidden: Option<Vec<appcore::fonts::FontRef>>,
    #[serde(default)]
    origin: Option<Value>,
}

fn fonts_put(b: &mut Backend, bytes: &[u8]) -> Routed {
    let f: FontBody = body_required(bytes)?;
    if parse_origin(f.origin.as_ref())? != Origin::Ui {
        return Err(AppError::Forbidden("font favourites are changed only by a person in the OpenEtch UI".into()));
    }
    let cur = b.fonts.clone();
    let next = appcore::fonts::FontPrefs { favourites: f.favourites.unwrap_or(cur.favourites), hidden: f.hidden.unwrap_or(cur.hidden) }.clean()?;
    b.fonts = next;
    let w = StoreWrite::json(names::FONTS, "prefs", &b.fonts);
    b.write(w);
    Ok(Answer::json(&json!(b.fonts)))
}

/// The field LightBurn files of the active device are laid out for.
fn lightburn_field(b: &Backend) -> f64 {
    let d = b.devices.active();
    d.lightburn_field_mm().unwrap_or_else(|| b.profile_field())
}

fn command_error(e: commands::CommandError) -> AppError {
    AppError::BadRequest(e.to_string())
}

fn store_test(b: &mut Backend, mut presets: Vec<TestPreset>) -> Result<(), AppError> {
    if presets.len() > MAX_PRESETS {
        return Err(AppError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    presets.sort_by_key(|p| p.name.to_lowercase());
    let w = StoreWrite::json(names::PRESETS, "material-test", &presets);
    b.test_presets = presets;
    b.write(w);
    Ok(())
}

fn store_images(b: &mut Backend, mut presets: Vec<ImagePreset>) -> Result<(), AppError> {
    if presets.len() > MAX_PRESETS {
        return Err(AppError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    presets.sort_by_key(|p| p.name.to_lowercase());
    let w = StoreWrite::json(names::PRESETS, "image-adjust", &presets);
    b.image_presets = presets;
    b.write(w);
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestSaveBody {
    params: Value,
    #[serde(default)]
    #[allow(dead_code)]
    origin: Option<Value>,
}

fn test_save(b: &mut Backend, name: &str, bytes: &[u8]) -> Routed {
    let name = check_name(name)?;
    let t: TestSaveBody = body_required(bytes)?;
    testgen_presets::check_material_test_params(&t.params).map_err(command_error)?;
    let mut all = b.test_presets.clone();
    let before = all.len();
    all.retain(|p| p.name != name);
    let replaced = all.len() != before;
    all.push(TestPreset { name: name.clone(), params: t.params });
    store_test(b, all)?;
    Ok(Answer::json(&json!({ "ok": true, "name": name, "replaced": replaced })))
}

fn test_import(b: &mut Backend, q: &HashMap<String, String>, bytes: &[u8]) -> Routed {
    check_query(q, &["replace"])?;
    let replace = match q.get("replace").map(String::as_str) {
        None | Some("false") => false,
        Some("true") => true,
        Some(v) => return Err(AppError::BadRequest(format!("replace must be true or false (got '{v}')"))),
    };
    let text = std::str::from_utf8(bytes).map_err(|_| AppError::BadRequest("the preset file is not UTF-8 text".into()))?;
    let (presets, warnings) = testgen_presets::lbmt_to_presets(text, lightburn_field(b)).map_err(command_error)?;
    let mut all = b.test_presets.clone();
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
    store_test(b, all)?;
    Ok(Answer::json(&json!({ "ok": true, "imported": imported, "warnings": warnings })))
}

fn test_export(b: &Backend, q: &HashMap<String, String>) -> Routed {
    check_query(q, &["names"])?;
    let names: Option<Vec<String>> = q.get("names").map(|n| n.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect());
    let mut all = b.test_presets.clone();
    if let Some(n) = &names {
        if let Some(missing) = n.iter().find(|n| !all.iter().any(|p| &&p.name == n)) {
            return Err(AppError::NotFound(format!("no material test preset named \"{missing}\"")));
        }
        all.retain(|p| n.contains(&p.name));
    }
    let (text, warnings) = testgen_presets::presets_to_lbmt(&all, lightburn_field(b));
    let header_text: String = warnings.join(" | ").chars().map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '?' }).take(4000).collect();
    Ok(Answer::bytes("application/json", text.into_bytes())
        .header("content-disposition", "attachment; filename=\"material_test_presets.lbmt\"")
        .header("x-openetch-warnings", header_text))
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

fn image_save(b: &mut Backend, name: &str, bytes: &[u8]) -> Routed {
    let name = check_name(name)?;
    let s: ImageSaveBody = body_required(bytes)?;
    let preset = ImagePreset { name: name.clone(), adjust: s.adjust, image: s.image };
    check_image_preset(&preset)?;
    let mut all = b.image_presets.clone();
    let before = all.len();
    all.retain(|p| p.name != name);
    let replaced = all.len() != before;
    all.push(preset);
    store_images(b, all)?;
    Ok(Answer::json(&json!({ "ok": true, "name": name, "replaced": replaced })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageImportBody {
    presets: Vec<ImagePreset>,
}

fn image_import(b: &mut Backend, bytes: &[u8]) -> Routed {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| AppError::BadRequest(format!("not a preset file (not JSON): {e}")))?;
    let (presets, warnings) = if v.get("presets").is_some_and(Value::is_array) {
        let i: ImageImportBody = serde_json::from_value(v).map_err(|e| AppError::BadRequest(format!("not an OpenEtch image preset export: {e}")))?;
        (i.presets, Vec::new())
    } else {
        appcore::presets::lbimg_to_presets(&v)?
    };
    if presets.len() > MAX_PRESETS {
        return Err(AppError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    for p in &presets {
        check_name(&p.name)?;
        check_image_preset(p)?;
    }
    let mut all = b.image_presets.clone();
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
    store_images(b, all)?;
    Ok(Answer::json(&json!({ "ok": true, "imported": imported, "warnings": warnings })))
}

fn devices_import(b: &mut Backend, q: &HashMap<String, String>, bytes: &[u8]) -> Routed {
    check_query(q, &["add", "from"])?;
    let filter = appcore::devices::ImportFilter::parse(q.get("add").map(String::as_str));
    if let Some(from) = q.get("from") {
        return Err(if from == "lightburn" {
            AppError::NotFound("the browser cannot read LightBurn's prefs.ini; import a .lbdev or prefs.ini file".into())
        } else {
            AppError::BadRequest(format!("unknown from '{from}' (only lightburn)"))
        });
    }
    if bytes.is_empty() {
        return Err(AppError::BadRequest("the upload is empty (send a .lbdev or prefs.ini)".into()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AppError::BadRequest("the file is not UTF-8 text (expected .lbdev or prefs.ini JSON)".into()))?;
    let (report, writes) = b.devices.import(text, &filter)?;
    b.writes.extend(writes);
    b.emit(appcore::Event::DevicesChanged);
    let mut v = json!(report);
    v["ok"] = json!(true);
    Ok(Answer::json(&v))
}

fn device_activate(b: &mut Backend, id: &str, bytes: &[u8]) -> Routed {
    #[derive(Default, Deserialize)]
    struct OriginBody {
        #[serde(default)]
        origin: Option<Value>,
    }
    let o: OriginBody = body_or_default(bytes)?;
    if parse_origin(o.origin.as_ref())? != Origin::Ui {
        return Err(AppError::Forbidden("devices can only be changed from the UI".into()));
    }
    let w = b.devices.set_active(id)?;
    b.writes.extend(w);
    b.generation += 1;
    b.emit(appcore::Event::DeviceChanged { id: id.to_owned() });
    Ok(Answer::json(&json!({ "ok": true, "active": b.devices.active().summary(), "warnings": [] })))
}
