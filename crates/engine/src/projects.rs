//! Named projects on disk: `<projects-dir>/<name>.openetch` (and LightBurn exports as `<name>.lbrn2`), plus the
//! project-name rules and the "current project" bookkeeping behind `GET /api/project`.
//!
//! Path safety: a name is 1-100 characters of letters, digits, space, `-`, `_`, `.`, `(` and `)`, so it can never
//! hold a path separator, a drive letter or `..`. On top of that every file is resolved against the canonical
//! projects directory and refused unless it stays inside it, so a symbolic link in the directory that points
//! elsewhere is neither read nor written through.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Windows device names, refused as names (also with an extension, e.g. `con.openetch`) on every OS so a project
/// saved on a Pi can be copied to a PC.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2",
    "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

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
    pub name: String,
    /// Last modification, Unix seconds (0 when the OS does not report it).
    pub modified_unix_s: u64,
    /// File size in bytes.
    pub size: u64,
}

/// The projects directory. Created on the first save; listing a missing directory gives no projects.
#[derive(Debug, Clone)]
pub struct ProjectDir {
    root: PathBuf,
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
        ProjectDir { root: root.into() }
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

    /// Every `*.openetch` file with a valid name, sorted by name (case-insensitive). Links are listed only when
    /// they point to a file inside the directory.
    pub fn list(&self) -> Result<Vec<ProjectInfo>, EngineError> {
        let Some(root) = self.canonical_root(false)? else { return Ok(Vec::new()) };
        let entries = fs::read_dir(&root).map_err(|e| io_err("list", &root, e))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str().and_then(|f| f.strip_suffix(&format!(".{PROJECT_EXT}"))) else {
                continue;
            };
            if validate_name(name).is_err() {
                continue;
            }
            let Ok(Some(real)) = Self::resolve_existing(&root, &format!("{name}.{PROJECT_EXT}")) else { continue };
            let Ok(meta) = fs::metadata(&real) else { continue };
            out.push(ProjectInfo { name: name.to_owned(), modified_unix_s: unix_s(&meta), size: meta.len() });
        }
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
        Ok(out)
    }

    /// Whether project `name` exists (as a usable file inside the directory).
    pub fn exists(&self, name: &str) -> Result<bool, EngineError> {
        validate_name(name)?;
        let Some(root) = self.canonical_root(false)? else { return Ok(false) };
        Ok(Self::resolve_existing(&root, &format!("{name}.{PROJECT_EXT}"))?.is_some())
    }

    /// Read project `name` as text.
    pub fn read(&self, name: &str) -> Result<String, EngineError> {
        validate_name(name)?;
        let missing = || EngineError::NotFound(format!("no project named \"{name}\""));
        let root = self.canonical_root(false)?.ok_or_else(missing)?;
        let path = Self::resolve_existing(&root, &format!("{name}.{PROJECT_EXT}"))?.ok_or_else(missing)?;
        let len = fs::metadata(&path).map_err(|e| io_err("read", &path, e))?.len();
        if len > MAX_FILE_BYTES as u64 {
            return Err(EngineError::TooLarge(format!(
                "project \"{name}\" is larger than {} MB",
                MAX_FILE_BYTES / (1024 * 1024)
            )));
        }
        let bytes = fs::read(&path).map_err(|e| io_err("read", &path, e))?;
        String::from_utf8(bytes).map_err(|_| {
            EngineError::BadRequest(format!("project \"{name}\" is not UTF-8 text; it is not an OpenEtch file"))
        })
    }

    /// Write `content` to `<name>.<ext>` (creating the directory), replacing any file of that name. Written to a
    /// fresh temporary file first and renamed over the old one, so a failed write never leaves a half file.
    /// Returns the path written.
    pub fn write(&self, name: &str, ext: &str, content: &str) -> Result<PathBuf, EngineError> {
        validate_name(name)?;
        let root = self.canonical_root(true)?.ok_or_else(|| EngineError::Internal("no projects directory".into()))?;
        let file = format!("{name}.{ext}");
        // Refuses a link that points outside (or nowhere) before anything is written.
        Self::resolve_existing(&root, &file)?;
        let target = root.join(&file);
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let tmp = root.join(format!(".{file}.{}-{n}.tmp", std::process::id()));
        let written = (|| {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &target)
        })();
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(io_err("write", &target, e));
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
}

impl ProjectState {
    /// No project, document clean at `revision` (startup).
    pub fn clean_at(revision: u64) -> Self {
        ProjectState { name: None, saved_revision: None, clean_revision: Some(revision) }
    }

    /// Saved as or opened from `name` at `revision`.
    pub fn saved(name: String, revision: u64) -> Self {
        ProjectState { name: Some(name), saved_revision: Some(revision), clean_revision: Some(revision) }
    }

    pub fn dirty(&self, revision: u64) -> bool {
        self.clean_revision != Some(revision)
    }
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
    }

    #[test]
    fn write_list_read_round_trip() {
        let dir = ProjectDir::new(temp_dir("projects").join("nested"));
        assert_eq!(dir.list().unwrap(), vec![], "a missing directory lists nothing");
        assert!(!dir.exists("a").unwrap());
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
        assert_eq!(dir.read("A").unwrap(), "second");
        assert!(dir.exists("b project").unwrap());
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
        assert_eq!(dir.read("alias").unwrap(), "in");
        let names: Vec<String> = dir.list().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alias", "inside"]);
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
}
