//! LightBurn devices in the engine ([`LightBurn`]): the device list of LightBurn's `prefs.ini` on this computer
//! (`--lightburn-prefs`, default the OS location, see [`machine::lightburn::default_prefs_paths`]; a missing file is
//! fine), or a copy uploaded through `POST /api/lightburn/prefs` for an engine on a Pi without LightBurn. Used for
//! the field size of LightBurn imports ([`LightBurn::field_for_import`]), `GET /api/lightburn`, the `lightburn`
//! part of `GET /api/machine` and `openetch profile from-lightburn` ([`profile_from_lightburn_command`]).
//!
//! **Privacy.** `prefs.ini` holds all of LightBurn's settings, secrets included. The file is read into memory only to
//! be parsed by [`machine::parse_devices`], which keeps the typed device list and nothing else; the text is dropped
//! right after. Only the parsed devices are ever stored (an upload, as `<config-dir>/lightburn-devices.json`),
//! returned (name, display name and sizes) or logged (device count and path). Nothing writes to `prefs.ini`.
//!
//! An uploaded device list wins over the local file while it exists (`DELETE /api/lightburn/prefs` removes it). The
//! local file is read again whenever its size or modification time changes, so LightBurn edits show up without a
//! restart.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use machine::lightburn::diff_table;
use machine::{LightBurnDevice, LightBurnPrefs, MachineProfile, find_device, parse_devices, profile_from_lightburn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::EngineError;

/// Largest `prefs.ini` `POST /api/lightburn/prefs` accepts.
pub const MAX_PREFS_UPLOAD_BYTES: usize = 2 * 1024 * 1024;
/// Largest local `prefs.ini` read (they are tens of kB; LightBurn keeps cut libraries elsewhere).
const MAX_PREFS_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The uploaded device list, in the config directory.
pub const STORE_FILE: &str = "lightburn-devices.json";
/// Format tag of [`STORE_FILE`].
const STORE_FORMAT: &str = "openetch-lightburn-devices";
/// Largest field size a LightBurn device may give an import, mm (the commands refuse more).
const MAX_FIELD_MM: f64 = 2000.0;

/// Where LightBurn devices come from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightBurnConfig {
    /// `prefs.ini` candidates, first existing wins (`--lightburn-prefs`, else the OS locations).
    pub prefs_paths: Vec<PathBuf>,
    /// `--lightburn-device`: the device to use when a file names none (default: LightBurn's default device).
    pub device: Option<String>,
    /// Where an uploaded device list is kept (the engine's config directory); `None` keeps it in memory only.
    pub store_dir: Option<PathBuf>,
}

/// Which device list is in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The local `prefs.ini`.
    File,
    /// A device list uploaded through the API.
    Uploaded,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::File => "file",
            Source::Uploaded => "uploaded",
        }
    }
}

/// The parsed local file and the size and mtime it was read at.
struct Cached {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
    prefs: Option<Arc<LightBurnPrefs>>,
}

/// [`STORE_FILE`] on disk.
#[derive(Serialize, Deserialize)]
struct Stored {
    format: String,
    version: u32,
    prefs: LightBurnPrefs,
}

