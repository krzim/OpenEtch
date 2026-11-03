//! Devices (moved from the engine's `devices` module, which keeps its registry on disk): the JSON of
//! `GET /api/devices` and `GET /api/devices/{id}`, LightBurn device file parsing and the import of its devices, and
//! [`DesignDevices`], the in-memory device list of the browser build (built-in design profiles plus devices imported
//! from a LightBurn `.lbdev` or `prefs.ini`, typed fields only).
//!
//! **Privacy.** Only the typed device fields of `machine::lightburn` are ever stored or returned.

use machine::device::{self, Bases, Device, DeviceSource, ProfileBase};
use machine::{LightBurnPrefs, MachineProfile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::AppError;
use crate::stores::StoreWrite;

/// Most devices kept (LightBurn users have a handful).
pub const MAX_DEVICES: usize = 200;

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

/// Parse an uploaded or given device file: a LightBurn `.lbdev` (no `DefaultDevice`) or `prefs.ini`. Returns the
/// devices and the source to record. Errors never quote file content.
pub fn parse_device_file(text: &str) -> Result<(LightBurnPrefs, DeviceSource), String> {
    let prefs = machine::parse_devices(text).map_err(|e| e.to_string())?;
    if prefs.devices.is_empty() {
        return Err("no LightBurn devices in this file (expected a .lbdev file or LightBurn's prefs.ini)".into());
    }
    // A prefs.ini names a default device; a .lbdev never does (and has nothing but DeviceList).
    let source = if prefs.default_index.is_some() || !is_bare_device_list(text) { DeviceSource::LightburnPrefs } else { DeviceSource::Lbdev };
    let prefs = if source == DeviceSource::Lbdev { machine::parse_lbdev(text).map_err(|e| e.to_string())? } else { prefs };
    Ok((prefs, source))
}

/// Whether the JSON's only top-level key is `DeviceList` (a `.lbdev`). Reads the keys only.
fn is_bare_device_list(text: &str) -> bool {
    #[derive(Deserialize)]
    struct Keys(std::collections::BTreeMap<String, serde::de::IgnoredAny>);
    serde_json::from_str::<Keys>(text.trim_start_matches('\u{feff}')).is_ok_and(|Keys(k)| k.len() == 1 && k.contains_key("DeviceList"))
}

/// The profile fields the Devices window shows.
pub fn profile_summary(p: &MachineProfile) -> Value {
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
    let mut v = vec![json!({ "role": "laser_enable", "bit": p.laser_enable_port_bit }), json!({ "role": "red_light", "bit": p.red_light_port_bit })];
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

/// `GET /api/devices`: `{active, devices: [summary + active]}`.
pub fn view<'a>(devices: impl IntoIterator<Item = &'a Device>, active: &str) -> Value {
    let devices: Vec<Value> = devices
        .into_iter()
        .map(|d| {
            let mut v = d.summary();
            v["active"] = json!(d.id == active);
            v
        })
        .collect();
    json!({ "active": active, "devices": devices })
}

/// `GET /api/devices/{id}`: the summary plus `settings` (LightBurn's Device Settings, read-only), `profile` (a
/// summary of the machine profile, JCZ galvos), `base` and `diffs` (the profile against the base it was built on).
pub fn details(d: &Device, active: &str, bases: &Bases) -> Value {
    let mut v = d.summary();
    v["active"] = json!(active == d.id);
    v["settings"] = d.settings_view();
    v["profile"] = d.profile.as_ref().map(profile_summary).unwrap_or(Value::Null);
    v["grbl"] = json!(d.grbl);
    v["ruida"] = json!(d.ruida);
    v["base"] = match (d.base, &d.profile) {
        (Some(ProfileBase::Builtin), _) => json!(bases.builtin.name),
        (Some(ProfileBase::GenericJczGalvo), _) => json!(bases.generic.name),
        (None, Some(_)) => json!("built in"),
        (None, None) => Value::Null,
    };
    v["diffs"] = json!(d.diffs(bases));
    v
}

/// A fresh id for `name` among the ids `taken` says are used.
pub fn unique_id(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = device::slug(name);
    if !taken(&base) {
        return base;
    }
    (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).expect("some suffix is free")
}

/// Import the devices of a `prefs.ini` / `.lbdev` (already parsed) into `devices` (the imported ones; `order` lists
/// every id; `builtin` is the built-in device's id). A device matching an imported one by identity is updated
/// (keeping its id); devices without a usable size are skipped with the reason. Returns the report and the devices
/// added or changed.
pub fn import_into(
    devices: &mut Vec<Device>,
    order: &mut Vec<String>,
    builtin: &[&str],
    prefs: &LightBurnPrefs,
    source: DeviceSource,
    filter: &ImportFilter,
    bases: &Bases,
) -> Result<(ImportReport, Vec<Device>), String> {
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
        let mut d = match device::from_lightburn(lb, source, bases) {
            Ok(d) => d,
            Err(reason) => {
                report.skipped.push(Skipped { name: shown, reason });
                continue;
            }
        };
        let identity = d.identity();
        if let Some(existing) = devices.iter_mut().find(|e| e.identity() == identity) {
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
        if devices.len() >= MAX_DEVICES {
            report.skipped.push(Skipped { name: shown, reason: format!("at most {MAX_DEVICES} devices are kept") });
            continue;
        }
        d.id = unique_id(&d.display_name, |id| builtin.contains(&id) || devices.iter().any(|e| e.id == id));
        order.push(d.id.clone());
        devices.push(d.clone());
        report.added.push(d.id.clone());
        changed.push(d);
    }
    Ok((report, changed))
}

/// The store the browser build keeps its devices in: one key per imported device (its TOML) and `index` (`{active,
/// order}`).
pub const STORE: &str = "devices";

/// The design devices of the browser build: built-in design profiles (the first is the built-in device that
/// LightBurn imports are based on) plus imported LightBurn devices, one of them active. Designs are planned against
/// the active one; nothing here drives a machine.
#[derive(Debug, Clone)]
pub struct DesignDevices {
    bases: Bases,
    builtins: Vec<Device>,
    devices: Vec<Device>,
    order: Vec<String>,
    active: String,
}

/// The stored index of [`DesignDevices`].
#[derive(Debug, Serialize, Deserialize)]
struct Index {
    active: String,
    order: Vec<String>,
}

fn no_device(id: &str) -> AppError {
    AppError::NotFound(format!("no device with id '{id}'"))
}

impl DesignDevices {
    /// The built-in devices for `profiles` (the first is the built-in machine: the base of LightBurn imports and the
    /// active device until another is chosen).
    pub fn new(profiles: Vec<MachineProfile>) -> Self {
        let builtins: Vec<Device> = profiles.into_iter().map(Device::builtin).collect();
        let first = builtins.first().expect("at least one built-in profile");
        let bases = Bases { builtin: first.profile.clone().expect("a built-in device has a profile"), generic: MachineProfile::generic_jcz_galvo() };
        let order = builtins.iter().map(|d| d.id.clone()).collect();
        let active = first.id.clone();
        DesignDevices { bases, builtins, devices: Vec::new(), order, active }
    }

    /// Take a stored record (`key`, `value` from [`STORE`]); unusable ones are skipped with a note.
    pub fn restore(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key == "index" {
            let index: Index = serde_json::from_str(value).map_err(|e| format!("devices index: {e}"))?;
            self.order = index.order;
            for b in &self.builtins {
                if !self.order.contains(&b.id) {
                    self.order.push(b.id.clone());
                }
            }
            self.active = index.active;
            return Ok(());
        }
        let d = Device::from_toml(value)?;
        if d.id != key || !device::is_valid_id(key) {
            return Err(format!("device record {key} names another device"));
        }
        self.devices.retain(|e| e.id != key);
        self.devices.push(d);
        Ok(())
    }

    /// Drop an imported device whose record was deleted (built-in ones stay).
    pub fn forget(&mut self, id: &str) {
        if self.builtins.iter().any(|b| b.id == id) {
            return;
        }
        self.devices.retain(|d| d.id != id);
        self.order.retain(|o| o != id);
    }

    pub fn bases(&self) -> &Bases {
        &self.bases
    }

    pub fn get(&self, id: &str) -> Option<&Device> {
        self.builtins.iter().chain(&self.devices).find(|d| d.id == id)
    }

    /// Every device in list order (an order entry without a device is skipped).
    pub fn list(&self) -> Vec<&Device> {
        self.order.iter().filter_map(|id| self.get(id)).collect()
    }

    /// The device `query` names: an id, else a display name or LightBurn name (ignoring case).
    pub fn find(&self, query: &str) -> Option<&Device> {
        self.get(query.trim()).or_else(|| self.list().into_iter().find(|d| d.is_named(query)))
    }

    /// The active device (the first built-in one if the stored one is gone).
    pub fn active(&self) -> &Device {
        self.get(&self.active).unwrap_or(&self.builtins[0])
    }

    pub fn view(&self) -> Value {
        view(self.list(), &self.active().id)
    }

    pub fn details(&self, id: &str) -> Result<Value, AppError> {
        let d = self.get(id).ok_or_else(|| no_device(id))?;
        Ok(details(d, &self.active().id, &self.bases))
    }

    fn index_write(&self) -> StoreWrite {
        let index = Index { active: self.active.clone(), order: self.order.clone() };
        StoreWrite::json(STORE, "index", &index)
    }

    /// Make `id` active.
    pub fn set_active(&mut self, id: &str) -> Result<Vec<StoreWrite>, AppError> {
        if self.get(id).is_none() {
            return Err(no_device(id));
        }
        self.active = id.to_owned();
        Ok(vec![self.index_write()])
    }

    /// Remove an imported device (not a built-in one, not the active one).
    pub fn remove(&mut self, id: &str) -> Result<Vec<StoreWrite>, AppError> {
        if self.builtins.iter().any(|b| b.id == id) {
            return Err(AppError::Conflict("the built-in device cannot be removed".into()));
        }
        let Some(i) = self.devices.iter().position(|d| d.id == id) else { return Err(no_device(id)) };
        if self.active().id == id {
            return Err(AppError::Conflict("the active device cannot be removed; make another one active first".into()));
        }
        self.devices.remove(i);
        self.order.retain(|o| o != id);
        Ok(vec![StoreWrite::delete(STORE, id), self.index_write()])
    }

    /// Import the devices of a `.lbdev` / `prefs.ini` (text). 404 when the file has no such device.
    pub fn import(&mut self, text: &str, filter: &ImportFilter) -> Result<(ImportReport, Vec<StoreWrite>), AppError> {
        let (prefs, source) = parse_device_file(text).map_err(AppError::BadRequest)?;
        let builtin: Vec<&str> = self.builtins.iter().map(|d| d.id.as_str()).collect();
        let (report, changed) = import_into(&mut self.devices, &mut self.order, &builtin, &prefs, source, filter, &self.bases)
            .map_err(|e| if e.starts_with("the file has no device") { AppError::NotFound(e) } else { AppError::Internal(e) })?;
        let mut writes = Vec::new();
        for d in &changed {
            let text = d.to_toml().map_err(AppError::Internal)?;
            writes.push(StoreWrite::text(STORE, &d.id, text));
        }
        if !changed.is_empty() {
            writes.push(self.index_write());
        }
        Ok((report, writes))
    }

    /// `GET /api/devices/{id}/lbdev`: the device as a LightBurn `.lbdev` and a file name for it.
    pub fn lbdev(&self, id: &str) -> Result<(String, String), AppError> {
        let d = self.get(id).ok_or_else(|| no_device(id))?;
        let text = machine::to_lbdev(&d.display_name, d.lightburn.as_ref(), d.profile.as_ref());
        Ok((format!("{}.lbdev", d.id), text))
    }
}
