//! Material libraries (moved from the engine's `library` module, which keeps the `.clb` files): a loaded library,
//! entries as presets, entry metadata, the browse tree.

use std::collections::BTreeMap;

use doc::{CutSettings, Layer};
use lbrn::{LibEntry, Library};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::AppError;
use crate::finish::Finish;
use crate::stores::StoreWrite;

/// Largest `.clb` accepted (the owner's is 34 KB; LightBurn's shared libraries a few hundred KB).
pub const MAX_LIBRARY_BYTES: usize = 8 * 1024 * 1024;
/// Most libraries loaded at once.
pub const MAX_LOADED: usize = 32;

/// One loaded library.
#[derive(Debug, Clone)]
pub struct LoadedLibrary {
    pub name: String,
    pub lib: Library,
    /// Changed since it was loaded or saved.
    pub dirty: bool,
    /// The finish of each entry by LinkPath (`crate::finish`; written to the sidecar on save).
    pub finish: BTreeMap<String, Finish>,
}

impl LoadedLibrary {
    pub fn new(name: &str, lib: Library, dirty: bool) -> LoadedLibrary {
        LoadedLibrary { name: name.to_string(), lib, dirty, finish: BTreeMap::new() }
    }

    /// Keep only the finishes of paths the library has (a sidecar left from an older library of the same name).
    pub fn keep_known_finishes(&mut self) {
        let paths = self.paths();
        self.finish.retain(|k, _| paths.contains(k));
    }

    /// Every entry's LinkPath.
    pub fn paths(&self) -> std::collections::BTreeSet<String> {
        self.lib.materials.iter().flat_map(|m| m.entries.iter().map(move |e| e.link_path(m.name()))).collect()
    }
}

/// A preset as `layer.apply_preset` takes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preset {
    pub settings: CutSettings,
    pub sub_layers: Vec<CutSettings>,
    pub global_passes: u32,
    pub extras: BTreeMap<String, String>,
}

/// Where an entry sits: `(material index, entry index)`, written `m<i>.e<j>` in the API.
pub type EntryId = (usize, usize);

pub fn entry_id(id: EntryId) -> String {
    format!("m{}.e{}", id.0, id.1)
}

pub fn parse_entry_id(s: &str) -> Result<EntryId, AppError> {
    let bad = || AppError::BadRequest(format!("'{s}' is not a library entry id (m<material>.e<entry>)"));
    let (m, e) = s.split_once('.').ok_or_else(bad)?;
    let m = m.strip_prefix('m').and_then(|v| v.parse().ok()).ok_or_else(bad)?;
    let e = e.strip_prefix('e').and_then(|v| v.parse().ok()).ok_or_else(bad)?;
    Ok((m, e))
}

/// The preset of an entry: its settings as an OpenEtch layer's.
pub fn preset_of(entry: &LibEntry) -> Result<Preset, AppError> {
    let (layer, _warnings) = entry.to_layer("layer-preset").map_err(|e| AppError::BadRequest(e.to_string()))?;
    let extras = layer
        .extras
        .into_iter()
        .filter(|(k, _)| k.starts_with(lbrn::EXTRA_PREFIX) && k != "lbrn2.index" && k != "lbrn2.LinkPath")
        .collect();
    Ok(Preset { settings: layer.settings, sub_layers: layer.sub_layers, global_passes: layer.global_passes, extras })
}

/// A layer's pauses for the operator: its own, and its sub-layers' by index.
pub fn pauses_of(settings: &CutSettings, sub_layers: &[CutSettings]) -> (bool, Vec<usize>) {
    let subs = sub_layers.iter().enumerate().filter(|(_, s)| s.options.operator_pause).map(|(i, _)| i).collect();
    (settings.options.operator_pause, subs)
}

impl Preset {
    /// The preset with the pauses kept in its entry's OpenEtch data (`finish`; a `.clb` has no place for them).
    pub fn with_pauses(mut self, finish: Option<&Finish>) -> Preset {
        self.settings.options.operator_pause = finish.is_some_and(|f| f.operator_pause);
        for (i, s) in self.sub_layers.iter_mut().enumerate() {
            s.options.operator_pause = finish.is_some_and(|f| f.sub_layer_pauses.contains(&i));
        }
        self
    }
}

