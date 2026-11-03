//! Store writes: what a mutation of a typed in-memory store needs persisted. The engine keeps its stores in files
//! of its config directory; the browser build hands each write to its worker shell, which keeps it in IndexedDB and
//! gives every record back when the core starts (`Backend::new`'s `stores`).
//!
//! A write is `{store, key, value}`: `value` is the record's text (JSON for JSON stores, TOML for devices, the
//! `.clb` XML of a material library), or `null` to delete the record.

use serde::Serialize;

/// One record to persist (or, with `value: None`, to delete).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreWrite {
    pub store: String,
    pub key: String,
    pub value: Option<String>,
}

impl StoreWrite {
    /// `value` as text.
    pub fn text(store: &str, key: &str, value: String) -> Self {
        StoreWrite { store: store.into(), key: key.into(), value: Some(value) }
    }

    /// `value` as pretty JSON.
    pub fn json<T: Serialize + ?Sized>(store: &str, key: &str, value: &T) -> Self {
        let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into());
        StoreWrite::text(store, key, text)
    }

    /// Delete the record.
    pub fn delete(store: &str, key: &str) -> Self {
        StoreWrite { store: store.into(), key: key.into(), value: None }
    }
}

/// The store names of the browser build.
pub mod names {
    /// `preferences` (the engine's `preferences.json`).
    pub const PREFS: &str = "prefs";
    /// `settings` (the engine's `files.json`).
    pub const FILES: &str = "files";
    /// Devices (see `crate::devices::STORE`).
    pub const DEVICES: &str = crate::devices::STORE;
    /// `material-test` and `image-adjust` preset lists.
    pub const PRESETS: &str = "presets";
    /// `prefs` (the engine's `font-prefs.json`).
    pub const FONTS: &str = "fonts";
}

/// Autosave and backup settings (`GET/PUT /api/files/settings`; the engine's `<config-dir>/files.json`).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileSettings {
    /// Minutes between autosaves of a changed document (LightBurn's Auto-save interval; 0 turns it off).
    pub autosave_minutes: u32,
    /// Autosaves kept per project (oldest removed first).
    pub autosave_keep: u32,
    /// Backups of a project's previous file kept on every save (0 = none).
    pub backups_keep: u32,
}

impl Default for FileSettings {
    fn default() -> Self {
        FileSettings { autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 }
    }
}

impl FileSettings {
    /// Why these settings cannot be saved, or `None`.
    pub fn problem(&self) -> Option<String> {
        if self.autosave_minutes > 24 * 60 {
            return Some("autosave_minutes must be 0..=1440".into());
        }
        if !(1..=50).contains(&self.autosave_keep) {
            return Some("autosave_keep must be 1..=50".into());
        }
        if self.backups_keep > 50 {
            return Some("backups_keep must be 0..=50".into());
        }
        None
    }
}
