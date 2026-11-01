//! The device registry ([`DeviceRegistry`]): OpenEtch's devices, LightBurn-style (see `machine::device`), kept in
//! `<config-dir>/devices/` as `index.json` (`{format, version, active, order}`) plus one `<id>.toml` per device. The
//! built-in device (the engine's own machine profile, the B6 fixture: "ComMarker B6 MOPA 60W (EzCad)") is never
//! stored; it is rebuilt from `--profile` at every start.
//!
//! **First run** (no `index.json`): the devices of LightBurn's `prefs.ini` on this computer are imported (read only,
//! through the typed parser) and LightBurn's default device becomes the active one if OpenEtch can drive it (a JCZ
//! galvo), else the built-in device does. **Imports** (`POST /api/devices/import`, `openetch devices import`) take a
//! `prefs.ini` or a `.lbdev`; a device whose GUID-free identity (display name, controller, connection, size) matches
//! one already imported is updated in place, keeping its id.
//!
//! **Privacy.** Only the typed device fields of [`machine::lightburn`] are ever stored or returned; the correction
//! file is a file name, looked up in the devices directory and then next to the built-in profile.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use corfile::CorTable;
use machine::device::{self, Bases, Device, DeviceKind, DeviceSource, ProfileBase};
use machine::{LightBurnPrefs, MachineProfile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;

/// The registry's directory inside the config directory.
pub const DEVICES_DIR: &str = "devices";
/// The index file.
pub const INDEX_FILE: &str = "index.json";
const INDEX_FORMAT: &str = "openetch-devices";
/// Most devices kept (LightBurn users have a handful).
pub const MAX_DEVICES: usize = 200;

/// `index.json`.
#[derive(Debug, Serialize, Deserialize)]
struct Index {
    format: String,
    version: u32,
    active: String,
    order: Vec<String>,
}

/// Which devices of a file to import (`?add=all|<name>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportFilter {
    All,
    /// One device by LightBurn name or display name (ignoring case).
    Named(String),
}

impl ImportFilter {
    /// `all` (or nothing) or a device name.
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            None | Some("") | Some("all") => ImportFilter::All,
            Some(name) => ImportFilter::Named(name.to_owned()),
        }
    }
}

/// What an import did: `{added, updated, skipped: [{name, reason}]}` (ids for added and updated).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ImportReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

/// The device registry. See the module docs.
#[derive(Debug)]
pub struct DeviceRegistry {
    /// `<config-dir>/devices`; `None` keeps everything in memory (tests, no config directory).
    dir: Option<PathBuf>,
    bases: Bases,
    builtin: Device,
    /// The built-in device's correction table, and the directory of its profile file (correction files of
    /// devices built on it are looked up there too).
    builtin_cor: Option<CorTable>,
    profile_dir: Option<PathBuf>,
    /// Imported devices.
    devices: Vec<Device>,
    /// Every id (the built-in device's too) in list order.
    order: Vec<String>,
    /// The stored active device.
    active: String,
    /// `--device`: the active device for this run only (not stored).
    active_override: Option<String>,
}

impl DeviceRegistry {
    /// A registry in memory holding only the built-in device for `profile`.
    pub fn in_memory(profile: MachineProfile, cor: Option<CorTable>) -> Self {
        let builtin = Device::builtin(profile.clone());
        DeviceRegistry {
            dir: None,
            bases: Bases { builtin: profile, generic: MachineProfile::generic_jcz_galvo() },
            order: vec![builtin.id.clone()],
            active: builtin.id.clone(),
            builtin,
            builtin_cor: cor,
            profile_dir: None,
            devices: Vec::new(),
            active_override: None,
        }
    }