/// An entry made from a layer's settings (LightBurn's Create new from layer / Update).
pub fn entry_from_layer(layer: &Layer, thickness: Option<f64>, title: &str, description: &str) -> Result<LibEntry, AppError> {
    let (cs, _warnings) = lbrn::cut_setting_from_layer(layer).map_err(|e| AppError::BadRequest(e.to_string()))?;
    Ok(LibEntry::new(thickness, title, description, cs))
}

/// Metadata of an entry as the API takes it.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryMeta {
    pub material: String,
    /// Thickness in mm; `null` or absent: No Thickness (then `title` names the folder).
    #[serde(default)]
    pub thickness_mm: Option<f64>,
    #[serde(default)]
    pub title: String,
    pub description: String,
}

impl EntryMeta {
    pub fn check(&self) -> Result<(), AppError> {
        let text = |field: &str, v: &str, required: bool| {
            if (required && v.trim().is_empty()) || v.chars().count() > 200 || v.contains('/') || v.chars().any(char::is_control) {
                Err(AppError::BadRequest(format!(
                    "{field} must be {}at most 200 characters without '/' or control characters",
                    if required { "1 to " } else { "" }
                )))
            } else {
                Ok(())
            }
        };
        text("material", &self.material, true)?;
        text("description", &self.description, true)?;
        text("title", &self.title, self.thickness_mm.is_none())?;
        if let Some(t) = self.thickness_mm
            && !(t.is_finite() && t > 0.0 && t <= 1000.0)
        {
            return Err(AppError::BadRequest(format!("thickness_mm must be within 0..=1000 mm (got {t})")));
        }
        Ok(())
    }
}

fn entry_json(id: EntryId, material: &str, e: &LibEntry, finish: Option<&Finish>) -> Value {
    let summary = preset_of(e).ok().map(|p| {
        let p = p.with_pauses(finish);
        let s = &p.settings;
        // The pauses for the operator come from OpenEtch's data beside the `.clb` (module `finish`), for Edit Cut.
        let (operator_pause, sub_layer_pauses) = pauses_of(s, &p.sub_layers);
        json!({
            "mode": s.mode, "power_pct": s.power_pct, "speed_mm_s": s.speed_mm_s, "frequency_khz": s.frequency_khz,
            "pulse_width_ns": s.pulse_width_ns, "passes": s.passes, "interval_mm": s.hatch.interval_mm,
            "sub_layers": p.sub_layers.len(), "operator_pause": operator_pause, "sub_layer_pauses": sub_layer_pauses,
        })
    });
    json!({
        "id": entry_id(id), "material": material, "thickness_mm": e.thickness(), "title": e.title(),
        "group": e.group_title(), "description": e.description(), "path": e.link_path(material), "summary": summary,
        "finish": finish.filter(|f| !f.is_empty()).map(Finish::summary),
    })
}

/// The browse tree of `lib`: materials, then the thickness/title groups, then the entries (each with its finish
/// summary from `finish`, by LinkPath).
pub fn tree(lib: &Library, finish: &BTreeMap<String, Finish>) -> Value {
    let materials: Vec<Value> = lib
        .materials
        .iter()
        .enumerate()
        .map(|(mi, m)| {
            let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
            for (ei, e) in m.entries.iter().enumerate() {
                let g = e.group_title();
                let v = entry_json((mi, ei), m.name(), e, finish.get(&e.link_path(m.name())));
                match groups.iter_mut().find(|(k, _)| *k == g) {
                    Some((_, list)) => list.push(v),
                    None => groups.push((g, vec![v])),
                }
            }
            let groups: Vec<Value> = groups.into_iter().map(|(g, entries)| json!({ "title": g, "entries": entries })).collect();
            json!({ "name": m.name(), "index": mi, "groups": groups })
        })
        .collect();
    json!({ "materials": materials, "entries": lib.entry_count() })
}

/// A library at [`lbrn::library::MAX_MATERIALS`] takes no new material (it could not be read back once saved).
pub fn refuse_new_material(lib: &Library, material: &str) -> Result<(), AppError> {
    if lib.materials.len() >= lbrn::library::MAX_MATERIALS && !lib.materials.iter().any(|m| m.name() == material) {
        return Err(AppError::Conflict(format!("the library has {} materials, the most it may hold", lib.materials.len())));
    }
    Ok(())
}

pub fn no_entry(id: EntryId) -> AppError {
    AppError::NotFound(format!("no library entry {}", entry_id(id)))
}

