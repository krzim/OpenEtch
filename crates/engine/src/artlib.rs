//! The Art Library (LightBurn's Art Library window, docs "Art Library"): libraries of reusable artwork, shared by
//! every client of the engine because they live in the projects directory, `<projects-dir>/art-libraries/<name>.oeart`.
//! An item is an Art Library fragment ([`commands::ArtFragment`]: objects, their groups, their layers' names and
//! colours; no cut settings, as in LightBurn) with a name and a 128 px PNG thumbnail rendered when it is added.
//!
//! **LightBurn's `.lbart`** (decoded from libraries saved by the owner's LightBurn 2.2, see `fileio::lbart`): a
//! binary container of items, each a name, a 256 px PNG thumbnail, its size and its art as LightBurn shapes XML
//! (`lbrn::shapes_to_document`). Load reads one into a new library; Import adds its items to the current one;
//! Download `?format=lbart` writes the library as one. Items keep LightBurn's thumbnail, size and compressed art
//! (`lbart_art_base64`), so a loaded `.lbart` downloads again byte for byte; art is placed in a fixed
//! [`LBART_FIELD_MM`] field (LightBurn inserts art where it is dropped, so its position does not matter).
//! OpenEtch's own `.oeart` is UTF-8 JSON `{format: "openetch-art-library", version: 1, next_id, items: [{id, name,
//! created_unix_s, size_mm: [w, h], thumbnail_png_base64, lbart_art_base64?, content: ArtFragment}]}`.
//!
//! Routes (under `/api`; library names follow the project name rules, see `projects::validate_name`):
//!
//! | Method | Path | |
//! |---|---|---|
//! | GET | `/art/libraries` | `{libraries: [{name, items, modified_unix_s, size}], formats: ["oeart", "lbart"]}` |
//! | POST | `/art/libraries` | `{name}` → **201** `{ok, name}` (New; an existing name, also by case, 409) |
//! | POST | `/art/libraries/load` | `?name=<file name>&overwrite=` with an `.oeart` or `.lbart` as the body → **201** `{ok, name, items, warnings}` (Load) |
//! | GET | `/art/libraries/{name}` | `{name, items: [{id, name, created_unix_s, size_mm, objects, thumbnail}]}` (`thumbnail` the PNG's URL) |
//! | GET | `/art/libraries/{name}/download` | the `.oeart` file; `?format=lbart` LightBurn's `.lbart` |
//! | POST | `/art/libraries/{name}/rename` | `{to}` → `{ok, name}` |
//! | DELETE | `/art/libraries/{name}` | deletes the library file → `{ok}` |
//! | POST | `/art/libraries/{name}/items` | `{ids, name?}`: Add Selection to Library from the active document → **201** `{ok, item}` |
//! | POST | `/art/libraries/{name}/import` | `?format=svg\|dxf\|image\|lbrn2\|openetch\|...&name=` with the file as the body: Import (each file is one item) → **201** `{ok, item}`; `format=lbart` adds every item of a LightBurn library → **201** `{ok, item, items, warnings}` |
//! | GET | `/art/libraries/{name}/items/{id}/thumbnail.png` | the thumbnail |
//! | PUT | `/art/libraries/{name}/items/{id}` | `{name}` → `{ok, item}` (Rename) |
//! | DELETE | `/art/libraries/{name}/items/{id}` | `{ok}` (Delete) |
//! | POST | `/art/libraries/{name}/items/{id}/copy` | `{to, move?}` → `{ok, item}`: drag to another library (copy; `move` like Shift-drag) |
//! | POST | `/art/libraries/{name}/items/{id}/insert` | `{center?, layer?, origin?}` → as `/api/execute`: `art.insert` into the active document (Add Graphic to Project; a drag passes the drop point as `center`) |
//!
//! Every change emits `art_library_changed {library}` (`null` when the list of libraries changed). File access is
//! serialised by [`ArtStore`]'s lock; writes go through a temporary file renamed into place.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::{ArtFragment, Invocation, Session, art_fragment};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::projects::{self, ProjectDir};
use crate::state::{lock, parse_origin};
use crate::{AppState, EngineError, Event};

/// The folder in the projects directory.
pub const ART_DIR: &str = "art-libraries";
/// OpenEtch's library file extension.
pub const ART_EXT: &str = "oeart";
/// The format tag of a library file.
pub const LIBRARY_FORMAT: &str = "openetch-art-library";
/// Thumbnail width in pixels.
pub const THUMBNAIL_PX: u32 = 128;
/// Largest library file.
pub const MAX_LIBRARY_BYTES: usize = projects::MAX_FILE_BYTES;
/// Most items in one library.
pub const MAX_ITEMS: usize = 2000;
/// The first bytes of every PNG file.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// LightBurn's thumbnail width (`.lbart` downloads render thumbnails this size when an item has none of it).
pub const LBART_THUMBNAIL_PX: u32 = 256;
/// The field `.lbart` art is placed in (LightBurn's coordinates are mm from its lower-left corner).
pub const LBART_FIELD_MM: f64 = 110.0;