    /// Open the registry in `dir` (`<config-dir>/devices`), with the built-in device for `profile` (read from
    /// `profile_file`, whose directory also holds correction files). Without an index this is the first run:
    /// `first_run` gives LightBurn's devices on this computer, which are imported (see the module docs). Returns the
    /// registry and lines for the startup log; unusable device files are skipped and reported there.
    pub fn open(
        dir: PathBuf,
        profile: MachineProfile,
        cor: Option<CorTable>,
        profile_file: Option<&Path>,
        first_run: impl FnOnce() -> Option<LightBurnPrefs>,
    ) -> Result<(Self, Vec<String>), String> {
        let mut r = Self::in_memory(profile, cor);
        r.profile_dir = profile_file.and_then(Path::parent).map(Path::to_path_buf);
        r.dir = Some(dir.clone());
        let mut notes = Vec::new();
        let index_path = dir.join(INDEX_FILE);
        if !index_path.exists() {
            match first_run() {
                Some(prefs) if !prefs.devices.is_empty() => {
                    let report = r.import(&prefs, DeviceSource::LightburnPrefs, &ImportFilter::All)?;
                    notes.push(format!(
                        "devices: first run, imported {} LightBurn device(s){}",
                        report.added.len(),
                        if report.skipped.is_empty() { String::new() } else { format!(" ({} skipped)", report.skipped.len()) }
                    ));
                    // LightBurn's default device, when OpenEtch can drive it.
                    if let Some(default) = prefs.default_device()
                        && let Some(d) = r.devices.iter().find(|d| d.lightburn.as_ref() == Some(default))
                        && d.kind == DeviceKind::JczGalvo
                        && d.controllable
                    {
                        r.active = d.id.clone();
                    }
                }
                _ => notes.push("devices: first run, no LightBurn devices found".into()),
            }
            r.save_index()?;
            return Ok((r, notes));
        }
        let text = std::fs::read_to_string(&index_path).map_err(|e| format!("cannot read {}: {e}", index_path.display()))?;
        let index: Index = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", index_path.display()))?;
        if index.format != INDEX_FORMAT || index.version != 1 {
            return Err(format!("{} is not an OpenEtch device index (version 1)", index_path.display()));
        }
        let mut order = Vec::new();
        for id in index.order.iter().take(MAX_DEVICES + 1) {
            if *id == r.builtin.id {
                order.push(id.clone());
                continue;
            }
            if !device::is_valid_id(id) || order.contains(id) {
                notes.push(format!("devices: ignored the index entry \"{id}\""));
                continue;
            }
            let path = dir.join(format!("{id}.toml"));
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Device::from_toml(&t)) {
                Ok(d) if d.id == *id => {
                    order.push(id.clone());
                    r.devices.push(d);
                }
                Ok(_) => notes.push(format!("devices: {} names another device; skipped", path.display())),
                Err(e) => notes.push(format!("devices: cannot use {}: {e}", path.display())),
            }
        }
        if !order.contains(&r.builtin.id) {
            order.insert(0, r.builtin.id.clone());
        }
        r.order = order;
        if r.get(&index.active).is_some() {
            r.active = index.active;
        } else {
            notes.push(format!("devices: the active device \"{}\" is gone; using the built-in device", index.active));
        }
        Ok((r, notes))
    }

    /// The bases devices are built on.
    pub fn bases(&self) -> &Bases {
        &self.bases
    }

    /// Every device in list order.
    pub fn list(&self) -> Vec<&Device> {
        self.order.iter().filter_map(|id| self.get(id)).collect()
    }

    pub fn get(&self, id: &str) -> Option<&Device> {
        if id == self.builtin.id { Some(&self.builtin) } else { self.devices.iter().find(|d| d.id == id) }
    }

    /// The device `query` names: an id, else a display name or LightBurn name (ignoring case; the first in list
    /// order).
    pub fn find(&self, query: &str) -> Option<&Device> {
        self.get(query.trim()).or_else(|| self.list().into_iter().find(|d| d.is_named(query)))
    }

    /// The active device (`--device` for this run, else the stored one).
    pub fn active(&self) -> &Device {
        self.active_override
            .as_deref()
            .and_then(|id| self.get(id))
            .or_else(|| self.get(&self.active))
            .unwrap_or(&self.builtin)
    }

    /// `--device <id or name>`: make it active for this run without storing it.
    pub fn override_active(&mut self, query: &str) -> Result<&Device, String> {
        let id = self.find(query).map(|d| d.id.clone()).ok_or_else(|| {
            let names: Vec<String> = self.list().iter().map(|d| format!("\"{}\" ({})", d.display_name, d.id)).collect();
            format!("no device named \"{query}\" (devices: {})", names.join(", "))
        })?;
        self.active_override = Some(id);
        Ok(self.active())
    }

    /// Make `id` the active device and store it.
    pub fn set_active(&mut self, id: &str) -> Result<(), EngineError> {
        if self.get(id).is_none() {
            return Err(EngineError::NotFound(format!("no device with id '{id}'")));
        }
        let before = (self.active.clone(), self.active_override.take());
        self.active = id.to_owned();
        if let Err(e) = self.save_index() {
            (self.active, self.active_override) = before;
            return Err(EngineError::Internal(e));
        }
        Ok(())
    }

    /// Remove an imported device (not the built-in one, not the active one).
    pub fn remove(&mut self, id: &str) -> Result<(), EngineError> {
        if id == self.builtin.id {
            return Err(EngineError::Conflict("the built-in device cannot be removed".into()));
        }
        let Some(i) = self.devices.iter().position(|d| d.id == id) else {
            return Err(EngineError::NotFound(format!("no device with id '{id}'")));
        };
        if self.active().id == id || self.active == id {
            return Err(EngineError::Conflict("the active device cannot be removed; make another one active first".into()));
        }
        let removed = self.devices.remove(i);
        self.order.retain(|o| o != id);
        if let Err(e) = self.save_index() {
            self.devices.insert(i, removed);
            self.order.push(id.to_owned());
            return Err(EngineError::Internal(e));
        }
        if let Some(dir) = &self.dir {
            match std::fs::remove_file(dir.join(format!("{id}.toml"))) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(id, error = %e, "cannot delete the device file"),
            }
        }
        Ok(())
    }

    /// Import the devices of a `prefs.ini` / `.lbdev` (already parsed). A device matching an imported one by
    /// identity is updated (keeping its id); devices without a usable size are skipped with the reason.
    pub fn import(&mut self, prefs: &LightBurnPrefs, source: DeviceSource, filter: &ImportFilter) -> Result<ImportReport, String> {
        let mut report = ImportReport::default();
        let chosen: Vec<_> = match filter {
            ImportFilter::All => prefs.devices.iter().collect(),
            ImportFilter::Named(name) => {
                let d = prefs.devices.iter().find(|d| d.is_named(name)).ok_or_else(|| {
                    let names: Vec<String> = prefs.devices.iter().map(|d| format!("\"{}\"", d.display_name)).collect();
                    format!("the file has no device named \"{name}\" (devices: {})", names.join(", "))
                })?;
                vec![d]
            }
        };
        let mut changed: Vec<Device> = Vec::new();
        for lb in chosen {
            let shown = if lb.display_name.trim().is_empty() { lb.name.clone() } else { lb.display_name.clone() };
            let mut d = match device::from_lightburn(lb, source, &self.bases) {
                Ok(d) => d,
                Err(reason) => {
                    report.skipped.push(Skipped { name: shown, reason });
                    continue;
                }
            };
            let identity = d.identity();
            if let Some(existing) = self.devices.iter_mut().find(|e| e.identity() == identity) {
                d.id = existing.id.clone();
                if *existing != d {
                    *existing = d.clone();
                    report.updated.push(d.id.clone());
                    changed.push(d);
                } else {
                    report.skipped.push(Skipped { name: shown, reason: "already imported, unchanged".into() });
                }
                continue;
            }
            if self.devices.len() >= MAX_DEVICES {
                report.skipped.push(Skipped { name: shown, reason: format!("at most {MAX_DEVICES} devices are kept") });
                continue;
            }
            d.id = self.unique_id(&d.display_name);
            self.order.push(d.id.clone());
            self.devices.push(d.clone());
            report.added.push(d.id.clone());
            changed.push(d);
        }
        for d in &changed {
            self.save_device(d)?;
        }
        if !changed.is_empty() {
            self.save_index()?;
        }
        Ok(report)
    }

    /// A fresh id for `name`.
    fn unique_id(&self, name: &str) -> String {
        let base = device::slug(name);
        let taken = |id: &str| self.get(id).is_some();
        if !taken(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).expect("some suffix is free")
    }

    /// The correction table for `d`, and a warning when its file cannot be used. The built-in device's was loaded
    /// at startup; other devices' files (a name only) are looked up in the devices directory, then next to the
    /// built-in profile.
    pub fn correction_for(&self, d: &Device) -> (Option<CorTable>, Option<String>) {
        if d.id == self.builtin.id {
            return (self.builtin_cor.clone(), None);
        }
        let Some(name) = d.profile.as_ref().and_then(|p| p.cor_file.as_ref()) else { return (None, None) };
        let candidates: Vec<PathBuf> = [self.dir.as_ref(), self.profile_dir.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|dir| name.file_name().map(|n| dir.join(n)))
            .collect();
        for path in &candidates {
            if path.is_file() {
                return match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| CorTable::parse(&b).map_err(|e| e.to_string())) {
                    Ok(t) => (Some(t), None),
                    Err(e) => (None, Some(format!("correction file {}: {e}; running without lens correction", path.display()))),
                };
            }
        }
        let looked: Vec<String> = candidates.iter().filter_map(|p| p.parent()).map(|p| p.display().to_string()).collect();
        (
            None,
            Some(format!(
                "correction file {} not found (looked in {}); copy it there for lens correction, until then output is \
                 uncorrected",
                name.display(),
                if looked.is_empty() { "nowhere: no devices directory".into() } else { looked.join(", ") }
            )),
        )
    }

    /// `GET /api/devices`: `{active, devices: [summary + active]}`.
    pub fn view(&self) -> Value {
        let active = self.active().id.clone();
        let devices: Vec<Value> = self
            .list()
            .into_iter()
            .map(|d| {
                let mut v = d.summary();
                v["active"] = json!(d.id == active);
                v
            })
            .collect();
        json!({ "active": active, "devices": devices })
    }

    /// `GET /api/devices/{id}`: the summary plus `settings` (LightBurn's Device Settings, read-only), `profile`
    /// (a summary of the machine profile, JCZ galvos), `base` and `diffs` (the profile against the base it was built
    /// on).
    pub fn details(&self, id: &str) -> Result<Value, EngineError> {
        let d = self.get(id).ok_or_else(|| EngineError::NotFound(format!("no device with id '{id}'")))?;
        let mut v = d.summary();
        v["active"] = json!(self.active().id == d.id);
        v["settings"] = d.settings_view();
        v["profile"] = d.profile.as_ref().map(profile_summary).unwrap_or(Value::Null);
        v["base"] = match (d.base, &d.profile) {
            (Some(ProfileBase::Builtin), _) => json!(self.bases.builtin.name),
            (Some(ProfileBase::GenericJczGalvo), _) => json!(self.bases.generic.name),
            (None, Some(_)) => json!("built in"),
            (None, None) => Value::Null,
        };
        v["diffs"] = json!(d.diffs(&self.bases));
        Ok(v)
    }

    /// The profile a device's hard limits come from: the base it was built on, else (the built-in device, devices
    /// made by hand or from EzCad) its own profile, so limits can only go down.
    fn limits_for(&self, d: &Device) -> Option<MachineProfile> {
        match d.base {
            Some(b) => Some(self.bases.get(b).clone()),
            None => d.profile.clone(),
        }
    }

    /// The limits a device brought in from a file (a user bundle) must keep: its base's, else the built-in
    /// profile's. Never its own: a file must not raise the limits a device on this machine can have.
    pub(crate) fn import_limits(&self, d: &Device) -> MachineProfile {
        self.bases.get(d.base.unwrap_or(ProfileBase::Builtin)).clone()
    }

    /// Edit a device's profile with a JSON merge patch (`PUT /api/devices/{id}`, see `machine::edit_profile`) and
    /// optionally rename it. The built-in device is read-only (duplicate it first).
    pub fn edit(&mut self, id: &str, patch: Option<&Value>, display_name: Option<&str>) -> Result<Device, EngineError> {
        if id == self.builtin.id {
            return Err(EngineError::Conflict(
                "the built-in device is read-only; duplicate it (POST /api/devices/{id}/duplicate) and edit the copy".into(),
            ));
        }
        let d = self.get(id).cloned().ok_or_else(|| EngineError::NotFound(format!("no device with id '{id}'")))?;
        let mut next = d.clone();
        if let Some(patch) = patch {
            let (Some(profile), Some(limits)) = (&d.profile, self.limits_for(&d)) else {
                return Err(EngineError::BadRequest("this device has no machine profile to edit (design-only)".into()));
            };
            let p = machine::edit_profile(profile, patch, &limits).map_err(EngineError::BadRequest)?;
            if d.kind.is_galvo() {
                next.workspace_mm = [p.field_mm, p.field_height_mm.unwrap_or(p.field_mm)];
            }
            next.profile = Some(p);
        }
        if let Some(name) = display_name {
            let name = name.trim();
            if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
                return Err(EngineError::BadRequest("the device name must be 1-100 printable characters".into()));
            }
            next.display_name = name.to_owned();
        }
        if next == d {
            return Ok(d);
        }
        self.save_device(&next).map_err(EngineError::Internal)?;
        if let Some(slot) = self.devices.iter_mut().find(|e| e.id == id) {
            *slot = next.clone();
        }
        Ok(next)
    }

    /// Add a device made in OpenEtch (a copy, an EzCad import) under a fresh id; returns it.
    pub fn add_manual(&mut self, mut d: Device) -> Result<Device, EngineError> {
        if self.devices.len() >= MAX_DEVICES {
            return Err(EngineError::Conflict(format!("at most {MAX_DEVICES} devices are kept")));
        }
        d.id = self.unique_id(&d.display_name);
        self.save_device(&d).map_err(EngineError::Internal)?;
        self.order.push(d.id.clone());
        self.devices.push(d.clone());
        if let Err(e) = self.save_index() {
            self.devices.pop();
            self.order.pop();
            return Err(EngineError::Internal(e));
        }
        Ok(d)
    }

    /// `POST /api/devices/{id}/duplicate`: an editable copy ("<name> copy", source `manual`). The copy keeps the
    /// original's limits (its base, or the original profile).
    pub fn duplicate(&mut self, id: &str) -> Result<Device, EngineError> {
        let d = self.get(id).cloned().ok_or_else(|| EngineError::NotFound(format!("no device with id '{id}'")))?;
        let limits_base = if d.id == self.builtin.id { Some(machine::ProfileBase::Builtin) } else { d.base };
        let copy = Device {
            display_name: format!("{} copy", d.display_name),
            source: machine::DeviceSource::Manual,
            base: limits_base,
            ..d
        };
        self.add_manual(copy)
    }

    /// `POST /api/devices/import/markcfg`: a device from an EzCad `markcfg7` (latin-1 bytes) named `name`. Its
    /// correction file is looked up by name like any other device's.
    pub fn import_markcfg(&mut self, bytes: &[u8], name: &str) -> Result<Device, EngineError> {
        let profile = MachineProfile::from_markcfg_bytes(bytes).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let name = name.trim();
        let name = if name.is_empty() { "EzCad galvo" } else { name };
        let d = Device {
            id: String::new(),
            name: "JCZFiber".into(),
            display_name: name.into(),
            source: machine::DeviceSource::Markcfg,
            kind: machine::DeviceKind::JczGalvo,
            lightburn_type: "LibUSB".into(),
            workspace_mm: [profile.field_mm, profile.field_mm],
            origin: machine::Origin::Center,
            controllable: true,
            base: None,
            profile: Some(profile),
            grbl: None,
            lightburn: None,
        };
        self.add_manual(d)
    }

    /// Store a lens correction file for device `id` as `<devices dir>/<file name>` (validated first) and point its
    /// profile at it; `None` clears the device's correction file. The built-in device is read-only.
    pub fn set_cor(&mut self, id: &str, file: Option<(&str, &[u8])>) -> Result<Device, EngineError> {
        let patch = match file {
            None => serde_json::json!({ "cor_file": null }),
            Some((name, bytes)) => {
                let name = name.trim();
                let ok_name = !name.is_empty()
                    && name.len() <= 100
                    && name.to_ascii_lowercase().ends_with(".cor")
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || " -_.()".contains(c))
                    && !name.starts_with('.');
                if !ok_name {
                    return Err(EngineError::BadRequest("the correction file name must be a plain name ending in .cor".into()));
                }
                corfile::CorTable::parse(bytes).map_err(|e| EngineError::BadRequest(format!("not a usable correction file: {e}")))?;
                if id == self.builtin.id {
                    return Err(EngineError::Conflict("the built-in device is read-only; duplicate it first".into()));
                }
                let dir = self.dir.as_ref().ok_or_else(|| EngineError::Conflict("no config directory to keep correction files in".into()))?;
                // Files are found by name, so replacing one another device uses would change that device's lens
                // correction behind its back (and a running job's on its next activation).
                let users: Vec<&str> = self
                    .devices
                    .iter()
                    .filter(|d| d.id != id)
                    .filter(|d| {
                        d.profile.as_ref().and_then(|p| p.cor_file.as_ref()).and_then(|f| f.file_name()).is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name))
                    })
                    .map(|d| d.display_name.as_str())
                    .collect();
                let same = [Some(dir), self.profile_dir.as_ref()]
                    .into_iter()
                    .flatten()
                    .map(|d| d.join(name))
                    .find(|p| p.is_file())
                    .is_none_or(|p| std::fs::read(p).is_ok_and(|b| b == bytes));
                if !users.is_empty() && !same {
                    return Err(EngineError::Conflict(format!(
                        "a different correction file named \"{name}\" is used by {}; give this one another name",
                        users.join(", ")
                    )));
                }
                std::fs::create_dir_all(dir).map_err(|e| EngineError::Internal(e.to_string()))?;
                let tmp = dir.join(format!(".{name}.tmp"));
                std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, dir.join(name))).map_err(|e| EngineError::Internal(e.to_string()))?;
                serde_json::json!({ "cor_file": name })
            }
        };
        self.edit(id, Some(&patch), None)
    }

    /// `GET /api/devices/{id}/lbdev`: the device as a LightBurn `.lbdev` and a file name for it.
    pub fn lbdev(&self, id: &str) -> Result<(String, String), EngineError> {
        let d = self.get(id).ok_or_else(|| EngineError::NotFound(format!("no device with id '{id}'")))?;
        let text = machine::to_lbdev(&d.display_name, d.lightburn.as_ref(), d.profile.as_ref());
        Ok((format!("{}.lbdev", d.id), text))
    }

    /// Write `index.json` (when the registry has a directory).
    fn save_index(&self) -> Result<(), String> {
        let Some(dir) = &self.dir else { return Ok(()) };
        let index = Index { format: INDEX_FORMAT.into(), version: 1, active: self.active.clone(), order: self.order.clone() };
        let text = serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?;
        write_atomic(dir, INDEX_FILE, &text)
    }

    fn save_device(&self, d: &Device) -> Result<(), String> {
        let Some(dir) = &self.dir else { return Ok(()) };
        write_atomic(dir, &format!("{}.toml", d.id), &d.to_toml()?)
    }
}