/// LightBurn devices for the engine. See the module docs.
#[derive(Default)]
pub struct LightBurn {
    config: Mutex<LightBurnConfig>,
    file: Mutex<Option<Cached>>,
    uploaded: Mutex<Option<Arc<LightBurnPrefs>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read and parse a local `prefs.ini`. Errors name the path, never file content.
pub fn read_prefs_file(path: &Path) -> Result<LightBurnPrefs, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?.len();
    if len > MAX_PREFS_FILE_BYTES {
        return Err(format!("{} is larger than {} MB", path.display(), MAX_PREFS_FILE_BYTES / (1024 * 1024)));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
    parse_devices(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The JSON of one device in `GET /api/lightburn` (only these fields).
fn device_summary(d: &LightBurnDevice) -> Value {
    json!({
        "name": d.name,
        "display_name": d.display_name,
        "field_mm": d.field_mm(),
        "width_mm": d.width_mm,
        "height_mm": d.height_mm,
    })
}

/// A device's field when it is usable for an import.
fn usable_field(d: &LightBurnDevice) -> Option<f64> {
    d.field_mm().filter(|f| f.is_finite() && *f > 0.0 && *f <= MAX_FIELD_MM)
}

impl LightBurn {
    /// Set where devices come from and load a previously uploaded list. Returns a line for the startup log.
    pub fn configure(&self, config: LightBurnConfig) -> String {
        let stored = config.store_dir.as_ref().map(|d| d.join(STORE_FILE));
        *lock(&self.config) = config;
        *lock(&self.file) = None;
        let mut uploaded = None;
        if let Some(path) = stored.filter(|p| p.exists()) {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| {
                let s: Stored = serde_json::from_str(&t).map_err(|e| e.to_string())?;
                if s.format != STORE_FORMAT || s.version != 1 {
                    return Err("not an OpenEtch LightBurn device list".into());
                }
                Ok(s.prefs)
            }) {
                Ok(p) => uploaded = Some(Arc::new(p)),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "ignoring the stored LightBurn device list"),
            }
        }
        *lock(&self.uploaded) = uploaded;
        match self.current() {
            Some((source, prefs)) => format!(
                "LightBurn: {} device(s) ({}){}",
                prefs.devices.len(),
                match source {
                    Source::Uploaded => "uploaded".to_owned(),
                    Source::File => self.file_path().map(|p| p.display().to_string()).unwrap_or_default(),
                },
                match self.matched() {
                    Some(d) => format!(", using \"{}\" ({} mm)", d.name, d.field_mm().unwrap_or(f64::NAN)),
                    None => String::new(),
                }
            ),
            None => "LightBurn: no prefs.ini found; LightBurn imports use the document's field size".into(),
        }
    }

    /// The local `prefs.ini` in use (the first candidate that exists).
    fn file_path(&self) -> Option<PathBuf> {
        lock(&self.config).prefs_paths.iter().find(|p| p.is_file()).cloned()
    }

    /// The local file's devices, re-read when its size or mtime changed. A broken file is logged once per change.
    fn file_devices(&self) -> Option<Arc<LightBurnPrefs>> {
        let path = self.file_path()?;
        let meta = std::fs::metadata(&path).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok());
        let mut cache = lock(&self.file);
        if let Some(c) = cache.as_ref()
            && c.path == path
            && c.len == len
            && c.modified == modified
        {
            return c.prefs.clone();
        }
        let prefs = match read_prefs_file(&path) {
            Ok(p) => {
                tracing::info!(path = %path.display(), devices = p.devices.len(), "read the LightBurn device list");
                Some(Arc::new(p))
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot use LightBurn's prefs.ini");
                None
            }
        };
        *cache = Some(Cached { path, len, modified, prefs: prefs.clone() });
        prefs
    }

    /// The device list in use: an uploaded one, else the local file's.
    pub fn current(&self) -> Option<(Source, Arc<LightBurnPrefs>)> {
        if let Some(p) = lock(&self.uploaded).clone() {
            return Some((Source::Uploaded, p));
        }
        self.file_devices().map(|p| (Source::File, p))
    }

    /// The device used when a file names none: `--lightburn-device`, else LightBurn's default device.
    pub fn matched(&self) -> Option<LightBurnDevice> {
        let (_, prefs) = self.current()?;
        let wanted = lock(&self.config).device.clone();
        match wanted {
            Some(name) => find_device(&prefs, &name).cloned(),
            None => prefs.default_device().or(if prefs.devices.len() == 1 { prefs.devices.first() } else { None }).cloned(),
        }
    }

    /// `GET /api/lightburn`: `{available, source, default_device, devices: [{name, display_name, field_mm,
    /// width_mm, height_mm}]}`; `default_device` is the name of the device used when a file names none.
    pub fn view(&self) -> Value {
        match self.current() {
            Some((source, prefs)) => json!({
                "available": true,
                "source": source.name(),
                "default_device": self.matched().map(|d| d.name),
                "devices": prefs.devices.iter().map(device_summary).collect::<Vec<_>>(),
            }),
            None => json!({ "available": false, "source": null, "default_device": null, "devices": [] }),
        }
    }

    /// The field size for a LightBurn import without `field_mm`: the device the file's `DeviceName` names, else the
    /// matched device ([`LightBurn::matched`]). Returns the size and its `field_source` text.
    pub fn field_for_import(&self, content: &str) -> Option<(f64, String)> {
        let (_, prefs) = self.current()?;
        if let Some(name) = commands::lbrn2_device_name(content)
            && let Some(d) = find_device(&prefs, &name)
            && let Some(f) = usable_field(d)
        {
            return Some((f, format!("file device {}", d.name)));
        }
        let d = self.matched()?;
        usable_field(&d).map(|f| (f, format!("LightBurn default device {}", d.name)))
    }