/// Serialises the library files (see the module docs) and keeps the last libraries read, parsed: a library window
/// asks for every item's thumbnail, and reading and checking the whole file again for each of 2000 items would take
/// minutes.
#[derive(Debug, Default)]
pub struct ArtStore {
    lock: Mutex<()>,
    parsed: Mutex<HashMap<PathBuf, Parsed>>,
}

/// Libraries kept parsed (see [`ArtStore`]).
const PARSED_KEPT: usize = 4;

/// A library file as last read: its size and modification time, which must still match for it to be used.
#[derive(Debug)]
struct Parsed {
    len: u64,
    modified: Option<std::time::SystemTime>,
    file: Arc<ArtLibraryFile>,
}

/// One item of a library file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtItem {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_unix_s: u64,
    pub size_mm: [f64; 2],
    #[serde(default)]
    pub thumbnail_png_base64: String,
    /// LightBurn's compressed art of an item loaded from an `.lbart` (written again while it still holds the same
    /// shapes XML, so the library downloads byte for byte).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lbart_art_base64: Option<String>,
    pub content: ArtFragment,
}

/// A library file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtLibraryFile {
    pub format: String,
    pub version: u32,
    #[serde(default = "one")]
    pub next_id: u64,
    pub items: Vec<ArtItem>,
}

fn one() -> u64 {
    1
}

impl ArtLibraryFile {
    fn empty() -> Self {
        ArtLibraryFile { format: LIBRARY_FORMAT.into(), version: 1, next_id: 1, items: Vec::new() }
    }

    /// Parse and check a library file's text.
    pub fn parse(text: &str) -> Result<Self, EngineError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let f: ArtLibraryFile = serde_json::from_str(text)
            .map_err(|e| EngineError::BadRequest(format!("not an OpenEtch art library (.{ART_EXT}): {e}")))?;
        if f.format != LIBRARY_FORMAT || f.version != 1 {
            return Err(EngineError::BadRequest(format!(
                "not an OpenEtch art library (format '{}' version {}, expected '{LIBRARY_FORMAT}' version 1)",
                f.format, f.version
            )));
        }
        if f.items.len() > MAX_ITEMS {
            return Err(EngineError::BadRequest(format!("the library has {} items (at most {MAX_ITEMS})", f.items.len())));
        }
        let mut ids = std::collections::HashSet::new();
        for it in &f.items {
            if !ids.insert(it.id.as_str()) {
                return Err(EngineError::BadRequest(format!("the library uses the item id '{}' twice", it.id)));
            }
            // Ids end up in URLs and names in the UI: the rules the engine's own items follow.
            if !item_id_ok(&it.id) {
                let shown: String = it.id.chars().take(40).collect();
                return Err(EngineError::BadRequest(format!("the library has an unusable item id {shown:?}")));
            }
            item_name(&it.name).map_err(|e| EngineError::BadRequest(format!("item '{}': {e}", it.id)))?;
            ArtFragment::from_json(&serde_json::to_string(&it.content).unwrap_or_default())
                .map_err(|e| EngineError::BadRequest(format!("item '{}': {e}", it.name)))?;
        }
        Ok(f)
    }

    fn alloc(&mut self) -> String {
        loop {
            let id = format!("art-{}", self.next_id);
            self.next_id += 1;
            if !self.items.iter().any(|i| i.id == id) {
                return id;
            }
        }
    }

    fn item(&self, id: &str) -> Result<&ArtItem, EngineError> {
        self.items.iter().find(|i| i.id == id).ok_or_else(|| EngineError::NotFound(format!("no art item '{id}'")))
    }
}

/// An item's listing row (`library` names the thumbnail URL).
fn item_row(library: &str, it: &ArtItem) -> Value {
    json!({
        "id": it.id,
        "name": it.name,
        "created_unix_s": it.created_unix_s,
        "size_mm": it.size_mm,
        "objects": it.content.objects.len(),
        "thumbnail": format!("/api/art/libraries/{}/items/{}/thumbnail.png", url_part(library), url_part(&it.id)),
    })
}

/// Percent-encode a path segment (names may hold spaces and parentheses).
fn url_part(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// An SVG of the fragment's outlines (layer colours) fitted to a square, +Y up.
pub fn fragment_svg(f: &ArtFragment) -> String {
    let b = f.bounds().unwrap_or(kurbo::Rect::new(-1.0, -1.0, 1.0, 1.0));
    let side = b.width().max(b.height()).max(0.1);
    let pad = side * 0.06;
    let (cx, cy) = (b.center().x, b.center().y);
    let half = side / 2.0 + pad;
    let stroke = side / 120.0;
    let mut s = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}"><rect x="{}" y="{}" width="{}" height="{}" fill="#ffffff"/><g transform="scale(1 -1)">"##,
        cx - half,
        -cy - half,
        2.0 * half,
        2.0 * half,
        cx - half,
        -cy - half,
        2.0 * half,
        2.0 * half
    );
    for o in &f.objects {
        let color = crate::render::safe_color(f.color_of(o));
        s.push_str(&format!(
            r#"<path d="{}" fill="none" stroke="{color}" stroke-width="{stroke}" stroke-linejoin="round"/>"#,
            o.field_path().to_svg()
        ));
    }
    s.push_str("</g></svg>");
    s
}

