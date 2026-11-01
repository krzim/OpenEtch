//! The material library store (LightBurn's Library window): `.clb` libraries kept in `<config-dir>/libraries/`
//! (`lbrn::Library`, byte-identical round trips), the loaded ones and the selected one (`libraries/index.json`),
//! the browse tree (material → thickness or title → description), and entry editing: create from a layer, update
//! from a layer, edit the cut settings, edit the description, duplicate, delete; save, save as, new, merge, rename,
//! unload. Edits stay in memory until saved, as in LightBurn. Applying presets to layers (assign, link, and keeping
//! linked layers in step) is in `api_libs`, which turns entries into `layer.apply_preset` invocations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use doc::{CutSettings, Layer};
use lbrn::{LibEntry, Library};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;
use crate::prefs::write_atomic;

pub const LIBRARIES_DIR: &str = "libraries";
const INDEX_FILE: &str = "index.json";
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
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    format: String,
    #[serde(default)]
    loaded: Vec<String>,
    #[serde(default)]
    selected: Option<String>,
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

pub fn parse_entry_id(s: &str) -> Result<EntryId, EngineError> {
    let bad = || EngineError::BadRequest(format!("'{s}' is not a library entry id (m<material>.e<entry>)"));
    let (m, e) = s.split_once('.').ok_or_else(bad)?;
    let m = m.strip_prefix('m').and_then(|v| v.parse().ok()).ok_or_else(bad)?;
    let e = e.strip_prefix('e').and_then(|v| v.parse().ok()).ok_or_else(bad)?;
    Ok((m, e))
}

/// The preset of an entry: its settings as an OpenEtch layer's.
pub fn preset_of(entry: &LibEntry) -> Result<Preset, EngineError> {
    let (layer, _warnings) = entry.to_layer("layer-preset").map_err(|e| EngineError::BadRequest(e.to_string()))?;
    let extras = layer
        .extras
        .into_iter()
        .filter(|(k, _)| k.starts_with(lbrn::EXTRA_PREFIX) && k != "lbrn2.index" && k != "lbrn2.LinkPath")
        .collect();
    Ok(Preset { settings: layer.settings, sub_layers: layer.sub_layers, global_passes: layer.global_passes, extras })
}

