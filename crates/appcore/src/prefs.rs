//! The engine's preferences (`preferences.json` on the engine, the `prefs` store in the browser; moved from the
//! engine's `prefs` module, which keeps the file): per device, LightBurn's layer defaults (Make Default / Reset to
//! Default / Make Default for All, per colour; "Load default layer settings on New"), the optimization defaults (Set
//! as Defaults), Galvo Basic Settings' Other Options (Require Framing Before Start, Play Sound on Job Finished, the
//! Job Checklist), the device's material library and a GRBL gantry's console macros; globally the debug log, the
//! opt-in update check and the User Tools.

use std::collections::BTreeMap;

use doc::{CutSettings, Optimization};
use serde::{Deserialize, Serialize};

/// Key of the "Make Default for All" layer default.
pub const ALL_COLORS: &str = "*";
/// Most checklist items and the longest item.
pub const MAX_CHECKLIST_ITEMS: usize = 30;
pub const MAX_CHECKLIST_ITEM_CHARS: usize = 300;
/// Most console macros per device, the longest name and the most lines of one macro.
pub const MAX_MACROS: usize = 32;
pub const MAX_MACRO_NAME_CHARS: usize = 40;
pub const MAX_MACRO_LINES: usize = 50;

/// A saved layer default: main settings and sub-layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerDefault {
    pub settings: CutSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_layers: Vec<CutSettings>,
}

/// LightBurn's Job Checklist: shown before every run; the operator must tick it before approval.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checklist {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub items: Vec<String>,
}

impl Checklist {
    pub fn problem(&self) -> Option<String> {
        if self.items.len() > MAX_CHECKLIST_ITEMS {
            return Some(format!("at most {MAX_CHECKLIST_ITEMS} checklist items"));
        }
        if let Some(i) = self.items.iter().find(|i| i.trim().is_empty() || i.chars().count() > MAX_CHECKLIST_ITEM_CHARS) {
            return Some(format!("checklist item \"{i}\" must be 1..={MAX_CHECKLIST_ITEM_CHARS} characters"));
        }
        if self.enabled && self.items.is_empty() {
            return Some("an enabled checklist needs at least one item".into());
        }
        None
    }
}

/// A console macro of a GRBL gantry (LightBurn's Console macros): a named list of console lines, sent one after
/// another through `POST /api/machine/console` (so through the same console policy: nothing that can turn the laser
/// on). Saving checks every line against that policy too, so a macro that could never run is refused when saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleMacro {
    pub name: String,
    pub lines: Vec<String>,
}

/// Why `macros` cannot be saved, if they cannot.
pub fn macros_problem(macros: &[ConsoleMacro]) -> Option<String> {
    if macros.len() > MAX_MACROS {
        return Some(format!("at most {MAX_MACROS} macros"));
    }
    for m in macros {
        let n = m.name.trim().chars().count();
        if n == 0 || n > MAX_MACRO_NAME_CHARS || m.name.chars().any(char::is_control) {
            return Some(format!("macro name \"{}\" must be 1..={MAX_MACRO_NAME_CHARS} printable characters", m.name));
        }
        if m.lines.is_empty() || m.lines.len() > MAX_MACRO_LINES {
            return Some(format!("macro \"{}\" must have 1..={MAX_MACRO_LINES} lines", m.name));
        }
        for l in &m.lines {
            if let Err(e) = machine::grbl::check_console_line(l) {
                return Some(format!("macro \"{}\", line \"{l}\": {e}", m.name));
            }
        }
    }
    None
}

/// A named head position of a GRBL gantry's Move window (LightBurn's saved positions), in machine mm from the origin
/// corner. Going there is an ordinary jog (`POST /api/machine/jog`, laser off, UI only); saving one is `PUT /api/prefs`
/// `move_positions` (UI only, never MCP).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovePosition {
    pub name: String,
    pub x_mm: f64,
    pub y_mm: f64,
}

/// Most saved Move positions per device.
pub const MAX_MOVE_POSITIONS: usize = 32;

/// Why `positions` cannot be saved for a bed of `work_area_mm`, if they cannot: names 1..=40 printable characters,
/// at most [`MAX_MOVE_POSITIONS`], every position on the bed.
pub fn move_positions_problem(positions: &[MovePosition], work_area_mm: [f64; 2]) -> Option<String> {
    if positions.len() > MAX_MOVE_POSITIONS {
        return Some(format!("at most {MAX_MOVE_POSITIONS} saved positions"));
    }
    for m in positions {
        let n = m.name.trim().chars().count();
        if n == 0 || n > MAX_MACRO_NAME_CHARS || m.name.chars().any(char::is_control) {
            return Some(format!("position name \"{}\" must be 1..={MAX_MACRO_NAME_CHARS} printable characters", m.name));
        }
        let on = |v: f64, max: f64| v.is_finite() && (0.0..=max).contains(&v);
        if !(on(m.x_mm, work_area_mm[0]) && on(m.y_mm, work_area_mm[1])) {
            return Some(format!(
                "position \"{}\" ({}, {}) mm is off the {} x {} mm bed",
                m.name, m.x_mm, m.y_mm, work_area_mm[0], work_area_mm[1]
            ));
        }
    }
    None
}

