//! Named projects on disk: `<projects-dir>/<name>.openetch` (and LightBurn exports as `<name>.lbrn2`), plus the
//! project-name rules and the "current project" bookkeeping behind `GET /api/project`.
//!
//! **Folders and locations** (LightBurn saves anywhere through the OS dialogs; OpenEtch is a server, so it offers
//! folders under a fixed set of roots instead). A project name may sit in up to [`MAX_FOLDER_DEPTH`] folders,
//! `/`-separated (`Clients/Acme/badge`). Further directories are mounted as **locations** under a label
//! ([`Location`]: `--projects-mount <label>=<path>` on the command line, or saved in `<config-dir>/
//! project-locations.json` through `PUT /api/projects/locations` from the UI): a name whose first folder is a
//! location's label lives in that directory (`NAS/coaster` is `<nas path>/coaster.openetch`). A location hides a
//! folder of the same name in the projects directory. `templates` and `art-libraries` hold OpenEtch's own files and
//! are never project folders ([`INTERNAL_DIRS`]).
//!
//! Path safety: every name and folder is 1-100 characters of letters, digits, space, `-`, `_`, `.`, `(` and `)`, so
//! it can never hold a path separator, a drive letter or `..`. On top of that every file and folder is resolved
//! against the canonical root (the projects directory or the location) and refused unless it stays inside it, so a
//! symbolic link that points elsewhere is neither read nor written through.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::EngineError;

/// Longest project name, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// Largest project file read or uploaded (the commands crate refuses larger contents anyway).
pub const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
/// File name extension of native projects, without the dot.
pub const PROJECT_EXT: &str = doc::file::EXTENSION;
/// File name extension of LightBurn exports, without the dot.
pub const LBRN2_EXT: &str = "lbrn2";
/// Most folders above a project (`a/b/c/d/name`).
pub const MAX_FOLDER_DEPTH: usize = 4;
/// Most projects (and folders) one listing returns; a bigger tree is cut off.
pub const MAX_LISTED: usize = 10_000;
/// Most locations besides the projects directory.
pub const MAX_LOCATIONS: usize = 16;
/// Folders of the projects directory that hold OpenEtch's own files, never projects (dot folders such as
/// `.backups` and `.autosave` are refused by the name rules anyway).
pub const INTERNAL_DIRS: [&str; 2] = ["templates", "art-libraries"];

/// Windows device names, refused as names in any case and with any extension (e.g. `CON.backup`) on every OS so a
/// project saved on a Pi can be copied to a PC. Windows also treats the superscript digits ¹ ² ³ as digits here.
const RESERVED: [&str; 30] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "com¹",
    "com²", "com³", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9", "lpt¹", "lpt²",
    "lpt³",
];

/// Whether two project names name the same file on a case-insensitive file system (Windows, macOS).
pub fn same_name_ignoring_case(a: &str, b: &str) -> bool {
    a == b || a.to_lowercase() == b.to_lowercase()
}

/// Check a project name (see the module docs). Also refuses a leading or trailing space and Windows device names.
pub fn validate_name(name: &str) -> Result<(), EngineError> {
    let bad = |why: &str| {
        Err(EngineError::BadRequest(format!(
            "invalid project name {name:?}: {why} (use 1-{MAX_NAME_CHARS} letters, digits, spaces and - _ . ( ))"
        )))
    };
    let chars = name.chars().count();
    if chars == 0 {
        return bad("it is empty");
    }
    if chars > MAX_NAME_CHARS {
        return bad("it is too long");
    }
    if let Some(c) = name.chars().find(|&c| !(c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')'))) {
        return bad(&format!("{c:?} is not allowed"));
    }
    if name.starts_with('.') {
        return bad("it starts with a dot");
    }
    if name.contains("..") {
        return bad("it contains \"..\"");
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return bad("it starts or ends with a space");
    }
    let stem = name.split('.').next().unwrap_or_default().trim_end().to_ascii_lowercase();
    if RESERVED.contains(&stem.as_str()) {
        return bad("it is a reserved device name on Windows");
    }
    Ok(())
}

/// Check a project path: a name ([`validate_name`]) inside at most [`MAX_FOLDER_DEPTH`] folders, `/`-separated,
/// each folder following the same rules; the first folder is never one of [`INTERNAL_DIRS`].
pub fn validate_project_name(name: &str) -> Result<(), EngineError> {
    check_path(name, "project name")
}

/// Check a folder path (`""` is the top of the projects directory): like [`validate_project_name`].
pub fn validate_folder(path: &str) -> Result<(), EngineError> {
    if path.is_empty() { Ok(()) } else { check_path(path, "folder") }
}

fn check_path(path: &str, what: &str) -> Result<(), EngineError> {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() > MAX_FOLDER_DEPTH + 1 {
        return Err(EngineError::BadRequest(format!(
            "invalid {what} {path:?}: at most {MAX_FOLDER_DEPTH} folders deep"
        )));
    }
    for s in &segs {
        validate_name(s)?;
    }
    let top_folder = if what == "folder" { segs.first() } else { segs.split_last().and_then(|(_, f)| f.first()) };
    if let Some(top) = top_folder.filter(|t| INTERNAL_DIRS.iter().any(|d| d.eq_ignore_ascii_case(t))) {
        return Err(EngineError::BadRequest(format!(
            "invalid {what} {path:?}: \"{top}\" holds OpenEtch's own files, not projects"
        )));
    }
    Ok(())
}