    /// The `lightburn` part of `GET /api/machine`: `{device, field_mm, diffs}` comparing `profile` with the matched
    /// device, or null.
    pub fn machine_json(&self, profile: &MachineProfile) -> Value {
        match self.matched() {
            Some(d) => {
                let (_, diffs) = profile_from_lightburn(&d, profile);
                json!({ "device": d.name, "field_mm": d.field_mm(), "diffs": diffs })
            }
            None => Value::Null,
        }
    }

    /// `POST /api/lightburn/prefs`: parse an uploaded `prefs.ini` and keep only its devices (also on disk when there
    /// is a config directory). Returns [`LightBurn::view`].
    pub fn upload(&self, bytes: &[u8]) -> Result<Value, EngineError> {
        if bytes.len() > MAX_PREFS_UPLOAD_BYTES {
            return Err(EngineError::TooLarge(format!(
                "prefs.ini is larger than {} MB",
                MAX_PREFS_UPLOAD_BYTES / (1024 * 1024)
            )));
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| EngineError::BadRequest("the file is not UTF-8 text (expected LightBurn's prefs.ini)".into()))?;
        let prefs = parse_devices(text).map_err(|e| EngineError::BadRequest(e.to_string()))?;
        if prefs.devices.is_empty() {
            return Err(EngineError::BadRequest(
                "no LightBurn devices in this file (expected LightBurn's prefs.ini, which lists them in DeviceList)"
                    .into(),
            ));
        }
        let dir = lock(&self.config).store_dir.clone();
        if let Some(dir) = dir {
            let stored = Stored { format: STORE_FORMAT.into(), version: 1, prefs: prefs.clone() };
            let text = serde_json::to_string_pretty(&stored).map_err(|e| EngineError::Internal(e.to_string()))?;
            write_atomic(&dir, STORE_FILE, &text).map_err(EngineError::Internal)?;
        }
        tracing::info!(devices = prefs.devices.len(), "stored an uploaded LightBurn device list");
        *lock(&self.uploaded) = Some(Arc::new(prefs));
        Ok(self.view())
    }

    /// `DELETE /api/lightburn/prefs`: forget the uploaded device list (the local file is used again, if any).
    pub fn delete_upload(&self) -> Result<Value, EngineError> {
        let dir = lock(&self.config).store_dir.clone();
        if let Some(dir) = dir {
            match std::fs::remove_file(dir.join(STORE_FILE)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(EngineError::Internal(format!("cannot remove the stored device list: {e}"))),
            }
        }
        *lock(&self.uploaded) = None;
        Ok(self.view())
    }
}

/// Write `dir/name` through a fresh temporary file renamed into place (creating `dir`).
fn write_atomic(dir: &Path, name: &str, text: &str) -> Result<(), String> {
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

/// The OS locations of LightBurn's `prefs.ini` on this computer, most likely first.
pub fn default_prefs_paths() -> Vec<PathBuf> {
    machine::lightburn::default_prefs_paths(std::env::consts::OS, |k| std::env::var(k).ok())
}

/// Options of `openetch profile from-lightburn`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileFromLightBurn {
    /// `--prefs`: the `prefs.ini` (default: the OS location).
    pub prefs: Option<PathBuf>,
    /// `--device`: name or display name (default: LightBurn's default device, or the only one).
    pub device: Option<String>,
    /// `--base`: the profile TOML to start from (default: the B6 fixture when found, else the generic profile).
    pub base: Option<PathBuf>,
    /// `--out`: the TOML to write.
    pub out: PathBuf,
    /// `--force`: replace an existing `--out`.
    pub force: bool,
}