/// An entry made from a layer's settings (LightBurn's Create new from layer / Update).
pub fn entry_from_layer(layer: &Layer, thickness: Option<f64>, title: &str, description: &str) -> Result<LibEntry, EngineError> {
    let (cs, _warnings) = lbrn::cut_setting_from_layer(layer).map_err(|e| EngineError::BadRequest(e.to_string()))?;
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
    pub fn check(&self) -> Result<(), EngineError> {
        let text = |field: &str, v: &str, required: bool| {
            if (required && v.trim().is_empty()) || v.chars().count() > 200 || v.contains('/') || v.chars().any(char::is_control) {
                Err(EngineError::BadRequest(format!(
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
            return Err(EngineError::BadRequest(format!("thickness_mm must be within 0..=1000 mm (got {t})")));
        }
        Ok(())
    }
}

fn entry_json(id: EntryId, material: &str, e: &LibEntry) -> Value {
    let summary = preset_of(e).ok().map(|p| {
        let s = &p.settings;
        json!({
            "mode": s.mode, "power_pct": s.power_pct, "speed_mm_s": s.speed_mm_s, "frequency_khz": s.frequency_khz,
            "pulse_width_ns": s.pulse_width_ns, "passes": s.passes, "interval_mm": s.hatch.interval_mm,
            "sub_layers": p.sub_layers.len(),
        })
    });
    json!({
        "id": entry_id(id), "material": material, "thickness_mm": e.thickness(), "title": e.title(),
        "group": e.group_title(), "description": e.description(), "path": e.link_path(material), "summary": summary,
    })
}

/// The browse tree of `lib`: materials, then the thickness/title groups, then the entries.
pub fn tree(lib: &Library) -> Value {
    let materials: Vec<Value> = lib
        .materials
        .iter()
        .enumerate()
        .map(|(mi, m)| {
            let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
            for (ei, e) in m.entries.iter().enumerate() {
                let g = e.group_title();
                let v = entry_json((mi, ei), m.name(), e);
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

/// The libraries the engine has loaded. See the module docs.
#[derive(Debug, Default)]
pub struct LibraryStore {
    dir: Option<PathBuf>,
    pub loaded: Vec<LoadedLibrary>,
    pub selected: Option<String>,
}

fn not_loaded(name: &str) -> EngineError {
    EngineError::NotFound(format!("no loaded library named \"{name}\""))
}

impl LibraryStore {
    /// Open `<config-dir>/libraries/` and load the libraries the index lists (skipping and reporting broken ones).
    pub fn open(config_dir: &Path) -> (LibraryStore, Vec<String>) {
        let dir = config_dir.join(LIBRARIES_DIR);
        let mut notes = Vec::new();
        let index: Index = match std::fs::read_to_string(dir.join(INDEX_FILE)) {
            Ok(t) => serde_json::from_str(&t).unwrap_or_else(|e| {
                notes.push(format!("libraries/{INDEX_FILE}: {e}"));
                Index::default()
            }),
            Err(_) => Index::default(),
        };
        let mut store = LibraryStore { dir: Some(dir), loaded: Vec::new(), selected: None };
        for name in index.loaded {
            match store.read_file(&name) {
                Ok(lib) => store.loaded.push(LoadedLibrary { name, lib, dirty: false }),
                Err(e) => notes.push(format!("library \"{name}\": {e}")),
            }
        }
        store.selected = index.selected.filter(|s| store.find(s).is_some());
        (store, notes)
    }

    fn file(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(format!("{name}.clb")))
    }

    fn read_file(&self, name: &str) -> Result<Library, EngineError> {
        crate::projects::validate_name(name)?;
        let path = self.file(name).ok_or_else(|| not_loaded(name))?;
        let bytes = std::fs::read(&path).map_err(|e| EngineError::NotFound(format!("{}: {e}", path.display())))?;
        parse(&bytes)
    }

    fn save_index(&self) -> Result<(), EngineError> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let index = Index {
            format: "openetch-libraries".into(),
            loaded: self.loaded.iter().map(|l| l.name.clone()).collect(),
            selected: self.selected.clone(),
        };
        let text = serde_json::to_string_pretty(&index).map_err(|e| EngineError::Internal(e.to_string()))?;
        write_atomic(&dir.join(INDEX_FILE), text.as_bytes())
    }

    pub fn find(&self, name: &str) -> Option<&LoadedLibrary> {
        self.loaded.iter().find(|l| crate::projects::same_name_ignoring_case(&l.name, name))
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut LoadedLibrary, EngineError> {
        self.loaded.iter_mut().find(|l| crate::projects::same_name_ignoring_case(&l.name, name)).ok_or_else(|| not_loaded(name))
    }

    pub fn get(&self, name: &str) -> Result<&LoadedLibrary, EngineError> {
        self.find(name).ok_or_else(|| not_loaded(name))
    }

    /// `GET /api/library`: the loaded libraries and the selected one's tree.
    pub fn view(&self) -> Value {
        let list: Vec<Value> = self
            .loaded
            .iter()
            .map(|l| json!({ "name": l.name, "entries": l.lib.entry_count(), "materials": l.lib.materials.len(), "dirty": l.dirty }))
            .collect();
        let tree = self.selected.as_deref().and_then(|s| self.find(s)).map(|l| tree(&l.lib));
        json!({ "selected": self.selected, "libraries": list, "tree": tree })
    }

    /// Refuse `name` when a library file of that name is on disk but not loaded: saving under it would replace a
    /// library the user never opened (LightBurn's Save As asks before replacing a file; load it instead).
    fn refuse_unloaded_file(&self, name: &str) -> Result<(), EngineError> {
        if self.file(name).is_some_and(|p| p.exists()) {
            return Err(EngineError::Conflict(format!(
                "a library file named \"{name}\" already exists in the libraries folder; load it or choose another name"
            )));
        }
        Ok(())
    }

    fn add(&mut self, name: &str, lib: Library, dirty: bool) -> Result<(), EngineError> {
        crate::projects::validate_name(name)?;
        if self.find(name).is_some() {
            return Err(EngineError::Conflict(format!("a library named \"{name}\" is already loaded")));
        }
        if self.loaded.len() >= MAX_LOADED {
            return Err(EngineError::Conflict(format!("at most {MAX_LOADED} libraries can be loaded; unload one first")));
        }
        self.loaded.push(LoadedLibrary { name: name.to_string(), lib, dirty });
        if self.selected.is_none() {
            self.selected = Some(name.to_string());
        }
        Ok(())
    }

    /// Load Library: a `.clb` file's bytes under `name`, stored in the libraries folder (byte for byte) and selected.
    pub fn load(&mut self, name: &str, bytes: &[u8]) -> Result<(), EngineError> {
        let lib = parse(bytes)?;
        self.add(name, lib, false)?;
        if let Some(path) = self.file(name)
            && let Err(e) = write_atomic(&path, bytes)
        {
            self.loaded.retain(|l| l.name != name);
            return Err(e);
        }
        self.selected = Some(name.to_string());
        self.save_index()
    }

    /// Create New Library: an empty library, selected.
    pub fn create(&mut self, name: &str) -> Result<(), EngineError> {
        crate::projects::validate_name(name)?;
        if self.find(name).is_none() {
            self.refuse_unloaded_file(name)?;
        }
        self.add(name, Library::new(), true)?;
        self.selected = Some(name.to_string());
        self.save_index()
    }

    pub fn select(&mut self, name: &str) -> Result<String, EngineError> {
        let n = self.get(name)?.name.clone();
        self.selected = Some(n.clone());
        self.save_index()?;
        Ok(n)
    }

    /// Unload: drop it from the list (the file stays). Unsaved changes are refused unless `discard`.
    pub fn unload(&mut self, name: &str, discard: bool) -> Result<(), EngineError> {
        let l = self.get(name)?;
        if l.dirty && !discard {
            return Err(EngineError::Conflict(format!("library \"{}\" has unsaved changes; save it or unload with discard", l.name)));
        }
        let n = l.name.clone();
        self.loaded.retain(|l| l.name != n);
        if self.selected.as_deref() == Some(n.as_str()) {
            self.selected = self.loaded.first().map(|l| l.name.clone());
        }
        self.save_index()
    }

    /// Save Library (`new_name`: Save Library As, which loads the copy under the new name and selects it).
    pub fn save(&mut self, name: &str, new_name: Option<&str>) -> Result<String, EngineError> {
        let target = match new_name {
            Some(n) => {
                let lib = self.get(name)?.lib.clone();
                crate::projects::validate_name(n)?;
                if self.find(n).is_none() {
                    self.refuse_unloaded_file(n)?;
                }
                self.add(n, lib, true)?;
                self.selected = Some(n.to_string());
                n.to_string()
            }
            None => self.get(name)?.name.clone(),
        };
        let text = lbrn::write_clb(&self.get(&target)?.lib);
        if let Some(path) = self.file(&target) {
            write_atomic(&path, text.as_bytes())?;
        }
        self.get_mut(&target)?.dirty = false;
        self.save_index()?;
        Ok(target)
    }

    /// Rename: the library and its file (its links in documents must follow; see `api_libs`).
    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<String, EngineError> {
        crate::projects::validate_name(new_name)?;
        let old = self.get(name)?.name.clone();
        if self.find(new_name).is_some_and(|l| l.name != old) {
            return Err(EngineError::Conflict(format!("a library named \"{new_name}\" is already loaded")));
        }
        // `fs::rename` replaces an existing file: never over another (unloaded) library. Only a change of case of
        // the same name may find "its own" file there.
        if !crate::projects::same_name_ignoring_case(&old, new_name) {
            self.refuse_unloaded_file(new_name)?;
        }
        if let (Some(from), Some(to)) = (self.file(&old), self.file(new_name))
            && from.is_file()
        {
            std::fs::rename(&from, &to).map_err(|e| EngineError::Internal(format!("cannot rename {}: {e}", from.display())))?;
        }
        self.get_mut(&old)?.name = new_name.to_string();
        if self.selected.as_deref() == Some(old.as_str()) {
            self.selected = Some(new_name.to_string());
        }
        self.save_index()?;
        Ok(old)
    }

    /// Merge Library With: add `from`'s entries to `name` (entries already there are skipped).
    pub fn merge(&mut self, name: &str, from: &str) -> Result<usize, EngineError> {
        let other = self.get(from)?.lib.clone();
        let l = self.get_mut(name)?;
        let added = l.lib.merge(&other).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        l.dirty |= added > 0;
        Ok(added)
    }

    pub fn entry(&self, name: &str, id: EntryId) -> Result<(&str, &LibEntry), EngineError> {
        let l = self.get(name)?;
        let m = l.lib.materials.get(id.0).ok_or_else(|| no_entry(id))?;
        Ok((m.name(), m.entries.get(id.1).ok_or_else(|| no_entry(id))?))
    }

    /// Add `entry` under `material` (created when missing) with its LinkPath; returns its id.
    pub fn add_entry(&mut self, name: &str, material: &str, mut entry: LibEntry) -> Result<EntryId, EngineError> {
        let l = self.get_mut(name)?;
        if l.lib.entry_count() >= lbrn::library::MAX_ENTRIES {
            return Err(EngineError::Conflict("the library is full".into()));
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

    /// Replace the settings of entry `id` with `entry`'s, keeping its place and metadata (Update / Edit Cut).
    pub fn replace_settings(&mut self, name: &str, id: EntryId, from: LibEntry) -> Result<(), EngineError> {
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

    /// Edit Description: material, thickness/title and description; an entry moved to another material goes to the
    /// end of it. Returns the new id.
    pub fn edit_meta(&mut self, name: &str, id: EntryId, meta: &EntryMeta) -> Result<EntryId, EngineError> {
        meta.check()?;
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let mut e = m.entries.get(id.1).ok_or_else(|| no_entry(id))?.clone();
        e.set_meta(meta.thickness_mm, &meta.title, &meta.description);
        e.set_link_path(&e.link_path(&meta.material));
        l.dirty = true;
        if m.name() == meta.material {
            m.entries[id.1] = e;
            return Ok(id);
        }
        refuse_new_material(&l.lib, &meta.material)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        m.entries.remove(id.1);
        let target = l.lib.material_mut(&meta.material);
        target.entries.push(e);
        let new = (l.lib.materials.iter().position(|x| x.name() == meta.material).expect("just added"), 0);
        let len = l.lib.materials[new.0].entries.len();
        Ok((new.0, len - 1))
    }

    /// Duplicate: a copy right after it. Returns the copy's id.
    pub fn duplicate(&mut self, name: &str, id: EntryId) -> Result<EntryId, EngineError> {
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let copy = m.entries.get(id.1).ok_or_else(|| no_entry(id))?.detached();
        m.entries.insert(id.1 + 1, copy);
        l.dirty = true;
        Ok((id.0, id.1 + 1))
    }

    /// Delete; a material left without entries is removed too. Returns the deleted entry's LinkPath.
    pub fn delete(&mut self, name: &str, id: EntryId) -> Result<String, EngineError> {
        let l = self.get_mut(name)?;
        let m = l.lib.materials.get_mut(id.0).ok_or_else(|| no_entry(id))?;
        let e = m.entries.get(id.1).ok_or_else(|| no_entry(id))?;
        let path = e.link_path(m.name());
        m.entries.remove(id.1);
        if m.entries.is_empty() {
            l.lib.materials.remove(id.0);
        }
        l.dirty = true;
        Ok(path)
    }

    /// The `.clb` text of a library as it would be saved.
    pub fn text(&self, name: &str) -> Result<(String, String), EngineError> {
        let l = self.get(name)?;
        Ok((l.name.clone(), lbrn::write_clb(&l.lib)))
    }
}

/// A library at [`lbrn::library::MAX_MATERIALS`] takes no new material (it could not be read back once saved).
fn refuse_new_material(lib: &Library, material: &str) -> Result<(), EngineError> {
    if lib.materials.len() >= lbrn::library::MAX_MATERIALS && !lib.materials.iter().any(|m| m.name() == material) {
        return Err(EngineError::Conflict(format!("the library has {} materials, the most it may hold", lib.materials.len())));
    }
    Ok(())
}

fn no_entry(id: EntryId) -> EngineError {
    EngineError::NotFound(format!("no library entry {}", entry_id(id)))
}

fn parse(bytes: &[u8]) -> Result<Library, EngineError> {
    if bytes.len() > MAX_LIBRARY_BYTES {
        return Err(EngineError::TooLarge(format!("a library may be at most {} MB", MAX_LIBRARY_BYTES / (1024 * 1024))));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| EngineError::BadRequest("the library is not UTF-8 text".into()))?;
    lbrn::parse_clb(text).map_err(|e| EngineError::BadRequest(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Engrave\" NoThickTitle=\"Finish\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Finish/Engrave\" />\r\n        <minPower Value=\"7\" />\r\n        <maxPower Value=\"38\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"25000\" />\r\n        <interval Value=\"0.025\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("openetch-lib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn load_edit_save_and_reopen() {
        let d = dir("a");
        let (mut s, notes) = LibraryStore::open(&d);
        assert!(notes.is_empty());
        s.load("steel", CLB.as_bytes()).unwrap();
        assert_eq!(std::fs::read(d.join("libraries/steel.clb")).unwrap(), CLB.as_bytes(), "stored byte for byte");
        let p = preset_of(s.entry("steel", (0, 0)).unwrap().1).unwrap();
        assert_eq!((p.settings.power_pct, p.settings.frequency_khz), (38.0, 25.0));
        assert_eq!(p.extras.get("lbrn2.minPower").map(String::as_str), Some("7"));
        assert!(!p.extras.contains_key("lbrn2.LinkPath"));

        // Create from a layer, edit its description, duplicate, delete.
        let mut layer = Layer::new("layer-1".into(), "Blue", "#00f");
        layer.settings.power_pct = 60.0;
        let e = entry_from_layer(&layer, None, "Colors", "Blue").unwrap();
        let id = s.add_entry("steel", "Steel", e).unwrap();
        assert_eq!(id, (0, 1));
        let meta = EntryMeta { material: "Titanium".into(), thickness_mm: None, title: "Colors".into(), description: "Gold".into() };
        let moved = s.edit_meta("steel", id, &meta).unwrap();
        assert_eq!(moved, (1, 0));
        assert_eq!(s.entry("steel", moved).unwrap().1.link_path("Titanium"), "Titanium/Colors/Gold");
        let copy = s.duplicate("steel", moved).unwrap();
        assert_eq!(copy, (1, 1));
        assert_eq!(s.delete("steel", copy).unwrap(), "Titanium/Colors/Gold");
        assert!(s.find("steel").unwrap().dirty);
        let t = tree(&s.get("steel").unwrap().lib);
        assert_eq!(t["materials"][1]["groups"][0]["entries"][0]["path"], "Titanium/Colors/Gold");
        assert_eq!(t["materials"][0]["groups"][0]["entries"][0]["summary"]["power_pct"], 38.0);
        s.save("steel", None).unwrap();
        assert!(!s.find("steel").unwrap().dirty);

        let (again, notes) = LibraryStore::open(&d);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(again.selected.as_deref(), Some("steel"));
        assert_eq!(again.get("steel").unwrap().lib.entry_count(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn manage_libraries() {
        let d = dir("b");
        let (mut s, _) = LibraryStore::open(&d);
        s.load("a", CLB.as_bytes()).unwrap();
        s.create("b").unwrap();
        assert_eq!(s.selected.as_deref(), Some("b"));
        assert!(s.load("A", CLB.as_bytes()).is_err(), "names ignore case");
        assert_eq!(s.merge("b", "a").unwrap(), 1);
        assert_eq!(s.merge("b", "a").unwrap(), 0, "already there");
        assert!(s.unload("b", false).is_err(), "unsaved");
        s.save("b", Some("c")).unwrap();
        assert!(d.join("libraries/c.clb").is_file());
        s.rename("c", "d").unwrap();
        assert!(d.join("libraries/d.clb").is_file() && !d.join("libraries/c.clb").exists());
        s.unload("b", true).unwrap();
        assert_eq!(s.loaded.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["a", "d"]);
        assert!(s.select("zzz").is_err());
        assert!(s.load("bad", b"<LightBurnProject/>").is_err());
        assert!(parse_entry_id("m1.e2").unwrap() == (1, 2) && parse_entry_id("x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn never_replaces_an_unloaded_library_file() {
        let d = dir("c");
        let (mut s, _) = LibraryStore::open(&d);
        s.load("keep", CLB.as_bytes()).unwrap();
        s.unload("keep", false).unwrap();
        let keep = d.join("libraries/keep.clb");
        s.create("other").unwrap();
        s.save("other", None).unwrap();
        // Rename, Save As and New onto the unloaded file are refused; the file is untouched.
        assert!(matches!(s.rename("other", "keep"), Err(EngineError::Conflict(_))));
        if cfg!(windows) {
            assert!(matches!(s.rename("other", "KEEP"), Err(EngineError::Conflict(_))), "file names ignore case");
        }
        assert!(matches!(s.save("other", Some("keep")), Err(EngineError::Conflict(_))));
        assert!(matches!(s.create("keep"), Err(EngineError::Conflict(_))));
        assert_eq!(std::fs::read(&keep).unwrap(), CLB.as_bytes());
        assert!(s.find("keep").is_none() && s.find("other").is_some());
        // A change of case of the library's own name is still a rename.
        s.rename("other", "Other").unwrap();
        assert_eq!(s.get("other").unwrap().name, "Other");
        let _ = std::fs::remove_dir_all(&d);
    }
}
