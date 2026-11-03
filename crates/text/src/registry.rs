//! Where fonts come from: the bundled Hershey fonts, installed system fonts (feature `system`, via fontdb), font
//! folders holding `.ttf`/`.otf`/`.ttc`, `.shx` and `.jsf` files (feature `system`: the `OPENETCH_FONT_DIRS`
//! environment variable, `;`-separated on Windows and `:` elsewhere, plus folders added with `add_dir`), and font
//! data added in memory ([`add_font_data`], for tests and browser builds). One registry per process
//! ([`with_registry`]); loaded fonts are cached.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use doc::{FontKind, FontSpec};

use crate::outline::OutlineFont;
use crate::stroke::StrokeFont;
use crate::{hershey, jsf, shx};

/// A loaded font.
#[derive(Debug, Clone)]
pub enum Font {
    Outline(Arc<OutlineFont>),
    Stroke(Arc<StrokeFont>),
}

/// A font chosen for a text, with what could not be honoured.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub font: Font,
    /// Slant the glyphs (italic asked for, but the family has no italic face, or a single-line font).
    pub slant: bool,
    pub warnings: Vec<String>,
}

/// A family the registry can render, for font lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FamilyInfo {
    pub family: String,
    pub kind: FontKind,
    pub bold: bool,
    pub italic: bool,
}

/// A font file or face the registry knows of but has not loaded yet.
#[derive(Debug, Clone)]
enum Source {
    Bundled(&'static str),
    Data(Arc<[u8]>, u32),
    #[cfg(feature = "system")]
    File(std::path::PathBuf, u32),
    #[cfg(feature = "system")]
    Db(fontdb::ID),
}

#[derive(Debug, Clone)]
struct Face {
    family: String,
    kind: FontKind,
    weight: u16,
    italic: bool,
    source: Source,
}

/// The fonts of this process.
#[derive(Default)]
pub struct Registry {
    faces: Vec<Face>,
    cache: HashMap<usize, Font>,
    #[cfg(feature = "system")]
    db: Option<fontdb::Database>,
    #[cfg(feature = "system")]
    dirs: Vec<std::path::PathBuf>,
}

fn global() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::with_bundled()))
}

/// Run `f` with the process's registry (locked for the duration).
pub fn with_registry<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut r = global().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut r)
}

/// Add an in-memory font (TrueType/OpenType, `.shx` or `.jsf` bytes, detected from the data). `family` names a
/// single-line font; a TrueType font is named by its own family name. Returns the family names added. Adding a face
/// again (same family, kind, weight and slant) replaces the one added before, so a browser can add its stored fonts
/// at every start.
pub fn add_font_data(family: &str, data: Vec<u8>) -> Result<Vec<String>, String> {
    with_registry(|r| r.add_data(family, data.into()))
}

/// Remove the fonts added with [`add_font_data`] under `family` (any kind; case does not matter). Bundled, system
/// and font-folder fonts stay. Returns how many faces were removed.
pub fn remove_font_data(family: &str) -> usize {
    with_registry(|r| r.remove_data(family))
}

/// Add a folder of fonts (native builds). Fonts already known are kept.
#[cfg(feature = "system")]
pub fn add_dir(dir: impl Into<std::path::PathBuf>) -> usize {
    with_registry(|r| r.add_dir(dir.into()))
}

/// Every family the registry can render, sorted by name.
pub fn families() -> Vec<FamilyInfo> {
    with_registry(|r| r.families())
}

/// The font for `spec` in the asked style.
pub fn resolve(spec: &FontSpec, bold: bool, italic: bool) -> Result<Resolved, String> {
    with_registry(|r| r.resolve(spec, bold, italic))
}

impl Registry {
    /// A registry with the bundled fonts only (system fonts are scanned on first use with feature `system`).
    pub fn with_bundled() -> Registry {
        let mut r = Registry::default();
        for (name, _, _) in hershey::FONTS {
            r.faces.push(Face {
                family: (*name).to_string(),
                kind: FontKind::Stroke,
                weight: 400,
                italic: false,
                source: Source::Bundled(name),
            });
        }
        r
    }

