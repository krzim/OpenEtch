//! LightBurn material libraries (`.clb`): [`parse_clb`] into a [`Library`], [`write_clb`] it back, and convert
//! entries to and from OpenEtch layers.
//!
//! # The format, as established from the owner's library
//!
//! Evidence: the owner's LightBurn 1.x `parameters-60w150x150mm.clb` (13 materials, 37 entries) and LightBurn's
//! Material Library documentation. A UTF-8 BOM, `<?xml version="1.0" encoding="UTF-8"?>`, CRLF line endings, two
//! spaces of indentation per level and `<tag Value="..." />` (a space before the slash):
//!
//! ```text
//! <LightBurnLibrary>
//!   <Material name="AR15">
//!     <Entry Thickness="-1.0000" Desc="NFA Engrave" NoThickTitle="Deep Engrave">
//!       <CutSetting type="Scan">
//!         <index Value="0" />
//!         <LinkPath Value="AR15/Deep Engrave/NFA Engrave" />
//!         ...the same tags as a project's CutSetting, then its SubLayers
//! ```
//!
//! The library tree is Material name → Thickness (or, with "No Thickness", the `NoThickTitle`) → `Desc`.
//! `Thickness="-1.0000"` means "no thickness" (all 37 sample entries), written with four decimals; a real thickness
//! is in mm. Each entry's `LinkPath` is `material/title/description` in every sample entry; for an entry with a
//! thickness the title part is the thickness as [`LibEntry::group_title`] writes it (no sample: unverified).
//!
//! Round trips: what was read is written back byte for byte (the BOM, declaration, line endings, indentation,
//! empty-element style, attribute order and spelling, and every entry that did not change is written from its
//! source text). Changed or new entries are written in the file's style.

use std::sync::Arc;

use doc::{Document, Layer};
use roxmltree::Node;

use crate::model::{CutParams, CutSetting};
use crate::parse::{cut_setting, raw, xml};
use crate::write::{escape_attr, safe_name};
use crate::{LbrnError, cut_setting_to_layer, from_document};

/// Most entries a library may hold (the owner's has 37; LightBurn's shared libraries a few hundred).
pub const MAX_ENTRIES: usize = 20_000;
/// Most materials a library may hold (empty `<Material/>` elements count too).
pub const MAX_MATERIALS: usize = MAX_ENTRIES;

/// The `Thickness` LightBurn writes for an entry without one.
pub const NO_THICKNESS: &str = "-1.0000";

/// How the file is laid out, so a library is written back as it was read. The default is LightBurn's (as in the
/// owner's file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFormat {
    pub bom: bool,
    /// The XML declaration (`None`: none in the file).
    pub declaration: Option<String>,
    pub crlf: bool,
    pub final_newline: bool,
    /// `<a Value="1" />` rather than `<a Value="1"/>`.
    pub space_before_slash: bool,
    /// Spaces per indentation level.
    pub indent: usize,
}

impl Default for LibraryFormat {
    fn default() -> Self {
        LibraryFormat {
            bom: true,
            declaration: Some("<?xml version=\"1.0\" encoding=\"UTF-8\"?>".into()),
            crlf: true,
            final_newline: true,
            space_before_slash: true,
            indent: 2,
        }
    }
}

/// A material library.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Library {
    pub format: LibraryFormat,
    /// Attributes of `<LightBurnLibrary>` (none in the sample), in file order.
    pub attributes: Vec<(String, String)>,
    pub materials: Vec<Material>,
}

/// `<Material name="...">`: one top-level folder of the library tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Material {
    /// Every attribute in file order (`name` among them).
    pub attributes: Vec<(String, String)>,
    pub entries: Vec<LibEntry>,
}

/// `<Entry Thickness Desc NoThickTitle>` with its `CutSetting` (normally one).
#[derive(Debug, Clone, Default)]
pub struct LibEntry {
    /// Every attribute in file order.
    pub attributes: Vec<(String, String)>,
    pub settings: Vec<CutSetting>,
    /// What was read and its source text, written back verbatim while the entry still equals it.
    source: Option<Arc<EntrySource>>,
}

