//! Material libraries (`/api/library/...`), as the engine's `api_libs` answers them: load a LightBurn `.clb`, new,
//! select, unload, save (as), rename, merge, the browse tree, download, entries (create from a layer, update, edit,
//! duplicate, delete) and applying a preset to a layer (linked or not). Layers linked to a preset follow it, as in
//! the engine (relinked on a library switch, updated or unlinked when the entry changes).

use std::collections::HashMap;

use appcore::AppError;
use appcore::library::{EntryMeta, Preset, entry_from_layer, entry_id, parse_entry_id};
use commands::{Invocation, Origin};
use doc::PresetLink;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Backend;
use super::reply::{Answer, Routed};

/// A route of this module, or `None` when the path is not one.
pub fn route(b: &mut Backend, method: &str, segs: &[&str], q: &HashMap<String, String>, body: &[u8]) -> Result<Option<Answer>, AppError> {
    let a = match (method, segs) {
        ("GET", ["library"]) => Answer::json(&b.libraries.view()),
        ("POST", ["library", "new"]) => {
            let n: NameBody = json_body(body)?;
            let w = b.libraries.create(&n.name)?;
            b.writes.extend(w);
            remember_device_library(b, &n.name);
            Answer::json(&b.libraries.view())
        }
        ("POST", ["library", "load"]) => {
            let name = q.get("name").cloned().ok_or_else(|| AppError::BadRequest("Failed to deserialize query string: missing field `name`".into()))?;
            let w = b.libraries.load(&name, body)?;
            b.writes.extend(w);
            remember_device_library(b, &name);
            relink_all(b, &name)?;
            Answer::json(&b.libraries.view())
        }
        ("POST", ["library", "select"]) => {
            let n: NameBody = json_body(body)?;
            let (name, w) = b.libraries.select(&n.name)?;
            b.writes.extend(w);
            remember_device_library(b, &name);
            let relinked = relink_all(b, &name)?;
            let mut v = b.libraries.view();
            v["relinked_layers"] = json!(relinked);
            Answer::json(&v)
        }
        ("POST", ["library", "unload"]) => {
            let n: NameBody = json_body(body)?;
            let w = b.libraries.unload(&n.name, n.discard)?;
            b.writes.extend(w);
            Answer::json(&b.libraries.view())
        }
        ("POST", ["library", "save"]) => {
            let n: NameBody = json_body(body)?;
            let (saved, w) = b.libraries.save(&n.name, n.save_as.as_deref())?;
            b.writes.extend(w);
            remember_device_library(b, &saved);
            let mut v = b.libraries.view();
            v["saved"] = json!(saved);
            Answer::json(&v)
        }
        ("POST", ["library", "rename"]) => {
            let n: NameBody = json_body(body)?;
            let new_name = n.new_name.ok_or_else(|| AppError::BadRequest("give new_name".into()))?;
            let (old, w) = b.libraries.rename(&n.name, &new_name)?;
            b.writes.extend(w);
            // Linked layers follow the library's new name.
            let plan: Vec<(String, Option<(PresetLink, Preset)>)> = b
                .session
                .document()
                .layers
                .iter()
                .filter_map(|l| l.link.as_ref().filter(|k| k.library == old).map(|k| (l.id.clone(), k.path.clone())))
                .filter_map(|(id, path)| {
                    let preset = preset_for(b, &new_name, &path).ok()?;
                    Some((id, Some((PresetLink { library: new_name.clone(), path }, preset))))
                })
                .collect();
            apply_links(b, plan)?;
            Answer::json(&b.libraries.view())
        }
        ("POST", ["library", "merge"]) => {
            let n: NameBody = json_body(body)?;
            let from = n.from.ok_or_else(|| AppError::BadRequest("give from".into()))?;
            let added = b.libraries.merge(&n.name, &from)?;
            let mut v = b.libraries.view();
            v["added"] = json!(added);
            Answer::json(&v)
        }
        ("GET", ["library", name]) => {
            let l = b.libraries.get(name)?;
            let mut t = appcore::library::tree(&l.lib, &l.finish);
            t["name"] = json!(l.name);
            t["dirty"] = json!(l.dirty);
            Answer::json(&t)
        }
        ("GET", ["library", name, "download"]) => {
            let (name, text) = b.libraries.text(name)?;
            let losses = b.libraries.clb_losses(&name)?;
            attachment(&format!("{name}.clb"), "application/xml", text.into_bytes()).header("x-openetch-warnings", losses.len().to_string())
        }
        ("POST", ["library", name, "entries"]) => entry_create(b, name, body)?,
        ("PUT", ["library", name, "entries", id]) => entry_edit(b, name, id, body)?,
        ("DELETE", ["library", name, "entries", id]) => {
            let path = b.libraries.delete(name, parse_entry_id(id)?)?;
            let unlinked = sync_linked(b, name, &path, None)?;
            Answer::json(&json!({ "ok": true, "path": path, "unlinked_layers": unlinked }))
        }
        ("POST", ["library", name, "entries", id, "duplicate"]) => {
            let id = b.libraries.duplicate(name, parse_entry_id(id)?)?;
            Answer::json(&json!({ "ok": true, "id": entry_id(id) }))
        }
        ("POST", ["library", name, "entries", id, "apply"]) => entry_apply(b, name, id, body)?,
        _ => return Ok(None),
    };
    Ok(Some(a))
}

