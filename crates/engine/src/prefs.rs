//! Engine preferences kept in `<config-dir>/preferences.json`: per device, LightBurn's layer defaults (Make Default /
//! Reset to Default / Make Default for All, per colour; "Load default layer settings on New"), the optimization
//! defaults (Set as Defaults), Galvo Basic Settings' Other Options (Require Framing Before Start, Play Sound on Job
//! Finished, the Job Checklist), the device's material library and a GRBL gantry's console macros; globally the
//! debug log and the opt-in update check. Written through a temporary file; a missing or unreadable file means the defaults (reported once).

use std::path::{Path, PathBuf};


use crate::EngineError;

pub const PREFS_FILE: &str = "preferences.json";
// The preference types and their checks (shared with the browser core).
pub use appcore::prefs::{
    ALL_COLORS, Checklist, ConsoleMacro, DevicePrefs, LayerDefault, MAX_CHECKLIST_ITEM_CHARS, MAX_CHECKLIST_ITEMS, MAX_MACRO_LINES,
    MAX_MACRO_NAME_CHARS, MAX_MACROS, MAX_MOVE_POSITIONS, MovePosition, Preferences, macros_problem, move_positions_problem,
};

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
    use doc::CutSettings;

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