/// An entry as read: its attributes, settings and source text.
#[derive(Debug)]
struct EntrySource {
    attributes: Vec<(String, String)>,
    settings: Vec<CutSetting>,
    text: String,
}

impl PartialEq for LibEntry {
    fn eq(&self, other: &Self) -> bool {
        self.attributes == other.attributes && self.settings == other.settings
    }
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn set_attr(attrs: &mut Vec<(String, String)>, key: &str, value: String) {
    match attrs.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = value,
        None => attrs.push((key.to_string(), value)),
    }
}

/// `3` → `3`, `2.5` → `2.5`: a thickness for display.
fn trim_number(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

impl Material {
    pub fn new(name: &str) -> Material {
        Material { attributes: vec![("name".into(), name.to_string())], entries: Vec::new() }
    }

    pub fn name(&self) -> &str {
        attr(&self.attributes, "name").unwrap_or_default()
    }

    pub fn set_name(&mut self, name: &str) {
        set_attr(&mut self.attributes, "name", name.to_string());
    }
}

impl LibEntry {
    /// A new entry: `thickness` in mm (`None`: "No Thickness", shown under `title`), the description, and the cut
    /// setting (its `index` is set to 0, as in LightBurn's library).
    pub fn new(thickness: Option<f64>, title: &str, description: &str, mut setting: CutSetting) -> LibEntry {
        setting.index = 0;
        let mut e = LibEntry { attributes: Vec::new(), settings: vec![setting], source: None };
        e.set_meta(thickness, title, description);
        e
    }

    /// The thickness in mm, `None` for "No Thickness" (LightBurn writes -1).
    pub fn thickness(&self) -> Option<f64> {
        attr(&self.attributes, "Thickness").and_then(|t| t.trim().parse::<f64>().ok()).filter(|t| t.is_finite() && *t > 0.0)
    }

    /// `NoThickTitle`: the folder name of an entry without a thickness.
    pub fn title(&self) -> &str {
        attr(&self.attributes, "NoThickTitle").unwrap_or_default()
    }

    /// `Desc`: the entry's name in the list.
    pub fn description(&self) -> &str {
        attr(&self.attributes, "Desc").unwrap_or_default()
    }

    /// The second level of the tree: the thickness (`"3 mm"`) or, without one, the title.
    pub fn group_title(&self) -> String {
        match self.thickness() {
            Some(t) => format!("{} mm", trim_number(t)),
            None => self.title().to_string(),
        }
    }

    /// `material/group title/description`, LightBurn's `LinkPath`.
    pub fn link_path(&self, material: &str) -> String {
        format!("{material}/{}/{}", self.group_title(), self.description())
    }

    /// Change the thickness (`None`: No Thickness), title and description (LightBurn's Edit Description). The
    /// `LinkPath` of the settings is not changed; [`Library::relink_paths`] does that.
    pub fn set_meta(&mut self, thickness: Option<f64>, title: &str, description: &str) {
        let t = match thickness.filter(|t| t.is_finite() && *t > 0.0) {
            Some(t) => format!("{t:.4}"),
            None => NO_THICKNESS.to_string(),
        };
        if self.attributes.is_empty() {
            self.attributes = vec![
                ("Thickness".into(), t),
                ("Desc".into(), description.to_string()),
                ("NoThickTitle".into(), title.to_string()),
            ];
            return;
        }
        let same = attr(&self.attributes, "Thickness").and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v.max(-1.0))
            == t.parse::<f64>().ok();
        if !same {
            set_attr(&mut self.attributes, "Thickness", t);
        }
        set_attr(&mut self.attributes, "Desc", description.to_string());
        set_attr(&mut self.attributes, "NoThickTitle", title.to_string());
    }

    /// The entry's settings as an OpenEtch layer with id `id` (its first `CutSetting`), with conversion warnings.
    pub fn to_layer(&self, id: &str) -> Result<(Layer, Vec<String>), LbrnError> {
        let cs = self.settings.first().ok_or_else(|| LbrnError::Malformed("the entry has no CutSetting".into()))?;
        Ok(cut_setting_to_layer(cs, id))
    }

    /// The same entry without its kept source text, so it is written by the structural writer.
    pub fn detached(&self) -> LibEntry {
        LibEntry { attributes: self.attributes.clone(), settings: self.settings.clone(), source: None }
    }

    /// The entry as the structural writer writes it (LightBurn's layout), for comparing entries by content.
    pub fn canonical_text(&self) -> String {
        let f = LibraryFormat::default();
        let mut o = Out { s: String::new(), f: &f };
        o.entry(0, &self.detached());
        o.s
    }

    /// Set the `LinkPath` tag of the entry's settings.
    pub fn set_link_path(&mut self, path: &str) {
        for s in &mut self.settings {
            set_param(&mut s.params, "LinkPath", path);
        }
    }
}