/// The profile fields the Devices window shows.
fn profile_summary(p: &MachineProfile) -> Value {
    json!({
        "name": p.name,
        "laser_source": p.laser_source,
        "field_mm": p.field_mm,
        // A non-square field's height (absent when square), so the device editor shows and keeps it.
        "field_height_mm": p.field_height_mm,
        "swap_xy": p.swap_xy,
        "negate_x": p.negate_x,
        "negate_y": p.negate_y,
        "cor_file": p.cor_file,
        "frequency_khz": [p.frequency_khz_min, p.frequency_khz_max],
        "max_power_pct": p.max_power_pct,
        "mark_speed_mm_s": [p.mark_speed.min_mm_s, p.mark_speed.max_mm_s],
        "max_jump_speed_mm_s": p.max_jump_speed_mm_s,
        "pulse_width_enabled": p.pulse_width_enabled,
        "timing": p.timing,
        // What the device editor shows and edits (`PUT /api/devices/{id}` takes the same shapes): the lens
        // correction (field offset and angle, per-axis scale, bulge, skew, trapezoid), the red-dot alignment (offset,
        // scale, rest position), framing blanking, and the pointer outputs.
        "calibration": p.calibration,
        "red_dot": p.red_dot,
        "blanking": p.blanking,
        "red_light_port_bit": p.red_light_port_bit,
        "red_light_speed_mm_s": p.red_light_speed_mm_s,
        "focus_light_port_bit": p.focus_light_port_bit,
        // Every input and output bit (LightBurn's Galvo IO Ports), so the editor shows what is set: the typed
        // fields as the profile (and `PUT /api/devices/{id}`) spell them, plus one list of each.
        "laser_enable_port_bit": p.laser_enable_port_bit,
        "start_mark_input_bit": p.start_mark_input_bit,
        "start_mark_active_low": p.start_mark_active_low,
        "stop_mark_input_bit": p.stop_mark_input_bit,
        "io": p.io,
        "inputs": io_inputs(p),
        "outputs": io_outputs(p),
    })
}