/// Per-device preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicePrefs {
    /// Layer defaults by display colour (lower-case `#rrggbb`), and [`ALL_COLORS`] for Make Default for All.
    pub layer_defaults: BTreeMap<String, LayerDefault>,
    /// LightBurn's "Load default layer settings on New / restart".
    pub load_defaults_on_new: bool,
    /// Optimization settings new documents start with (Set as Defaults); `None`: OpenEtch's defaults.
    pub optimization_defaults: Option<Optimization>,
    /// Require Framing Before Start: a run is approved only after a frame since the last edit.
    pub require_framing: bool,
    /// Play Sound on Job Finished: the engine sends `machine` event `job_finished_sound`; the UI plays it.
    pub play_sound: bool,
    pub checklist: Checklist,
    /// The material library loaded and selected when this device is active (LightBurn links a saved library to the
    /// active device).
    pub library: Option<String>,
    /// A GRBL gantry's console macros ([`ConsoleMacro`]), in button order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub console_macros: Vec<ConsoleMacro>,
    /// A GRBL gantry's saved Move positions ([`MovePosition`]), in button order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub move_positions: Vec<MovePosition>,
    /// Keep-out zones of the device (a fixture, a clamp, field mm): a run marking inside one is refused (`zones`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keep_out: Vec<crate::zones::DeviceZone>,
}

impl DevicePrefs {
    /// The default for a layer of `color`: its own, else Make Default for All, else none.
    pub fn layer_default(&self, color: &str) -> Option<&LayerDefault> {
        self.layer_defaults.get(&color.to_ascii_lowercase()).or_else(|| self.layer_defaults.get(ALL_COLORS))
    }
}

/// Everything in `preferences.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub format: String,
    pub devices: BTreeMap<String, DevicePrefs>,
    /// Keep debug-level engine logs in the diagnostics log (LightBurn's Enable Debug Log).
    pub debug_log: bool,
    /// Check GitHub for a newer release when asked or at start (opt-in; nothing is sent but the request itself).
    pub check_for_updates: bool,
    /// The User Tools menu (`crate::user_tools`): web pages and document command sequences, never machine actions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user_tools: Vec<crate::user_tools::UserTool>,
}

/// The colour of a new document's layer (`doc::Document::default`).
pub const NEW_LAYER_COLOR: &str = "#1f77b4";

/// Fill in the saved defaults of a device (LightBurn's layer defaults and optimization defaults), so the recorded
/// invocations replay without the preferences: `layer.add` without `defaults` gets the per-colour layer defaults;
/// `doc.new` without `layer_settings` gets the default for the new layer's colour when "load defaults on New" is on,
/// and without `optimization` the saved optimization defaults.
pub fn fill_defaults(p: &DevicePrefs, invs: &mut [commands::Invocation]) {
    for inv in invs.iter_mut() {
        if inv.params.is_null() && inv.id == "doc.new" {
            inv.params = serde_json::json!({});
        }
        if let Some(params) = inv.params.as_object_mut() {
            fill_one(&inv.id, params, p);
        }
    }
}

/// Whether [`fill_defaults`] has anything to do in `invs`.
pub fn wants_defaults(invs: &[commands::Invocation]) -> bool {
    invs.iter().any(|i| matches!(i.id.as_str(), "layer.add" | "doc.new"))
}

fn fill_one(id: &str, params: &mut serde_json::Map<String, serde_json::Value>, p: &DevicePrefs) {
    match id {
        "layer.add" if !params.contains_key("defaults") && !p.layer_defaults.is_empty() => {
            let map: BTreeMap<&String, &CutSettings> = p.layer_defaults.iter().map(|(k, v)| (k, &v.settings)).collect();
            if let Ok(v) = serde_json::to_value(map) {
                params.insert("defaults".into(), v);
            }
        }
        "doc.new" => {
            if p.load_defaults_on_new
                && !params.contains_key("layer_settings")
                && let Some(d) = p.layer_default(NEW_LAYER_COLOR)
                && let Ok(v) = serde_json::to_value(&d.settings)
            {
                params.insert("layer_settings".into(), v);
            }
            if !params.contains_key("optimization")
                && let Some(o) = &p.optimization_defaults
                && let Ok(v) = serde_json::to_value(o)
            {
                params.insert("optimization".into(), v);
            }
        }
        _ => {}
    }
}