fn set_param(p: &mut CutParams, tag: &str, value: &str) {
    p.other.insert(tag.to_string(), value.to_string());
    if let Some((_, v)) = p.source.iter_mut().find(|(t, _)| t == tag) {
        *v = value.to_string();
    }
}

/// A layer's settings (main settings, sub-layers and kept LightBurn values) as a library `CutSetting` (index 0, the
/// layer's name left out, as LightBurn writes library entries), with conversion warnings.
pub fn cut_setting_from_layer(layer: &Layer) -> Result<(CutSetting, Vec<String>), LbrnError> {
    let mut l = layer.clone();
    l.extras.remove(&format!("{}index", crate::EXTRA_PREFIX));
    l.output = true;
    let d = Document { field_mm: Some(150.0), layers: vec![l], objects: Vec::new(), ..Document::default() };
    let (p, warnings) = from_document(&d, 150.0)?;
    let mut cs = p
        .cut_settings
        .into_iter()
        .find(|c| !c.image)
        .ok_or_else(|| LbrnError::Invalid(format!("layer \"{}\" has no LightBurn cut setting", layer.name)))?;
    cs.index = 0;
    cs.params.name = Some(String::new());
    cs.params.other.remove("priority");
    cs.params.priority = Some(0);
    Ok((cs, warnings))
}

impl Library {
    /// An empty library in LightBurn's layout.
    pub fn new() -> Library {
        Library::default()
    }

    pub fn entry_count(&self) -> usize {
        self.materials.iter().map(|m| m.entries.len()).sum()
    }

    /// The material named `name` (exact), creating it at the end when missing.
    pub fn material_mut(&mut self, name: &str) -> &mut Material {
        let i = match self.materials.iter().position(|m| m.name() == name) {
            Some(i) => i,
            None => {
                self.materials.push(Material::new(name));
                self.materials.len() - 1
            }
        };
        &mut self.materials[i]
    }

    /// The first entry whose `LinkPath` (material/group title/description) is `path`, as `(material, entry)`
    /// indices.
    pub fn find_path(&self, path: &str) -> Option<(usize, usize)> {
        self.materials.iter().enumerate().find_map(|(m, mat)| {
            mat.entries.iter().position(|e| e.link_path(mat.name()) == path).map(|e| (m, e))
        })
    }

    /// Write every entry's `LinkPath` from its place in the tree (after a rename or an edited description).
    pub fn relink_paths(&mut self) {
        for m in &mut self.materials {
            let name = m.name().to_string();
            for e in &mut m.entries {
                let path = e.link_path(&name);
                let current = e.settings.first().and_then(|s| s.params.other.get("LinkPath")).cloned();
                if current.as_deref() != Some(path.as_str()) {
                    e.set_link_path(&path);
                }
            }
        }
    }

