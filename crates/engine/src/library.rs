//! The material library store (LightBurn's Library window): `.clb` libraries kept in `<config-dir>/libraries/`
//! (`lbrn::Library`, byte-identical round trips), the loaded ones and the selected one (`libraries/index.json`),
//! the browse tree (material → thickness or title → description), and entry editing: create from a layer, update
//! from a layer, edit the cut settings, edit the description, duplicate, delete; save, save as, new, merge, rename,
//! unload. Edits stay in memory until saved, as in LightBurn. Applying presets to layers (assign, link, and keeping
//! linked layers in step) is in `api_libs`, which turns entries into `layer.apply_preset` invocations.
//!
//! Each library also has its finish data (`crate::finish`: result photo, colour, notes, lens, focus, lot, surface),
//! kept by LinkPath in an OpenEtch sidecar next to the `.clb`, saved, renamed and merged with it; and the store keeps
//! the per-device hue step models.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use doc::CutSettings;
use lbrn::{LibEntry, Library};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;
use crate::finish::{self, Finish};
use crate::prefs::write_atomic;

pub const LIBRARIES_DIR: &str = "libraries";
// The library types, entries, presets and the browse tree (shared with the browser core).
pub use appcore::library::{
    EntryId, EntryMeta, LoadedLibrary, MAX_LIBRARY_BYTES, MAX_LOADED, Preset, entry_from_layer, entry_id, parse_entry_id, pauses_of,
    preset_of, tree,
};
use appcore::library::{no_entry, parse, refuse_new_material};
const INDEX_FILE: &str = "index.json";
#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    format: String,
    #[serde(default)]
    loaded: Vec<String>,
    #[serde(default)]
    selected: Option<String>,
}