/// A new item from a fragment (thumbnail rendered here).
fn make_item(id: String, name: String, content: ArtFragment) -> Result<ArtItem, EngineError> {
    let size = content.bounds().map_or([0.0, 0.0], |b| [round3(b.width()), round3(b.height())]);
    let png = crate::render::svg_to_png(&fragment_svg(&content), THUMBNAIL_PX)?;
    Ok(ArtItem {
        id,
        name,
        created_unix_s: now_unix(),
        size_mm: size,
        thumbnail_png_base64: STANDARD.encode(png),
        lbart_art_base64: None,
        content,
    })
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// An item id: 1-64 ASCII letters, digits, `-` or `_` (the engine allocates `art-N`).
fn item_id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An item name: trimmed, 1-100 printable characters.
fn item_name(name: &str) -> Result<String, EngineError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 100 || n.chars().any(char::is_control) {
        return Err(EngineError::BadRequest("an art item name must be 1-100 printable characters".into()));
    }
    Ok(n.to_owned())
}

impl ArtStore {
    fn dir(st: &AppState) -> PathBuf {
        st.projects.root().join(ART_DIR)
    }

    /// The canonical library folder, `None` when it does not exist yet.
    fn canonical(st: &AppState) -> Option<PathBuf> {
        fs::canonicalize(Self::dir(st)).ok().filter(|p| p.is_dir())
    }