/// The profile's input bits: `[{role, bit, active_low}]` (roles `start_marking`, `stop_marking`, `door_protect`,
/// `start_framing`; stop marking is active high).
fn io_inputs(p: &MachineProfile) -> Value {
    let mut v = Vec::new();
    if let Some(b) = p.start_mark_input_bit {
        v.push(json!({ "role": "start_marking", "bit": b, "active_low": p.start_mark_active_low }));
    }
    if let Some(b) = p.stop_mark_input_bit {
        v.push(json!({ "role": "stop_marking", "bit": b, "active_low": false }));
    }
    for (role, port) in [("door_protect", p.io.door_protect), ("start_framing", p.io.start_framing)] {
        if let Some(i) = port {
            v.push(json!({ "role": role, "bit": i.bit, "active_low": i.active_low }));
        }
    }
    Value::Array(v)
}

/// The profile's output bits: `[{role, bit}]` (`laser_enable`, `red_light`, `focus_light`, `busy_light`,
/// `ready_light`, `done_marking`).
fn io_outputs(p: &MachineProfile) -> Value {
    let mut v = vec![
        json!({ "role": "laser_enable", "bit": p.laser_enable_port_bit }),
        json!({ "role": "red_light", "bit": p.red_light_port_bit }),
    ];
    for (role, bit) in [
        ("focus_light", p.focus_light_port_bit),
        ("busy_light", p.io.busy_light),
        ("ready_light", p.io.ready_light),
        ("done_marking", p.io.done_marking),
    ] {
        if let Some(b) = bit {
            v.push(json!({ "role": role, "bit": b }));
        }
    }
    Value::Array(v)
}

