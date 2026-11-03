//! File commands: `doc.new`, `doc.load` and `doc.save` (the native `.openetch` project), `doc.import_lbrn2` and
//! `doc.export_lbrn2` (LightBurn). File contents travel as text in the parameters and results; the engine reads and
//! writes the actual files. Replacing the document is one undo step like any other edit.

use std::collections::HashMap;

use doc::file::{from_openetch_json, try_to_openetch_json};
use doc::{Document, ObjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::layer::validate_settings;
use super::util::{self, invalid};
use crate::{Command, CommandError, Result, Risk};

/// Largest accepted file content (the engine's upload limit is lower).
const MAX_CONTENT_BYTES: usize = lbrn::MAX_BYTES;
/// Largest galvo field accepted, in mm.
const MAX_FIELD_MM: f64 = 2000.0;
/// Document `extras` key: the field size in mm the last merging LightBurn import was read with, which
/// `doc.export_lbrn2` uses by default so the art goes back where it came from. (A replacing import sets the
/// document's `field_mm` instead.)
const IMPORT_FIELD_KEY: &str = "lbrn2._field_mm";

/// Document `extras` key: the bed `"width height"` in mm a merging gantry LightBurn import was read with (the
/// [`IMPORT_FIELD_KEY`] of beds).
const IMPORT_WORKSPACE_KEY: &str = "lbrn2._workspace_mm";
/// Document `extras` key: the LightBurn origin corner of the device the document was made or imported for
/// ([`WorkspaceOrigin`] name), which `doc.export_lbrn2` uses by default.
pub const ORIGIN_KEY: &str = "lbrn2._origin";
/// Largest bed side accepted, in mm.
const MAX_BED_MM: f64 = lbrn::MAX_BED_MM;

/// The field size a merging LightBurn import remembered in `doc`, when usable.
fn import_field(doc: &Document) -> Option<f64> {
    let f = doc.extras.get(IMPORT_FIELD_KEY)?.parse::<f64>().ok()?;
    (f.is_finite() && f > 0.0 && f <= MAX_FIELD_MM).then_some(f)
}

/// The bed a merging gantry LightBurn import remembered in `doc`, when usable.
fn import_workspace(doc: &Document) -> Option<[f64; 2]> {
    let text = doc.extras.get(IMPORT_WORKSPACE_KEY)?;
    let mut it = text.split_whitespace().map(|v| v.parse::<f64>().ok());
    let (w, h) = (it.next()??, it.next()??);
    [w, h].iter().all(|v| v.is_finite() && *v > 0.0 && *v <= MAX_BED_MM).then_some([w, h])
}

/// Where LightBurn's origin is on a device: the centre of the field for a galvo, else the bed corner its
/// coordinates are measured from (LightBurn's Device Settings "Origin", stored as the device's and the project's
/// `MirrorX` / `MirrorY`; see [`lbrn::Frame::Bed`]). Front = toward the operator = bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOrigin {
    /// A galvo: LightBurn coordinates are absolute, the field centre is (W/2, H/2).
    Center,
    /// Front left (neither mirror flag): LightBurn's usual GCode origin.
    BottomLeft,
    /// Rear left (`MirrorY`).
    TopLeft,
    /// Front right (`MirrorX`; inferred, unconfirmed).
    BottomRight,
    /// Rear right (both flags; inferred, unconfirmed): DSP machines usually.
    TopRight,
}

impl WorkspaceOrigin {
    /// The snake_case name (`"bottom_left"`).
    pub fn name(self) -> &'static str {
        match self {
            WorkspaceOrigin::Center => "center",
            WorkspaceOrigin::BottomLeft => "bottom_left",
            WorkspaceOrigin::TopLeft => "top_left",
            WorkspaceOrigin::BottomRight => "bottom_right",
            WorkspaceOrigin::TopRight => "top_right",
        }
    }

    /// Parse [`WorkspaceOrigin::name`].
    pub fn parse(s: &str) -> Option<Self> {
        [Self::Center, Self::BottomLeft, Self::TopLeft, Self::BottomRight, Self::TopRight].into_iter().find(|o| o.name() == s)
    }

    /// The corner as LightBurn's `(MirrorX, MirrorY)`; `None` for the centre (a galvo).
    pub fn mirror(self) -> Option<(bool, bool)> {
        match self {
            WorkspaceOrigin::Center => None,
            WorkspaceOrigin::BottomLeft => Some((false, false)),
            WorkspaceOrigin::TopLeft => Some((false, true)),
            WorkspaceOrigin::BottomRight => Some((true, false)),
            WorkspaceOrigin::TopRight => Some((true, true)),
        }
    }

    /// The corner LightBurn's `(MirrorX, MirrorY)` stand for.
    pub fn from_mirror(mirror_x: bool, mirror_y: bool) -> Self {
        match (mirror_x, mirror_y) {
            (false, false) => WorkspaceOrigin::BottomLeft,
            (false, true) => WorkspaceOrigin::TopLeft,
            (true, false) => WorkspaceOrigin::BottomRight,
            (true, true) => WorkspaceOrigin::TopRight,
        }
    }
}

/// The origin a document was made or imported for ([`ORIGIN_KEY`]).
fn doc_origin(doc: &Document) -> Option<WorkspaceOrigin> {
    doc.extras.get(ORIGIN_KEY).and_then(|o| WorkspaceOrigin::parse(o))
}

/// A bed size from the caller, checked.
fn bed(cmd: &str, [w, h]: [f64; 2]) -> Result<[f64; 2]> {
    util::positive(cmd, "workspace_mm[0]", w)?;
    util::positive(cmd, "workspace_mm[1]", h)?;
    if w > MAX_BED_MM || h > MAX_BED_MM {
        return Err(invalid(cmd, format!("workspace_mm sides must be at most {MAX_BED_MM} mm (got [{w}, {h}])")));
    }
    Ok([w, h])
}

/// How a LightBurn import or export lays the document out: a galvo field, or a bed with its origin corner.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Layout {
    Galvo(f64),
    /// A non-square galvo field `[width, height]` (LightBurn's separate field Width and Height): LightBurn measures
    /// from its lower-left corner with y up, as for a square field, so it converts like a front-left bed.
    Field([f64; 2]),
    Bed([f64; 2], WorkspaceOrigin),
}

/// A galvo field `[width, height]`, checked: [`Layout::Galvo`] when square, else [`Layout::Field`].
fn galvo_field(cmd: &str, [w, h]: [f64; 2]) -> Result<Layout> {
    for (name, v) in [("workspace_mm[0]", w), ("workspace_mm[1]", h)] {
        util::positive(cmd, name, v)?;
        if v > MAX_FIELD_MM {
            return Err(invalid(cmd, format!("a galvo field side must be at most {MAX_FIELD_MM} mm (got {v})")));
        }
    }
    Ok(if (w - h).abs() < 1e-9 { Layout::Galvo(w) } else { Layout::Field([w, h]) })
}

/// The non-square galvo field a document was made or imported for: its `workspace_mm` with a centre origin.
fn doc_galvo_field(doc: &Document) -> Option<[f64; 2]> {
    (doc_origin(doc) == Some(WorkspaceOrigin::Center)).then_some(doc.workspace_mm).flatten()
}

impl Layout {
    /// The LightBurn frame for this layout; a bed's mirror flags are `mirror` (the file's or the target's).
    fn frame(self, mirror: (bool, bool)) -> lbrn::Frame {
        match self {
            Layout::Galvo(field_mm) => lbrn::Frame::Galvo { field_mm },
            Layout::Field([width_mm, height_mm]) => {
                lbrn::Frame::Bed { width_mm, height_mm, mirror_x: false, mirror_y: false }
            }
            Layout::Bed([width_mm, height_mm], _) => {
                lbrn::Frame::Bed { width_mm, height_mm, mirror_x: mirror.0, mirror_y: mirror.1 }
            }
        }
    }
}