    /// The file of library `name` (as spelled on disk, matched ignoring case), refused when it is a link out of the
    /// folder. 404 when there is none.
    fn locate(st: &AppState, name: &str) -> Result<(String, PathBuf), EngineError> {
        projects::validate_name(name)?;
        let missing = || EngineError::NotFound(format!("no art library named \"{name}\""));
        let root = Self::canonical(st).ok_or_else(missing)?;
        let mut matches: Vec<String> = fs::read_dir(&root)
            .map_err(|e| EngineError::Internal(format!("cannot list {}: {e}", root.display())))?
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|f| f.strip_suffix(&format!(".{ART_EXT}"))).map(str::to_owned))
            .filter(|stem| projects::validate_name(stem).is_ok() && projects::same_name_ignoring_case(stem, name))
            .collect();
        matches.sort();
        let stem = if matches.iter().any(|m| m == name) {
            name.to_owned()
        } else if matches.len() == 1 {
            matches.remove(0)
        } else if matches.is_empty() {
            return Err(missing());
        } else {
            return Err(EngineError::Conflict(format!("several art libraries match \"{name}\" ignoring case; use the exact name")));
        };
        let path = root.join(format!("{stem}.{ART_EXT}"));
        let real = fs::canonicalize(&path).map_err(|_| EngineError::Forbidden(format!("{stem}.{ART_EXT} is a broken link")))?;
        if !real.starts_with(&root) || !real.is_file() {
            return Err(EngineError::Forbidden(format!("{stem}.{ART_EXT} points outside the art library folder; refusing it")));
        }
        Ok((stem, real))
    }

    /// Library `name`, parsed (from [`ArtStore`]'s copy while the file's size and modification time are unchanged).
    fn read(st: &AppState, name: &str) -> Result<(String, Arc<ArtLibraryFile>), EngineError> {
        let (stem, path) = Self::locate(st, name)?;
        let meta = fs::metadata(&path).ok();
        let len = meta.as_ref().map_or(0, fs::Metadata::len);
        let modified = meta.and_then(|m| m.modified().ok());
        if len > MAX_LIBRARY_BYTES as u64 {
            return Err(EngineError::TooLarge(format!("art library \"{stem}\" is larger than {} MB", MAX_LIBRARY_BYTES >> 20)));
        }
        if let Some(p) = lock(&st.art.parsed).get(&path)
            && p.len == len
            && p.modified.is_some()
            && p.modified == modified
        {
            return Ok((stem, p.file.clone()));
        }
        let text = fs::read_to_string(&path).map_err(|e| EngineError::Internal(format!("cannot read {}: {e}", path.display())))?;
        let file = Arc::new(ArtLibraryFile::parse(&text)?);
        let mut cache = lock(&st.art.parsed);
        if cache.len() >= PARSED_KEPT {
            cache.clear();
        }
        cache.insert(path, Parsed { len, modified, file: file.clone() });
        Ok((stem, file))
    }

    /// Forget the parsed copy of every library (after a write, rename or delete).
    fn forget(st: &AppState) {
        lock(&st.art.parsed).clear();
    }

    fn write(st: &AppState, name: &str, file: &ArtLibraryFile) -> Result<(), EngineError> {
        let text = serde_json::to_string(file).map_err(|e| EngineError::Internal(e.to_string()))?;
        if text.len() > MAX_LIBRARY_BYTES {
            return Err(EngineError::TooLarge(format!("the art library would be larger than {} MB", MAX_LIBRARY_BYTES >> 20)));
        }
        Self::forget(st);
        ProjectDir::new(Self::dir(st)).write(name, ART_EXT, &text)?;
        Ok(())
    }

    /// Run `f` on library `name` under the store lock and write it back. Emits `art_library_changed`.
    fn update<T>(st: &AppState, name: &str, f: impl FnOnce(&mut ArtLibraryFile) -> Result<T, EngineError>) -> Result<T, EngineError> {
        let stem = {
            let _g = lock(&st.art.lock);
            let (stem, file) = Self::read(st, name)?;
            let mut file = ArtLibraryFile::clone(&file);
            let out = f(&mut file)?;
            Self::write(st, &stem, &file)?;
            (stem, out)
        };
        st.emit(Event::ArtLibraryChanged { library: Some(stem.0) });
        Ok(stem.1)
    }

    /// `GET /art/libraries`.
    pub fn list(st: &AppState) -> Result<Value, EngineError> {
        let _g = lock(&st.art.lock);
        let mut rows = Vec::new();
        if let Some(root) = Self::canonical(st) {
            let entries = fs::read_dir(&root).map_err(|e| EngineError::Internal(format!("cannot list {}: {e}", root.display())))?;
            for e in entries.flatten() {
                let file_name = e.file_name();
                let Some(stem) = file_name.to_str().and_then(|f| f.strip_suffix(&format!(".{ART_EXT}"))) else { continue };
                if projects::validate_name(stem).is_err() {
                    continue;
                }
                let Ok((stem, file)) = Self::read(st, stem) else { continue };
                let meta = e.metadata().ok();
                let modified = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                rows.push(json!({ "name": stem, "items": file.items.len(), "modified_unix_s": modified, "size": meta.map_or(0, |m| m.len()) }));
            }
        }
        rows.sort_by_key(|r| r["name"].as_str().unwrap_or_default().to_lowercase());
        Ok(json!({ "libraries": rows, "formats": ["oeart", "lbart"] }))
    }

    /// New library `name` (409 when one exists, also by case).
    pub fn create(st: &AppState, name: &str, file: Option<ArtLibraryFile>, overwrite: bool) -> Result<Value, EngineError> {
        projects::validate_name(name)?;
        let file = file.unwrap_or_else(ArtLibraryFile::empty);
        {
            let _g = lock(&st.art.lock);
            match Self::locate(st, name) {
                Ok((existing, _)) if !overwrite => {
                    return Err(EngineError::Conflict(format!("an art library named \"{existing}\" already exists")));
                }
                Ok(_) | Err(EngineError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            Self::write(st, name, &file)?;
        }
        st.emit(Event::ArtLibraryChanged { library: None });
        Ok(json!({ "ok": true, "name": name, "items": file.items.len() }))
    }

    /// `GET /art/libraries/{name}`.
    pub fn view(st: &AppState, name: &str) -> Result<Value, EngineError> {
        let _g = lock(&st.art.lock);
        let (stem, file) = Self::read(st, name)?;
        let items: Vec<Value> = file.items.iter().map(|i| item_row(&stem, i)).collect();
        Ok(json!({ "name": stem, "items": items }))
    }

    /// Add `content` as a new item named `name`. Returns the item's row.
    pub fn add(st: &AppState, library: &str, name: String, content: ArtFragment) -> Result<Value, EngineError> {
        let name = item_name(&name)?;
        Self::update(st, library, |f| {
            if f.items.len() >= MAX_ITEMS {
                return Err(EngineError::Conflict(format!("the library holds {MAX_ITEMS} items; use another one")));
            }
            let id = f.alloc();
            let item = make_item(id, name, content)?;
            f.items.push(item.clone());
            Ok(item)
        })
        .map(|item| item_row(library, &item))
    }

    /// Library `name`, parsed.
    pub fn file(st: &AppState, name: &str) -> Result<Arc<ArtLibraryFile>, EngineError> {
        let _g = lock(&st.art.lock);
        Self::read(st, name).map(|(_, f)| f)
    }

    /// The item (a copy).
    pub fn item(st: &AppState, library: &str, id: &str) -> Result<ArtItem, EngineError> {
        let _g = lock(&st.art.lock);
        let (_, file) = Self::read(st, library)?;
        file.item(id).cloned()
    }

    fn delete_library(st: &AppState, name: &str) -> Result<Value, EngineError> {
        {
            let _g = lock(&st.art.lock);
            let (_, path) = Self::locate(st, name)?;
            Self::forget(st);
            fs::remove_file(&path).map_err(|e| EngineError::Internal(format!("cannot delete {}: {e}", path.display())))?;
        }
        st.emit(Event::ArtLibraryChanged { library: None });
        Ok(json!({ "ok": true }))
    }

    fn rename_library(st: &AppState, name: &str, to: &str) -> Result<Value, EngineError> {
        projects::validate_name(to)?;
        {
            let _g = lock(&st.art.lock);
            let (stem, path) = Self::locate(st, name)?;
            match Self::locate(st, to) {
                Ok((other, _)) if !projects::same_name_ignoring_case(&other, &stem) => {
                    return Err(EngineError::Conflict(format!("an art library named \"{other}\" already exists")));
                }
                Ok(_) | Err(EngineError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            let (_, file) = Self::read(st, &stem)?;
            Self::write(st, to, &file)?;
            if stem != to {
                // A rename that only changes case wrote over the same file on Windows and macOS.
                let renamed = Self::locate(st, to).map(|(_, p)| p);
                if renamed.as_ref().map(|p| p != &path).unwrap_or(true) {
                    Self::forget(st);
                    let _ = fs::remove_file(&path);
                }
            }
        }
        st.emit(Event::ArtLibraryChanged { library: None });
        Ok(json!({ "ok": true, "name": to }))
    }
}

/// The art of an uploaded file (`format` as `POST /api/document/import` takes it, plus `openetch`), read in a scratch
/// session so the active document is untouched. The fragment holds every object of the file.
fn fragment_from_file(st: &AppState, format: &str, bytes: Vec<u8>) -> Result<ArtFragment, EngineError> {
    let mut scratch = Session::new(doc::Document::default());
    if format == "lbart" {
        return Err(EngineError::BadRequest("an .lbart file holds several items: import it as a library".into()));
    }
    if format == "openetch" {
        let text = String::from_utf8(bytes).map_err(|_| EngineError::BadRequest("an .openetch file must be UTF-8".into()))?;
        let d = doc::file::from_openetch_json(text.strip_prefix('\u{feff}').unwrap_or(&text))
            .map_err(|e| EngineError::BadRequest(e.to_string()))?;
        let ids: Vec<String> = d.objects.iter().map(|o| o.id.clone()).collect();
        if ids.is_empty() {
            return Err(EngineError::BadRequest("the file holds no objects".into()));
        }
        return Ok(art_fragment(&d, &ids)?);
    }
    let mut inv = crate::userfiles::import_invocation(format, bytes, &HashMap::new())?;
    if inv.id == "doc.import_lbrn2" {
        inv.params["field_mm"] = json!(st.document().field_mm.unwrap_or_else(|| st.machine.profile().field_mm));
    }
    scratch.execute(inv, commands::Origin::Ui)?;
    let ids: Vec<String> = scratch.document().objects.iter().map(|o| o.id.clone()).collect();
    if ids.is_empty() {
        return Err(EngineError::BadRequest("the file holds no art".into()));
    }
    Ok(art_fragment(scratch.document(), &ids)?)
}

/// The items of a LightBurn `.lbart` file, as library items (ids from `alloc`). Items whose art OpenEtch cannot show
/// are left out with a warning.
pub fn items_from_lbart(bytes: &[u8], mut alloc: impl FnMut() -> String) -> Result<(Vec<ArtItem>, Vec<String>), EngineError> {
    let lib = fileio::lbart::read(bytes).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    if lib.items.len() > MAX_ITEMS {
        return Err(EngineError::BadRequest(format!("the .lbart file has {} items (at most {MAX_ITEMS})", lib.items.len())));
    }
    let frame = lbrn::Frame::Galvo { field_mm: LBART_FIELD_MM };
    let (mut items, mut warnings) = (Vec::new(), Vec::new());
    for (k, it) in lib.items.iter().enumerate() {
        let name = item_name(&it.name).unwrap_or_else(|_| format!("Art {}", k + 1));
        let content = (|| -> Result<ArtFragment, String> {
            let i = lbrn::shapes_to_document(&it.shapes_xml, frame).map_err(|e| e.to_string())?;
            for w in &i.warnings {
                warnings.push(format!("{name}: {w}"));
            }
            let d = doc::Document {
                layers: i.layers,
                objects: i.objects,
                groups: i.groups,
                field_mm: Some(LBART_FIELD_MM),
                ..doc::Document::default()
            };
            let ids: Vec<String> = d.objects.iter().map(|o| o.id.clone()).collect();
            if ids.is_empty() {
                return Err("it holds nothing OpenEtch can show".into());
            }
            let f = art_fragment(&d, &ids).map_err(|e| e.to_string())?;
            ArtFragment::from_json(&serde_json::to_string(&f).unwrap_or_default())
        })();
        let content = match content {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("\"{name}\" was left out: {e}"));
                continue;
            }
        };
        let size = if it.size_mm.iter().all(|v| v.is_finite() && *v >= 0.0) {
            it.size_mm
        } else {
            content.bounds().map_or([0.0, 0.0], |b| [round3(b.width()), round3(b.height())])
        };
        items.push(ArtItem {
            id: alloc(),
            name,
            created_unix_s: now_unix(),
            size_mm: size,
            thumbnail_png_base64: STANDARD.encode(&it.thumbnail_png),
            lbart_art_base64: it.art_block.as_ref().map(|b| STANDARD.encode(b)),
            content,
        });
    }
    Ok((items, warnings))
}

/// A whole `.lbart` file as a new library file.
pub fn library_from_lbart(bytes: &[u8]) -> Result<(ArtLibraryFile, Vec<String>), EngineError> {
    let mut file = ArtLibraryFile::empty();
    let mut probe = ArtLibraryFile::empty();
    let (items, warnings) = items_from_lbart(bytes, || probe.alloc())?;
    file.next_id = probe.next_id;
    file.items = items;
    Ok((file, warnings))
}

/// A PNG's width in pixels (from its header).
fn png_width(png: &[u8]) -> Option<u32> {
    (png.starts_with(PNG_SIGNATURE) && png.len() >= 24).then(|| u32::from_be_bytes([png[16], png[17], png[18], png[19]]))
}

/// A library as LightBurn's `.lbart` (thumbnails LightBurn's size, art as shapes XML). Returns the file and the
/// export's warnings.
pub fn library_to_lbart(file: &ArtLibraryFile) -> Result<(Vec<u8>, Vec<String>), EngineError> {
    let frame = lbrn::Frame::Galvo { field_mm: LBART_FIELD_MM };
    let mut out = fileio::lbart::Lbart::default();
    let mut warnings = Vec::new();
    for it in &file.items {
        let c = &it.content;
        let mut layers: Vec<doc::Layer> = Vec::new();
        for l in &c.layers {
            if !layers.iter().any(|x| x.id == l.id) {
                layers.push(doc::Layer::new(l.id.clone(), &l.name, &l.color));
            }
        }
        for o in &c.objects {
            // An object whose layer the fragment does not list goes on a black one.
            if !layers.iter().any(|x| x.id == o.layer) {
                layers.push(doc::Layer::new(o.layer.clone(), "C00", "#000000"));
            }
        }
        let d = doc::Document {
            layers,
            objects: c.objects.clone(),
            groups: c.groups.clone(),
            field_mm: Some(LBART_FIELD_MM),
            ..doc::Document::default()
        };
        let (xml, w) = lbrn::shapes_from_document(&d, frame).map_err(|e| EngineError::BadRequest(format!("{}: {e}", it.name)))?;
        warnings.extend(w.into_iter().filter(|w| !w.contains("outside the")).map(|w| format!("{}: {w}", it.name)));
        let thumb = STANDARD.decode(it.thumbnail_png_base64.as_bytes()).ok().filter(|p| png_width(p) == Some(LBART_THUMBNAIL_PX));
        let thumb = match thumb {
            Some(t) => t,
            None => crate::render::svg_to_png(&fragment_svg(c), LBART_THUMBNAIL_PX)?,
        };
        let mut item = fileio::lbart::Item::new(it.name.clone(), thumb, xml, it.size_mm);
        item.art_block = it.lbart_art_base64.as_ref().and_then(|b| STANDARD.decode(b.as_bytes()).ok());
        out.items.push(item);
    }
    let bytes = fileio::lbart::write(&out).map_err(|e| EngineError::BadRequest(e.to_string()))?;
    Ok((bytes, warnings))
}

/// The art routes (merged into `/api`).
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/art/libraries", get(libraries).post(library_new))
        .route("/art/libraries/load", post(library_load).layer(axum::extract::DefaultBodyLimit::max(MAX_LIBRARY_BYTES)))
        .route("/art/libraries/{name}", get(library_view).delete(library_delete))
        .route("/art/libraries/{name}/download", get(library_download))
        .route("/art/libraries/{name}/rename", post(library_rename))
        .route("/art/libraries/{name}/items", post(item_add))
        .route(
            "/art/libraries/{name}/import",
            post(item_import).layer(axum::extract::DefaultBodyLimit::max(projects::MAX_FILE_BYTES)),
        )
        .route("/art/libraries/{name}/items/{id}", put(item_rename).delete(item_delete))
        .route("/art/libraries/{name}/items/{id}/thumbnail.png", get(item_thumbnail))
        .route("/art/libraries/{name}/items/{id}/copy", post(item_copy))
        .route("/art/libraries/{name}/items/{id}/insert", post(item_insert))
}