/// Write `dir/name` through a fresh temporary file renamed into place (creating `dir`).
pub(crate) fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    crate::pin::create_config_dir(dir)?;
    let tmp = dir.join(format!("{name}.tmp"));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("cannot remove {}: {e}", tmp.display())),
    }
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.sync_all()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", tmp.display()));
    }
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot write {}: {e}", dir.join(name).display())
    })
}

/// Load a machine profile TOML and its correction table (next to it). A missing default profile (`is_default`)
/// falls back to the generic LMC fiber profile.
pub fn load_profile(path: &Path, is_default: bool) -> Result<(MachineProfile, Option<CorTable>), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if is_default => {
            tracing::warn!(path = %path.display(), error = %e, "default profile not found; using the generic LMC fiber profile");
            return Ok((MachineProfile::generic_lmc_fiber(), None));
        }
        Err(e) => return Err(format!("cannot read profile {}: {e}", path.display())),
    };
    let profile = MachineProfile::from_toml(&text).map_err(|e| format!("profile {}: {e}", path.display()))?;
    let cor = match profile.cor_path(path) {
        Some(cor_path) => {
            let bytes =
                std::fs::read(&cor_path).map_err(|e| format!("cannot read correction file {}: {e}", cor_path.display()))?;
            Some(CorTable::parse(&bytes).map_err(|e| format!("correction file {}: {e}", cor_path.display()))?)
        }
        None => None,
    };
    Ok((profile, cor))
}

/// The devices of LightBurn's `prefs.ini` at the first of `paths` that exists (the first-run import). Read only;
/// an unreadable file is logged and gives none.
pub fn first_run_prefs(paths: &[PathBuf]) -> Option<LightBurnPrefs> {
    let path = paths.iter().find(|p| p.is_file())?;
    match crate::lightburn::read_prefs_file(path) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read LightBurn's prefs.ini for the first-run device import");
            None
        }
    }
}

/// What `openetch devices` does.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicesAction {
    /// `devices list`.
    List,
    /// `devices import <file> [--add all|<name>]`.
    Import { file: PathBuf, filter: ImportFilter },
}

/// `openetch devices ...`.
#[derive(Debug, Clone, PartialEq)]
pub struct DevicesCommand {
    pub action: DevicesAction,
    pub config_dir: PathBuf,
    /// The built-in device's profile (`--profile`, default the B6 fixture).
    pub profile: PathBuf,
    pub profile_is_default: bool,
    /// Where a first run looks for LightBurn's prefs.ini (the OS locations).
    pub prefs_paths: Vec<PathBuf>,
}