/// The layout from the caller's `field_mm` / `workspace_mm` / `origin`, else the document's. A corner origin is a
/// bed (its size: `workspace_mm`, else `remembered`, else the document's workspace); otherwise a galvo field
/// (`field_mm`; a non-square one from `workspace_mm`; else the remembered field, else for a document made for a
/// non-square galvo field its workspace, else the document's `field_mm`).
fn layout(
    cmd: &str,
    field_mm: Option<f64>,
    workspace_mm: Option<[f64; 2]>,
    origin: Option<WorkspaceOrigin>,
    remembered: (Option<f64>, Option<[f64; 2]>),
    doc: &Document,
) -> Result<Layout> {
    match origin {
        Some(o) if o != WorkspaceOrigin::Center => {
            if field_mm.is_some() {
                return Err(invalid(cmd, "field_mm is for galvo layouts; a gantry layout takes workspace_mm"));
            }
            let size = workspace_mm.or(remembered.1).or_else(|| doc.workspace()).ok_or_else(|| {
                invalid(cmd, "workspace_mm is required: the document has no workspace size; pass [width, height] in mm")
            })?;
            Ok(Layout::Bed(bed(cmd, size)?, o))
        }
        _ => {
            if let Some(size) = workspace_mm {
                // A galvo with separate field Width and Height.
                if field_mm.is_some() {
                    return Err(invalid(
                        cmd,
                        "pass field_mm (a square galvo field) or workspace_mm (a non-square one), not both",
                    ));
                }
                return galvo_field(cmd, size);
            }
            if field_mm.is_none() && remembered.0.is_none() {
                let galvo = doc_galvo_field(doc);
                if let Some(size) = remembered.1.filter(|_| galvo.is_some()).or(galvo) {
                    return galvo_field(cmd, size);
                }
            }
            Ok(Layout::Galvo(field(cmd, field_mm.or(remembered.0), doc)?))
        }
    }
}

/// The result fields describing a layout: `field_mm`, `workspace_mm` (null for a galvo) and `origin`.
fn layout_json(l: Layout, r: &mut Value) {
    match l {
        Layout::Galvo(f) => {
            r["field_mm"] = json!(f);
            r["workspace_mm"] = Value::Null;
            r["origin"] = json!(WorkspaceOrigin::Center.name());
        }
        Layout::Field([w, h]) => {
            r["field_mm"] = json!(w);
            r["workspace_mm"] = json!([w, h]);
            r["origin"] = json!(WorkspaceOrigin::Center.name());
        }
        Layout::Bed(size, o) => {
            r["field_mm"] = Value::Null;
            r["workspace_mm"] = json!(size);
            r["origin"] = json!(o.name());
        }
    }
}

fn content<'a>(cmd: &str, text: &'a str) -> Result<&'a str> {
    if text.len() > MAX_CONTENT_BYTES {
        return Err(invalid(cmd, format!("content is larger than {} MB", MAX_CONTENT_BYTES / (1024 * 1024))));
    }
    Ok(text)
}

/// The field size to convert with: the argument, else the document's, else an error saying what to pass.
fn field(cmd: &str, given: Option<f64>, doc: &Document) -> Result<f64> {
    let f = match given.or(doc.field_mm) {
        Some(f) => f,
        None => {
            return Err(invalid(
                cmd,
                "field_mm is required: the document has no field size; pass the galvo field size in mm the \
                 LightBurn project is laid out for (e.g. 150)",
            ));
        }
    };
    util::positive(cmd, "field_mm", f)?;
    if f > MAX_FIELD_MM {
        return Err(invalid(cmd, format!("field_mm must be at most {MAX_FIELD_MM} (got {f})")));
    }
    Ok(f)
}

/// Raise `next_id` past every numbered id, so later allocations never collide.
fn fix_next_id(doc: &mut Document) {
    let ids = doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id)).chain(doc.groups.iter().map(|g| &g.id));
    if let Some(max) = ids.filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok())).max() {
        doc.next_id = doc.next_id.max(max.saturating_add(1));
    }
}

