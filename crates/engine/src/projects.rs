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
        validate_name(name)?;
        let Some(root) = self.canonical_root(false)? else { return Ok(Vec::new()) };
        let matches = Self::case_matches_in(&root, name, ext)?;
        if matches.iter().any(|m| m == name) {
            return Ok(vec![name.to_owned()]);
        }
        Ok(matches)
    }

    /// The project `name` refers to, as spelled on disk: the exact name, else the only one that differs just in
    /// case. Several case variants and no exact match (possible on Linux) is a 409; none is a 404.
    pub fn resolve_name(&self, name: &str) -> Result<String, EngineError> {
        validate_name(name)?;
        let missing = || EngineError::NotFound(format!("no project named \"{name}\""));
        let root = self.canonical_root(false)?.ok_or_else(missing)?;
        let matches = Self::case_matches_in(&root, name, PROJECT_EXT)?;
        if matches.iter().any(|m| m == name) {
            return Ok(name.to_owned());
        }
        match matches.as_slice() {
            [] => Err(missing()),
            [only] => Ok(only.clone()),
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
        validate_name(name)?;
        let root = self.canonical_root(true)?.ok_or_else(|| EngineError::Internal("no projects directory".into()))?;
        let file = format!("{name}.{ext}");
        // Refuses a link that points outside (or nowhere) before anything is written.
        Self::resolve_existing(&root, &file)?;
        let target = root.join(&file);
        let matches = Self::case_matches_in(&root, name, ext)?;
        let variant = match matches.as_slice() {
            [only] if only != name => Some(format!("{only}.{ext}")),
            _ => None,
        };
        if let Some(v) = &variant {
            Self::resolve_existing(&root, v)?;
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