/// A JSON body of these routes (`{}` when empty).
fn json_body<T: for<'de> Deserialize<'de>>(b: &[u8]) -> Result<T, AppError> {
    let b: &[u8] = if b.iter().all(u8::is_ascii_whitespace) { b"{}" } else { b };
    serde_json::from_slice(b).map_err(|e| AppError::BadRequest(format!("bad JSON body: {e}")))
}

/// A download as the engine's `api_libs::attachment`.
fn attachment(name: &str, content_type: &str, body: Vec<u8>) -> Answer {
    let ascii: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect();
    Answer::bytes(content_type, body).header("content-disposition", format!("attachment; filename=\"{ascii}\"")).header("cache-control", "no-store")
}

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

/// LightBurn: a library saved or loaded while a device is active loads with that device.
fn remember_device_library(b: &mut Backend, name: &str) {
    let device = b.devices.active().id.clone();
    b.prefs.devices.entry(device).or_default().library = Some(name.to_string());
    b.save_prefs();
}

fn preset_for(b: &Backend, name: &str, path: &str) -> Result<Preset, AppError> {
    let l = b.libraries.get(name)?;
    let (m, e) = l.lib.find_path(path).ok_or_else(|| AppError::NotFound(format!("library \"{name}\" has no preset \"{path}\"")))?;
    b.libraries.preset(name, (m, e))
}

/// `layer.apply_preset` of a preset (the engine's `api_libs::apply_invocation`).
pub fn apply_invocation(layer: &str, preset: &Preset, link: Option<&PresetLink>) -> Invocation {
    Invocation {
        id: "layer.apply_preset".into(),
        params: json!({
            "layer": layer, "settings": preset.settings, "sub_layers": preset.sub_layers,
            "global_passes": preset.global_passes, "extras": preset.extras, "link": link,
        }),
    }
}

/// Apply or unlink presets on layers as one undo step (origin `script`). Returns the layer ids changed.
fn apply_links(b: &mut Backend, plan: Vec<(String, Option<(PresetLink, Preset)>)>) -> Result<Vec<String>, AppError> {
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
    b.batch_opening(invs, Origin::Script, None, None)?;
    Ok(ids)
}

/// Layers linked to `(library, path)` brought up to date with the entry now at `new_path` (or unlinked).
fn sync_linked(b: &mut Backend, name: &str, old_path: &str, new_path: Option<&str>) -> Result<Vec<String>, AppError> {
    let lib_name = b.libraries.get(name)?.name.clone();
    let linked: Vec<String> = b
        .session
        .document()
        .layers
        .iter()
        .filter(|l| l.link.as_ref().is_some_and(|k| appcore::project::same_name_ignoring_case(&k.library, &lib_name) && k.path == old_path))
        .map(|l| l.id.clone())
        .collect();
    if linked.is_empty() {
        return Ok(Vec::new());
    }
    let plan = match new_path {
        Some(p) => {
            let preset = preset_for(b, &lib_name, p)?;
            let link = PresetLink { library: lib_name.clone(), path: p.to_string() };
            linked.into_iter().map(|id| (id, Some((link.clone(), preset.clone())))).collect()
        }
        None => linked.into_iter().map(|id| (id, None)).collect(),
    };
    apply_links(b, plan)
}