/// A project path as one flat file name (`Clients/Acme/badge` → `Clients__Acme__badge`), for autosaves and backups,
/// which live in one folder.
pub fn flat_name(name: &str) -> String {
    name.replace('/', "__")
}

/// A directory mounted as a location of projects (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Location {
    /// The folder name it appears as.
    pub label: String,
    /// The directory (absolute).
    pub path: PathBuf,
    /// `"cli"` (`--projects-mount`, fixed for the run) or `"settings"` (saved, editable from the UI).
    #[serde(default = "settings_source")]
    pub source: String,
}

fn settings_source() -> String {
    "settings".into()
}

/// Check a list of locations: valid, unique labels (ignoring case) that are not [`INTERNAL_DIRS`], absolute paths of
/// existing directories, at most [`MAX_LOCATIONS`].
pub fn validate_locations(locs: &[Location]) -> Result<(), EngineError> {
    if locs.len() > MAX_LOCATIONS {
        return Err(EngineError::BadRequest(format!("at most {MAX_LOCATIONS} locations")));
    }
    for (i, l) in locs.iter().enumerate() {
        validate_name(&l.label)?;
        if INTERNAL_DIRS.iter().any(|d| d.eq_ignore_ascii_case(&l.label)) {
            return Err(EngineError::BadRequest(format!("\"{}\" holds OpenEtch's own files; pick another label", l.label)));
        }
        if locs[..i].iter().any(|o| same_name_ignoring_case(&o.label, &l.label)) {
            return Err(EngineError::BadRequest(format!("two locations are labelled \"{}\"", l.label)));
        }
        if !l.path.is_absolute() {
            return Err(EngineError::BadRequest(format!("location \"{}\": {} is not an absolute path", l.label, l.path.display())));
        }
        if !l.path.is_dir() {
            return Err(EngineError::BadRequest(format!(
                "location \"{}\": {} is not a directory that exists",
                l.label,
                l.path.display()
            )));
        }
    }
    Ok(())
}

/// The default projects directory: the OS data dir + `openetch/projects` (`%APPDATA%\openetch\projects`,
/// `~/Library/Application Support/openetch/projects`, `$XDG_DATA_HOME` or `~/.local/share/openetch/projects`).
pub fn default_projects_dir() -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| env("HOME").map(|h| h.join(".local").join("share")))
    };
    base.map(|b| b.join("openetch").join("projects"))
}

/// One saved project, as `GET /api/projects` lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectInfo {
    /// The project path: folders and name, `/`-separated.
    pub name: String,
    /// The folders part of `name` (`""` at the top).
    pub folder: String,
    /// Last modification, Unix seconds (0 when the OS does not report it).
    pub modified_unix_s: u64,
    /// File size in bytes.
    pub size: u64,
}

/// The projects directory and its locations. Created on the first save; listing a missing directory gives no
/// projects. Clones share the locations.
#[derive(Debug, Clone)]
pub struct ProjectDir {
    root: PathBuf,
    locations: Arc<RwLock<Vec<Location>>>,
}

/// Where a project path lives: the canonical root it must stay in, the folders below it and the file stem.
struct Place<'a> {
    root: PathBuf,
    folders: Vec<&'a str>,
    stem: &'a str,
    /// The folders as given, with a trailing `/` (`""` at the top), to rebuild names.
    prefix: String,
    /// The location's label as configured, when the path is in one.
    label: Option<String>,
}

impl Place<'_> {
    /// `<folders>/<file>` relative to the root.
    fn rel(&self, file: &str) -> String {
        let mut p: String = self.folders.iter().map(|f| format!("{f}/")).collect();
        p.push_str(file);
        p
    }
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> EngineError {
    EngineError::Internal(format!("cannot {what} {}: {e}", path.display()))
}