/// `openetch profile from-lightburn`: write a profile for a LightBurn device and return the report to print (the
/// diff table and where the file went). Refuses to replace `out` unless `force`.
pub fn profile_from_lightburn_command(opts: &ProfileFromLightBurn) -> Result<String, String> {
    let prefs_path = match &opts.prefs {
        Some(p) => p.clone(),
        None => {
            let candidates = default_prefs_paths();
            candidates.iter().find(|p| p.is_file()).cloned().ok_or_else(|| {
                let list: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();
                format!("no LightBurn prefs.ini found (looked in: {}); pass --prefs <path>", list.join(", "))
            })?
        }
    };
    let prefs = read_prefs_file(&prefs_path)?;
    let names = || prefs.devices.iter().map(|d| format!("\"{}\"", d.name)).collect::<Vec<_>>().join(", ");
    let device = match &opts.device {
        Some(name) => find_device(&prefs, name)
            .ok_or_else(|| format!("no LightBurn device named \"{name}\" in {} (devices: {})", prefs_path.display(), names()))?,
        None => prefs
            .default_device()
            .or(if prefs.devices.len() == 1 { prefs.devices.first() } else { None })
            .ok_or_else(|| format!("{} has no default device; pass --device (devices: {})", prefs_path.display(), names()))?,
    };
    let (base, base_name) = match &opts.base {
        Some(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            (MachineProfile::from_toml(&text).map_err(|e| format!("{}: {e}", p.display()))?, p.display().to_string())
        }
        None => {
            let p = crate::cli::resolve_default(crate::cli::DEFAULT_PROFILE);
            match std::fs::read_to_string(&p).ok().and_then(|t| MachineProfile::from_toml(&t).ok()) {
                Some(b) => (b, p.display().to_string()),
                None => (MachineProfile::generic_lmc_fiber(), "the generic LMC fiber profile".into()),
            }
        }
    };
    let (profile, diffs) = profile_from_lightburn(device, &base);
    let toml = profile.to_toml().map_err(|e| e.to_string())?;
    let header = format!(
        "# Machine profile made by `openetch profile from-lightburn` from LightBurn device \"{}\" on {}.\n# See `notes`; \
         the swap/negate mapping from LightBurn is unverified on hardware.\n\n",
        device.name, base_name
    );
    let mut opts_w = std::fs::OpenOptions::new();
    opts_w.write(true);
    if opts.force {
        opts_w.create(true).truncate(true);
    } else {
        opts_w.create_new(true);
    }
    let mut f = opts_w.open(&opts.out).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} already exists; pass --force to replace it", opts.out.display()),
        _ => format!("cannot write {}: {e}", opts.out.display()),
    })?;
    f.write_all(header.as_bytes())
        .and_then(|_| f.write_all(toml.as_bytes()))
        .map_err(|e| format!("cannot write {}: {e}", opts.out.display()))?;
    let mut report = format!(
        "LightBurn device \"{}\" ({}) from {}, onto {}:\n\n{}\nwrote {}\n",
        device.name,
        device.display_name,
        prefs_path.display(),
        base_name,
        diff_table(&diffs),
        opts.out.display()
    );
    if let Some(cor) = &profile.cor_file {
        let dir = opts.out.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if !dir.join(cor).is_file() {
            report.push_str(&format!(
                "note: the correction file {} is looked up next to the profile; copy it into {}\n",
                cor.display(),
                dir.display()
            ));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::temp_dir;

    const PREFS: &str = include_str!("../../machine/tests/fixtures/lightburn-prefs.json");

    fn with_file() -> (LightBurn, PathBuf) {
        let dir = temp_dir("lightburn");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.ini");
        std::fs::write(&path, PREFS).unwrap();
        let lb = LightBurn::default();
        lb.configure(LightBurnConfig {
            prefs_paths: vec![dir.join("missing.ini"), path.clone()],
            device: None,
            store_dir: Some(dir.join("config")),
        });
        (lb, path)
    }

    #[test]
    fn file_devices_and_the_field_rule() {
        let (lb, path) = with_file();
        let v = lb.view();
        assert_eq!((v["available"].clone(), v["source"].clone(), v["default_device"].clone()), (json!(true), json!("file"), json!("JCZFiber")));
        assert_eq!(
            v["devices"][0],
            json!({"name": "JCZFiber", "display_name": "JCZFiber (Unknown)", "field_mm": 153.8, "width_mm": 153.8, "height_mm": 153.8})
        );
        assert_eq!(v["devices"][1]["field_mm"], json!(110.0));
        let v2 = "<?xml version=\"1.0\"?>\n<LightBurnProject DeviceName=\"Bench 110 lens\" FormatVersion=\"1\">";
        assert_eq!(lb.field_for_import(v2), Some((110.0, "file device Bench110".into())));
        let other = "<LightBurnProject DeviceName=\"Someone else's laser\">";
        assert_eq!(lb.field_for_import(other), Some((153.8, "LightBurn default device JCZFiber".into())));
        assert_eq!(lb.field_for_import("<LightBurnProject>"), Some((153.8, "LightBurn default device JCZFiber".into())));

        // --lightburn-device picks another default.
        let cfg = LightBurnConfig { device: Some("bench110".into()), ..lock(&lb.config).clone() };
        lb.configure(cfg);
        assert_eq!(lb.view()["default_device"], json!("Bench110"));
        assert_eq!(lb.field_for_import("<LightBurnProject>").unwrap().1, "LightBurn default device Bench110");

        // The file is read again when it changes.
        let one = PREFS.replace("\"Name\": \"Bench110\"", "\"Name\": \"Renamed\"").replace("\"DisplayName\": \"Bench 110 lens\"", "\"DisplayName\": \"R\"");
        std::fs::write(&path, format!("{one}\n")).unwrap();
        assert_eq!(lb.view()["devices"][1]["name"], json!("Renamed"));
        assert_eq!(lb.view()["default_device"], json!(null), "the configured device is gone");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(lb.view(), json!({"available": false, "source": null, "default_device": null, "devices": []}));
        assert_eq!(lb.field_for_import("<LightBurnProject>"), None);
    }

    #[test]
    fn uploads_are_stored_parsed_and_win_over_the_file() {
        let (lb, path) = with_file();
        let store = lock(&lb.config).store_dir.clone().unwrap().join(STORE_FILE);
        let one_device = r#"{"RestPendantSecret": "s3cr3t", "DefaultDevice": 0,
            "DeviceList": [{"Name": "Pi laser", "Width": 110, "Height": 110, "Settings": {"CorrectionFile": "/home/me/x.cor"}}]}"#;
        let v = lb.upload(one_device.as_bytes()).unwrap();
        assert_eq!((v["source"].clone(), v["default_device"].clone()), (json!("uploaded"), json!("Pi laser")));
        let stored = std::fs::read_to_string(&store).unwrap();
        assert!(!stored.contains("s3cr3t") && !stored.contains("/home/me") && stored.contains("x.cor"), "{stored}");
        // A restarted engine finds it again.
        let again = LightBurn::default();
        again.configure(lock(&lb.config).clone());
        assert_eq!(again.view()["source"], json!("uploaded"));
        // Refusals.
        assert!(matches!(lb.upload(b"{}"), Err(EngineError::BadRequest(_))));
        assert!(matches!(lb.upload(b"not json"), Err(EngineError::BadRequest(_))));
        assert!(matches!(lb.upload(&[0xff, 0xfe]), Err(EngineError::BadRequest(_))));
        assert!(matches!(lb.upload(&vec![b' '; MAX_PREFS_UPLOAD_BYTES + 1]), Err(EngineError::TooLarge(_))));
        assert_eq!(lb.view()["default_device"], json!("Pi laser"), "a refused upload changes nothing");
        // Delete: back to the file; deleting twice is fine.
        assert_eq!(lb.delete_upload().unwrap()["source"], json!("file"));
        assert!(!store.exists());
        assert_eq!(lb.delete_upload().unwrap()["source"], json!("file"));
        assert!(path.exists(), "prefs.ini is never touched");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), PREFS);
    }

    #[test]
    fn machine_diffs() {
        let (lb, _) = with_file();
        let b6 = MachineProfile::from_toml(include_str!("../../../fixtures/b6-mopa-60w.toml")).unwrap();
        let v = lb.machine_json(&b6);
        assert_eq!((v["device"].clone(), v["field_mm"].clone()), (json!("JCZFiber"), json!(153.8)));
        let fields: Vec<&str> = v["diffs"].as_array().unwrap().iter().map(|d| d["field"].as_str().unwrap()).collect();
        assert!(fields.contains(&"mark_speed.max_mm_s") && fields.contains(&"field_mm"), "{fields:?}");
        assert_eq!(LightBurn::default().machine_json(&b6), Value::Null);
    }

    #[test]
    fn profile_command_writes_once() {
        let (_, path) = with_file();
        let dir = path.parent().unwrap().join("out");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("b6.toml");
        let opts = ProfileFromLightBurn { prefs: Some(path.clone()), device: None, base: None, out: out.clone(), force: false };
        let report = profile_from_lightburn_command(&opts).unwrap();
        assert!(report.contains("mark_speed.max_mm_s") && report.contains("jcz150.cor"), "{report}");
        let written = MachineProfile::from_toml(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!((written.field_mm, written.mark_speed.max_mm_s, written.start_mark_input_bit), (153.75, 7000.0, Some(14)));
        let e = profile_from_lightburn_command(&opts).unwrap_err();
        assert!(e.contains("--force"), "{e}");
        let bench = ProfileFromLightBurn { device: Some("Bench 110 lens".into()), force: true, ..opts.clone() };
        profile_from_lightburn_command(&bench).unwrap();
        let written = MachineProfile::from_toml(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(written.field_mm, 110.0);
        let e = profile_from_lightburn_command(&ProfileFromLightBurn { device: Some("nope".into()), ..bench }).unwrap_err();
        assert!(e.contains("JCZFiber") && e.contains("Bench110"), "{e}");
        let e = profile_from_lightburn_command(&ProfileFromLightBurn { prefs: Some(dir.join("none.ini")), ..opts }).unwrap_err();
        assert!(e.contains("cannot read"), "{e}");
    }
}