/// On a library switch, a linked layer relinks to the new library when it has a preset of the same path.
fn relink_all(b: &mut Backend, name: &str) -> Result<Vec<String>, AppError> {
    let lib_name = b.libraries.get(name)?.name.clone();
    let plan: Vec<(String, Option<(PresetLink, Preset)>)> = b
        .session
        .document()
        .layers
        .iter()
        .filter_map(|l| l.link.as_ref().map(|k| (l.id.clone(), k.path.clone())))
        .filter_map(|(id, path)| {
            let preset = preset_for(b, &lib_name, &path).ok()?;
            Some((id, Some((PresetLink { library: lib_name.clone(), path }, preset))))
        })
        .collect();
    apply_links(b, plan)
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

fn entry_create(b: &mut Backend, name: &str, body: &[u8]) -> Routed {
    let c: CreateBody = json_body(body)?;
    let meta = EntryMeta { material: c.material, thickness_mm: c.thickness_mm, title: c.title, description: c.description };
    meta.check()?;
    let layer = b.session.document().layer(&c.layer).cloned().ok_or_else(|| AppError::NotFound(format!("no layer with id '{}'", c.layer)))?;
    let entry = entry_from_layer(&layer, meta.thickness_mm, &meta.title, &meta.description)?;
    let id = b.libraries.add_entry(name, &meta.material, entry)?;
    let (m, e) = b.libraries.entry(name, id)?;
    let (material, path) = (m.to_string(), e.link_path(m));
    b.libraries.set_pauses(name, &path, &layer.settings, &layer.sub_layers)?;
    Ok(Answer::json(&json!({ "ok": true, "id": entry_id(id), "material": material, "path": path })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditBody {
    #[serde(default)]
    from_layer: Option<String>,
    #[serde(default)]
    settings: Option<doc::CutSettings>,
    #[serde(default)]
    sub_layers: Option<Vec<doc::CutSettings>>,
    #[serde(default)]
    meta: Option<EntryMeta>,
}

fn entry_edit(b: &mut Backend, name: &str, id: &str, body: &[u8]) -> Routed {
    let e: EditBody = json_body(body)?;
    let mut id = parse_entry_id(id)?;
    let old_path = {
        let (m, en) = b.libraries.entry(name, id)?;
        en.link_path(m)
    };
    let current = b.libraries.entry(name, id)?.1.clone();
    let with_pauses = b.libraries.preset(name, id)?;
    let new_layer = if let Some(layer_id) = &e.from_layer {
        Some(b.session.document().layer(layer_id).cloned().ok_or_else(|| AppError::NotFound(format!("no layer with id '{layer_id}'")))?)
    } else if e.settings.is_some() || e.sub_layers.is_some() {
        let p = with_pauses;
        let finish = b.libraries.finish_of(name, &old_path)?;
        // Edit Cut changes the cut values, never the preset's pauses for the operator (kept beside the entry).
        let preset = Preset {
            settings: e.settings.unwrap_or(p.settings),
            sub_layers: e.sub_layers.unwrap_or(p.sub_layers),
            global_passes: p.global_passes,
            extras: p.extras,
        }
        .with_pauses(Some(&finish));
        // Checked as layer.apply_preset checks a preset, on a scratch document.
        let mut scratch = commands::Session::new(doc::Document::default());
        let layer_id = scratch.document().layers[0].id.clone();
        scratch.execute(apply_invocation(&layer_id, &preset, None), Origin::Script)?;
        Some(scratch.document().layers[0].clone())
    } else {
        None
    };
    if new_layer.is_none() && e.meta.is_none() {
        return Err(AppError::BadRequest("give from_layer, settings / sub_layers, or meta".into()));
    }
    if let Some(l) = new_layer {
        let entry = entry_from_layer(&l, current.thickness(), current.title(), current.description())?;
        b.libraries.replace_settings(name, id, entry)?;
        b.libraries.set_pauses(name, &old_path, &l.settings, &l.sub_layers)?;
    }
    if let Some(m) = &e.meta {
        id = b.libraries.edit_meta(name, id, m)?;
    }
    let new_path = {
        let (m, en) = b.libraries.entry(name, id)?;
        en.link_path(m)
    };
    let synced = sync_linked(b, name, &old_path, Some(&new_path))?;
    Ok(Answer::json(&json!({ "ok": true, "id": entry_id(id), "path": new_path, "updated_layers": synced })))
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

fn entry_apply(b: &mut Backend, name: &str, id: &str, body: &[u8]) -> Routed {
    let a: ApplyBody = json_body(body)?;
    let origin = appcore::http::parse_origin(a.origin.as_ref())?;
    let (lib_name, path, preset, color) = {
        let l = b.libraries.get(name)?;
        let lib_name = l.name.clone();
        let id = parse_entry_id(id)?;
        let (m, e) = b.libraries.entry(name, id)?;
        let path = e.link_path(m);
        let color = l.finish.get(&path).and_then(|f| f.color.clone());
        (lib_name, path, b.libraries.preset(name, id)?, color)
    };
    let link = a.link.then_some(PresetLink { library: lib_name, path });
    let mut invs = vec![apply_invocation(&a.layer, &preset, link.as_ref())];
    // The layer takes the preset's expected result colour, or loses one it no longer has.
    let had = b.session.document().layer(&a.layer).and_then(|l| l.extras.get(commands::finish::RESULT_COLOR_KEY).cloned());
    if color.is_some() || had.is_some() {
        invs.push(Invocation { id: "layer.set_result_color".into(), params: json!({ "layer": a.layer, "color": color }) });
    }
    let mut outs = b.batch_opening(invs, origin, None, None)?;
    let revision = outs.last().map(|o| o.revision).unwrap_or_default();
    let out = outs.remove(0);
    Ok(Answer::json(&json!({ "ok": true, "result": out.result, "revision": revision })))
}