/// The modification time of `meta` in Unix seconds, 0 when unknown.
fn unix_s(meta: &fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

impl ProjectDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ProjectDir { root: root.into(), locations: Arc::default() }
    }

    /// The mounted locations.
    pub fn locations(&self) -> Vec<Location> {
        self.locations.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Replace the mounted locations (checked by the caller with [`validate_locations`]).
    pub fn set_locations(&self, locs: Vec<Location>) {
        *self.locations.write().unwrap_or_else(PoisonError::into_inner) = locs;
    }

    /// Split a checked project path (`is_name`) or folder path into its root, folders and stem. `None` root when the
    /// projects directory does not exist and `create` is false; a location that is gone is an error.
    fn place<'a>(&self, path: &'a str, is_name: bool, create: bool) -> Result<Option<Place<'a>>, EngineError> {
        let mut segs: Vec<&str> = if path.is_empty() { Vec::new() } else { path.split('/').collect() };
        let stem = if is_name { segs.pop().unwrap_or_default() } else { "" };
        let prefix: String = segs.iter().map(|f| format!("{f}/")).collect();
        let location = segs.first().and_then(|top| self.locations().into_iter().find(|l| same_name_ignoring_case(&l.label, top)));
        let label = location.as_ref().map(|l| l.label.clone());
        let root = match location {
            Some(l) => {
                segs.remove(0);
                match fs::canonicalize(&l.path) {
                    Ok(p) if p.is_dir() => p,
                    _ => {
                        return Err(EngineError::NotFound(format!(
                            "location \"{}\" ({}) is not reachable",
                            l.label,
                            l.path.display()
                        )));
                    }
                }
            }
            None => match self.canonical_root(create)? {
                Some(r) => r,
                None => return Ok(None),
            },
        };
        Ok(Some(Place { root, folders: segs, stem, prefix, label }))
    }

    /// The canonical folder of `place`, refused when it leaves the root (a linked folder); `None` when it does not
    /// exist and `create` is false.
    fn folder_of(place: &Place<'_>, create: bool) -> Result<Option<PathBuf>, EngineError> {
        let dir: PathBuf = place.folders.iter().fold(place.root.clone(), |p, f| p.join(f));
        if create {
            // One level at a time, each checked, so a linked folder never has folders created through it.
            let mut cur = place.root.clone();
            for f in &place.folders {
                cur = cur.join(f);
                match fs::create_dir(&cur) {
                    Ok(()) => {}
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(io_err("create the folder", &cur, e)),
                }
                let real = fs::canonicalize(&cur).map_err(|e| io_err("open the folder", &cur, e))?;
                if !real.starts_with(&place.root) || !real.is_dir() {
                    return Err(EngineError::Forbidden(format!("{} is not a folder inside the projects directory", place.prefix)));
                }
                cur = real;
            }
        }
        match fs::canonicalize(&dir) {
            Ok(real) if real.starts_with(&place.root) && real.is_dir() => Ok(Some(real)),
            Ok(_) => Err(EngineError::Forbidden(format!("{} is not a folder inside the projects directory", place.prefix))),
            Err(e) if e.kind() == ErrorKind::NotFound && !create => Ok(None),
            Err(e) => Err(io_err("open the folder", &dir, e)),
        }
    }

    /// Create folder `path` (and the folders above it). Returns the folder path.
    pub fn create_folder(&self, path: &str) -> Result<PathBuf, EngineError> {
        validate_folder(path)?;
        if path.is_empty() {
            return Err(EngineError::BadRequest("name the folder to create".into()));
        }
        let place = self.place(path, false, true)?.ok_or_else(|| EngineError::Internal("no projects directory".into()))?;
        Self::folder_of(&place, true)?.ok_or_else(|| EngineError::Internal("the folder was not created".into()))
    }

    /// Every folder (`/`-separated paths, sorted, case-insensitive), locations included as top folders.
    pub fn folders(&self) -> Result<Vec<String>, EngineError> {
        let mut projects = Vec::new();
        let mut folders = Vec::new();
        self.walk_all(&mut projects, &mut folders)?;
        folders.sort_by_key(|f| f.to_lowercase());
        Ok(folders)
    }

    /// Projects and folders of the projects directory and every location, at most [`MAX_LISTED`] together.
    fn walk_all(&self, projects: &mut Vec<ProjectInfo>, folders: &mut Vec<String>) -> Result<(), EngineError> {
        let locations = self.locations();
        let hidden: Vec<&str> = locations.iter().map(|l| l.label.as_str()).chain(INTERNAL_DIRS).collect();
        if let Some(root) = self.canonical_root(false)? {
            Self::walk(&root, &root, "", 0, &hidden, projects, folders);
        }
        for l in &locations {
            folders.push(l.label.clone());
            // A location that is gone lists nothing (opening from it says so).
            if let Ok(root) = fs::canonicalize(&l.path) {
                Self::walk(&root, &root, &format!("{}/", l.label), 1, &[], projects, folders);
            }
        }
        Ok(())
    }

    /// Walk `dir` (inside `root`) for projects and folders, `prefix` naming it, `depth` folders deep.
    fn walk(
        root: &Path,
        dir: &Path,
        prefix: &str,
        depth: usize,
        hidden: &[&str],
        projects: &mut Vec<ProjectInfo>,
        folders: &mut Vec<String>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let suffix = format!(".{PROJECT_EXT}");
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            if projects.len() + folders.len() >= MAX_LISTED {
                return;
            }
            let file_name = entry.file_name();
            let Some(file) = file_name.to_str() else { continue };
            if let Some(name) = file.strip_suffix(&suffix) {
                if validate_name(name).is_err() {
                    continue;
                }
                let Ok(Some(real)) = Self::resolve_existing(root, &format!("{}{file}", Self::rel_of(root, dir))) else {
                    continue;
                };
                let Ok(meta) = fs::metadata(&real) else { continue };
                projects.push(ProjectInfo {
                    name: format!("{prefix}{name}"),
                    folder: prefix.trim_end_matches('/').to_owned(),
                    modified_unix_s: unix_s(&meta),
                    size: meta.len(),
                });
            } else if depth < MAX_FOLDER_DEPTH
                && validate_name(file).is_ok()
                && !(depth == 0 && hidden.iter().any(|h| h.eq_ignore_ascii_case(file)))
                && let Ok(real) = fs::canonicalize(entry.path())
                && real.is_dir()
                && real.starts_with(root)
            {
                subdirs.push((file.to_owned(), real));
            }
        }
        for (name, real) in subdirs {
            if projects.len() + folders.len() >= MAX_LISTED {
                return;
            }
            let path = format!("{prefix}{name}");
            folders.push(path.clone());
            Self::walk(root, &real, &format!("{path}/"), depth + 1, hidden, projects, folders);
        }
    }

    /// The folders of `place` as spelled on disk (`dir` is its canonical folder), after its location's label.
    fn disk_prefix(place: &Place<'_>, dir: &Path) -> String {
        let mut p = place.label.as_ref().map_or_else(String::new, |l| format!("{l}/"));
        p.push_str(&Self::rel_of(&place.root, dir));
        p
    }

    /// `dir` relative to `root` as `a/b/` (`""` for the root itself).
    fn rel_of(root: &Path, dir: &Path) -> String {
        dir.strip_prefix(root)
            .map(|r| r.components().map(|c| format!("{}/", c.as_os_str().to_string_lossy())).collect())
            .unwrap_or_default()
    }

    /// The directory as configured (not canonicalized).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical directory; `None` when it does not exist and `create` is false.
    fn canonical_root(&self, create: bool) -> Result<Option<PathBuf>, EngineError> {
        if create {
            fs::create_dir_all(&self.root).map_err(|e| io_err("create the projects directory", &self.root, e))?;
        }
        match fs::canonicalize(&self.root) {
            Ok(p) if p.is_dir() => Ok(Some(p)),
            Ok(p) => Err(EngineError::Internal(format!("the projects path {} is not a directory", p.display()))),
            Err(e) if e.kind() == ErrorKind::NotFound && !create => Ok(None),
            Err(e) => Err(io_err("open the projects directory", &self.root, e)),
        }
    }

    /// Where `file` (already a valid name plus extension) resolves to, refused when it leaves `root`. `None` when
    /// nothing exists there.
    fn resolve_existing(root: &Path, file: &str) -> Result<Option<PathBuf>, EngineError> {
        let path = root.join(file);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err("read", &path, e)),
            Ok(_) => {}
        }
        let real = fs::canonicalize(&path).map_err(|_| {
            EngineError::Forbidden(format!("{file} is a link that points nowhere; refusing to use it"))
        })?;
        if !real.starts_with(root) {
            tracing::warn!(
                path = %path.display(),
                target = %real.display(),
                "refused a project link pointing outside the projects directory"
            );
            return Err(EngineError::Forbidden(format!("{file} points outside the projects directory; refusing it")));
        }
        if !real.is_file() {
            return Err(EngineError::Conflict(format!("{file} is not a regular file")));
        }
        Ok(Some(real))
    }

    /// Every `*.openetch` file with a valid name, in the projects directory, its folders and the locations, sorted
    /// by path (case-insensitive). Links are listed only when they point inside the same root.
    pub fn list(&self) -> Result<Vec<ProjectInfo>, EngineError> {
        let mut out = Vec::new();
        self.walk_all(&mut out, &mut Vec::new())?;
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    /// The names (as spelled on disk) of the `<name>.<ext>` files in `root` whose name equals `name` ignoring case,
    /// sorted. Only entries with a valid name count; links are not followed here.
    fn case_matches_in(root: &Path, name: &str, ext: &str) -> Result<Vec<String>, EngineError> {
        let suffix = format!(".{ext}");
        let entries = fs::read_dir(root).map_err(|e| io_err("list", root, e))?;
        let mut out: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|f| f.strip_suffix(&suffix)).map(str::to_owned))
            .filter(|stem| validate_name(stem).is_ok() && same_name_ignoring_case(stem, name))
            .collect();
        out.sort();
        Ok(out)
    }

    /// The existing `<stem>.<ext>` files that writing `<name>.<ext>` replaces or clashes with: the exact name when it
    /// exists, else every file whose name differs only in case (on Windows and macOS that is the same file; on
    /// Linux [`ProjectDir::write`] replaces a single one too, so a project copied to a PC never clashes).
    pub fn replaced_by(&self, name: &str, ext: &str) -> Result<Vec<String>, EngineError> {
        validate_project_name(name)?;
        let Some(place) = self.place(name, true, false)? else { return Ok(Vec::new()) };
        let Some(dir) = Self::folder_of(&place, false)? else { return Ok(Vec::new()) };
        let matches = Self::case_matches_in(&dir, place.stem, ext)?;
        let prefix = Self::disk_prefix(&place, &dir);
        if matches.iter().any(|m| m == place.stem) {
            return Ok(vec![format!("{prefix}{}", place.stem)]);
        }
        Ok(matches.into_iter().map(|m| format!("{prefix}{m}")).collect())
    }

    /// The project `name` refers to, as spelled on disk: the exact name, else the only one that differs just in
    /// case. Several case variants and no exact match (possible on Linux) is a 409; none is a 404.
    pub fn resolve_name(&self, name: &str) -> Result<String, EngineError> {
        validate_project_name(name)?;
        let missing = || EngineError::NotFound(format!("no project named \"{name}\""));
        let place = self.place(name, true, false)?.ok_or_else(missing)?;
        let dir = Self::folder_of(&place, false)?.ok_or_else(missing)?;
        let matches = Self::case_matches_in(&dir, place.stem, PROJECT_EXT)?;
        // The folders as spelled on disk (and a location's label as configured).
        let prefix = Self::disk_prefix(&place, &dir);
        if matches.iter().any(|m| m == place.stem) {
            return Ok(format!("{prefix}{}", place.stem));
        }
        match matches.as_slice() {
            [] => Err(missing()),
            [only] => Ok(format!("{prefix}{only}")),
            many => Err(EngineError::Conflict(format!(
                "several projects match \"{name}\" ignoring case ({}); use the exact name",
                many.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    /// Read project `name` (found as [`ProjectDir::resolve_name`] does) as text. Returns the name as spelled on disk
    /// and the content.
    pub fn read(&self, name: &str) -> Result<(String, String), EngineError> {
        let name = self.resolve_name(name)?;
        let missing = || EngineError::NotFound(format!("no project named \"{name}\""));
        let place = self.place(&name, true, false)?.ok_or_else(missing)?;
        Self::folder_of(&place, false)?.ok_or_else(missing)?;
        let path = Self::resolve_existing(&place.root, &place.rel(&format!("{}.{PROJECT_EXT}", place.stem)))?
            .ok_or_else(missing)?;
        let len = fs::metadata(&path).map_err(|e| io_err("read", &path, e))?.len();
        if len > MAX_FILE_BYTES as u64 {
            return Err(EngineError::TooLarge(format!(
                "project \"{name}\" is larger than {} MB",
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        let bytes = fs::read(&path).map_err(|e| io_err("read", &path, e))?;
        let text = String::from_utf8(bytes).map_err(|_| {
            EngineError::BadRequest(format!("project \"{name}\" is not UTF-8 text; it is not an OpenEtch file"))
        })?;
        Ok((name, text))
    }

    /// Write `content` to `<name>.<ext>` (creating the directory), replacing any file of that name. Written to a
    /// fresh temporary file first and renamed over the old one, so a failed write never leaves a half file. When no
    /// file has exactly this name but one differs only in case, that one is replaced (it is moved aside first and
    /// restored if the write fails), so the file ends up spelled `name` on every OS. Returns the path written.
    pub fn write(&self, name: &str, ext: &str, content: &str) -> Result<PathBuf, EngineError> {
        validate_project_name(name)?;
        let place = self.place(name, true, true)?.ok_or_else(|| EngineError::Internal("no projects directory".into()))?;
        let root = Self::folder_of(&place, true)?.ok_or_else(|| EngineError::Internal("no projects folder".into()))?;
        let name = place.stem;
        let file = format!("{name}.{ext}");
        // Refuses a link that points outside (or nowhere) before anything is written.
        Self::resolve_existing(&place.root, &place.rel(&file))?;
        let target = root.join(&file);
        let matches = Self::case_matches_in(&root, name, ext)?;
        let variant = match matches.as_slice() {
            [only] if only != name => Some(format!("{only}.{ext}")),
            _ => None,
        };
        if let Some(v) = &variant {
            Self::resolve_existing(&place.root, &place.rel(v))?;
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let tmp = root.join(format!(".{file}.{}-{n}.tmp", std::process::id()));
        let aside = root.join(format!(".{file}.{}-{n}.old.tmp", std::process::id()));
        let written = (|| {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            drop(f);
            if let Some(v) = &variant {
                fs::rename(root.join(v), &aside)?;
            }
            fs::rename(&tmp, &target).inspect_err(|_| {
                if let Some(v) = &variant {
                    let _ = fs::rename(&aside, root.join(v));
                }
            })
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(io_err("write", &target, e));
        }
        if variant.is_some() {
            let _ = fs::remove_file(&aside);
        }
        Ok(target)
    }
}

/// The current project behind `GET /api/project`. `clean` is the revision the document last matched a file (or the
/// empty document) at; the document is dirty whenever its revision differs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectState {
    /// The project the document was opened from or last saved as.
    pub name: Option<String>,
    /// The revision last saved to or opened from `name`.
    pub saved_revision: Option<u64>,
    /// The revision with nothing unsaved; `None` after an edit no file holds.
    pub clean_revision: Option<u64>,
    /// The name of the file a replacing upload or LightBurn import came from (`name` of the upload or of
    /// `doc.import_lbrn2`), used for download file names while no project holds the document.
    pub display_name: Option<String>,
}

impl ProjectState {
    /// No project, document clean at `revision` (startup).
    pub fn clean_at(revision: u64) -> Self {
        ProjectState { name: None, saved_revision: None, clean_revision: Some(revision), display_name: None }
    }

    /// Saved as or opened from `name` at `revision`.
    pub fn saved(name: String, revision: u64) -> Self {
        ProjectState { name: Some(name), saved_revision: Some(revision), clean_revision: Some(revision), display_name: None }
    }

    pub fn dirty(&self, revision: u64) -> bool {
        self.clean_revision != Some(revision)
    }
}

/// A file name as a display name: control characters dropped, a known extension (`.lbrn2`, `.lbrn`, `.lbt`,
/// `.openetch`) removed, trimmed, at most 200 characters; `None` when nothing is left.
pub fn display_name(raw: &str) -> Option<String> {
    let clean: String = raw.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\')).collect();
    let mut name = clean.trim();
    for ext in [".lbrn2", ".lbrn", ".lbt", ".openetch"] {
        let cut = name.len().checked_sub(ext.len()).filter(|&i| name.is_char_boundary(i));
        if let Some(i) = cut.filter(|&i| name[i..].eq_ignore_ascii_case(ext)) {
            name = name[..i].trim_end();
            break;
        }
    }
    let name: String = name.chars().take(200).collect();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pin::temp_dir;

    #[test]
    fn names() {
        for ok in ["Coaster", "Coaster v2 (final)", "a", "x.y", "Mug_wrap-3", "Namensschild Müller", &"n".repeat(100)] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../x",
            "a/b",
            "a\\b",
            "..",
            ".",
            ".hidden",
            "C:\\x",
            "C:x",
            "x..y",
            "x\0y",
            "tab\there",
            " lead",
            "trail ",
            "con",
            "NUL.backup",
            "CON",
            "Aux.openetch",
            "prn.x.y",
            "nul .txt",
            "COM0",
            "lpt0.bak",
            "com9",
            "COM\u{b9}",
            "com\u{b2}.x",
            "Com\u{b3}",
            "LPT\u{b9}",
            "lpt\u{b2}",
            "lpt\u{b3}.openetch",
            "a*b",
            "a?b",
            "a\"b",
            "a<b>",
            "a|b",
            &"n".repeat(101),
        ] {
            let e = validate_name(bad).expect_err(bad);
            assert!(matches!(e, EngineError::BadRequest(_)), "{bad}: {e}");
        }
        assert!(validate_name("../x").unwrap_err().to_string().contains("'/'"));
        assert!(validate_name("x..y").unwrap_err().to_string().contains(".."));
        // Only the device names themselves: longer names that start with one are fine.
        for ok in ["console", "com10", "lpt\u{b9}0", "nullable", "auxiliary (2)", "com\u{2074}", "my con"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        assert!(validate_name("COM\u{b9}").unwrap_err().to_string().contains("reserved device name"));
    }

    #[test]
    fn names_differing_in_case_are_one_project() {
        let dir = ProjectDir::new(temp_dir("projects-case").join("p"));
        dir.write("ok name (1)", PROJECT_EXT, "lower").unwrap();
        // Writing a differently-cased name replaces the file and spells it the new way, on every OS.
        assert_eq!(dir.replaced_by("OK NAME (1)", PROJECT_EXT).unwrap(), ["ok name (1)"]);
        assert_eq!(dir.resolve_name("OK Name (1)").unwrap(), "ok name (1)");
        assert_eq!(dir.read("OK NAME (1)").unwrap(), ("ok name (1)".into(), "lower".into()));
        dir.write("OK NAME (1)", PROJECT_EXT, "upper").unwrap();
        let names: Vec<String> = dir.list().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["OK NAME (1)"]);
        assert_eq!(dir.read("ok name (1)").unwrap(), ("OK NAME (1)".into(), "upper".into()));
        assert!(matches!(dir.resolve_name("other"), Err(EngineError::NotFound(_))));
        // The extension counts: a .lbrn2 of that name is a different file.
        assert_eq!(dir.replaced_by("ok name (1)", LBRN2_EXT).unwrap(), Vec::<String>::new());
        dir.write("ok name (1)", LBRN2_EXT, "<x/>").unwrap();
        assert_eq!(dir.replaced_by("OK NAME (1)", LBRN2_EXT).unwrap(), ["ok name (1)"]);
        assert_eq!(dir.read("OK NAME (1)").unwrap().1, "upper");
        let leftovers: Vec<_> = fs::read_dir(dir.root())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "the replaced file is removed");

        // Two files differing only in case (Linux): an open by another spelling is ambiguous; the exact name works.
        fs::write(dir.root().join("Twin.openetch"), "a").unwrap();
        if fs::write(dir.root().join("TWIN.openetch"), "b").is_ok() && dir.case_count("twin") == 2 {
            assert!(matches!(dir.resolve_name("twin"), Err(EngineError::Conflict(_))));
            assert_eq!(dir.read("TWIN").unwrap().1, "b");
            assert_eq!(dir.replaced_by("Twin", PROJECT_EXT).unwrap(), ["Twin"]);
            assert_eq!(dir.replaced_by("twin", PROJECT_EXT).unwrap(), ["TWIN", "Twin"]);
        } else {
            println!("note: the file system ignores case; skipping the ambiguous-name checks");
            assert_eq!(dir.read("twin").unwrap().1, "b");
        }
        let _ = fs::remove_dir_all(dir.root().parent().unwrap());
    }

    impl ProjectDir {
        /// How many `.openetch` files match `name` ignoring case.
        fn case_count(&self, name: &str) -> usize {
            Self::case_matches_in(self.root(), name, PROJECT_EXT).unwrap().len()
        }
    }

    #[test]
    fn write_list_read_round_trip() {
        let dir = ProjectDir::new(temp_dir("projects").join("nested"));
        assert_eq!(dir.list().unwrap(), vec![], "a missing directory lists nothing");
        assert_eq!(dir.replaced_by("a", PROJECT_EXT).unwrap(), Vec::<String>::new());
        assert!(matches!(dir.read("a"), Err(EngineError::NotFound(_))));

        let p = dir.write("b project", PROJECT_EXT, "{\"x\":1}").unwrap();
        assert!(p.ends_with("b project.openetch"));
        dir.write("A", PROJECT_EXT, "first").unwrap();
        dir.write("A", PROJECT_EXT, "second").unwrap();
        dir.write("A", LBRN2_EXT, "<xml/>").unwrap();
        // Files that are not valid projects are not listed.
        fs::write(dir.root().join(".hidden.openetch"), "x").unwrap();
        fs::write(dir.root().join("notes.txt"), "x").unwrap();
        fs::create_dir(dir.root().join("folder.openetch")).unwrap();

        let list = dir.list().unwrap();
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "b project"]);
        assert_eq!(list[0].size, 6);
        assert!(list[0].modified_unix_s > 1_600_000_000);
        assert_eq!(dir.read("A").unwrap(), ("A".into(), "second".into()));
        assert_eq!(dir.replaced_by("b project", PROJECT_EXT).unwrap(), ["b project"]);
        assert!(matches!(dir.read("../A"), Err(EngineError::BadRequest(_))));
        assert!(matches!(dir.read("folder"), Err(EngineError::Conflict(_))));
        let leftovers: Vec<_> = fs::read_dir(dir.root())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temporary files are renamed away");
        fs::write(dir.root().join("bad.openetch"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(matches!(dir.read("bad"), Err(EngineError::BadRequest(_))));
        let _ = fs::remove_dir_all(dir.root().parent().unwrap());
    }

    /// Make a file symlink; `false` when the OS refuses (Windows without developer mode).
    fn symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link).is_ok();
        #[cfg(windows)]
        return std::os::windows::fs::symlink_file(target, link).is_ok();
        #[cfg(not(any(unix, windows)))]
        return false;
    }

    #[test]
    fn links_out_of_the_directory_are_refused() {
        let base = temp_dir("projects-links");
        let dir = ProjectDir::new(base.join("projects"));
        dir.write("inside", PROJECT_EXT, "in").unwrap();
        fs::create_dir_all(base.join("elsewhere")).unwrap();
        let secret = base.join("elsewhere").join("secret.txt");
        fs::write(&secret, "secret").unwrap();
        // The containment check itself, for paths that resolve outside (works without symlink support).
        let root = fs::canonicalize(dir.root()).unwrap();
        let e = ProjectDir::resolve_existing(&root, "../elsewhere/secret.txt").unwrap_err();
        assert!(matches!(e, EngineError::Forbidden(_)), "{e}");
        assert_eq!(ProjectDir::resolve_existing(&root, "inside.openetch").unwrap(), Some(root.join("inside.openetch")));
        if !symlink(&secret, &dir.root().join("leak.openetch")) {
            println!("note: cannot create symlinks here; skipping the link checks");
            let _ = fs::remove_dir_all(&base);
            return;
        }
        assert!(matches!(dir.read("leak"), Err(EngineError::Forbidden(_))));
        assert!(matches!(dir.write("leak", PROJECT_EXT, "overwrite"), Err(EngineError::Forbidden(_))));
        assert_eq!(fs::read_to_string(&secret).unwrap(), "secret", "nothing was written through the link");
        // A dangling link is refused too (writing through it would create the file outside).
        assert!(symlink(&base.join("elsewhere").join("new.txt"), &dir.root().join("dangling.openetch")));
        assert!(matches!(dir.write("dangling", PROJECT_EXT, "x"), Err(EngineError::Forbidden(_))));
        assert!(!base.join("elsewhere").join("new.txt").exists());
        // A link to a project inside the directory is fine.
        assert!(symlink(&dir.root().join("inside.openetch"), &dir.root().join("alias.openetch")));
        assert_eq!(dir.read("alias").unwrap().1, "in");
        let names: Vec<String> = dir.list().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alias", "inside"]);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn projects_live_in_folders_and_locations() {
        let base = temp_dir("projects-folders");
        let dir = ProjectDir::new(base.join("projects"));
        for ok in ["a/b", "Clients/Acme/badge (2)", "1/2/3/4/name"] {
            assert!(validate_project_name(ok).is_ok(), "{ok}");
        }
        for bad in ["a//b", "/a", "a/", "1/2/3/4/5/name", "a/../b", "templates/x", "Art-Libraries/x", "a\\b", ".x/y"] {
            assert!(matches!(validate_project_name(bad), Err(EngineError::BadRequest(_))), "{bad}");
        }
        // A project may be called "templates"; only the folder is OpenEtch's.
        assert!(validate_project_name("templates").is_ok() && validate_folder("templates").is_err());

        dir.write("top", PROJECT_EXT, "t").unwrap();
        let p = dir.write("Clients/Acme/badge", PROJECT_EXT, "b").unwrap();
        assert!(p.ends_with(Path::new("Clients").join("Acme").join("badge.openetch")), "{}", p.display());
        dir.create_folder("Empty/Inner").unwrap();
        fs::create_dir_all(dir.root().join("templates")).unwrap();
        fs::write(dir.root().join("templates").join("t.openetch"), "x").unwrap();
        // Folders as spelled on disk (a case-insensitive file system finds them by another spelling).
        if let Ok((n, text)) = dir.read("clients/acme/BADGE") {
            assert_eq!((n.as_str(), text.as_str()), ("Clients/Acme/badge", "b"));
        }
        assert_eq!(dir.read("Clients/Acme/BADGE").unwrap(), ("Clients/Acme/badge".into(), "b".into()));
        assert_eq!(dir.replaced_by("Clients/Acme/Badge", PROJECT_EXT).unwrap(), ["Clients/Acme/badge"]);
        assert!(matches!(dir.read("Clients/badge"), Err(EngineError::NotFound(_))));
        assert!(matches!(dir.read("Nowhere/badge"), Err(EngineError::NotFound(_))));
        let list = dir.list().unwrap();
        let names: Vec<(&str, &str)> = list.iter().map(|p| (p.name.as_str(), p.folder.as_str())).collect();
        assert_eq!(names, [("Clients/Acme/badge", "Clients/Acme"), ("top", "")], "templates are not projects");
        assert_eq!(dir.folders().unwrap(), ["Clients", "Clients/Acme", "Empty", "Empty/Inner"]);

        // A location: its label is a top folder, its files are in its own directory.
        let nas = base.join("nas");
        fs::create_dir_all(nas.join("Jobs")).unwrap();
        fs::write(nas.join("Jobs").join("coaster.openetch"), "c").unwrap();
        let loc = |label: &str, path: &Path| Location { label: label.into(), path: path.to_path_buf(), source: "settings".into() };
        assert!(validate_locations(&[loc("NAS", &nas)]).is_ok());
        for bad in [vec![loc("NAS", &nas), loc("nas", &nas)], vec![loc("templates", &nas)], vec![loc("a/b", &nas)], vec![loc("X", Path::new("relative"))], vec![loc("X", &base.join("missing"))]] {
            assert!(validate_locations(&bad).is_err());
        }
        dir.set_locations(vec![loc("NAS", &nas)]);
        assert_eq!(dir.read("nas/Jobs/coaster").unwrap(), ("NAS/Jobs/coaster".into(), "c".into()));
        dir.write("NAS/new", PROJECT_EXT, "n").unwrap();
        assert_eq!(fs::read_to_string(nas.join("new.openetch")).unwrap(), "n");
        let names: Vec<String> = dir.list().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Clients/Acme/badge", "NAS/Jobs/coaster", "NAS/new", "top"]);
        assert!(dir.folders().unwrap().contains(&"NAS/Jobs".to_owned()));
        // A location that is gone: listed as a folder with nothing in it, opening from it says why.
        dir.set_locations(vec![loc("Gone", &base.join("gone"))]);
        assert!(matches!(dir.read("Gone/x"), Err(EngineError::NotFound(_))));
        assert!(dir.write("Gone/x", PROJECT_EXT, "x").is_err());
        assert!(!base.join("gone").exists(), "a missing location is never created");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn project_state_dirty() {
        let s = ProjectState::clean_at(0);
        assert!(!s.dirty(0) && s.dirty(1));
        let s = ProjectState::saved("x".into(), 4);
        assert!(!s.dirty(4) && s.dirty(5));
        assert!(ProjectState { clean_revision: None, ..s }.dirty(4));
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("Coaster.lbrn2").as_deref(), Some("Coaster"));
        assert_eq!(display_name(" Mug wrap.LBRN2 ").as_deref(), Some("Mug wrap"));
        assert_eq!(display_name("tag.openetch").as_deref(), Some("tag"));
        assert_eq!(display_name("plain").as_deref(), Some("plain"));
        assert_eq!(display_name("a/b\\c\n.lbt").as_deref(), Some("abc"));
        assert_eq!(display_name(".lbrn2"), None);
        assert_eq!(display_name("  "), None);
        assert_eq!(display_name("Namensschild Müller.lbrn2").as_deref(), Some("Namensschild Müller"));
        assert_eq!(display_name(&"x".repeat(300)).unwrap().len(), 200);
    }
}