fn json_body<T: for<'de> Deserialize<'de>>(bytes: &Bytes) -> Result<T, EngineError> {
    let b: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) { b"{}" } else { bytes };
    serde_json::from_slice(b).map_err(|e| EngineError::BadRequest(format!("bad JSON body: {e}")))
}

type Q = Query<HashMap<String, String>>;

async fn libraries(State(st): State<Arc<AppState>>) -> Result<axum::Json<Value>, EngineError> {
    Ok(axum::Json(st.blocking(ArtStore::list).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameBody {
    name: String,
}

async fn library_new(State(st): State<Arc<AppState>>, body: Bytes) -> Result<Response, EngineError> {
    let b: NameBody = json_body(&body)?;
    let v = st.blocking(move |st| ArtStore::create(st, &b.name, None, false)).await?;
    Ok((StatusCode::CREATED, axum::Json(v)).into_response())
}

async fn library_load(State(st): State<Arc<AppState>>, Query(q): Q, body: Bytes) -> Result<Response, EngineError> {
    if let Some(k) = q.keys().find(|k| !matches!(k.as_str(), "name" | "overwrite")) {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: name, overwrite)")));
    }
    let raw = q.get("name").cloned().ok_or_else(|| EngineError::BadRequest("name is required".into()))?;
    let (stem, ext) = raw.rsplit_once('.').map_or((raw.as_str(), ""), |(s, e)| (s, e));
    let name = stem.trim().to_owned();
    let overwrite = q.get("overwrite").is_some_and(|v| v == "true" || v == "1");
    let (file, warnings) = if ext.eq_ignore_ascii_case("lbart") || fileio::lbart::is_lbart(&body) {
        let body = body.to_vec();
        st.blocking(move |_| library_from_lbart(&body)).await?
    } else {
        let text = String::from_utf8(body.to_vec()).map_err(|_| {
            EngineError::Unsupported("not an art library: expected an OpenEtch .oeart or a LightBurn .lbart file".into())
        })?;
        (ArtLibraryFile::parse(&text)?, Vec::new())
    };
    let mut v = st.blocking(move |st| ArtStore::create(st, &name, Some(file), overwrite)).await?;
    v["warnings"] = json!(warnings);
    Ok((StatusCode::CREATED, axum::Json(v)).into_response())
}

async fn library_view(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<axum::Json<Value>, EngineError> {
    Ok(axum::Json(st.blocking(move |st| ArtStore::view(st, &name)).await?))
}

async fn library_delete(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>) -> Result<axum::Json<Value>, EngineError> {
    Ok(axum::Json(st.blocking(move |st| ArtStore::delete_library(st, &name)).await?))
}

async fn library_download(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, Query(q): Q) -> Result<Response, EngineError> {
    if let Some(k) = q.keys().find(|k| k.as_str() != "format") {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: format)")));
    }
    let lbart = match q.get("format").map(String::as_str) {
        None | Some("oeart") => false,
        Some("lbart") => true,
        Some(f) => return Err(EngineError::BadRequest(format!("format must be oeart or lbart (got '{f}')"))),
    };
    if lbart {
        let (stem, bytes) = st
            .blocking(move |st| {
                let (stem, file) = {
                    let _g = lock(&st.art.lock);
                    ArtStore::read(st, &name)?
                };
                let (bytes, _) = library_to_lbart(&file)?;
                Ok((stem, bytes))
            })
            .await?;
        let ascii: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
        let disposition = format!("attachment; filename=\"{ascii}.lbart\"");
        return Ok((
            [
                (header::CONTENT_TYPE, "application/octet-stream".to_string()),
                (header::CONTENT_DISPOSITION, disposition),
                (header::CACHE_CONTROL, "no-store".to_string()),
            ],
            bytes,
        )
            .into_response());
    }
    let (stem, text) = st
        .blocking(move |st| {
            let _g = lock(&st.art.lock);
            let (stem, path) = ArtStore::locate(st, &name)?;
            let text = fs::read_to_string(&path).map_err(|e| EngineError::Internal(format!("cannot read {}: {e}", path.display())))?;
            Ok((stem, text))
        })
        .await?;
    let ascii: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    let disposition = format!("attachment; filename=\"{ascii}.{ART_EXT}\"");
    Ok((
        [(header::CONTENT_TYPE, "application/json".to_string()), (header::CONTENT_DISPOSITION, disposition), (header::CACHE_CONTROL, "no-store".to_string())],
        text,
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameLibrary {
    to: String,
}

async fn library_rename(State(st): State<Arc<AppState>>, UrlPath(name): UrlPath<String>, body: Bytes) -> Result<axum::Json<Value>, EngineError> {
    let b: RenameLibrary = json_body(&body)?;
    Ok(axum::Json(st.blocking(move |st| ArtStore::rename_library(st, &name, &b.to)).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddBody {
    ids: Vec<String>,
    name: Option<String>,
}

async fn item_add(State(st): State<Arc<AppState>>, UrlPath(library): UrlPath<String>, body: Bytes) -> Result<Response, EngineError> {
    let b: AddBody = json_body(&body)?;
    let v = st
        .blocking(move |st| {
            let doc = st.document();
            let frag = art_fragment(&doc, &b.ids)?;
            // The single object's name, unless it would not do as an item name (empty, too long).
            let name = b.name.unwrap_or_else(|| match frag.objects.as_slice() {
                [only] if item_name(&only.name).is_ok() => only.name.clone(),
                [_] => "Art".into(),
                _ => format!("{} objects", frag.objects.len()),
            });
            ArtStore::add(st, &library, name, frag)
        })
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({ "ok": true, "item": v }))).into_response())
}

async fn item_import(State(st): State<Arc<AppState>>, UrlPath(library): UrlPath<String>, Query(q): Q, body: Bytes) -> Result<Response, EngineError> {
    if let Some(k) = q.keys().find(|k| !matches!(k.as_str(), "format" | "name")) {
        return Err(EngineError::BadRequest(format!("unknown query parameter '{k}' (allowed: format, name)")));
    }
    if body.is_empty() {
        return Err(EngineError::BadRequest("the upload is empty".into()));
    }
    let format = q.get("format").cloned().ok_or_else(|| EngineError::BadRequest("format is required".into()))?;
    let name = q.get("name").map(|n| n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s).to_owned()).unwrap_or_else(|| "Imported art".into());
    if format == "lbart" {
        // A LightBurn library: every item of it.
        let v = st
            .blocking(move |st| {
                let mut warnings = Vec::new();
                let rows = ArtStore::update(st, &library, |f| {
                    let mut probe = f.clone();
                    let (items, w) = items_from_lbart(&body, || probe.alloc())?;
                    warnings = w;
                    if items.is_empty() {
                        return Err(EngineError::BadRequest("the .lbart file holds no art OpenEtch can show".into()));
                    }
                    if f.items.len() + items.len() > MAX_ITEMS {
                        return Err(EngineError::Conflict(format!("the library would hold more than {MAX_ITEMS} items")));
                    }
                    f.next_id = probe.next_id;
                    let rows: Vec<Value> = items.iter().map(|i| item_row(&library, i)).collect();
                    f.items.extend(items);
                    Ok(rows)
                })?;
                Ok(json!({ "ok": true, "item": rows[0], "items": rows, "warnings": warnings }))
            })
            .await?;
        return Ok((StatusCode::CREATED, axum::Json(v)).into_response());
    }
    let v = st
        .blocking(move |st| {
            let frag = fragment_from_file(st, &format, body.to_vec())?;
            ArtStore::add(st, &library, name, frag)
        })
        .await?;
    Ok((StatusCode::CREATED, axum::Json(json!({ "ok": true, "item": v }))).into_response())
}

async fn item_thumbnail(State(st): State<Arc<AppState>>, UrlPath((library, id)): UrlPath<(String, String)>) -> Result<Response, EngineError> {
    let png = st
        .blocking(move |st| {
            let it = ArtStore::item(st, &library, &id)?;
            match STANDARD.decode(it.thumbnail_png_base64.as_bytes()) {
                // Only a PNG is served as one: a loaded library's thumbnail could hold anything.
                Ok(png) if png.starts_with(PNG_SIGNATURE) => Ok(png),
                // A loaded library without (usable) thumbnails: render one now.
                _ => crate::render::svg_to_png(&fragment_svg(&it.content), THUMBNAIL_PX),
            }
        })
        .await?;
    Ok(([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "no-cache")], png).into_response())
}

async fn item_rename(State(st): State<Arc<AppState>>, UrlPath((library, id)): UrlPath<(String, String)>, body: Bytes) -> Result<axum::Json<Value>, EngineError> {
    let b: NameBody = json_body(&body)?;
    let v = st
        .blocking(move |st| {
            let name = item_name(&b.name)?;
            let lib = library.clone();
            ArtStore::update(st, &library, |f| {
                let it = f.items.iter_mut().find(|i| i.id == id).ok_or_else(|| EngineError::NotFound(format!("no art item '{id}'")))?;
                it.name = name;
                Ok(item_row(&lib, it))
            })
        })
        .await?;
    Ok(axum::Json(json!({ "ok": true, "item": v })))
}

async fn item_delete(State(st): State<Arc<AppState>>, UrlPath((library, id)): UrlPath<(String, String)>) -> Result<axum::Json<Value>, EngineError> {
    st.blocking(move |st| {
        ArtStore::update(st, &library, |f| {
            let before = f.items.len();
            f.items.retain(|i| i.id != id);
            if f.items.len() == before {
                return Err(EngineError::NotFound(format!("no art item '{id}'")));
            }
            Ok(())
        })
    })
    .await?;
    Ok(axum::Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyBody {
    to: String,
    #[serde(default)]
    r#move: bool,
}

async fn item_copy(State(st): State<Arc<AppState>>, UrlPath((library, id)): UrlPath<(String, String)>, body: Bytes) -> Result<axum::Json<Value>, EngineError> {
    let b: CopyBody = json_body(&body)?;
    let v = st
        .blocking(move |st| {
            let it = ArtStore::item(st, &library, &id)?;
            let (from, _) = ArtStore::locate(st, &library)?;
            let (to, _) = ArtStore::locate(st, &b.to)?;
            if from == to {
                return Err(EngineError::BadRequest("the item is already in that library".into()));
            }
            let row = ArtStore::update(st, &to, |f| {
                if f.items.len() >= MAX_ITEMS {
                    return Err(EngineError::Conflict(format!("the library holds {MAX_ITEMS} items; use another one")));
                }
                let mut copy = it.clone();
                copy.id = f.alloc();
                f.items.push(copy.clone());
                Ok(item_row(&to, &copy))
            })?;
            if b.r#move {
                ArtStore::update(st, &from, |f| {
                    f.items.retain(|i| i.id != id);
                    Ok(())
                })?;
            }
            Ok(row)
        })
        .await?;
    Ok(axum::Json(json!({ "ok": true, "item": v })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InsertBody {
    center: Option<[f64; 2]>,
    layer: Option<String>,
    origin: Option<Value>,
}

async fn item_insert(State(st): State<Arc<AppState>>, UrlPath((library, id)): UrlPath<(String, String)>, body: Bytes) -> Result<Response, EngineError> {
    let b: InsertBody = json_body(&body)?;
    let origin = parse_origin(b.origin.as_ref())?;
    let result = st
        .blocking(move |st| {
            let it = ArtStore::item(st, &library, &id)?;
            let content = serde_json::to_string(&it.content).map_err(|e| EngineError::Internal(e.to_string()))?;
            let mut params = json!({ "content": content, "name": it.name });
            if let Some(c) = b.center {
                params["center"] = json!(c);
            }
            if let Some(l) = b.layer {
                params["layer"] = json!(l);
            }
            st.execute(Invocation { id: "art.insert".into(), params }, origin)
        })
        .await;
    match result {
        Ok(o) => Ok(axum::Json(json!({ "ok": true, "result": o.result, "revision": o.revision })).into_response()),
        // A refused insert answers like a failing /api/execute.
        Err(EngineError::Command(e)) => Ok(axum::Json(json!({ "ok": false, "error": e.to_string() })).into_response()),
        Err(e) => Err(e),
    }
}