    /// Merge `other` into this library (LightBurn's Merge Library With): its materials' entries are added to the
    /// material of the same name (created at the end when missing); an entry equal to one already there (same
    /// place and settings) is skipped. Returns the number of entries added.
    ///
    /// Linear in the entries (each entry's canonical text is computed once), and all or nothing: a merge that would
    /// go over [`MAX_ENTRIES`] or [`MAX_MATERIALS`] changes nothing.
    pub fn merge(&mut self, other: &Library) -> Result<usize, LbrnError> {
        use std::collections::{HashMap, HashSet};
        // Material name → index (first of a name, as `material_mut` finds it) and the entries already there.
        let mut index: HashMap<String, usize> = HashMap::new();
        for (i, m) in self.materials.iter().enumerate() {
            index.entry(m.name().to_string()).or_insert(i);
        }
        let mut seen: HashMap<usize, HashSet<String>> = HashMap::new();
        let mut new_materials: Vec<Material> = Vec::new();
        let mut additions: Vec<(usize, LibEntry)> = Vec::new();
        for m in &other.materials {
            let name = m.name();
            let target = match index.get(name) {
                Some(&i) => i,
                None => {
                    let i = self.materials.len() + new_materials.len();
                    new_materials.push(Material::new(name));
                    index.insert(name.to_string(), i);
                    i
                }
            };
            let set = seen.entry(target).or_insert_with(|| {
                self.materials.get(target).map(|t| t.entries.iter().map(LibEntry::canonical_text).collect()).unwrap_or_default()
            });
            for e in &m.entries {
                if set.insert(e.canonical_text()) {
                    additions.push((target, e.clone()));
                }
            }
        }
        // Materials named but left without an entry are not added.
        if self.entry_count() + additions.len() > MAX_ENTRIES {
            return Err(LbrnError::TooLarge(format!("a library may hold at most {MAX_ENTRIES} entries")));
        }
        if self.materials.len() + new_materials.len() > MAX_MATERIALS {
            return Err(LbrnError::TooLarge(format!("a library may hold at most {MAX_MATERIALS} materials")));
        }
        let first_new = self.materials.len();
        self.materials.extend(new_materials);
        let added = additions.len();
        for (i, e) in additions {
            self.materials[i].entries.push(e);
        }
        // A new material that got no entry (all of `other`'s were empty) is dropped again.
        let mut i = first_new;
        while i < self.materials.len() {
            if self.materials[i].entries.is_empty() {
                self.materials.remove(i);
            } else {
                i += 1;
            }
        }
        Ok(added)
    }
}

/// Parse a LightBurn material library (`.clb`).
pub fn parse_clb(text: &str) -> Result<Library, LbrnError> {
    let doc = xml(text)?;
    let root = doc.root_element();
    if !root.has_tag_name("LightBurnLibrary") {
        return Err(LbrnError::NotProject(format!(
            "the root element is <{}>, not <LightBurnLibrary> (not a LightBurn material library)",
            root.tag_name().name()
        )));
    }
    let mut lib = Library { format: format_of(text, &doc), attributes: attributes(root), materials: Vec::new() };
    let mut count = 0usize;
    for m in root.children().filter(Node::is_element) {
        if !m.has_tag_name("Material") {
            return Err(LbrnError::Malformed(format!("unexpected <{}> in the library", m.tag_name().name())));
        }
        if lib.materials.len() >= MAX_MATERIALS {
            return Err(LbrnError::TooLarge(format!("more than {MAX_MATERIALS} materials")));
        }
        let mut material = Material { attributes: attributes(m), entries: Vec::new() };
        for e in m.children().filter(Node::is_element) {
            if !e.has_tag_name("Entry") {
                return Err(LbrnError::Malformed(format!("unexpected <{}> in a material", e.tag_name().name())));
            }
            count += 1;
            if count > MAX_ENTRIES {
                return Err(LbrnError::TooLarge(format!("more than {MAX_ENTRIES} entries")));
            }
            let mut settings = Vec::new();
            for c in e.children().filter(Node::is_element) {
                if c.has_tag_name("CutSetting") || c.has_tag_name("CutSetting_Img") {
                    settings.push(cut_setting(c)?);
                } else {
                    return Err(LbrnError::Malformed(format!("unexpected <{}> in an entry", c.tag_name().name())));
                }
            }
            let attrs = attributes(e);
            let source = Arc::new(EntrySource { attributes: attrs.clone(), settings: settings.clone(), text: raw(e) });
            material.entries.push(LibEntry { attributes: attrs, settings, source: Some(source) });
        }
        lib.materials.push(material);
    }
    Ok(lib)
}

fn attributes(n: Node<'_, '_>) -> Vec<(String, String)> {
    n.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect()
}