    fn add_data(&mut self, family: &str, data: Arc<[u8]>) -> Result<Vec<String>, String> {
        if jsf::is_jsf(&data) || shx::is_shx(&data) {
            let kind = if jsf::is_jsf(&data) { FontKind::Jsf } else { FontKind::Shx };
            // Check it parses before listing it.
            load_stroke(kind, family, &data)?;
            self.faces.retain(|f| !(f.kind == kind && f.family.eq_ignore_ascii_case(family)));
            self.faces.push(Face { family: family.to_string(), kind, weight: 400, italic: false, source: Source::Data(data, 0) });
            self.cache.clear();
            return Ok(vec![family.to_string()]);
        }
        let mut faces = Vec::new();
        for index in 0..64u32 {
            let Ok(f) = OutlineFont::from_data(data.clone(), index) else { break };
            faces.push(Face {
                family: if f.family.is_empty() { family.to_string() } else { f.family.clone() },
                kind: FontKind::System,
                weight: f.weight.round().clamp(1.0, 1000.0) as u16,
                italic: f.italic,
                source: Source::Data(data.clone(), index),
            });
        }
        if faces.is_empty() {
            return Err(format!("{family}: not a TrueType/OpenType, SHX or JSF font"));
        }
        self.faces.retain(|old| {
            !(matches!(old.source, Source::Data(..))
                && faces.iter().any(|f| {
                    f.kind == old.kind && f.weight == old.weight && f.italic == old.italic && f.family.eq_ignore_ascii_case(&old.family)
                }))
        });
        let mut added: Vec<String> = Vec::new();
        for f in &faces {
            if !added.iter().any(|a| a.eq_ignore_ascii_case(&f.family)) {
                added.push(f.family.clone());
            }
        }
        self.faces.extend(faces);
        self.cache.clear();
        Ok(added)
    }

    fn remove_data(&mut self, family: &str) -> usize {
        let before = self.faces.len();
        self.faces.retain(|f| !(matches!(f.source, Source::Data(..)) && f.family.eq_ignore_ascii_case(family.trim())));
        let n = before - self.faces.len();
        if n > 0 {
            self.cache.clear();
        }
        n
    }

    #[cfg(feature = "system")]
    fn ensure_system(&mut self) {
        if self.db.is_some() {
            return;
        }
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for face in db.faces() {
            let Some((family, _)) = face.families.first() else { continue };
            self.faces.push(Face {
                family: family.clone(),
                kind: FontKind::System,
                weight: face.weight.0,
                italic: face.style != fontdb::Style::Normal,
                source: Source::Db(face.id),
            });
        }
        self.db = Some(db);
        if let Ok(v) = std::env::var("OPENETCH_FONT_DIRS") {
            let sep = if cfg!(windows) { ';' } else { ':' };
            for d in v.split(sep).map(str::trim).filter(|d| !d.is_empty()) {
                self.add_dir(d.into());
            }
        }
    }

    #[cfg(not(feature = "system"))]
    fn ensure_system(&mut self) {}