pub fn parse(bytes: &[u8]) -> Result<Library, AppError> {
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(AppError::TooLarge(format!("a library may be at most {} MB", MAX_LIBRARY_BYTES / (1024 * 1024))));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| AppError::BadRequest("the library is not UTF-8 text".into()))?;
    lbrn::parse_clb(text).map_err(|e| AppError::BadRequest(e.to_string()))
}


/// The store the browser build keeps material libraries in: `<name>` the `.clb` text (as saved), `<name>.finish` its
/// finishes (JSON by LinkPath), `index` `{loaded, selected}`.
pub const STORE: &str = "libraries";

/// The stored index of [`MemLibraries`].
#[derive(Debug, Default, Serialize, Deserialize)]
struct MemIndex {
    #[serde(default)]
    loaded: Vec<String>,
    #[serde(default)]
    selected: Option<String>,
}

fn not_loaded(name: &str) -> AppError {
    AppError::NotFound(format!("no loaded library named \"{name}\""))
}

/// The material libraries of the browser build: the engine's `LibraryStore` without its files. Edits stay in memory
/// until saved, as in LightBurn; changes return the records to keep. Every saved library stays in the store
/// (unloaded ones too, so the name of an unloaded library is refused as the engine refuses a file on disk).
#[derive(Debug, Default)]
pub struct MemLibraries {
    pub loaded: Vec<LoadedLibrary>,
    pub selected: Option<String>,
    /// Saved `.clb` texts and finishes by name (loaded or not).
    saved: BTreeMap<String, (String, BTreeMap<String, Finish>)>,
}

impl MemLibraries {
    /// Take a stored record (`key`, `value` of [`STORE`]); call [`MemLibraries::restored`] after the last one.
    pub fn restore(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key == "index" {
            let i: MemIndex = serde_json::from_str(value).map_err(|e| e.to_string())?;
            self.loaded = i.loaded.into_iter().map(|n| LoadedLibrary::new(&n, Library::new(), false)).collect();
            self.selected = i.selected;
            return Ok(());
        }
        if let Some(name) = key.strip_suffix(".finish") {
            let f: BTreeMap<String, Finish> = serde_json::from_str(value).map_err(|e| e.to_string())?;
            self.saved.entry(name.to_owned()).or_default().1 = f;
            return Ok(());
        }
        self.saved.entry(key.to_owned()).or_default().0 = value.to_owned();
        Ok(())
    }

    /// After every record was restored: the loaded libraries are parsed from their saved text. Returns problems.
    pub fn restored(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        let names: Vec<String> = self.loaded.iter().map(|l| l.name.clone()).collect();
        self.loaded.clear();
        for n in names {
            match self.saved.get(&n).map(|(t, f)| (parse(t.as_bytes()), f.clone())) {
                Some((Ok(lib), finish)) => {
                    let mut l = LoadedLibrary::new(&n, lib, false);
                    l.finish = finish;
                    self.loaded.push(l);
                }
                Some((Err(e), _)) => notes.push(format!("library \"{n}\": {e}")),
                None => notes.push(format!("library \"{n}\" is not in the store")),
            }
        }
        self.selected = self.selected.take().filter(|s| self.find(s).is_some());
        notes
    }

    fn has_saved(&self, name: &str) -> bool {
        self.saved.keys().any(|k| crate::project::same_name_ignoring_case(k, name))
    }

    fn index_write(&self) -> StoreWrite {
        let i = MemIndex { loaded: self.loaded.iter().map(|l| l.name.clone()).collect(), selected: self.selected.clone() };
        StoreWrite::json(STORE, "index", &i)
    }