/// The libraries the engine has loaded. See the module docs.
#[derive(Debug, Default)]
pub struct LibraryStore {
    dir: Option<PathBuf>,
    pub loaded: Vec<LoadedLibrary>,
    pub selected: Option<String>,
    /// The hue step model of each device (`crate::finish::HUE_MODELS_FILE`).
    pub hue: BTreeMap<String, commands::finish::HueModel>,
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
        let hue = finish::read_hue_models(&dir);
        let mut store = LibraryStore { dir: Some(dir), loaded: Vec::new(), selected: None, hue };
        for name in index.loaded {
            match store.read_file(&name) {
                Ok(lib) => {
                    let mut l = LoadedLibrary::new(&name, lib, false);
                    match store.read_finish(&name) {
                        Ok(f) => l.finish = f,
                        Err(e) => notes.push(format!("library \"{name}\" finishes: {e}")),
                    }
                    store.loaded.push(l);
                }
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
        Ok(parse(&bytes)?)
    }

    fn read_finish(&self, name: &str) -> Result<BTreeMap<String, Finish>, EngineError> {
        match &self.dir {
            Some(d) => finish::read_sidecar(d, name),
            None => Ok(BTreeMap::new()),
        }
    }

    /// The hue step model of `device` (xTool's published one until it is set).
    pub fn hue_model(&self, device: &str) -> commands::finish::HueModel {
        self.hue.get(device).cloned().unwrap_or_default()
    }

    /// Set (`None`: reset) the hue step model of `device`, saved at once.
    pub fn set_hue_model(&mut self, device: &str, model: Option<commands::finish::HueModel>) -> Result<(), EngineError> {
        if let Some(p) = model.as_ref().and_then(commands::finish::HueModel::problem) {
            return Err(EngineError::BadRequest(format!("hue model: {p}")));
        }
        let mut next = self.hue.clone();
        match model {
            Some(m) => next.insert(device.to_string(), m),
            None => next.remove(device),
        };
        if let Some(d) = &self.dir {
            finish::write_hue_models(d, &next)?;
        }
        self.hue = next;
        Ok(())
    }

    /// The finish of the entry at `path` in library `name` (empty when none).
    pub fn finish_of(&self, name: &str, path: &str) -> Result<Finish, EngineError> {
        Ok(self.get(name)?.finish.get(path).cloned().unwrap_or_default())
    }

    /// Change the finish of the entry at `path` (it must exist) with `f`; marks the library unsaved.
    pub fn edit_finish<R>(&mut self, name: &str, path: &str, f: impl FnOnce(&mut Finish) -> Result<R, EngineError>) -> Result<R, EngineError> {
        let l = self.get_mut(name)?;
        if l.lib.find_path(path).is_none() {
            return Err(EngineError::NotFound(format!("library \"{}\" has no preset \"{path}\"", l.name)));
        }
        let mut cur = l.finish.get(path).cloned().unwrap_or_default();
        let r = f(&mut cur)?;
        if cur.is_empty() {
            l.finish.remove(path);
        } else {
            l.finish.insert(path.to_string(), cur);
        }
        l.dirty = true;
        Ok(r)
    }

    /// Import a finish sidecar into library `name`: the finishes of paths it has replace theirs; returns `(imported,
    /// skipped)` (skipped: paths the library does not have).
    pub fn import_finish(&mut self, name: &str, bytes: &[u8]) -> Result<(usize, usize), EngineError> {
        let incoming = finish::parse_sidecar(bytes)?;
        let l = self.get_mut(name)?;
        let paths = l.paths();
        let (mut imported, mut skipped) = (0, 0);
        for (k, f) in incoming {
            if paths.contains(&k) {
                l.finish.insert(k, f);
                imported += 1;
            } else {
                skipped += 1;
            }
        }
        l.dirty |= imported > 0;
        Ok((imported, skipped))
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
        let tree = self.selected.as_deref().and_then(|s| self.find(s)).map(|l| tree(&l.lib, &l.finish));
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
        self.loaded.push(LoadedLibrary::new(name, lib, dirty));
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
        // The finishes kept for a library of this name (loaded before and unloaded), where it still has the path.
        if let Ok(f) = self.read_finish(name) {
            let l = self.get_mut(name)?;
            l.finish = f;
            l.keep_known_finishes();
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
                let finishes = self.get(name)?.finish.clone();
                crate::projects::validate_name(n)?;
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
        if let Some(path) = self.file(&target) {
            write_atomic(&path, text.as_bytes())?;
        }
        if let Some(d) = &self.dir {
            let l = self.get(&target)?;
            finish::write_sidecar(d, &l.name, &l.finish)?;
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
        if let Some(d) = &self.dir {
            let (from, to) = (d.join(format!("{old}{}", finish::SIDECAR_SUFFIX)), d.join(format!("{new_name}{}", finish::SIDECAR_SUFFIX)));
            if from.is_file() {
                std::fs::rename(&from, &to).map_err(|e| EngineError::Internal(format!("cannot rename {}: {e}", from.display())))?;
            }
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
        let other_finish = self.get(from)?.finish.clone();
        let l = self.get_mut(name)?;
        let added = l.lib.merge(&other).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        // Finishes come along for the paths that have none here.
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
        let old_path = e.link_path(m.name());
        let moves = m.name() != meta.material;
        // Refused before anything changes (the library stays as it was, saved or not).
        if moves {
            refuse_new_material(&l.lib, &meta.material)?;
        }
        e.set_meta(meta.thickness_mm, &meta.title, &meta.description);
        let new_path = e.link_path(&meta.material);
        e.set_link_path(&new_path);
        l.dirty = true;
        // The finish follows the entry (copied when a duplicate still has the old path).
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
        if l.lib.find_path(&path).is_none() {
            l.finish.remove(&path);
        }
        l.dirty = true;
        Ok(path)
    }

    /// The `.clb` text of a library as it would be saved.
    pub fn text(&self, name: &str) -> Result<(String, String), EngineError> {
        let l = self.get(name)?;
        Ok((l.name.clone(), lbrn::write_clb(&l.lib)))
    }

    /// Keep the pauses for the operator of `settings` / `sub_layers` for the entry at `path` (OpenEtch's own data:
    /// a `.clb` has no place for them). Unchanged pauses change nothing.
    pub fn set_pauses(&mut self, name: &str, path: &str, settings: &CutSettings, sub_layers: &[CutSettings]) -> Result<(), EngineError> {
        let (main, subs) = pauses_of(settings, sub_layers);
        let cur = self.get(name)?.finish.get(path);
        if cur.is_some_and(|f| f.operator_pause) == main && cur.map_or(&[][..], |f| &f.sub_layer_pauses) == subs.as_slice() {
            return Ok(());
        }
        self.edit_finish(name, path, |f| {
            f.operator_pause = main;
            f.sub_layer_pauses = subs;
            Ok(())
        })
    }

    /// The preset of entry `id` with its pauses for the operator.
    pub fn preset(&self, name: &str, id: EntryId) -> Result<Preset, EngineError> {
        let (material, e) = self.entry(name, id)?;
        let path = e.link_path(material);
        Ok(preset_of(e)?.with_pauses(self.get(name)?.finish.get(&path)))
    }

    /// What a `.clb` of library `name` leaves out (said on its download, as a `.lbrn2` export says what it loses):
    /// the presets' pauses for the operator, which stay in OpenEtch's library data.
    pub fn clb_losses(&self, name: &str) -> Result<Vec<String>, EngineError> {
        let l = self.get(name)?;
        Ok(l.paths()
            .into_iter()
            .filter(|p| l.finish.get(p).is_some_and(|f| f.operator_pause || !f.sub_layer_pauses.is_empty()))
            .map(|p| format!("preset \"{p}\": its pause for the operator (OpenEtch only) was not written"))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::Layer;

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
        let t = tree(&s.get("steel").unwrap().lib, &BTreeMap::new());
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