    /// Add every font file in `dir` (not recursive). Returns how many faces were added.
    #[cfg(feature = "system")]
    fn add_dir(&mut self, dir: std::path::PathBuf) -> usize {
        if self.dirs.contains(&dir) {
            return 0;
        }
        self.dirs.push(dir.clone());
        let Ok(entries) = std::fs::read_dir(&dir) else { return 0 };
        let mut n = 0;
        for e in entries.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            let kind = match ext.as_str() {
                "jsf" => FontKind::Jsf,
                "shx" => FontKind::Shx,
                "ttf" | "otf" | "ttc" | "otc" => {
                    if let Ok(data) = std::fs::read(&p) {
                        let data: Arc<[u8]> = data.into();
                        for index in 0..64u32 {
                            let Ok(f) = OutlineFont::from_data(data.clone(), index) else { break };
                            self.faces.push(Face {
                                family: if f.family.is_empty() { stem.clone() } else { f.family.clone() },
                                kind: FontKind::System,
                                weight: f.weight.round().clamp(1.0, 1000.0) as u16,
                                italic: f.italic,
                                source: Source::File(p.clone(), index),
                            });
                            n += 1;
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            self.faces.push(Face { family: stem, kind, weight: 400, italic: false, source: Source::File(p, 0) });
            n += 1;
        }
        self.cache.clear();
        n
    }

    pub fn families(&mut self) -> Vec<FamilyInfo> {
        self.ensure_system();
        let mut by: BTreeMap<(String, u8), FamilyInfo> = BTreeMap::new();
        for f in &self.faces {
            let e = by.entry((f.family.to_lowercase(), f.kind as u8)).or_insert_with(|| FamilyInfo {
                family: f.family.clone(),
                kind: f.kind,
                bold: false,
                italic: false,
            });
            e.bold |= f.weight >= 600;
            e.italic |= f.italic;
        }
        by.into_values().collect()
    }

    /// The best face of `spec`'s family: the weight closest to the asked one, the asked slant first.
    pub fn resolve(&mut self, spec: &FontSpec, bold: bool, italic: bool) -> Result<Resolved, String> {
        let want_weight: i32 = if bold { 700 } else { 400 };
        let pick = |faces: &[Face]| {
            faces
                .iter()
                .enumerate()
                .filter(|(_, f)| f.kind == spec.kind && f.family.eq_ignore_ascii_case(spec.family.trim()))
                .min_by_key(|(_, f)| (i32::from(f.italic != italic) * 10_000) + (i32::from(f.weight) - want_weight).abs())
                .map(|(i, _)| i)
        };
        let mut found = pick(&self.faces);
        if found.is_none() && spec.kind != FontKind::Stroke {
            self.ensure_system();
            found = pick(&self.faces);
        }
        let Some(i) = found else {
            let kind = match spec.kind {
                FontKind::System => "font",
                FontKind::Stroke => "single-line font",
                FontKind::Shx => "SHX font",
                FontKind::Jsf => "JSF font",
            };
            return Err(format!(
                "the {kind} \"{}\" is not installed (font.list shows the fonts OpenEtch can use)",
                spec.family
            ));
        };
        let face = self.faces[i].clone();
        let font = match self.cache.get(&i) {
            Some(f) => f.clone(),
            None => {
                let f = self.load(&face)?;
                self.cache.insert(i, f.clone());
                f
            }
        };
        let mut warnings = Vec::new();
        let stroke = matches!(font, Font::Stroke(_));
        if bold && face.weight < 600 && !stroke {
            warnings.push(format!("\"{}\" has no bold face; the regular one is used", spec.family));
        }
        if bold && stroke {
            warnings.push(format!("\"{}\" is a single-line font: bold is not available", spec.family));
        }
        let slant = italic && !face.italic;
        if slant && !stroke {
            warnings.push(format!("\"{}\" has no italic face; the glyphs are slanted instead", spec.family));
        }
        Ok(Resolved { font, slant, warnings })
    }

    fn load(&self, face: &Face) -> Result<Font, String> {
        let bytes: Arc<[u8]> = match &face.source {
            Source::Bundled(name) => {
                let (_, _, data) = hershey::FONTS.iter().find(|(n, _, _)| n == name).expect("bundled");
                return hershey::parse(name, data).map(|f| Font::Stroke(Arc::new(f)));
            }
            Source::Data(d, _) => d.clone(),
            #[cfg(feature = "system")]
            Source::File(p, _) => std::fs::read(p).map_err(|e| format!("cannot read {}: {e}", p.display()))?.into(),
            #[cfg(feature = "system")]
            Source::Db(id) => {
                let db = self.db.as_ref().ok_or("system fonts not loaded")?;
                let (data, _) = db
                    .with_face_data(*id, |d, index| (Arc::<[u8]>::from(d), index))
                    .ok_or_else(|| format!("cannot read the font \"{}\"", face.family))?;
                data
            }
        };
        let index = match &face.source {
            Source::Data(_, i) => *i,
            #[cfg(feature = "system")]
            Source::File(_, i) => *i,
            #[cfg(feature = "system")]
            Source::Db(id) => self.db.as_ref().and_then(|db| db.face(*id)).map_or(0, |f| f.index),
            Source::Bundled(_) => 0,
        };
        match face.kind {
            FontKind::System => OutlineFont::from_data(bytes, index)
                .map(|f| Font::Outline(Arc::new(f)))
                .map_err(|e| format!("\"{}\": {e}", face.family)),
            kind => load_stroke(kind, &face.family, &bytes).map(|f| Font::Stroke(Arc::new(f))),
        }
    }
}

fn load_stroke(kind: FontKind, family: &str, data: &[u8]) -> Result<StrokeFont, String> {
    match kind {
        FontKind::Jsf => jsf::parse(family, data),
        FontKind::Shx => shx::parse(family, data),
        _ => Err(format!("{family}: not a single-line font")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_resolve_without_system_fonts() {
        let mut r = Registry::with_bundled();
        let got = r.resolve(&FontSpec { family: "hershey sans".into(), kind: FontKind::Stroke }, false, true).unwrap();
        assert!(matches!(got.font, Font::Stroke(_)));
        assert!(got.slant, "single-line italic is a slant");
        let e = r.resolve(&FontSpec { family: "Nope".into(), kind: FontKind::Stroke }, false, false).unwrap_err();
        assert!(e.contains("not installed"), "{e}");
        assert!(r.families().iter().any(|f| f.family == "Hershey Script" && f.kind == FontKind::Stroke));
    }

    #[test]
    fn in_memory_single_line_fonts() {
        let mut r = Registry::with_bundled();
        assert_eq!(r.add_data("TESTJ", crate::jsf::tests::sample().into()).unwrap(), ["TESTJ"]);
        let got = r.resolve(&FontSpec { family: "testj".into(), kind: FontKind::Jsf }, false, false).unwrap();
        assert!(matches!(got.font, Font::Stroke(f) if f.glyph('A').is_some()));
        assert!(r.add_data("junk", b"junk".to_vec().into()).is_err());
    }

    /// A generated TrueType font (tests/fixtures/ttf, made by make-box-font.mjs there).
    const BOX_TTF: &[u8] = include_bytes!("../tests/fixtures/ttf/openetch-box.ttf");

    #[test]
    fn in_memory_truetype_fonts_are_listed_replaced_and_removed() {
        let mut r = Registry::with_bundled();
        let n = r.faces.len();
        // Named by the font's own family, whatever the caller calls it.
        assert_eq!(r.add_data("upload.ttf", BOX_TTF.to_vec().into()).unwrap(), ["OpenEtch Box"]);
        let listed: Vec<_> = r.families().into_iter().filter(|f| f.family == "OpenEtch Box").collect();
        assert_eq!(listed, [FamilyInfo { family: "OpenEtch Box".into(), kind: FontKind::System, bold: false, italic: false }]);
        let got = r.resolve(&FontSpec::system("openetch box"), false, false).unwrap();
        assert!(matches!(&got.font, Font::Outline(f) if f.family == "OpenEtch Box"));
        // Adding the same face again (a browser adding its stored fonts at every start) replaces it.
        r.add_data("again", BOX_TTF.to_vec().into()).unwrap();
        assert_eq!(r.faces.len(), n + 1);
        // Removing takes the added faces only.
        assert_eq!(r.remove_data("Hershey Sans"), 0);
        assert_eq!(r.remove_data(" openetch BOX "), 1);
        assert_eq!(r.faces.len(), n);
        assert!(r.resolve(&FontSpec::system("OpenEtch Box"), false, false).is_err());
    }
}