    pub fn find(&self, name: &str) -> Option<&LoadedLibrary> {
        self.loaded.iter().find(|l| crate::project::same_name_ignoring_case(&l.name, name))
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut LoadedLibrary, AppError> {
        self.loaded.iter_mut().find(|l| crate::project::same_name_ignoring_case(&l.name, name)).ok_or_else(|| not_loaded(name))
    }

    pub fn get(&self, name: &str) -> Result<&LoadedLibrary, AppError> {
        self.find(name).ok_or_else(|| not_loaded(name))
    }

    /// `GET /api/library`: the loaded libraries and the selected one's tree.
    pub fn view(&self) -> Value {
        let list: Vec<Value> = self
            .loaded
            .iter()
            .map(|l| json!({ "name": l.name, "entries": l.lib.entry_count(), "materials": l.lib.materials.len(), "dirty": l.dirty }))
            .collect();
        let tree = self.selected.as_deref().and_then(|s| self.find(s)).map(|l| tree(&l.lib, &l.finish));
        json!({ "selected": self.selected, "libraries": list, "tree": tree })
    }

    fn refuse_unloaded_file(&self, name: &str) -> Result<(), AppError> {
        if self.has_saved(name) {
            return Err(AppError::Conflict(format!(
                "a library file named \"{name}\" already exists in the libraries folder; load it or choose another name"
            )));
        }
        Ok(())
    }

    fn add(&mut self, name: &str, lib: Library, dirty: bool) -> Result<(), AppError> {
        crate::project::validate_name(name)?;
        if self.find(name).is_some() {
            return Err(AppError::Conflict(format!("a library named \"{name}\" is already loaded")));
        }
        if self.loaded.len() >= MAX_LOADED {
            return Err(AppError::Conflict(format!("at most {MAX_LOADED} libraries can be loaded; unload one first")));
        }
        self.loaded.push(LoadedLibrary::new(name, lib, dirty));
        if self.selected.is_none() {
            self.selected = Some(name.to_string());
        }
        Ok(())
    }

    /// Load Library: a `.clb` file's bytes under `name`, kept byte for byte and selected.
    pub fn load(&mut self, name: &str, bytes: &[u8]) -> Result<Vec<StoreWrite>, AppError> {
        let lib = parse(bytes)?;
        self.add(name, lib, false)?;
        let text = String::from_utf8_lossy(bytes).into_owned();
        // The finishes kept for a library of this name (loaded before and unloaded), where it still has the path.
        let kept = self.saved.get(name).map(|(_, f)| f.clone()).unwrap_or_default();
        self.saved.insert(name.to_owned(), (text.clone(), kept.clone()));
        let l = self.get_mut(name)?;
        l.finish = kept;
        l.keep_known_finishes();
        self.selected = Some(name.to_string());
        Ok(vec![StoreWrite::text(STORE, name, text), self.index_write()])
    }

    /// Create New Library: an empty library, selected.
    pub fn create(&mut self, name: &str) -> Result<Vec<StoreWrite>, AppError> {
        crate::project::validate_name(name)?;
        if self.find(name).is_none() {
            self.refuse_unloaded_file(name)?;
        }
        self.add(name, Library::new(), true)?;
        self.selected = Some(name.to_string());
        Ok(vec![self.index_write()])
    }

    pub fn select(&mut self, name: &str) -> Result<(String, Vec<StoreWrite>), AppError> {
        let n = self.get(name)?.name.clone();
        self.selected = Some(n.clone());
        Ok((n, vec![self.index_write()]))
    }

    /// Unload: drop it from the list (its saved copy stays). Unsaved changes are refused unless `discard`.
    pub fn unload(&mut self, name: &str, discard: bool) -> Result<Vec<StoreWrite>, AppError> {
        let l = self.get(name)?;
        if l.dirty && !discard {
            return Err(AppError::Conflict(format!("library \"{}\" has unsaved changes; save it or unload with discard", l.name)));
        }
        let n = l.name.clone();
        self.loaded.retain(|l| l.name != n);
        if self.selected.as_deref() == Some(n.as_str()) {
            self.selected = self.loaded.first().map(|l| l.name.clone());
        }
        Ok(vec![self.index_write()])
    }

    /// Save Library (`new_name`: Save Library As, which loads the copy under the new name and selects it).
    pub fn save(&mut self, name: &str, new_name: Option<&str>) -> Result<(String, Vec<StoreWrite>), AppError> {
        let target = match new_name {
            Some(n) => {
                let lib = self.get(name)?.lib.clone();
                let finishes = self.get(name)?.finish.clone();
                crate::project::validate_name(n)?;
                if self.find(n).is_none() {
                    self.refuse_unloaded_file(n)?;
                }
                self.add(n, lib, true)?;
                self.get_mut(n)?.finish = finishes;
                self.selected = Some(n.to_string());
                n.to_string()
            }
            None => self.get(name)?.name.clone(),
        };
        let text = lbrn::write_clb(&self.get(&target)?.lib);
        let finish = self.get(&target)?.finish.clone();
        self.saved.insert(target.clone(), (text.clone(), finish.clone()));
        self.get_mut(&target)?.dirty = false;
        let writes = vec![StoreWrite::text(STORE, &target, text), StoreWrite::json(STORE, &format!("{target}.finish"), &finish), self.index_write()];
        Ok((target, writes))
    }

    /// Rename: the library and its saved copy. Returns the old name.
    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<(String, Vec<StoreWrite>), AppError> {
        crate::project::validate_name(new_name)?;
        let old = self.get(name)?.name.clone();
        if self.find(new_name).is_some_and(|l| l.name != old) {
            return Err(AppError::Conflict(format!("a library named \"{new_name}\" is already loaded")));
        }
        if !crate::project::same_name_ignoring_case(&old, new_name) {
            self.refuse_unloaded_file(new_name)?;
        }
        let mut writes = Vec::new();
        if let Some(copy) = self.saved.remove(&old) {
            writes.push(StoreWrite::delete(STORE, &old));
            writes.push(StoreWrite::delete(STORE, &format!("{old}.finish")));
            writes.push(StoreWrite::text(STORE, new_name, copy.0.clone()));
            writes.push(StoreWrite::json(STORE, &format!("{new_name}.finish"), &copy.1));
            self.saved.insert(new_name.to_owned(), copy);
        }
        self.get_mut(&old)?.name = new_name.to_string();
        if self.selected.as_deref() == Some(old.as_str()) {
            self.selected = Some(new_name.to_string());
        }
        writes.push(self.index_write());
        Ok((old, writes))
    }

    /// Merge Library With: add `from`'s entries to `name` (entries already there are skipped).
    pub fn merge(&mut self, name: &str, from: &str) -> Result<usize, AppError> {
        let other = self.get(from)?.lib.clone();
        let other_finish = self.get(from)?.finish.clone();
        let l = self.get_mut(name)?;
        let added = l.lib.merge(&other).map_err(|e| AppError::BadRequest(e.to_string()))?;
        let paths = l.paths();
        let mut took = 0;
        for (k, f) in other_finish {
            if paths.contains(&k) && !l.finish.contains_key(&k) {
                l.finish.insert(k, f);
                took += 1;
            }
        }
        l.dirty |= added > 0 || took > 0;
        Ok(added)
    }

    pub fn entry(&self, name: &str, id: EntryId) -> Result<(&str, &LibEntry), AppError> {
        let l = self.get(name)?;
        let m = l.lib.materials.get(id.0).ok_or_else(|| no_entry(id))?;
        Ok((m.name(), m.entries.get(id.1).ok_or_else(|| no_entry(id))?))
    }

    /// Add `entry` under `material` (created when missing) with its LinkPath; returns its id.
    pub fn add_entry(&mut self, name: &str, material: &str, mut entry: LibEntry) -> Result<EntryId, AppError> {
        let l = self.get_mut(name)?;
        if l.lib.entry_count() >= lbrn::library::MAX_ENTRIES {
            return Err(AppError::Conflict("the library is full".into()));
        }
        refuse_new_material(&l.lib, material)?;
        entry.set_link_path(&entry.link_path(material));
        let m = l.lib.material_mut(material);
        m.entries.push(entry);
        let e = m.entries.len() - 1;
        let mi = l.lib.materials.iter().position(|x| x.name() == material).expect("just added");
        l.dirty = true;
        Ok((mi, e))
    }

    /// Replace the settings of entry `id` with `entry`'s, keeping its place and metadata.
    pub fn replace_settings(&mut self, name: &str, id: EntryId, from: LibEntry) -> Result<(), AppError> {
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let material = m.name().to_string();
        let e = m.entries.get_mut(id.1).ok_or_else(|| no_entry(id))?;
        e.settings = from.settings;
        let path = e.link_path(&material);
        e.set_link_path(&path);
        l.dirty = true;
        Ok(())
    }

    /// Edit Description (as the engine's `LibraryStore::edit_meta`). Returns the new id.
    pub fn edit_meta(&mut self, name: &str, id: EntryId, meta: &EntryMeta) -> Result<EntryId, AppError> {
        meta.check()?;
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let mut e = m.entries.get(id.1).ok_or_else(|| no_entry(id))?.clone();
        let old_path = e.link_path(m.name());
        let moves = m.name() != meta.material;
        if moves {
            refuse_new_material(&l.lib, &meta.material)?;
        }
        e.set_meta(meta.thickness_mm, &meta.title, &meta.description);
        let new_path = e.link_path(&meta.material);
        e.set_link_path(&new_path);
        l.dirty = true;
        if old_path != new_path
            && let Some(f) = l.finish.get(&old_path).cloned()
        {
            let shared = l.lib.materials.iter().map(|m| m.entries.iter().filter(|x| x.link_path(m.name()) == old_path).count()).sum::<usize>() > 1;
            if !shared {
                l.finish.remove(&old_path);
            }
            l.finish.insert(new_path, f);
        }
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        if !moves {
            m.entries[id.1] = e;
            return Ok(id);
        }
        m.entries.remove(id.1);
        let target = l.lib.material_mut(&meta.material);
        target.entries.push(e);
        let mi = l.lib.materials.iter().position(|x| x.name() == meta.material).expect("just added");
        let len = l.lib.materials[mi].entries.len();
        Ok((mi, len - 1))
    }

    /// Duplicate: a copy right after it. Returns the copy's id.
    pub fn duplicate(&mut self, name: &str, id: EntryId) -> Result<EntryId, AppError> {
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let copy = m.entries.get(id.1).ok_or_else(|| no_entry(id))?.detached();
        m.entries.insert(id.1 + 1, copy);
        l.dirty = true;
        Ok((id.0, id.1 + 1))
    }

    /// Delete; a material left without entries is removed too. Returns the deleted entry's LinkPath.
    pub fn delete(&mut self, name: &str, id: EntryId) -> Result<String, AppError> {
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let e = m.entries.get(id.1).ok_or_else(|| no_entry(id))?;
        let path = e.link_path(m.name());
        m.entries.remove(id.1);
        if m.entries.is_empty() {
            l.lib.materials.remove(id.0);
        }
        if l.lib.find_path(&path).is_none() {
            l.finish.remove(&path);
        }
        l.dirty = true;
        Ok(path)
    }

    /// The `.clb` text of a library as it would be saved.
    pub fn text(&self, name: &str) -> Result<(String, String), AppError> {
        let l = self.get(name)?;
        Ok((l.name.clone(), lbrn::write_clb(&l.lib)))
    }

    /// The finish of the entry at `path` in library `name` (empty when none).
    pub fn finish_of(&self, name: &str, path: &str) -> Result<Finish, AppError> {
        Ok(self.get(name)?.finish.get(path).cloned().unwrap_or_default())
    }

    /// Keep the pauses for the operator of `settings` / `sub_layers` for the entry at `path` (a `.clb` has no place
    /// for them). Unchanged pauses change nothing.
    pub fn set_pauses(&mut self, name: &str, path: &str, settings: &CutSettings, sub_layers: &[CutSettings]) -> Result<(), AppError> {
        let (main, subs) = pauses_of(settings, sub_layers);
        let cur = self.get(name)?.finish.get(path);
        if cur.is_some_and(|f| f.operator_pause) == main && cur.map_or(&[][..], |f| &f.sub_layer_pauses) == subs.as_slice() {
            return Ok(());
        }
        let l = self.get_mut(name)?;
        if l.lib.find_path(path).is_none() {
            return Err(AppError::NotFound(format!("library \"{}\" has no preset \"{path}\"", l.name)));
        }
        let mut f = l.finish.get(path).cloned().unwrap_or_default();
        f.operator_pause = main;
        f.sub_layer_pauses = subs;
        if f.is_empty() {
            l.finish.remove(path);
        } else {
            l.finish.insert(path.to_string(), f);
        }
        l.dirty = true;
        Ok(())
    }

    /// The preset of entry `id` with its pauses for the operator.
    pub fn preset(&self, name: &str, id: EntryId) -> Result<Preset, AppError> {
        let (material, e) = self.entry(name, id)?;
        let path = e.link_path(material);
        Ok(preset_of(e)?.with_pauses(self.get(name)?.finish.get(&path)))
    }

    /// What a `.clb` of library `name` leaves out: the presets' pauses for the operator.
    pub fn clb_losses(&self, name: &str) -> Result<Vec<String>, AppError> {
        let l = self.get(name)?;
        Ok(l.paths()
            .into_iter()
            .filter(|p| l.finish.get(p).is_some_and(|f| f.operator_pause || !f.sub_layer_pauses.is_empty()))
            .map(|p| format!("preset \"{p}\": its pause for the operator (OpenEtch only) was not written"))
            .collect())
    }
}