fn format_of(text: &str, doc: &roxmltree::Document<'_>) -> LibraryFormat {
    let body = doc.input_text();
    let root = doc.root_element();
    let before = body[..root.range().start].trim();
    let declaration = before.starts_with("<?xml").then(|| before.find("?>").map(|end| before[..end + 2].to_string())).flatten();
    let after = &body[root.range().end..];
    // The first indented element after the root's start tag gives the indentation step.
    let inner = &body[root.range().start..root.range().end];
    let indent = inner
        .lines()
        .skip(1)
        .find(|l| l.trim_start().starts_with('<'))
        .map(|l| l.len() - l.trim_start().len())
        .filter(|n| (1..=8).contains(n))
        .unwrap_or(2);
    LibraryFormat {
        bom: text.starts_with('\u{feff}'),
        declaration,
        crlf: body.contains("\r\n"),
        final_newline: after.starts_with('\n') || after.starts_with("\r\n"),
        space_before_slash: body.matches("\" />").count() >= body.matches("\"/>").count(),
        indent,
    }
}

struct Out<'a> {
    s: String,
    f: &'a LibraryFormat,
}

impl Out<'_> {
    fn pad(&mut self, depth: usize) {
        for _ in 0..depth * self.f.indent {
            self.s.push(' ');
        }
    }

    fn attrs(&mut self, attrs: &[(String, String)]) {
        for (k, v) in attrs.iter().filter(|(k, _)| safe_name(k)) {
            self.s.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
        }
    }

    fn open(&mut self, depth: usize, name: &str, attrs: &[(String, String)]) {
        self.pad(depth);
        self.s.push('<');
        self.s.push_str(name);
        self.attrs(attrs);
        self.s.push_str(">\n");
    }

    fn empty(&mut self, depth: usize, name: &str, attrs: &[(String, String)]) {
        self.pad(depth);
        self.s.push('<');
        self.s.push_str(name);
        self.attrs(attrs);
        self.s.push_str(if self.f.space_before_slash { " />\n" } else { "/>\n" });
    }

    fn close(&mut self, depth: usize, name: &str) {
        self.pad(depth);
        self.s.push_str(&format!("</{name}>\n"));
    }

    fn value(&mut self, depth: usize, tag: &str, value: &str) {
        if safe_name(tag) {
            self.empty(depth, tag, &[("Value".into(), value.to_string())]);
        }
    }

    fn setting(&mut self, depth: usize, c: &CutSetting) {
        let name = if c.image { "CutSetting_Img" } else { "CutSetting" };
        self.open(depth, name, &[("type".into(), c.kind.clone())]);
        self.value(depth + 1, "index", &c.index.to_string());
        for (tag, v) in c.params.to_values() {
            self.value(depth + 1, &tag, &v);
        }
        for s in &c.sub_layers {
            self.open(depth + 1, "SubLayer", &[("type".into(), s.kind.clone()), ("index".into(), s.index.to_string())]);
            for (tag, v) in s.params.to_values() {
                self.value(depth + 2, &tag, &v);
            }
            self.close(depth + 1, "SubLayer");
        }
        self.close(depth, name);
    }

    fn entry(&mut self, depth: usize, e: &LibEntry) {
        if let Some(src) = &e.source
            && src.attributes == e.attributes
            && src.settings == e.settings
        {
            self.pad(depth);
            self.s.push_str(&src.text);
            self.s.push('\n');
            return;
        }
        if e.settings.is_empty() {
            self.empty(depth, "Entry", &e.attributes);
            return;
        }
        self.open(depth, "Entry", &e.attributes);
        for c in &e.settings {
            self.setting(depth + 1, c);
        }
        self.close(depth, "Entry");
    }
}

