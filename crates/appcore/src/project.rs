//! Project names and the "current project" bookkeeping behind `GET /api/project` (moved from the engine's
//! `projects` module, which keeps the files on disk).
//!
//! Path safety: every name and folder is 1-100 characters of letters, digits, space, `-`, `_`, `.`, `(` and `)`, so
//! it can never hold a path separator, a drive letter or `..`.

use crate::error::AppError;

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
pub fn validate_name(name: &str) -> Result<(), AppError> {
    let bad = |why: &str| {
        Err(AppError::BadRequest(format!(
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
pub fn validate_project_name(name: &str) -> Result<(), AppError> {
    check_path(name, "project name")
}

/// Check a folder path (`""` is the top of the projects directory): like [`validate_project_name`].
pub fn validate_folder(path: &str) -> Result<(), AppError> {
    if path.is_empty() { Ok(()) } else { check_path(path, "folder") }
}

fn check_path(path: &str, what: &str) -> Result<(), AppError> {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() > MAX_FOLDER_DEPTH + 1 {
        return Err(AppError::BadRequest(format!("invalid {what} {path:?}: at most {MAX_FOLDER_DEPTH} folders deep")));
    }
    for s in &segs {
        validate_name(s)?;
    }
    let top_folder = if what == "folder" { segs.first() } else { segs.split_last().and_then(|(_, f)| f.first()) };
    if let Some(top) = top_folder.filter(|t| INTERNAL_DIRS.iter().any(|d| d.eq_ignore_ascii_case(t))) {
        return Err(AppError::BadRequest(format!("invalid {what} {path:?}: \"{top}\" holds OpenEtch's own files, not projects")));
    }
    Ok(())
}

/// A project path as one flat file name (`Clients/Acme/badge` → `Clients__Acme__badge`), for autosaves and backups,
/// which live in one folder.
pub fn flat_name(name: &str) -> String {
    name.replace('/', "__")
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

    /// `GET /api/project` for the document at `revision`: `{name, display_name, saved_revision, revision, dirty}`.
    pub fn status(&self, revision: u64) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "display_name": self.display_name,
            "saved_revision": self.saved_revision,
            "revision": revision,
            "dirty": self.dirty(revision),
        })
    }

    /// The name downloads get: the project, else the opened file, else `document_name`.
    pub fn stem(&self, document_name: &str) -> String {
        self.name.clone().or_else(|| self.display_name.clone()).unwrap_or_else(|| document_name.to_owned())
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