/// One line per device: `* id  display name  kind  type  W x H mm  origin  source` (`*` marks the active one).
pub fn device_table(r: &DeviceRegistry) -> String {
    let active = r.active().id.clone();
    let rows: Vec<[String; 8]> = r
        .list()
        .into_iter()
        .map(|d| {
            [
                if d.id == active { "*".into() } else { " ".into() },
                d.id.clone(),
                d.display_name.clone(),
                d.kind.name().into(),
                d.lightburn_type.clone(),
                format!("{} x {} mm", d.workspace_mm[0], d.workspace_mm[1]),
                d.origin.name().into(),
                if d.controllable { d.source.name().into() } else { format!("{} (design-only)", d.source.name()) },
            ]
        })
        .collect();
    let mut width = [0usize; 8];
    for r in &rows {
        for (w, c) in width.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    rows.iter()
        .map(|r| {
            let cells: Vec<String> = r.iter().zip(width).map(|(c, w)| format!("{c:<w$}")).collect();
            cells.join("  ").trim_end().to_owned() + "\n"
        })
        .collect()
}

/// `openetch devices list|import`: open the registry in `<config-dir>/devices` (a first run imports LightBurn's
/// devices on this computer) and list it, or import a `.lbdev` / `prefs.ini`. Returns the report to print.
pub fn devices_command(cmd: &DevicesCommand) -> Result<String, String> {
    let (profile, cor) = load_profile(&cmd.profile, cmd.profile_is_default)?;
    let dir = cmd.config_dir.join(DEVICES_DIR);
    let (mut r, notes) = DeviceRegistry::open(dir.clone(), profile, cor, Some(&cmd.profile), || first_run_prefs(&cmd.prefs_paths))?;
    let mut out: String = notes.iter().map(|n| format!("{n}\n")).collect();
    match &cmd.action {
        DevicesAction::List => {}
        DevicesAction::Import { file, filter } => {
            let len = std::fs::metadata(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?.len();
            if len > crate::lightburn::MAX_PREFS_UPLOAD_BYTES as u64 {
                return Err(format!("{} is larger than 2 MB", file.display()));
            }
            let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let (prefs, source) = parse_device_file(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            drop(text);
            let report = r.import(&prefs, source, filter)?;
            out.push_str(&format!(
                "imported from {}: {} added, {} updated, {} skipped\n",
                file.display(),
                report.added.len(),
                report.updated.len(),
                report.skipped.len()
            ));
            for s in &report.skipped {
                out.push_str(&format!("  skipped \"{}\": {}\n", s.name, s.reason));
            }
        }
    }
    out.push_str(&format!("devices in {}:\n", dir.display()));
    out.push_str(&device_table(&r));
    Ok(out)
}

/// Parse an uploaded or given device file: a LightBurn `.lbdev` (no `DefaultDevice`) or `prefs.ini`. Returns the
/// devices and the source to record. Errors never quote file content.
pub fn parse_device_file(text: &str) -> Result<(LightBurnPrefs, DeviceSource), String> {
    let prefs = machine::parse_devices(text).map_err(|e| e.to_string())?;
    if prefs.devices.is_empty() {
        return Err("no LightBurn devices in this file (expected a .lbdev file or LightBurn's prefs.ini)".into());
    }
    // A prefs.ini names a default device; a .lbdev never does (and has nothing but DeviceList).
    let source = if prefs.default_index.is_some() || !is_bare_device_list(text) {
        DeviceSource::LightburnPrefs
    } else {
        DeviceSource::Lbdev
    };
    let prefs = if source == DeviceSource::Lbdev { machine::parse_lbdev(text).map_err(|e| e.to_string())? } else { prefs };
    Ok((prefs, source))
}

/// Whether the JSON's only top-level key is `DeviceList` (a `.lbdev`). Reads the keys only.
fn is_bare_device_list(text: &str) -> bool {
    #[derive(Deserialize)]
    struct Keys(std::collections::BTreeMap<String, serde::de::IgnoredAny>);
    serde_json::from_str::<Keys>(text.trim_start_matches('\u{feff}'))
        .is_ok_and(|Keys(k)| k.len() == 1 && k.contains_key("DeviceList"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::temp_dir;

    const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
    const PREFS: &str = include_str!("../../machine/tests/fixtures/lightburn-prefs.json");
    const LBDEV: &str = include_str!("../../machine/tests/fixtures/gantry.lbdev");

    fn b6() -> MachineProfile {
        MachineProfile::from_toml(B6).unwrap()
    }

    fn fixtures_profile() -> PathBuf {
        crate::cli::resolve_default(crate::cli::DEFAULT_PROFILE)
    }

    fn open(dir: &Path, prefs: Option<&str>) -> (DeviceRegistry, Vec<String>) {
        let prefs = prefs.map(|p| machine::parse_devices(p).unwrap());
        DeviceRegistry::open(dir.to_path_buf(), b6(), None, Some(&fixtures_profile()), || prefs).unwrap()
    }

    #[test]
    fn first_run_imports_lightburn_and_activates_its_default_galvo() {
        let dir = temp_dir("devices-first");
        let (r, notes) = open(&dir, Some(PREFS));
        assert!(notes[0].contains("imported 2 LightBurn device(s)"), "{notes:?}");
        let ids: Vec<&str> = r.list().iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["commarker-b6-mopa-60w-ezcad", "jczfiber-unknown", "bench-110-lens"]);
        assert_eq!(r.active().id, "jczfiber-unknown", "LightBurn's default device is a JCZ galvo");
        assert!(dir.join(INDEX_FILE).is_file() && dir.join("jczfiber-unknown.toml").is_file());
        // Stored files hold nothing but the typed fields.
        for f in ["jczfiber-unknown.toml", "bench-110-lens.toml", INDEX_FILE] {
            let text = std::fs::read_to_string(dir.join(f)).unwrap().to_lowercase();
            for leaked in ["secret", "license", "guid", "someone", "documents", "doorprotect", "apptheme"] {
                assert!(!text.contains(leaked), "{leaked} in {f}");
            }
        }
        // A restart reads the registry back, without importing again.
        let (again, notes) = open(&dir, None);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(again.active().id, "jczfiber-unknown");
        assert_eq!(again.list().len(), 3);
        assert_eq!(again.get("jczfiber-unknown"), r.get("jczfiber-unknown"));
        // The B6-like device finds its correction file next to the built-in profile.
        let (cor, warning) = again.correction_for(again.active());
        assert!(cor.is_some() && warning.is_none(), "{warning:?}");
    }

    #[test]
    fn first_run_without_a_drivable_default_keeps_the_builtin_device() {
        let dir = temp_dir("devices-nodefault");
        let prefs = PREFS.replace("\"DefaultDevice\": 0", "\"DefaultDevice\": 1");
        let (r, _) = open(&dir, Some(&prefs));
        assert_eq!(r.active().id, "commarker-b6-mopa-60w-ezcad", "Bench110 is not a JCZ galvo");
        let dir = temp_dir("devices-none");
        let (r, notes) = open(&dir, None);
        assert!(notes[0].contains("no LightBurn devices"), "{notes:?}");
        assert_eq!((r.list().len(), r.active().id.as_str()), (1, "commarker-b6-mopa-60w-ezcad"));
        assert!(dir.join(INDEX_FILE).is_file(), "the first run is over");
    }

    #[test]
    fn imports_update_by_identity_and_skip_with_reasons() {
        let dir = temp_dir("devices-import");
        let (mut r, _) = open(&dir, None);
        let (lbdev, source) = parse_device_file(LBDEV).unwrap();
        assert_eq!(source, DeviceSource::Lbdev);
        let report = r.import(&lbdev, source, &ImportFilter::All).unwrap();
        assert_eq!(report.added, ["test-bed-400"]);
        let bed = r.get("test-bed-400").unwrap();
        assert_eq!((bed.kind, bed.controllable, bed.source), (DeviceKind::Gantry, false, DeviceSource::Lbdev));
        // The same file again: unchanged, skipped.
        let report = r.import(&lbdev, source, &ImportFilter::All).unwrap();
        assert!(report.added.is_empty() && report.updated.is_empty());
        assert_eq!(report.skipped[0].reason, "already imported, unchanged");
        // Changed settings: updated in place, same id.
        let (changed, _) = parse_device_file(&LBDEV.replace("\"BaudRate\": 115200", "\"BaudRate\": 9600")).unwrap();
        let report = r.import(&changed, source, &ImportFilter::All).unwrap();
        assert_eq!(report.updated, ["test-bed-400"]);
        assert_eq!(r.get("test-bed-400").unwrap().lightburn.as_ref().unwrap().settings.baud_rate, Some(9600));
        // Another size is another device (a second lens); the id gets a suffix.
        let (bigger, _) = parse_device_file(&LBDEV.replace("\"Width\": 400", "\"Width\": 600")).unwrap();
        assert_eq!(r.import(&bigger, source, &ImportFilter::All).unwrap().added, ["test-bed-400-2"]);
        // A device without a size is skipped with the reason; a name filter picks one device.
        let (prefs, source) = parse_device_file(&PREFS.replace("\"Width\": 110,", "")).unwrap();
        assert_eq!(source, DeviceSource::LightburnPrefs);
        let report = r.import(&prefs, source, &ImportFilter::Named("jczfiber".into())).unwrap();
        assert_eq!(report.added, ["jczfiber-unknown"]);
        let report = r.import(&prefs, source, &ImportFilter::All).unwrap();
        assert_eq!(report.skipped.len(), 2, "{report:?}");
        assert!(report.skipped.iter().any(|s| s.name == "Bench 110 lens" && s.reason.contains("field size")));
        assert!(r.import(&prefs, source, &ImportFilter::Named("nope".into())).unwrap_err().contains("JCZFiber (Unknown)"));
        // Stored: a restart sees the same devices.
        let (again, _) = open(&dir, None);
        assert_eq!(again.list().len(), r.list().len());
        // Refused files.
        assert!(parse_device_file("{}").is_err());
        assert!(parse_device_file("not json secret-zz").unwrap_err().find("secret").is_none());
    }

    #[test]
    fn activation_and_removal_rules() {
        let dir = temp_dir("devices-active");
        let (mut r, _) = open(&dir, Some(PREFS));
        let builtin = "commarker-b6-mopa-60w-ezcad";
        assert!(matches!(r.remove(builtin), Err(EngineError::Conflict(_))));
        assert!(matches!(r.remove("jczfiber-unknown"), Err(EngineError::Conflict(_))), "active");
        assert!(matches!(r.remove("nope"), Err(EngineError::NotFound(_))));
        assert!(matches!(r.set_active("nope"), Err(EngineError::NotFound(_))));
        r.set_active(builtin).unwrap();
        r.remove("jczfiber-unknown").unwrap();
        assert!(!dir.join("jczfiber-unknown.toml").exists());
        let (again, _) = open(&dir, None);
        assert_eq!((again.active().id.as_str(), again.list().len()), (builtin, 2));
        // --device overrides for this run only.
        let mut again = again;
        assert_eq!(again.override_active("Bench 110 LENS").unwrap().id, "bench-110-lens");
        assert!(again.override_active("missing").unwrap_err().contains("ComMarker"));
        let (third, _) = open(&dir, None);
        assert_eq!(third.active().id, builtin, "not stored");
        // Lookups.
        assert_eq!(third.find("bench110").unwrap().id, "bench-110-lens", "by LightBurn name");
        let v = third.view();
        assert_eq!(v["active"], builtin);
        assert_eq!(v["devices"][0]["active"], true);
        let details = third.details("bench-110-lens").unwrap();
        assert_eq!((details["kind"].clone(), details["controllable"].clone()), (json!("galvo_other"), json!(false)));
        assert!(details["reason"].as_str().unwrap().contains("design-only"));
        assert!(details["settings"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn the_devices_command_lists_and_imports() {
        let cfg = temp_dir("devices-cli");
        let lbdev = cfg.join("bed.lbdev");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(&lbdev, LBDEV).unwrap();
        let cmd = |action| DevicesCommand { action, config_dir: cfg.clone(), profile: fixtures_profile(), profile_is_default: false, prefs_paths: vec![] };
        let out = devices_command(&cmd(DevicesAction::Import { file: lbdev.clone(), filter: ImportFilter::All })).unwrap();
        assert!(out.contains("1 added") && out.contains("test-bed-400") && out.contains("design-only"), "{out}");
        let out = devices_command(&cmd(DevicesAction::List)).unwrap();
        let active = out.lines().find(|l| l.starts_with('*')).unwrap();
        assert!(active.contains("commarker-b6-mopa-60w-ezcad") && active.contains("153.75 x 153.75 mm"), "{out}");
        assert!(out.lines().any(|l| l.contains("400 x 300 mm") && l.contains("top_left")), "{out}");
        let e = devices_command(&cmd(DevicesAction::Import { file: cfg.join("none.lbdev"), filter: ImportFilter::All })).unwrap_err();
        assert!(e.contains("cannot read"), "{e}");
        let _ = std::fs::remove_dir_all(&cfg);
    }

    #[test]
    fn broken_files_are_skipped_and_reported() {
        let dir = temp_dir("devices-broken");
        open(&dir, Some(PREFS));
        std::fs::write(dir.join("bench-110-lens.toml"), "format = \"nope\"").unwrap();
        let (r, notes) = open(&dir, None);
        assert_eq!(r.list().len(), 2);
        assert!(notes.iter().any(|n| n.contains("bench-110-lens.toml")), "{notes:?}");
        std::fs::write(dir.join(INDEX_FILE), "{\"format\": \"other\", \"version\": 1, \"active\": \"x\", \"order\": []}").unwrap();
        assert!(DeviceRegistry::open(dir.clone(), b6(), None, None, || None).is_err());
    }

    #[test]
    fn correction_files_are_looked_up_by_name() {
        let r = DeviceRegistry::in_memory(b6(), None);
        let prefs = machine::parse_devices(PREFS).unwrap();
        let d = device::from_lightburn(&prefs.devices[0], DeviceSource::LightburnPrefs, r.bases()).unwrap();
        let (cor, warning) = r.correction_for(&d);
        assert!(cor.is_none() && warning.unwrap().contains("jcz150.cor not found"));
    }

    #[test]
    fn devices_are_edited_duplicated_exported_and_imported_from_ezcad() {
        let dir = crate::pin::temp_dir("devedit");
        let (mut r, _) = open(&dir, None);
        let builtin = r.active().id.clone();
        assert!(matches!(r.edit(&builtin, Some(&serde_json::json!({"max_power_pct": 50})), None), Err(EngineError::Conflict(_))));
        let copy = r.duplicate(&builtin).unwrap();
        assert!(copy.display_name.ends_with("copy") && copy.source == machine::DeviceSource::Manual);
        let edited = r.edit(&copy.id, Some(&serde_json::json!({"calibration": {"angle_deg": 0.5}, "max_power_pct": 60})), Some("Mine")).unwrap();
        let p = edited.profile.as_ref().unwrap();
        assert_eq!((p.calibration.angle_deg, p.max_power_pct, edited.display_name.as_str()), (0.5, 60.0, "Mine"));
        assert!(r.edit(&copy.id, Some(&serde_json::json!({"max_power_pct": 101})), None).is_err());
        // Stored: a reopened registry has the edit.
        let (again, _) = open(&dir, None);
        assert_eq!(again.get(&copy.id).unwrap().profile.as_ref().unwrap().calibration.angle_deg, 0.5);
        // The details carry what the device editor edits: calibration, red dot, blanking, pointer outputs.
        r.edit(&copy.id, Some(&serde_json::json!({"red_dot": {"offset_x_mm": 0.25, "rest_y_mm": -3.0}, "blanking": false})), None)
            .unwrap();
        let p = &r.details(&copy.id).unwrap()["profile"];
        assert_eq!((p["calibration"]["angle_deg"].clone(), p["calibration"]["scale_x"].clone()), (json!(0.5), json!(1.0)));
        assert_eq!((p["red_dot"]["offset_x_mm"].clone(), p["red_dot"]["rest_y_mm"].clone()), (json!(0.25), json!(-3.0)));
        assert_eq!(p["red_dot"]["scale_y"], json!(1.0));
        assert_eq!((p["blanking"].clone(), p["focus_light_port_bit"].clone()), (json!(false), Value::Null));
        assert!(p["red_light_port_bit"].is_u64() && p["red_light_speed_mm_s"].is_number());
        // A field height from the device editor gives a non-square workspace and comes back in the details; null
        // makes the field square again.
        assert_eq!(p["field_height_mm"], Value::Null);
        let field = r.details(&copy.id).unwrap()["profile"]["field_mm"].as_f64().unwrap();
        let tall = r.edit(&copy.id, Some(&serde_json::json!({"field_height_mm": 100.0})), None).unwrap();
        assert_eq!(tall.workspace_mm, [field, 100.0]);
        assert_eq!(r.details(&copy.id).unwrap()["profile"]["field_height_mm"], json!(100.0));
        let square = r.edit(&copy.id, Some(&serde_json::json!({"field_height_mm": null})), None).unwrap();
        assert_eq!(square.workspace_mm, [field, field]);
        let (name, text) = r.lbdev(&copy.id).unwrap();
        assert!(name.ends_with(".lbdev") && text.contains("\"FieldAngle\": 0.5"), "{text}");
        let markcfg = include_bytes!("../../../fixtures/markcfg7.ini");
        let ez = r.import_markcfg(markcfg, "Shop B6").unwrap();
        assert_eq!((ez.source, ez.controllable, ez.workspace_mm[0]), (machine::DeviceSource::Markcfg, true, 153.75));
        assert!(r.import_markcfg(b"nothing here", "x").is_err());
        let cor = include_bytes!("../../../fixtures/jcz150.cor");
        let with_cor = r.set_cor(&ez.id, Some(("lens 150.cor", cor))).unwrap();
        assert_eq!(with_cor.profile.as_ref().unwrap().cor_file.as_deref(), Some(std::path::Path::new("lens 150.cor")));
        assert!(dir.join(DEVICES_DIR).join("lens 150.cor").is_file() || dir.join("lens 150.cor").is_file());
        assert!(r.correction_for(&with_cor).0.is_some(), "found by name");
        assert!(r.set_cor(&ez.id, Some(("../x.cor", cor))).is_err());
        assert!(r.set_cor(&ez.id, Some(("bad.cor", b"junk"))).is_err());
        // Another device's file of the same name is not replaced by different content (the same content is fine).
        let other = r.duplicate(&ez.id).unwrap();
        assert_eq!(other.profile.as_ref().unwrap().cor_file.as_deref(), Some(std::path::Path::new("lens 150.cor")));
        let mut changed = cor.to_vec();
        let last = changed.len() - 1;
        changed[last] ^= 1;
        assert!(corfile::CorTable::parse(&changed).is_ok());
        assert!(matches!(r.set_cor(&other.id, Some(("LENS 150.cor", &changed))), Err(EngineError::Conflict(_))));
        assert!(r.set_cor(&other.id, Some(("lens 150.cor", cor))).is_ok());
        let cleared = r.set_cor(&ez.id, None).unwrap();
        assert!(cleared.profile.unwrap().cor_file.is_none());
    }
}