/// Write a library as `.clb` text (see the module docs for the layout).
pub fn write_clb(lib: &Library) -> String {
    let mut o = Out { s: String::with_capacity(4096 + 1000 * lib.entry_count()), f: &lib.format };
    if lib.format.bom {
        o.s.push('\u{feff}');
    }
    if let Some(d) = &lib.format.declaration {
        o.s.push_str(d);
        o.s.push('\n');
    }
    o.open(0, "LightBurnLibrary", &lib.attributes);
    for m in &lib.materials {
        if m.entries.is_empty() {
            o.empty(1, "Material", &m.attributes);
            continue;
        }
        o.open(1, "Material", &m.attributes);
        for e in &m.entries {
            o.entry(2, e);
        }
        o.close(1, "Material");
    }
    o.s.push_str("</LightBurnLibrary>");
    if lib.format.final_newline {
        o.s.push('\n');
    }
    if lib.format.crlf { o.s.replace('\n', "\r\n") } else { o.s }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Engrave &amp; fill\" NoThickTitle=\"Finish\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Finish/Engrave &amp; fill\" />\r\n        <maxPower Value=\"38\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"25000\" />\r\n        <interval Value=\"0.025\" />\r\n        <angle Value=\"45\" />\r\n        <priority Value=\"0\" />\r\n        <SubLayer type=\"Scan\" index=\"1\">\r\n          <maxPower Value=\"11\" />\r\n          <speed Value=\"750\" />\r\n          <subname Value=\"light\" />\r\n        </SubLayer>\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n  <Material name=\"Wood\">\r\n    <Entry Thickness=\"3.0000\" Desc=\"Cut\" NoThickTitle=\"\">\r\n      <CutSetting type=\"Cut\">\r\n        <index Value=\"0\" />\r\n        <maxPower Value=\"80\" />\r\n        <speed Value=\"20\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

    #[test]
    fn round_trip_is_byte_identical() {
        let lib = parse_clb(SAMPLE).unwrap();
        assert_eq!(lib.materials.len(), 2);
        assert!(lib.format.bom && lib.format.crlf && lib.format.space_before_slash);
        assert_eq!(lib.format.indent, 2);
        assert_eq!(write_clb(&lib), SAMPLE);
    }

    #[test]
    fn tree_values() {
        let lib = parse_clb(SAMPLE).unwrap();
        let e = &lib.materials[0].entries[0];
        assert_eq!(e.description(), "Engrave & fill");
        assert_eq!(e.thickness(), None);
        assert_eq!(e.group_title(), "Finish");
        assert_eq!(e.link_path("Steel"), "Steel/Finish/Engrave & fill");
        let w = &lib.materials[1].entries[0];
        assert_eq!(w.thickness(), Some(3.0));
        assert_eq!(w.group_title(), "3 mm");
        assert_eq!(lib.find_path("Wood/3 mm/Cut"), Some((1, 0)));
        let (layer, _) = e.to_layer("layer-9").unwrap();
        assert_eq!(layer.settings.power_pct, 38.0);
        assert_eq!(layer.settings.frequency_khz, 25.0);
        assert_eq!(layer.sub_layers.len(), 1);
        assert_eq!(layer.sub_layers[0].power_pct, 11.0);
    }

    #[test]
    fn structural_writer_matches_source_for_unchanged_values() {
        // Dropping the kept source forces the structural writer: it must still produce the same text.
        let mut lib = parse_clb(SAMPLE).unwrap();
        for m in &mut lib.materials {
            for e in &mut m.entries {
                e.source = None;
            }
        }
        assert_eq!(write_clb(&lib), SAMPLE);
    }

    #[test]
    fn edits_create_duplicate_and_merge() {
        let mut lib = parse_clb(SAMPLE).unwrap();
        let mut e = lib.materials[0].entries[0].clone();
        e.set_meta(Some(2.5), "", "Thin");
        assert_eq!(e.attributes[0].1, "2.5000");
        lib.material_mut("Steel").entries.push(e);
        lib.relink_paths();
        let text = write_clb(&lib);
        assert!(text.contains("<LinkPath Value=\"Steel/2.5 mm/Thin\" />"), "{text}");
        let again = parse_clb(&text).unwrap();
        assert_eq!(write_clb(&again), text);
        assert_eq!(again.entry_count(), 3);

        let mut other = Library::new();
        let (cs, _) = cut_setting_from_layer(&doc::Layer::new("l".into(), "Mine", "#000")).unwrap();
        other.material_mut("Brass").entries.push(LibEntry::new(None, "Finish", "Engrave", cs));
        other.materials.push(again.materials[1].clone());
        let added = lib.merge(&other).unwrap();
        assert_eq!(added, 1, "the Wood entry is already there");
        assert_eq!(lib.materials.last().unwrap().name(), "Brass");
    }

    #[test]
    fn layer_to_entry_and_back() {
        let mut l = doc::Layer::new("layer-1".into(), "Colour", "#000");
        l.settings.mode = doc::FillMode::Fill;
        l.settings.power_pct = 42.0;
        l.settings.frequency_khz = 120.0;
        l.settings.pulse_width_ns = Some(200.0);
        l.settings.hatch.interval_mm = 0.02;
        let mut sub = l.settings.clone();
        sub.power_pct = 10.0;
        l.sub_layers.push(sub);
        let (cs, _) = cut_setting_from_layer(&l).unwrap();
        let mut lib = Library::new();
        lib.material_mut("Steel").entries.push(LibEntry::new(None, "Colors", "Blue", cs));
        lib.relink_paths();
        let text = write_clb(&lib);
        assert!(text.starts_with('\u{feff}'));
        let back = parse_clb(&text).unwrap();
        let (layer, _) = back.materials[0].entries[0].to_layer("x").unwrap();
        assert_eq!(layer.settings.mode, doc::FillMode::Fill);
        assert_eq!(layer.settings.power_pct, 42.0);
        assert_eq!(layer.settings.frequency_khz, 120.0);
        assert_eq!(layer.settings.pulse_width_ns, Some(200.0));
        assert_eq!(layer.settings.hatch.interval_mm, 0.02);
        assert_eq!(layer.sub_layers.len(), 1);
        assert_eq!(layer.sub_layers[0].power_pct, 10.0);
        assert_eq!(back.materials[0].entries[0].settings[0].params.other.get("LinkPath").map(String::as_str), Some("Steel/Colors/Blue"));
    }

    #[test]
    fn hostile_libraries_are_bounded() {
        // Empty materials are capped (each would otherwise cost a linear search on every merge).
        let mut text = String::from("<LightBurnLibrary>");
        for i in 0..=MAX_MATERIALS {
            text.push_str(&format!("<Material name=\"m{i}\"/>"));
        }
        text.push_str("</LightBurnLibrary>");
        assert!(matches!(parse_clb(&text), Err(LbrnError::TooLarge(_))));

        // Merging two large libraries is linear (the old merge compared every pair: minutes for 2 x 5000).
        let big = |tag: &str| {
            let mut t = String::from("<LightBurnLibrary><Material name=\"M\">");
            for i in 0..5000 {
                t.push_str(&format!("<Entry Thickness=\"-1.0000\" Desc=\"{tag}{i}\" NoThickTitle=\"t\"/>"));
            }
            t.push_str("</Material></LightBurnLibrary>");
            parse_clb(&t).unwrap()
        };
        let (mut a, b) = (big("a"), big("b"));
        let started = std::time::Instant::now();
        assert_eq!(a.merge(&b).unwrap(), 5000);
        assert_eq!(a.merge(&b).unwrap(), 0, "already there");
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());

        // Over the entry limit: refused and nothing changes.
        let mut full = Library::new();
        let m = full.material_mut("M");
        for i in 0..MAX_ENTRIES - 10 {
            let mut e = LibEntry::default();
            e.set_meta(None, "t", &format!("x{i}"));
            m.entries.push(e);
        }
        let before = full.clone();
        assert!(matches!(full.merge(&b), Err(LbrnError::TooLarge(_))));
        assert_eq!(full, before, "a refused merge leaves the library as it was");
    }

    #[test]
    fn refuses_other_files() {
        assert!(parse_clb("<LightBurnProject/>").is_err());
        assert!(parse_clb("<LightBurnLibrary><Junk/></LightBurnLibrary>").is_err());
        assert!(parse_clb("not xml").is_err());
        let empty = parse_clb("<LightBurnLibrary>\n</LightBurnLibrary>").unwrap();
        assert_eq!(empty.entry_count(), 0);
    }
}