fn object_ids(doc: &Document) -> Vec<ObjectId> {
    doc.objects.iter().map(|o| o.id.clone()).collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewInput {
    /// Name of the new document. Default "Untitled".
    pub name: Option<String>,
    /// Galvo field size in mm the new document is laid out for. Default: the current document's (the OpenEtch
    /// engine passes the active device's, so New always starts at the machine's field).
    pub field_mm: Option<f64>,
    /// Bed [width, height] in mm, for a gantry device (with a corner `origin`); the engine passes the active
    /// device's. Then field_mm defaults to the smaller side. With the centre origin it is a galvo's separate field
    /// Width and Height (LightBurn's non-square galvo field): the document's workspace, field_mm its width.
    pub workspace_mm: Option<[f64; 2]>,
    /// The device's LightBurn origin: "center" (galvo, default) or a bed corner, used by doc.export_lbrn2.
    pub origin: Option<WorkspaceOrigin>,
    /// Settings for the new document's layer (LightBurn's "Load default layer settings on New"; the engine passes
    /// the saved default when that option is on). Default: the built-in defaults.
    pub layer_settings: Option<doc::CutSettings>,
    /// Optimization settings of the new document (the engine passes the saved defaults, LightBurn's "Set as
    /// Defaults"). Default: the built-in defaults.
    pub optimization: Option<doc::Optimization>,
    /// Start from an example project (`example.list` lists them) instead of an empty document.
    pub example: Option<String>,
    /// With `example`: finishes from the material library (fills with a result colour; the OpenEtch engine passes
    /// the selected library's) to offer as the colour examples' choices instead of the built-in ones.
    #[serde(default)]
    pub finishes: Vec<super::examples::ExampleFinish>,
}

/// `doc.new`.
pub struct DocNew;

impl Command for DocNew {
    type Input = NewInput;
    const ID: &'static str = "doc.new";
    const TITLE: &'static str = "New document";
    const DESCRIPTION: &'static str = "Replace the document with an empty one: one default layer, no stock, no \
        objects (with `example`: an example project from example.list instead). field_mm: the given one, else the engine's active device field (else the current one); a gantry \
        device gives workspace_mm [width, height] and its origin corner instead. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: NewInput) -> Result<(Value, String)> {
        let name = input.name.as_deref().map(|n| util::name(Self::ID, "name", n)).transpose()?;
        let corner = input.origin.filter(|o| *o != WorkspaceOrigin::Center);
        // With a centre origin, workspace_mm is a galvo's separate field Width and Height: a square one is just
        // field_mm; a non-square one is the workspace, its width the field_mm, and the centre origin recorded.
        let (field_mm, workspace_mm) = match (input.workspace_mm, corner) {
            (Some(size), Some(_)) => {
                let [w, h] = bed(Self::ID, size)?;
                let f = input.field_mm.map(|f| field(Self::ID, Some(f), doc)).transpose()?;
                (Some(f.unwrap_or(w.min(h))), Some([w, h]))
            }
            (Some(size), None) => match layout(Self::ID, input.field_mm, Some(size), None, (None, None), doc)? {
                Layout::Field([w, h]) => (Some(w), Some([w, h])),
                Layout::Galvo(f) => (Some(f), None),
                Layout::Bed(..) => return Err(invalid(Self::ID, "a centre origin is not a bed")),
            },
            (None, _) => (input.field_mm.map(|f| field(Self::ID, Some(f), doc)).transpose()?.or(doc.field_mm), None),
        };
        let mut fresh = Document { revision: doc.revision, field_mm, workspace_mm, ..Document::default() };
        if corner.is_none() && workspace_mm.is_some() {
            fresh.extras.insert(ORIGIN_KEY.into(), WorkspaceOrigin::Center.name().into());
        }
        if let Some(n) = name {
            fresh.name = n;
        }
        if let Some(o) = corner {
            fresh.extras.insert(ORIGIN_KEY.into(), o.name().into());
        }
        if let Some(s) = input.layer_settings {
            super::layer::validate_settings(Self::ID, &s)?;
            fresh.layers[0].settings = s;
        }
        if let Some(o) = input.optimization {
            if let Some(p) = o.problem() {
                return Err(invalid(Self::ID, format!("optimization: {p}")));
            }
            fresh.optimization = o;
        }
        if let Some(ex) = &input.example {
            super::examples::build(ex, &mut fresh, &input.finishes)?;
            if let Some(n) = &input.name {
                fresh.name = util::name(Self::ID, "name", n)?;
            }
        }
        let summary = match &input.example {
            Some(ex) => format!("new document from example {ex}"),
            None => format!("new document \"{}\"", fresh.name),
        };
        *doc = fresh;
        let r = json!({
            "created": [], "affected": [], "name": doc.name, "bounds": Value::Null, "field_mm": doc.field_mm,
            "workspace_mm": doc.workspace_mm,
        });
        Ok((r, summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadInput {
    /// The .openetch project as text (the JSON from doc.save or a .openetch file), not a path.
    pub content: String,
}

/// `doc.load`.
pub struct DocLoad;

impl Command for DocLoad {
    type Input = LoadInput;
    const ID: &'static str = "doc.load";
    const TITLE: &'static str = "Load project";
    const DESCRIPTION: &'static str = "Replace the document with a saved .openetch project given as text (JSON \
        {\"format\": \"openetch\", \"version\": 1, \"document\": ...}). Other formats and newer versions are refused. \
        Keeps the current field_mm when the file has none. Undoable. Returns the object ids as `created`, the layer \
        ids, the name and field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LoadInput) -> Result<(Value, String)> {
        let text = content(Self::ID, &input.content)?;
        let mut loaded = from_openetch_json(text)
            .map_err(|e| invalid(Self::ID, format!("content is not a usable .openetch project: {e}")))?;
        for l in &loaded.layers {
            for s in std::iter::once(&l.settings).chain(&l.sub_layers) {
                validate_settings(Self::ID, s)
                    .map_err(|e| invalid(Self::ID, format!("layer {} (\"{}\"): {e}", l.id, l.name)))?;
            }
        }
        loaded.field_mm = loaded.field_mm.or(doc.field_mm);
        loaded.revision = doc.revision;
        // Projects saved before variable text was modelled keep LightBurn's counter only in the kept element.
        if loaded.variables.is_default()
            && let Some(v) = lbrn::text::variables_from_extras(&loaded.extras)
        {
            loaded.variables = v;
        }
        let summary = format!("loaded project \"{}\" ({} object(s))", loaded.name, loaded.objects.len());
        *doc = loaded;
        let mut r = util::edit_result(doc, &object_ids(doc), &[]);
        r["name"] = json!(doc.name);
        r["layers"] = json!(doc.layers.iter().map(|l| &l.id).collect::<Vec<_>>());
        r["field_mm"] = json!(doc.field_mm);
        Ok((r, summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveInput {}

/// `doc.save`.
pub struct DocSave;

impl Command for DocSave {
    type Input = SaveInput;
    const ID: &'static str = "doc.save";
    const TITLE: &'static str = "Save project";
    const DESCRIPTION: &'static str = "Serialize the document as a .openetch project: returns {content (JSON text), \
        revision}. Read-only; the engine's project routes write it to disk.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _input: SaveInput) -> Result<(Value, String)> {
        let content = try_to_openetch_json(doc).map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok((json!({ "content": content, "revision": doc.revision }), String::new()))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Swap the whole document; field_mm becomes the one the file was read with.
    #[default]
    Replace,
    /// Add the file's layers and objects to the document.
    Merge,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportLbrn2Input {
    /// The LightBurn .lbrn2 file contents as text (XML), not a path.
    pub content: String,
    /// "replace" (default) swaps the whole document, field_mm becoming the one used here; "merge" adds layers
    /// (reusing one whose name and settings match) and objects with fresh ids.
    #[serde(default)]
    pub mode: ImportMode,
    /// Galvo field size in mm the LightBurn project was laid out for (LightBurn files do not record it). Leave it
    /// out and the OpenEtch engine picks it (see the command description); without the engine, the document's
    /// field_mm. Remembered for doc.export_lbrn2, so exports go back to the same place.
    pub field_mm: Option<f64>,
    /// Gantry devices: the bed [width, height] in mm the project was laid out for (with a corner `origin`; the
    /// engine fills it from the device). LightBurn measures from the bed corner the file's MirrorX/MirrorY record.
    pub workspace_mm: Option<[f64; 2]>,
    /// "center" (galvo, default) or the device's bed corner (gantry): bottom_left, top_left, bottom_right,
    /// top_right. Remembered for doc.export_lbrn2.
    pub origin: Option<WorkspaceOrigin>,
    /// File name (without extension). A merge names the group it creates after it (default "Imported LightBurn");
    /// the engine names later downloads after it when a replacing import is not saved as a project.
    pub name: Option<String>,
}

/// The `DeviceName` attribute of a LightBurn project's root element (LightBurn 2.x writes the device's display name,
/// e.g. `JCZFiber (Unknown)`), without parsing the whole file: only the root start tag near the top is read.
pub fn lbrn2_device_name(content: &str) -> Option<String> {
    // The root start tag follows the XML declaration (and possibly comments) within the first few hundred bytes.
    let mut cut = content.len().min(64 * 1024);
    while !content.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &content[..cut];
    let start = head.find("<LightBurnProject")? + "<LightBurnProject".len();
    let mut rest = &head[start..];
    loop {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('>') || rest.starts_with('/') {
            return None;
        }
        let eq = rest.find('=')?;
        let attr = rest[..eq].trim();
        rest = rest[eq + 1..].trim_start();
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let end = rest[1..].find(quote)? + 1;
        let value = &rest[1..end];
        rest = &rest[end + 1..];
        if attr == "DeviceName" {
            let v = unescape_xml(value);
            let v = v.trim();
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
}

/// The five predefined XML entities and numeric character references (anything else is kept as written).
fn unescape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').map(|d| d.parse().ok()))
                .flatten()
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The name of the group holding a merged file's objects.
fn merged_group_name(name: Option<&str>) -> String {
    let clean: String = name.unwrap_or_default().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let clean = clean.trim();
    if clean.is_empty() { "Imported LightBurn".into() } else { clean.chars().take(200).collect() }
}

/// `doc.import_lbrn2`.
pub struct ImportLbrn2;

impl Command for ImportLbrn2 {
    type Input = ImportLbrn2Input;
    const ID: &'static str = "doc.import_lbrn2";
    const TITLE: &'static str = "Import LightBurn project";
    const DESCRIPTION: &'static str = "Import a LightBurn .lbrn2 project or .lbt template given as text. Cut \
        settings become layers, shapes objects and groups in the field frame; the rest is kept for \
        doc.export_lbrn2. replace swaps the document; merge adds the file's objects as one group. Files do not \
        record the workspace size; without field_mm the engine uses the device the file names (DeviceName), else \
        the active device, else the document's, else the machine's. Gantry devices give workspace_mm [w, h] and a \
        corner origin instead. Returns field_mm, workspace_mm, origin, field_source, created/reused ids, warnings, \
        bounds. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: ImportLbrn2Input) -> Result<(Value, String)> {
        let id = Self::ID;
        let text = content(id, &input.content)?;
        let lay = layout(id, input.field_mm, input.workspace_mm, input.origin, (None, None), doc)?;
        // A legacy `.lbrn` (vertex elements) reads as `.lbrn2` (crate fileio).
        let text = super::files::legacy_lbrn(id, text)?;
        let project = lbrn::parse(&text).map_err(|e| invalid(id, e.to_string()))?;
        // A bed uses the file's own origin corner, so the art lands where LightBurn showed it.
        let frame = lay.frame(project.mirror());
        let lbrn::Imported { layers, objects, groups, mut extras, variables, warnings } =
            lbrn::to_document_in(&project, frame).map_err(|e| invalid(id, e.to_string()))?;
        let notes = super::files::notes_from_extras(&extras);
        // A square size for what needs one (the field of a galvo; the bed's smaller side).
        let (field_mm, workspace_mm) = match lay {
            Layout::Galvo(f) => (f, None),
            Layout::Field([w, h]) => {
                extras.insert(ORIGIN_KEY.into(), WorkspaceOrigin::Center.name().into());
                (w, Some([w, h]))
            }
            Layout::Bed([w, h], o) => {
                extras.insert(ORIGIN_KEY.into(), o.name().into());
                (w.min(h), Some([w, h]))
            }
        };
        // Nothing below can fail, so the document is never left half-changed.
        let (mut created_layers, mut reused_layers, mut created) = (Vec::new(), Vec::new(), Vec::new());
        let mut created_groups = Vec::new();
        match input.mode {
            ImportMode::Replace => {
                // The document is now laid out for the field the file was read with, so an export puts every
                // shape back where LightBurn had it.
                let mut fresh = Document {
                    revision: doc.revision,
                    field_mm: Some(field_mm),
                    workspace_mm,
                    layers,
                    objects,
                    groups,
                    variables,
                    notes,
                    extras,
                    ..Document::default()
                };
                fix_next_id(&mut fresh);
                created_layers = fresh.layers.iter().map(|l| l.id.clone()).collect();
                created = object_ids(&fresh);
                created_groups = fresh.groups.iter().map(|g| g.id.clone()).collect();
                *doc = fresh;
            }
            ImportMode::Merge => {
                // Check the ids up front, so the allocations below cannot fail half-way.
                let needed = (layers.len() + objects.len() + groups.len() + 1) as u64;
                if doc.next_id.saturating_add(needed) > doc::MAX_ID {
                    return Err(doc::IdsExhausted(doc.next_id).into());
                }
                doc.field_mm = doc.field_mm.or(Some(field_mm));
                // The document keeps its own field size; export uses the merged file's by default.
                match workspace_mm {
                    None => {
                        doc.extras.insert(IMPORT_FIELD_KEY.into(), field_mm.to_string());
                        doc.extras.remove(IMPORT_WORKSPACE_KEY);
                        doc.extras.remove(ORIGIN_KEY);
                    }
                    Some([w, h]) => {
                        doc.workspace_mm = doc.workspace_mm.or(Some([w, h]));
                        doc.extras.insert(IMPORT_WORKSPACE_KEY.into(), format!("{w} {h}"));
                        doc.extras.remove(IMPORT_FIELD_KEY);
                        if let Some(o) = extras.get(ORIGIN_KEY) {
                            doc.extras.insert(ORIGIN_KEY.into(), o.clone());
                        }
                    }
                }
                let before = doc.layers.len();
                let mut map: HashMap<String, String> = HashMap::new();
                for mut l in layers {
                    let same = doc.layers[..before]
                        .iter()
                        .find(|e| e.name == l.name && e.settings == l.settings && e.sub_layers == l.sub_layers)
                        .map(|e| e.id.clone());
                    let old = l.id.clone();
                    let target = match same {
                        Some(e) => {
                            if !reused_layers.contains(&e) {
                                reused_layers.push(e.clone());
                            }
                            e
                        }
                        None => {
                            l.id = doc.alloc_id("layer")?;
                            created_layers.push(l.id.clone());
                            let new_id = l.id.clone();
                            doc.layers.push(l);
                            new_id
                        }
                    };
                    map.insert(old, target);
                }
                let mut objects = objects;
                for o in &mut objects {
                    if let Some(layer) = map.get(&o.layer) {
                        o.layer = layer.clone();
                    }
                    o.id = doc.alloc_id("obj")?;
                    created.push(o.id.clone());
                }
                // The file's objects come in as one group when there is more than one item (LightBurn's "group
                // imported shapes"), its own groups nested inside. The file's document-level LightBurn data
                // (root attributes, notes, thumbnail) stays with the file.
                let top_items = objects.iter().filter(|o| o.group.is_none()).count()
                    + groups.iter().filter(|g| g.parent.is_none()).count();
                let outer = (top_items > 1).then(|| doc.alloc_id("group")).transpose()?;
                let mut group_map: HashMap<String, String> = HashMap::new();
                for g in &groups {
                    group_map.insert(g.id.clone(), doc.alloc_id("group")?);
                }
                if let Some(outer) = &outer {
                    doc.groups.push(doc::Group::new(outer.clone(), merged_group_name(input.name.as_deref()), None));
                    created_groups.push(outer.clone());
                }
                for mut g in groups {
                    g.id = group_map[&g.id].clone();
                    g.parent = match &g.parent {
                        Some(p) => group_map.get(p).cloned(),
                        None => outer.clone(),
                    };
                    created_groups.push(g.id.clone());
                    doc.groups.push(g);
                }
                for mut o in objects {
                    o.group = match &o.group {
                        Some(g) => group_map.get(g).cloned(),
                        None => outer.clone(),
                    };
                    doc.objects.push(o);
                }
                doc.prune_groups();
            }
        }
        let mode = match input.mode {
            ImportMode::Replace => "replace",
            ImportMode::Merge => "merge",
        };
        let summary = format!(
            "imported LightBurn project ({mode}): {} object(s), {} new layer(s), {} reused",
            created.len(),
            created_layers.len(),
            reused_layers.len()
        );
        let mut r = util::edit_result(doc, &created, &[]);
        r["created_groups"] = json!(created_groups);
        r["created_layers"] = json!(created_layers);
        r["reused_layers"] = json!(reused_layers);
        // LAY-034: settings the file's layers have on that the job will not execute.
        let mut warnings = warnings;
        let mut unexecuted = Vec::new();
        for lid in created_layers.iter().chain(&reused_layers) {
            let Some(l) = doc.layer(lid) else { continue };
            let list = lbrn::unexecuted_settings(l);
            for u in list.iter().filter(|u| u.active == Some(true)) {
                let sub = u.sub_layer.map(|i| format!(" (sub-layer {})", i + 1)).unwrap_or_default();
                warnings.push(format!("{}{sub}: {} is on in the file but will not be executed by OpenEtch", l.name, u.setting));
            }
            if !list.is_empty() {
                unexecuted.push(json!({ "layer_id": l.id, "name": l.name, "unsupported": list }));
            }
        }
        r["warnings"] = json!(warnings);
        r["unsupported_settings"] = json!(unexecuted);
        r["mode"] = json!(mode);
        layout_json(lay, &mut r);
        // The engine replaces this when it chose the size itself.
        let given = input.field_mm.is_some() || input.workspace_mm.is_some();
        r["field_source"] = json!(if given { "caller" } else { "document" });
        Ok((r, summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportLbrn2Input {
    /// Galvo field size in mm to lay the LightBurn project out for. Default: the field size the last LightBurn
    /// import was read with, else the document's field_mm.
    pub field_mm: Option<f64>,
    /// Gantry: the bed [width, height] in mm (with a corner `origin`). Default: the bed the last LightBurn import
    /// was read with, else the document's workspace.
    pub workspace_mm: Option<[f64; 2]>,
    /// "center" (galvo) or the target device's bed corner, whose MirrorX/MirrorY are written to the project.
    /// Default: the origin the document was made or imported for, else center (bottom_left for a document with a
    /// workspace_mm).
    pub origin: Option<WorkspaceOrigin>,
}

/// `doc.export_lbrn2`.
pub struct ExportLbrn2;

impl Command for ExportLbrn2 {
    type Input = ExportLbrn2Input;
    const ID: &'static str = "doc.export_lbrn2";
    const TITLE: &'static str = "Export LightBurn project";
    const DESCRIPTION: &'static str = "Convert the document to a LightBurn .lbrn2 project: returns {content (XML \
        text), warnings, field_mm, workspace_mm, origin}. field_mm defaults to the one the last LightBurn import used, \
        so shapes go back where they were; a gantry document (workspace_mm) is laid out for its bed and origin \
        corner, whose MirrorX/MirrorY are written. Layers keep the LightBurn index and settings they were imported with; the stock outline \
        goes on tool layer T1; non-output and hidden layers stay so. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, input: ExportLbrn2Input) -> Result<(Value, String)> {
        let id = Self::ID;
        let origin = input.origin.or_else(|| doc_origin(doc)).unwrap_or(if doc.workspace_mm.is_some() {
            WorkspaceOrigin::BottomLeft
        } else {
            WorkspaceOrigin::Center
        });
        let remembered = (import_field(doc), import_workspace(doc));
        let lay = layout(id, input.field_mm, input.workspace_mm, Some(origin), remembered, doc)?;
        let frame = match lay {
            Layout::Bed(_, o) => lay.frame(o.mirror().unwrap_or_default()),
            _ => lay.frame((false, false)),
        };
        let (mut project, mut warnings) = lbrn::from_document_in(doc, frame).map_err(|e| invalid(id, e.to_string()))?;
        // LightBurn's page storage is unknown: only the active page is written, so say what is left out.
        let other_pages = doc.pages.iter().filter(|p| Some(&p.id) != doc.active_page.as_ref() && !p.objects.is_empty()).count();
        if other_pages > 0 {
            warnings.push(format!("only the active page is written: {other_pages} other page(s) with shapes are left out (save the project as .openetch to keep them)"));
        }
        super::files::notes_into_project(&mut project, &doc.notes);
        let mut r = json!({ "content": lbrn::write(&project), "warnings": warnings });
        layout_json(lay, &mut r);
        Ok((r, String::new()))
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::{Value, json};

    use crate::{CommandError, Invocation, Origin, Session};

    const LBRN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.00" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="Fill"/>
        <maxPower Value="30"/>
        <speed Value="2000"/>
        <frequency Value="100000"/>
        <QPulseWidth Value="200"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="1" W="20" H="10" Cr="0">
        <XForm>1 0 0 1 80 75</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0">
        <XForm>1 0 0 1 75 75</XForm>
        <VertList>V0 0c0x1c1x1V5 0c0x1c1x1V5 5c0x1c1x1</VertList>
        <PrimList>L0 1L1 2L2 0</PrimList>
    </Shape>
    <Shape Type="Bitmap" CutIndex="1" W="1" H="1" Data="">
        <XForm>1 0 0 1 75 75</XForm>
    </Shape>
</LightBurnProject>"#;

    fn session(field_mm: Option<f64>) -> Session {
        Session::new(Document { field_mm, ..Document::default() })
    }

    fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, CommandError> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn ok(s: &mut Session, id: &str, params: Value) -> Value {
        run(s, id, params).unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn strip_rev(d: &Document) -> Document {
        Document { revision: 0, ..d.clone() }
    }

    #[test]
    fn save_then_load_round_trips_and_undoes() {
        let mut s = session(Some(153.75));
        ok(&mut s, "layer.add", json!({"name": "Cut", "settings": {"pulse_width_ns": 50}}));
        ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 5, "layer": "layer-2"}));
        let saved = s.document().clone();
        let r = ok(&mut s, "doc.save", json!({}));
        assert_eq!(r["revision"], json!(2));
        assert_eq!(s.document().revision, 2, "save is read-only");
        let content = r["content"].as_str().unwrap().to_string();
        assert!(content.starts_with(r#"{"format":"openetch","version":1,"document":"#), "{content}");

        ok(&mut s, "doc.new", json!({"name": "Blank"}));
        assert_eq!(s.document().name, "Blank");
        assert!(s.document().objects.is_empty());
        assert_eq!(s.document().layers.len(), 1);
        assert_eq!(s.document().field_mm, Some(153.75));

        let r = ok(&mut s, "doc.load", json!({"content": content}));
        assert_eq!(r["created"], json!(["obj-3"]));
        assert_eq!(r["layers"], json!(["layer-1", "layer-2"]));
        assert_eq!(s.document().revision, 4, "revisions keep counting up");
        assert_eq!(strip_rev(s.document()), strip_rev(&saved));
        s.undo().unwrap();
        assert_eq!(s.document().name, "Blank");
        // doc.load started a fresh timeline whose start is the document it replaced.
        assert!(matches!(s.undo(), Err(CommandError::NothingTo("undo"))));
        s.redo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&saved));
    }

    #[test]
    fn load_keeps_field_when_the_file_has_none() {
        let mut s = session(Some(153.75));
        let without = doc::file::to_openetch_json(&Document::default());
        ok(&mut s, "doc.load", json!({"content": without}));
        assert_eq!(s.document().field_mm, Some(153.75));
        let with = doc::file::to_openetch_json(&Document { field_mm: Some(110.0), ..Document::default() });
        let r = ok(&mut s, "doc.load", json!({"content": with}));
        assert_eq!(r["field_mm"], json!(110.0));
        assert_eq!(s.document().field_mm, Some(110.0));
    }

    #[test]
    fn load_refuses_other_formats_and_bad_settings() {
        let mut s = session(None);
        let before = s.document().clone();
        let e = run(&mut s, "doc.load", json!({"content": LBRN})).unwrap_err();
        assert!(matches!(e, CommandError::InvalidInput { .. }) && e.to_string().contains("LightBurn"), "{e}");
        let newer = json!({"format": "openetch", "version": 9, "document": Document::default()}).to_string();
        let e = run(&mut s, "doc.load", json!({"content": newer})).unwrap_err();
        assert!(e.to_string().contains("update OpenEtch"), "{e}");
        let mut d = Document::default();
        d.layers[0].settings.passes = 5000;
        let e = run(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&d)})).unwrap_err();
        assert!(e.to_string().contains("passes"), "{e}");
        assert!(run(&mut s, "doc.load", json!({})).is_err());
        assert_eq!(s.document(), &before);
        assert!(s.history().is_empty());
    }

    #[test]
    fn import_replace_swaps_everything_but_field() {
        let mut s = session(None);
        ok(&mut s, "stock.define", json!({"shape": {"kind": "circle", "diameter_mm": 30}, "thickness_mm": 1}));
        let before = s.document().clone();
        let e = run(&mut s, "doc.import_lbrn2", json!({"content": LBRN})).unwrap_err();
        assert!(e.to_string().contains("field_mm is required"), "{e}");
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "append", "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": "<svg/>", "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": -1})).is_err());

        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150}));
        assert_eq!(r["mode"], json!("replace"));
        assert_eq!(r["created"], json!(["obj-2", "obj-3"]));
        assert_eq!(r["created_layers"], json!(["layer-1"]));
        assert!(r["warnings"].to_string().contains("bitmap with no usable image data was kept"), "{}", r["warnings"]);
        assert_eq!(r["created_groups"], json!([]));
        assert_eq!(r["bounds"]["min"], json!([-5.0, -5.0]));
        assert_eq!(r["bounds"]["max"], json!([15.0, 5.0]));
        let d = s.document();
        assert_eq!(d.field_mm, Some(150.0), "set from the import when the document had none");
        assert!(d.stock.is_none());
        assert_eq!(d.layers[0].name, "Fill");
        assert_eq!(d.layers[0].settings.frequency_khz, 100.0);
        assert_eq!(d.layers[0].settings.pulse_width_ns, Some(200.0));
        assert_eq!(d.next_id, 4);
        s.undo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&before));

        // Without field_mm the document's is used; with one, the document takes it (the file is laid out for it).
        let mut s = session(Some(153.75));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
        assert_eq!(r["bounds"]["min"], json!([-6.875, -6.875]), "uses the document's field");
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150}));
        assert_eq!(r["field_mm"], json!(150.0));
        assert_eq!(s.document().field_mm, Some(150.0));
    }

    /// The translation of every exported shape (group members instead of their group), in LightBurn mm.
    fn exported_positions(s: &mut Session, params: Value) -> Vec<(f64, f64)> {
        fn walk(shapes: &[lbrn::Shape], out: &mut Vec<(f64, f64)>) {
            for sh in shapes {
                match &sh.kind {
                    lbrn::ShapeKind::Group(children) => walk(children, out),
                    _ => out.push((sh.xform.translation().x, sh.xform.translation().y)),
                }
            }
        }
        let r = ok(s, "doc.export_lbrn2", params);
        let mut out = Vec::new();
        walk(&lbrn::parse(r["content"].as_str().unwrap()).unwrap().shapes, &mut out);
        out
    }

    #[test]
    fn export_uses_the_field_size_the_import_used() {
        // Regression: a file imported at 150 mm into a 153.75 mm document came back +1.875 mm in X and Y.
        let source = [(80.0, 75.0), (75.0, 75.0), (75.0, 75.0)];
        let mut s = session(Some(153.75));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150}));
        assert_eq!(exported_positions(&mut s, json!({})), source);
        assert_eq!(ok(&mut s, "doc.export_lbrn2", json!({}))["field_mm"], json!(150.0));
        // field_mm still overrides.
        assert_eq!(exported_positions(&mut s, json!({"field_mm": 153.75}))[0], (81.875, 76.875));
        // Kept through save and load.
        let saved = ok(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
        ok(&mut s, "doc.new", json!({}));
        ok(&mut s, "doc.load", json!({"content": saved}));
        assert_eq!(exported_positions(&mut s, json!({})), source);

        // A merge keeps the document's field size and remembers the file's for export.
        let mut s = session(Some(153.75));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "field_mm": 150}));
        assert_eq!(s.document().field_mm, Some(153.75));
        assert_eq!(s.document().extras.get(super::IMPORT_FIELD_KEY).map(String::as_str), Some("150"));
        assert_eq!(exported_positions(&mut s, json!({})), source);
        // A new document forgets it.
        ok(&mut s, "doc.new", json!({}));
        ok(&mut s, "shape.rect", json!({"width_mm": 2, "height_mm": 2}));
        assert_eq!(exported_positions(&mut s, json!({})), [(76.875, 76.875)]);
        // An unusable remembered value falls back to the document's field.
        let mut d = Document { field_mm: Some(150.0), ..Document::default() };
        d.extras.insert(super::IMPORT_FIELD_KEY.into(), "-3".into());
        let mut s = Session::new(d);
        ok(&mut s, "shape.rect", json!({"width_mm": 2, "height_mm": 2}));
        assert_eq!(exported_positions(&mut s, json!({})), [(75.0, 75.0)]);
    }

    /// The owner's LightBurn files (read in place, never copied), imported at a field size other than the
    /// document's, saved and opened again, then exported with no field_mm: each comes back byte for byte.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_lightburn_files_export_at_their_import_field_size() {
        let Some(files) = super::super::dxf::sample_files(&["lbrn2", "lbt"]) else {
            println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
            return;
        };
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let text = text.trim_start_matches('\u{feff}');
            // The quick DeviceName scan agrees with the full parser.
            let parsed = lbrn::parse(text).unwrap();
            let full = parsed.attributes.iter().find(|(k, _)| k == "DeviceName").map(|(_, v)| v.trim().to_owned());
            assert_eq!(super::lbrn2_device_name(text), full.filter(|v| !v.is_empty()), "{}", f.display());
            if let Some(d) = super::lbrn2_device_name(text) {
                println!("{}: DeviceName \"{d}\"", f.display());
            }
            let mut s = session(Some(153.75));
            ok(&mut s, "doc.import_lbrn2", json!({"content": text, "field_mm": 150}));
            let saved = ok(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
            ok(&mut s, "doc.new", json!({}));
            ok(&mut s, "doc.load", json!({"content": saved}));
            let out = ok(&mut s, "doc.export_lbrn2", json!({}))["content"].as_str().unwrap().to_string();
            assert!(out == text, "{} does not come back byte for byte", f.display());
        }
        println!("{} LightBurn file(s) came back byte for byte at their import field size", files.len());
    }

    #[test]
    fn import_merge_reuses_matching_layers_with_fresh_ids() {
        let mut s = session(Some(150.0));
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4}));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "name": "badge"}));
        assert_eq!(r["created_layers"], json!(["layer-3"]));
        assert_eq!(r["reused_layers"], json!([]));
        assert_eq!(r["created"], json!(["obj-4", "obj-5"]));
        assert_eq!(r["created_groups"], json!(["group-6"]), "the file's objects come in as one group");
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge"}));
        assert_eq!(r["created_layers"], json!([]));
        assert_eq!(r["reused_layers"], json!(["layer-3"]));
        assert_eq!(r["created"], json!(["obj-7", "obj-8"]));
        let d = s.document();
        assert_eq!(d.layers.len(), 2);
        assert_eq!(d.objects.len(), 5);
        assert!(d.objects[1..].iter().all(|o| o.layer == "layer-3"));
        assert_eq!(d.objects[0].layer, "layer-1");
        let names: Vec<&str> = d.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["badge", "Imported LightBurn"]);
        assert_eq!(d.group_members("group-6"), ["obj-4", "obj-5"]);
        let keys: Vec<&str> = d.extras.keys().map(String::as_str).collect();
        assert_eq!(keys, [super::IMPORT_FIELD_KEY], "a merge keeps the document's own file data");
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 3);
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 1);
        assert_eq!(s.document().layers.len(), 1);
        assert!(s.document().groups.is_empty());
    }

    #[test]
    fn import_keeps_lightburn_groups_text_and_file_data_for_export() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="2.1.04" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <Notes ShowOnLoad="1" Notes="keep me"/>
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="C00"/>
        <maxPower Value="20"/>
        <speed Value="100"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Group">
        <XForm>1 0 0 1 0 0</XForm>
        <Children>
            <Shape Type="Rect" CutIndex="0" W="4" H="2" Cr="0">
                <XForm>1 0 0 1 70 75</XForm>
            </Shape>
            <Shape Type="Text" CutIndex="0" Font="Arial,-1,100,5,50,0,0,0,0,0" Str="Hi" H="2" HasBackupPath="1">
                <BackupPath Type="Path" CutIndex="0">
                    <XForm>1 0 0 1 80 75</XForm>
                    <VertList>V0 0c0x1c1x1V1 0c0x1c1x1V1 1c0x1c1x1</VertList>
                    <PrimList>L0 1L1 2L2 0</PrimList>
                </BackupPath>
                <XForm>1 0 0 1 80 75</XForm>
            </Shape>
        </Children>
    </Shape>
