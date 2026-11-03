//! Engine preferences kept in `<config-dir>/preferences.json`: per device, LightBurn's layer defaults (Make Default /
//! Reset to Default / Make Default for All, per colour; "Load default layer settings on New"), the optimization
//! defaults (Set as Defaults), Galvo Basic Settings' Other Options (Require Framing Before Start, Play Sound on Job
//! Finished, the Job Checklist), the device's material library and a GRBL gantry's console macros; globally the
//! debug log and the opt-in update check. Written through a temporary file; a missing or unreadable file means the defaults (reported once).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use doc::{CutSettings, Optimization};
use serde::{Deserialize, Serialize};

use crate::EngineError;

pub const PREFS_FILE: &str = "preferences.json";
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

/// The preference store: in memory, saved to `path` when there is one.
#[derive(Debug, Default)]
pub struct PrefStore {
    pub prefs: Preferences,
    path: Option<PathBuf>,
}

impl PrefStore {
    /// Read `<dir>/preferences.json` (defaults when missing; a broken file is reported and replaced on the next save).
    pub fn open(dir: &Path) -> (PrefStore, Option<String>) {
        let path = dir.join(PREFS_FILE);
        let (prefs, note) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Preferences>(&text) {
                Ok(p) => (p, None),
                Err(e) => (Preferences::default(), Some(format!("{}: {e}; using the defaults", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Preferences::default(), None),
            Err(e) => (Preferences::default(), Some(format!("{}: {e}; using the defaults", path.display()))),
        };
        (PrefStore { prefs, path: Some(path) }, note)
    }

    pub fn device(&self, id: &str) -> DevicePrefs {
        self.prefs.devices.get(id).cloned().unwrap_or_default()
    }

    pub fn device_mut(&mut self, id: &str) -> &mut DevicePrefs {
        self.prefs.devices.entry(id.to_string()).or_default()
    }

    /// Write the preferences (no-op without a file).
    pub fn save(&mut self) -> Result<(), EngineError> {
        let Some(path) = &self.path else { return Ok(()) };
        self.prefs.format = "openetch-preferences".into();
        let text = serde_json::to_string_pretty(&self.prefs).map_err(|e| EngineError::Internal(e.to_string()))?;
        write_atomic(path, text.as_bytes())
    }
}

/// Write `bytes` to `path` through a temporary file in the same directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    use std::io::Write as _;
    let dir = path.parent().ok_or_else(|| EngineError::Internal(format!("{} has no directory", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| EngineError::Internal(format!("cannot create {}: {e}", dir.display())))?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    r.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::Internal(format!("cannot write {}: {e}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_by_colour_then_all() {
        let mut d = DevicePrefs::default();
        let red = LayerDefault { settings: CutSettings { power_pct: 50.0, ..CutSettings::default() }, sub_layers: vec![] };
        let all = LayerDefault { settings: CutSettings { power_pct: 10.0, ..CutSettings::default() }, sub_layers: vec![] };
        d.layer_defaults.insert("#d62728".into(), red);
        assert!(d.layer_default("#1f77b4").is_none());
        d.layer_defaults.insert(ALL_COLORS.into(), all);
        assert_eq!(d.layer_default("#D62728").unwrap().settings.power_pct, 50.0);
        assert_eq!(d.layer_default("#1f77b4").unwrap().settings.power_pct, 10.0);
    }

    #[test]
    fn store_round_trips_and_survives_a_broken_file() {
        let dir = std::env::temp_dir().join(format!("openetch-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut s, note) = PrefStore::open(&dir);
        assert!(note.is_none());
        s.device_mut("b6").require_framing = true;
        s.device_mut("b6").checklist = Checklist { enabled: true, items: vec!["Door closed".into()] };
        s.save().unwrap();
        let (again, _) = PrefStore::open(&dir);
        assert!(again.device("b6").require_framing);
        assert_eq!(again.device("b6").checklist.items, ["Door closed"]);
        assert!(!again.device("other").require_framing);
        std::fs::write(dir.join(PREFS_FILE), "{ broken").unwrap();
        let (broken, note) = PrefStore::open(&dir);
        assert!(note.is_some() && broken.prefs == Preferences::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn macros_must_pass_the_console_policy() {
        let m = |name: &str, lines: &[&str]| ConsoleMacro { name: name.into(), lines: lines.iter().map(|l| (*l).to_owned()).collect() };
        assert!(macros_problem(&[m("Air on", &["M8"]), m("Park", &["$J=G90 G21 X0 Y0 F3000", "M9"])]).is_none());
        for bad in [
            vec![m("Fire", &["M3 S1000"])],
            vec![m("Sneaky", &["G1 X10 F100 S5"])],
            vec![m("Startup", &["$N0=M4"])],
            vec![m("", &["M8"])],
            vec![m("Empty", &[])],
            vec![m("Status", &["?"])],
        ] {
            assert!(macros_problem(&bad).is_some(), "{bad:?}");
        }
        assert!(macros_problem(&vec![m("x", &["M8"]); MAX_MACROS + 1]).is_some());
        // Older preference files without macros load as none, and none are written.
        let d: DevicePrefs = serde_json::from_str("{}").unwrap();
        assert!(d.console_macros.is_empty());
        assert!(serde_json::to_value(&d).unwrap().get("console_macros").is_none());
    }

    #[test]
    fn checklist_rules() {
        assert!(Checklist { enabled: true, items: vec![] }.problem().is_some());
        assert!(Checklist { enabled: false, items: vec![" ".into()] }.problem().is_some());
        assert!(Checklist { enabled: true, items: vec!["Eyewear on".into()] }.problem().is_none());
    }
}