</LightBurnProject>
"#;
        let mut s = session(Some(150.0));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": text}));
        assert_eq!(r["created_groups"], json!(["group-2"]));
        let d = s.document();
        assert_eq!(d.group_members("group-2"), ["obj-3", "obj-4"]);
        assert_eq!(d.objects[1].name, "Hi");
        assert_eq!(d.objects[1].extras.get("lbrn2.Str").map(String::as_str), Some("Hi"));
        assert!(d.extras.contains_key("lbrn2._root_attrs"));
        // Saved and opened again, then exported: the file comes back as it was.
        let saved = ok(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
        ok(&mut s, "doc.new", json!({}));
        ok(&mut s, "doc.load", json!({"content": saved}));
        let out = ok(&mut s, "doc.export_lbrn2", json!({}))["content"].as_str().unwrap().to_string();
        assert_eq!(out, text);
    }

    #[test]
    fn load_refuses_documents_the_edit_commands_would_never_make() {
        let mut s = session(None);
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2}));
        let good = s.document().clone();
        let before = s.document().clone();
        type Spoil = fn(&mut Document);
        let cases: [(&str, Spoil); 8] = [
            ("does not exist", |d| d.objects[0].layer = "layer-404".into()),
            ("no layers", |d| {
                d.layers.clear();
                d.objects.clear();
            }),
            ("not positive", |d| {
                d.objects[0].geometry = doc::Geometry::Rect { width_mm: -4.0, height_mm: 2.0, corner_radius_mm: 0.0 }
            }),
            ("cannot be inverted", |d| d.objects[0].transform = kurbo::Affine::new([0.0; 6])),
            ("control character", |d| d.name = "x\u{0}".into()),
            ("control character", |d| d.layers[0].name = "\u{1}".into()),
            ("too large", |d| d.objects[0].id = "obj-18446744073709551615".into()),
            ("extras key", |d| {
                d.layers[0].extras.insert("lbrn2.evil a=\"1\"/><Shape/><x".into(), "1".into());
            }),
        ];
        for (what, f) in cases {
            let mut d = good.clone();
            f(&mut d);
            let e = run(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&d)})).unwrap_err();
            assert!(e.to_string().contains(what), "{what}: {e}");
            assert_eq!(s.document(), &before);
        }
        ok(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&good)}));
    }

    #[test]
    fn edits_that_overflow_to_non_finite_geometry_are_refused() {
        let mut s = session(Some(150.0));
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2}));
        ok(&mut s, "object.transform", json!({"ids": ["obj-2"], "scale": 1e100, "about": "origin"}));
        let before = s.document().clone();
        let history = s.history().len();
        // 1e100 x 1e250 overflows to infinity.
        let e = run(&mut s, "object.transform", json!({"ids": ["obj-2"], "scale": 1e250, "about": "origin"}))
            .unwrap_err();
        assert!(matches!(e, CommandError::Failed(_)) && e.to_string().contains("unusable geometry"), "{e}");
        assert_eq!(s.document(), &before);
        assert_eq!(s.history().len(), history);
        // What is committed can always be saved and loaded again.
        let saved = ok(&mut s, "doc.save", json!({}));
        ok(&mut s, "doc.load", json!({"content": saved["content"]}));
    }

    #[test]
    fn save_refuses_a_document_that_would_not_load_again() {
        let mut d = Document::default();
        d.layers[0].settings.speed_mm_s = f64::INFINITY;
        let mut s = Session::new(d);
        let e = run(&mut s, "doc.save", json!({})).unwrap_err();
        assert!(e.to_string().contains("cannot save"), "{e}");
    }

    #[test]
    fn exhausted_ids_are_an_error_not_a_panic() {
        let mut s = Session::new(Document { next_id: doc::MAX_ID, field_mm: Some(150.0), ..Document::default() });
        let before = s.document().clone();
        for (id, params) in [
            ("shape.rect", json!({"width_mm": 4, "height_mm": 2})),
            ("layer.add", json!({"name": "x"})),
            ("doc.import_lbrn2", json!({"content": LBRN, "mode": "merge"})),
        ] {
            let e = run(&mut s, id, params).unwrap_err();
            assert!(matches!(e, CommandError::Failed(_)) && e.to_string().contains("used up"), "{id}: {e}");
            assert_eq!(s.document(), &before, "{id}");
        }
        // A replacing import starts a fresh id range.
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
    }

    #[test]
    fn setting_the_frequency_clears_the_lightburn_no_frequency_marker() {
        let no_freq = LBRN.replace("        <frequency Value=\"100000\"/>\n", "");
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": no_freq}));
        let marker = |s: &Session| s.document().layers[0].extras.contains_key(lbrn::FREQUENCY_ABSENT_KEY);
        assert!(marker(&s));
        let export = |s: &mut Session| ok(s, "doc.export_lbrn2", json!({}))["content"].as_str().unwrap().to_string();
        assert!(!export(&mut s).contains("<frequency"), "the source had none");
        ok(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 40}}));
        assert!(marker(&s), "other settings leave it");
        ok(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"frequency_khz": 60}}));
        assert!(!marker(&s));
        assert!(export(&mut s).contains("<frequency Value=\"60000\"/>"));
    }

    #[test]
    fn names_with_control_characters_are_refused() {
        let mut s = session(None);
        assert!(run(&mut s, "layer.add", json!({"name": "a\u{1}b"})).is_err());
        assert!(run(&mut s, "doc.rename", json!({"name": "x\ny"})).is_err());
    }

    #[test]
    fn export_needs_a_field_and_writes_lightburn_xml() {
        let mut s = session(None);
        ok(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 2, "center": [10, 0]}));
        let e = run(&mut s, "doc.export_lbrn2", json!({})).unwrap_err();
        assert!(e.to_string().contains("field_mm is required"), "{e}");
        let r = ok(&mut s, "doc.export_lbrn2", json!({"field_mm": 100}));
        assert_eq!(r["warnings"], json!([]));
        let p = lbrn::parse(r["content"].as_str().unwrap()).unwrap();
        assert_eq!(p.shapes.len(), 1);
        assert_eq!(p.shapes[0].xform.translation(), kurbo::Vec2::new(60.0, 50.0));
        assert_eq!(s.document().revision, 1, "export is read-only");

        // Import, then export: the LightBurn index and kept values come back.
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        let p = lbrn::parse(r["content"].as_str().unwrap()).unwrap();
        assert_eq!(p.cut_settings[0].index, 1);
        assert_eq!(p.cut_settings[0].params.q_pulse_width, Some(200.0));
        assert_eq!(p.shapes.len(), 3, "the unusable bitmap is kept for LightBurn");
    }

    #[test]
    fn new_takes_a_field_size() {
        let mut s = session(Some(150.0));
        let r = ok(&mut s, "doc.new", json!({"field_mm": 153.75}));
        assert_eq!((r["field_mm"].clone(), s.document().field_mm), (json!(153.75), Some(153.75)));
        ok(&mut s, "doc.new", json!({}));
        assert_eq!(s.document().field_mm, Some(153.75), "kept without field_mm");
        assert!(run(&mut s, "doc.new", json!({"field_mm": -1})).is_err());
        assert!(run(&mut s, "doc.new", json!({"field_mm": 5000})).is_err());
    }

    #[test]
    fn gantry_documents_have_a_bed_and_an_origin() {
        let mut s = session(Some(150.0));
        let r = ok(&mut s, "doc.new", json!({"workspace_mm": [400, 300], "origin": "top_left"}));
        assert_eq!((r["workspace_mm"].clone(), r["field_mm"].clone()), (json!([400.0, 300.0]), json!(300.0)));
        assert_eq!(s.document().workspace(), Some([400.0, 300.0]));
        assert_eq!(s.document().extras.get("lbrn2._origin").map(String::as_str), Some("top_left"));
        assert!(run(&mut s, "doc.new", json!({"workspace_mm": [400, 0], "origin": "bottom_left"})).is_err());
        assert!(run(&mut s, "doc.new", json!({"origin": "middle"})).is_err());

        // Import into the bed: LBRN's rect at (80, 75) with MirrorY="False" is 80 mm right of and 75 mm in front
        // of... the front-left corner: field (-120, -75).
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "workspace_mm": [400, 300], "origin": "top_left"}));
        assert_eq!((r["origin"].clone(), r["workspace_mm"].clone(), r["field_mm"].clone()), (json!("top_left"), json!([400.0, 300.0]), Value::Null));
        let d = s.document();
        assert_eq!(d.workspace_mm, Some([400.0, 300.0]));
        assert_eq!(d.objects[0].bounds().center(), kurbo::Point::new(-120.0, -75.0), "the file's own flags place it");
        // Export for the device's top-left origin: y now counts from the rear (300 - 75) in a mirrored frame, so the
        // shape's own y axis is mirrored too (it looks the same in LightBurn), and MirrorY is written.
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        assert_eq!(r["origin"], json!("top_left"));
        let text = r["content"].as_str().unwrap();
        assert!(text.contains("MirrorY=\"True\"") && text.contains("<XForm>1 0 0 -1 80 225</XForm>"), "{text}");
        // An explicit corner and bed win; field_mm is refused for a bed and workspace_mm for a galvo.
        let text = ok(&mut s, "doc.export_lbrn2", json!({"origin": "bottom_left", "workspace_mm": [400, 300]}))["content"].clone();
        assert!(text.as_str().unwrap().contains("<XForm>1 0 0 1 80 75</XForm>"));
        assert!(run(&mut s, "doc.export_lbrn2", json!({"origin": "bottom_left", "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.export_lbrn2", json!({"workspace_mm": [400, 300], "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.export_lbrn2", json!({"workspace_mm": [4000, 300], "origin": "center"})).is_err());

        // Merging a bed file into a galvo document remembers the bed for export.
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "workspace_mm": [200, 100], "origin": "bottom_left"}));
        let d = s.document();
        assert_eq!((d.field_mm, d.workspace_mm), (Some(150.0), Some([200.0, 100.0])));
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        assert_eq!((r["origin"].clone(), r["workspace_mm"].clone()), (json!("bottom_left"), json!([200.0, 100.0])));
        // A later galvo merge makes galvo exports the default again.
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "field_mm": 110}));
        let r = ok(&mut s, "doc.export_lbrn2", json!({"origin": "center"}));
        assert_eq!((r["origin"].clone(), r["field_mm"].clone()), (json!("center"), json!(110.0)));
    }

    /// A galvo with LightBurn's separate field Width and Height: the document gets the field as its workspace with a
    /// centre origin, and LightBurn files measure from the field's lower-left corner on both axes.
    #[test]
    fn non_square_galvo_fields_are_the_documents_workspace() {
        let mut s = session(Some(150.0));
        let r = ok(&mut s, "doc.new", json!({"workspace_mm": [200, 120], "origin": "center"}));
        assert_eq!((r["field_mm"].clone(), r["workspace_mm"].clone()), (json!(200.0), json!([200.0, 120.0])));
        let d = s.document();
        assert_eq!((d.field_mm, d.workspace(), d.extras.get(super::ORIGIN_KEY).map(String::as_str)), (Some(200.0), Some([200.0, 120.0]), Some("center")));
        // No origin means the centre too; a square "workspace" is just a field.
        ok(&mut s, "doc.new", json!({"workspace_mm": [200, 120]}));
        assert_eq!(s.document().workspace_mm, Some([200.0, 120.0]));
        ok(&mut s, "doc.new", json!({"workspace_mm": [110, 110]}));
        assert_eq!((s.document().field_mm, s.document().workspace_mm), (Some(110.0), None));
        assert!(run(&mut s, "doc.new", json!({"workspace_mm": [200, 120], "field_mm": 200})).is_err(), "one or the other");
        assert!(run(&mut s, "doc.new", json!({"workspace_mm": [2500, 120]})).is_err(), "a field is at most 2000 mm");

        // LBRN's rect centre at LightBurn (80, 75) lands at (80 - 100, 75 - 60) in a 200 x 120 field.
        ok(&mut s, "doc.new", json!({"workspace_mm": [200, 120]}));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
        assert_eq!((r["origin"].clone(), r["workspace_mm"].clone(), r["field_mm"].clone()), (json!("center"), json!([200.0, 120.0]), json!(200.0)));
        let d = s.document();
        assert_eq!(d.workspace_mm, Some([200.0, 120.0]));
        assert_eq!(d.objects[0].bounds().center(), kurbo::Point::new(-20.0, 15.0));
        // Export goes back to the same LightBurn coordinates, without mirror flags.
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        assert_eq!((r["origin"].clone(), r["workspace_mm"].clone()), (json!("center"), json!([200.0, 120.0])));
        let text = r["content"].as_str().unwrap();
        assert!(text.contains("MirrorY=\"False\"") && text.contains("<XForm>1 0 0 1 80 75</XForm>"), "{text}");
        // An explicit square field still wins, and saving keeps the layout.
        assert_eq!(exported_positions(&mut s, json!({"field_mm": 150}))[0], (55.0, 90.0));
        let saved = ok(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
        ok(&mut s, "doc.load", json!({"content": saved}));
        assert_eq!(exported_positions(&mut s, json!({}))[0], (80.0, 75.0));

        // Merging a non-square galvo file into a square document remembers it for export.
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "workspace_mm": [200, 120], "origin": "center"}));
        let d = s.document();
        assert_eq!((d.field_mm, d.workspace_mm), (Some(150.0), Some([200.0, 120.0])));
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        assert_eq!((r["origin"].clone(), r["workspace_mm"].clone()), (json!("center"), json!([200.0, 120.0])));
    }

    #[test]
    fn workspace_origins_name_the_mirror_flags() {
        use super::WorkspaceOrigin as O;
        for o in [O::Center, O::BottomLeft, O::TopLeft, O::BottomRight, O::TopRight] {
            assert_eq!(O::parse(o.name()), Some(o));
            if let Some((x, y)) = o.mirror() {
                assert_eq!(O::from_mirror(x, y), o);
            }
            assert_eq!(serde_json::to_value(o).unwrap(), json!(o.name()));
        }
        assert_eq!(O::TopLeft.mirror(), Some((false, true)), "rear-left is MirrorY (xTool)");
        assert_eq!(O::Center.mirror(), None);
    }

    #[test]
    fn import_reports_the_field_source() {
        let mut s = session(Some(150.0));
        assert_eq!(ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}))["field_source"], json!("document"));
        assert_eq!(ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 110}))["field_source"], json!("caller"));
    }

    #[test]
    fn device_name_is_read_from_the_root_tag() {
        use super::lbrn2_device_name as dn;
        let v2 = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<LightBurnProject AppVersion=\"2.1.02\" \
                  DeviceName=\"JCZFiber (Unknown)\" FormatVersion=\"1\">\n<Shape/></LightBurnProject>";
        assert_eq!(dn(v2).as_deref(), Some("JCZFiber (Unknown)"));
        assert_eq!(dn(LBRN), None, "1.x files have none");
        assert_eq!(dn("<LightBurnProject a='x>y' DeviceName='A &amp; B &#x41;'>").as_deref(), Some("A & B A"));
        assert_eq!(dn("<LightBurnProject DeviceName=\"  \">"), None);
        assert_eq!(dn("<LightBurnProject><Shape DeviceName=\"no\"/>"), None, "only the root's attributes");
        assert_eq!(dn("<LightBurnLibrary DeviceName=\"x\">"), None);
        assert_eq!(dn("<LightBurnProject DeviceName=\"unterminated"), None);
        assert_eq!(dn(""), None);
        let long = format!("{}<LightBurnProject DeviceName=\"é\">", "é".repeat(40_000));
        assert_eq!(dn(&long), None, "past the head; never panics on a char boundary");
    }
}
