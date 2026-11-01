//! [`LbrnProject`] ⇄ OpenEtch document. See the crate docs for the frame and the full mapping table.
//!
//! Everything of the file that OpenEtch does not model travels in `extras` (`lbrn2.*` keys, see [`keys`]): on the
//! document (root attributes, `Thumbnail`, `VariableText`, `UIPrefs`, `Notes`, the root's child order, the file's
//! layout), on layers (unmapped cut-setting tags, the tags' source order and spelling, a second `CutSetting_Img`), on
//! objects (the shape's attributes in order, e.g. Text's `Str`/`Font`, its `XForm` text, path ids and lists, unknown
//! child elements, and shapes OpenEtch cannot show that followed it) and on groups (their `XForm` and attributes). So
//! a file imported and exported again unchanged comes back as it was (see `tests/real_samples.rs`), and an edited
//! one keeps everything the edit did not touch.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use doc::{CutSettings, Document, FillMode, Geometry, Group, ImageSettings, Layer, Object, StockShape};
use kurbo::{Affine, BezPath, Rect};

use crate::LbrnError;
use crate::model::{
    Backup, CutParams, CutSetting, Element, Encoded, Format, LbrnProject, PathData, RootItem, Shape, ShapeKind,
    SubLayer, TYPED_TAGS,
};
use crate::palette::{self, COLOR_LAYERS, T1};
use crate::path::{parse_prims, parse_vertices, write_vertices};
use crate::write::escape_attr;

/// Prefix of the `extras` keys this crate reads and writes.
pub const EXTRA_PREFIX: &str = "lbrn2.";
/// Most passes a layer may have (the `commands` and `plan` limit).
pub const MAX_PASSES: u32 = 1000;
/// Largest field size accepted, in mm.
const MAX_FIELD_MM: f64 = 2000.0;
/// Largest bed (gantry workspace) side accepted, in mm.
pub const MAX_BED_MM: f64 = 5000.0;

/// The `extras` keys (after [`EXTRA_PREFIX`]) of what OpenEtch keeps for export. Keys starting with `_` are
/// bookkeeping, never LightBurn tag or attribute names; any other key on an object or group is one of the shape's
/// attributes (`lbrn2.Str`, `lbrn2.Font`, `lbrn2.VertID`, ...), on a layer one of its cut-setting tags.
pub mod keys {
    /// Object, group: the shape's `Type`.
    pub const TYPE: &str = "_type";
    /// Object, group: attribute names in file order (`Type` left out), space-separated.
    pub const ATTRS: &str = "_attrs";
    /// Object, group: the `XForm` text.
    pub const XFORM: &str = "_xform";
    /// Path or Text object: the `VertList` / `PrimList` text the shape (or its backup outline) carried.
    pub const VERT_LIST: &str = "_VertList";
    pub const PRIM_LIST: &str = "_PrimList";
    /// Text object: the backup outline's attribute names in order, its `XForm` text, and `_b.<name>` values.
    pub const BACKUP_ATTRS: &str = "_battrs";
    pub const BACKUP_XFORM: &str = "_bxform";
    pub const BACKUP_ATTR: &str = "_b.";
    /// Object, group: an unknown child element (`_child.0`, ...), as XML.
    pub const CHILD: &str = "_child.";
    /// Object: a shape OpenEtch cannot show (unknown type, text without outline, ...) that followed this object in
    /// the file (`_after.0`, ...), as XML; exported right after it.
    pub const AFTER: &str = "_after.";
    /// Layer (and `subN.`): the source tags as `<s tag="value" .../>`, for their order and spelling.
    pub const SOURCE: &str = "_src";
    /// Layer: the `CutSetting_Img` that shared this layer's index, as XML.
    pub const IMAGE_SETTING: &str = "_img";
    /// Document: the root's attributes as `<LightBurnProject .../>`.
    pub const ROOT_ATTRS: &str = "_root_attrs";
    /// Document: the root's children in order: `E<k>` (element `_el.<k>`), `C<i>` / `I<i>` (the `CutSetting` /
    /// `CutSetting_Img` of index i), `S` (the shapes).
    pub const ROOT_ORDER: &str = "_root";
    pub const ROOT_ELEMENT: &str = "_el.";
    /// Document: shapes OpenEtch cannot show that came before every object (`_lead.0`, ...).
    pub const LEAD: &str = "_lead.";
    /// Document: the XML declaration, and layout flags (`crlf`, `no-final-newline`, `space-before-slash`).
    pub const DECLARATION: &str = "_decl";
    pub const FORMAT: &str = "_format";
}

/// What [`to_document`] makes of a project. Ids are those of a fresh document (`layer-1`.. then `obj-n`/`group-n`..),
/// so callers merging into an existing document re-id them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Imported {
    pub layers: Vec<Layer>,
    pub objects: Vec<Object>,
    pub groups: Vec<Group>,
    /// File-level LightBurn data for [`Document::extras`].
    pub extras: BTreeMap<String, String>,
    /// The serial number counter of the file's `<VariableText>` (defaults when it has none), for
    /// [`Document::variables`]; the element itself stays in `extras` and export writes these values into it.
    pub variables: doc::VariableState,
    pub warnings: Vec<String>,
}

/// Marker extra (`lbrn2.frequency_absent`, and `lbrn2.subN.frequency_absent` for sub-layers): the source gave no
/// frequency, so export leaves it out again and LightBurn keeps using its own default. `layer.update` removes it when
/// the frequency is set.
pub const FREQUENCY_ABSENT: &str = "frequency_absent";
/// The full [`FREQUENCY_ABSENT`] key of a layer's main settings.
pub const FREQUENCY_ABSENT_KEY: &str = "lbrn2.frequency_absent";
/// Most distinct warnings reported; the rest are summed up in one last line.
const MAX_WARNINGS: usize = 200;
/// Most path elements (points with their move and close markers) [`to_document`] creates in total; rectangles,
/// ellipses and images count [`FIXED_SHAPE_ELEMENTS`]. Bounds the document (and its undo copies) whatever the file
/// holds.
const MAX_ELEMENTS: usize = 4_000_000;
const FIXED_SHAPE_ELEMENTS: usize = 10;

/// LightBurn draws a bitmap's `Data` with its first pixel row at the bottom (lowest y) of the shape, the reverse of
/// an image file and of [`Geometry::Image`], so a bitmap's transform is its `XForm` times this flip (on export the
/// flip is taken off again; it is exact, so unchanged `XForm`s keep their text). Evidence: `photo on namecard.lbrn2`
/// has a positive `XForm` and `Data` upside down against its upright thumbnail, and the 3D sliced sample's `Data`
/// is its source file (`File`, beside it) flipped top to bottom, pixel for pixel.
const BITMAP_ROWS: Affine = Affine::FLIP_Y;

fn key(k: &str) -> String {
    format!("{EXTRA_PREFIX}{k}")
}

/// Warnings with repeats counted, in first-seen order. Lookups are hashed and at most [`MAX_WARNINGS`] distinct
/// messages are kept, so a file with many different warnings costs linear time and bounded memory.
#[derive(Default)]
struct Warnings {
    list: Vec<(String, usize)>,
    index: HashMap<String, usize>,
    dropped: usize,
}

impl Warnings {
    fn push(&mut self, m: impl Into<String>) {
        let m = m.into();
        if let Some(&i) = self.index.get(&m) {
            self.list[i].1 += 1;
        } else if self.list.len() < MAX_WARNINGS {
            self.index.insert(m.clone(), self.list.len());
            self.list.push((m, 1));
        } else {
            self.dropped += 1;
        }
    }

    fn into_vec(self) -> Vec<String> {
        let mut out: Vec<String> =
            self.list.into_iter().map(|(m, n)| if n == 1 { m } else { format!("{m} (x{n})") }).collect();
        if self.dropped > 0 {
            out.push(format!("... and {} more warning(s) not listed", self.dropped));
        }
        out
    }
}

/// `s` with control characters (tabs, line breaks, ...) replaced by spaces: names must not hold them.
fn clean_name(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// `key` shortened and escaped for a warning.
fn shown(key: &str) -> String {
    format!("{:?}", key.chars().take(60).collect::<String>())
}

/// `sub3.speed` → `(3, "speed")`: the extras tags of sub-layer 3. `subname` and other tags merely starting with
/// "sub" are not sub-layer tags.
fn sub_layer_tag(tag: &str) -> Option<(usize, &str)> {
    let rest = tag.strip_prefix("sub")?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    let (n, rest) = rest.split_at(digits);
    Some((n.parse().ok()?, rest.strip_prefix('.')?))
}

fn check_field(field_mm: f64) -> Result<f64, LbrnError> {
    if field_mm.is_finite() && field_mm > 0.0 && field_mm <= MAX_FIELD_MM {
        Ok(field_mm)
    } else {
        Err(LbrnError::Invalid(format!("field_mm must be within 0..={MAX_FIELD_MM} mm (got {field_mm})")))
    }
}

/// How a LightBurn project's coordinates relate to OpenEtch's field frame (origin at the centre of the workspace,
/// +X to the operator's right, +Y away from the operator). LightBurn files do not record the workspace size, so the
/// caller gives it (normally the device's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Frame {
    /// A galvo: a square field of `field_mm`. LightBurn measures from the field's lower-left corner with y up, so
    /// field = LightBurn - field/2 on both axes (checked on the owner's B6 projects). Galvos always use absolute
    /// coordinates; the file's `MirrorX`/`MirrorY` are not applied (a set flag is reported).
    Galvo { field_mm: f64 },
    /// A bed (gantry) of `width_mm` x `height_mm`. LightBurn measures from the device's origin corner, which the
    /// project root records as `MirrorX` / `MirrorY`: front-left = neither, rear-left = `MirrorY`, rear-right = both
    /// (Ruida), front-right = `MirrorX`. So field = S · (LightBurn - [w/2, h/2]), S negating the mirrored axes: the
    /// outline itself is mirrored, matching the thumbnails of public projects saved for all four corners (see
    /// `tests/frames.rs`). On import the flags are the file's ([`LbrnProject::mirror`]); on export the target
    /// device's, and they are written to the root.
    Bed { width_mm: f64, height_mm: f64, mirror_x: bool, mirror_y: bool },
}

impl Frame {
    /// Width and height of the workspace, mm.
    pub fn size(&self) -> (f64, f64) {
        match *self {
            Frame::Galvo { field_mm } => (field_mm, field_mm),
            Frame::Bed { width_mm, height_mm, .. } => (width_mm, height_mm),
        }
    }

    /// The frame with its sizes checked (finite, positive, at most 2000 mm for a field, [`MAX_BED_MM`] for a bed).
    fn checked(self) -> Result<Self, LbrnError> {
        match self {
            Frame::Galvo { field_mm } => check_field(field_mm).map(|field_mm| Frame::Galvo { field_mm }),
            Frame::Bed { width_mm, height_mm, .. } => {
                for (name, v) in [("width_mm", width_mm), ("height_mm", height_mm)] {
                    if !(v.is_finite() && v > 0.0 && v <= MAX_BED_MM) {
                        return Err(LbrnError::Invalid(format!("{name} must be within 0..={MAX_BED_MM} mm (got {v})")));
                    }
                }
                Ok(self)
            }
        }
    }

    /// LightBurn coordinates -> field frame.
    pub fn from_lightburn(&self) -> Affine {
        match *self {
            Frame::Galvo { field_mm } => Affine::translate((-field_mm / 2.0, -field_mm / 2.0)),
            Frame::Bed { width_mm, height_mm, mirror_x, mirror_y } => {
                Affine::scale_non_uniform(sign(mirror_x), sign(mirror_y))
                    * Affine::translate((-width_mm / 2.0, -height_mm / 2.0))
            }
        }
    }

    /// Field frame -> LightBurn coordinates (the inverse of [`Frame::from_lightburn`]).
    pub fn to_lightburn(&self) -> Affine {
        match *self {
            Frame::Galvo { field_mm } => Affine::translate((field_mm / 2.0, field_mm / 2.0)),
            Frame::Bed { width_mm, height_mm, mirror_x, mirror_y } => {
                Affine::translate((width_mm / 2.0, height_mm / 2.0))
                    * Affine::scale_non_uniform(sign(mirror_x), sign(mirror_y))
            }
        }
    }

    /// Whether `b` (field frame) lies inside the workspace.
    fn contains(&self, b: Rect) -> bool {
        let (w, h) = self.size();
        b.x0 >= -w / 2.0 && b.y0 >= -h / 2.0 && b.x1 <= w / 2.0 && b.y1 <= h / 2.0
    }

    /// "the 150 mm field" / "the 400 x 300 mm workspace", for warnings.
    fn describe(&self) -> String {
        match *self {
            Frame::Galvo { field_mm } => format!("the {field_mm} mm field"),
            Frame::Bed { width_mm, height_mm, .. } => format!("the {width_mm} x {height_mm} mm workspace"),
        }
    }
}

/// -1 for a mirrored axis, else 1.
fn sign(mirrored: bool) -> f64 {
    if mirrored { -1.0 } else { 1.0 }
}

/// A LightBurn boolean attribute value (`True`, `true`, `1`).
pub(crate) fn truthy(v: &str) -> bool {
    matches!(v.trim(), "True" | "true" | "TRUE" | "1")
}

/// `<tag a="1" b="2"/>`: attributes kept as one XML string, read back with [`xml_attributes`].
fn attributes_xml(tag: &str, attrs: &[(String, String)]) -> String {
    let mut s = format!("<{tag}");
    let mut seen = HashSet::new();
    for (k, v) in attrs.iter().filter(|(k, _)| doc::is_plain_name(k)) {
        if seen.insert(k.as_str()) {
            s.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
        }
    }
    s.push_str("/>");
    s
}

/// The attributes of the one element in `xml`, in order.
fn xml_attributes(xml: &str) -> Option<Vec<(String, String)>> {
    let d = roxmltree::Document::parse(xml).ok()?;
    Some(d.root_element().attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect())
}

/// The first shape in a `<Shape>` fragment kept by the importer.
fn shape_from_xml(xml: &str) -> Option<Shape> {
    let p = crate::parse(&format!("<LightBurnProject>{xml}</LightBurnProject>")).ok()?;
    p.shapes.into_iter().next()
}

/// A shape as one self-contained XML fragment: path lists given only by id are written out, without ids, so the
/// fragment does not depend on any other shape.
fn shape_to_xml(s: &Shape) -> String {
    fn standalone(s: &mut Shape) {
        let fix = |d: &mut PathData| {
            if let Some(e) = &mut d.encoded
                && (e.vert_list.is_none() || e.prim_list.is_none())
            {
                *e = Encoded {
                    vert_id: None,
                    prim_id: None,
                    vert_list: Some(write_vertices(&d.vertices)),
                    prim_list: Some(crate::path::write_prims(&d.prims)),
                };
            }
        };
        match &mut s.kind {
            ShapeKind::Path(d) => fix(d),
            ShapeKind::Text { backup: Some(b) } => fix(&mut b.path),
            ShapeKind::Group(children) => children.iter_mut().for_each(standalone),
            _ => {}
        }
        s.attributes.retain(|(k, _)| !matches!(k.as_str(), "VertID" | "PrimID"));
    }
    let mut s = s.clone();
    standalone(&mut s);
    fragment(LbrnProject { shapes: vec![s], order: vec![RootItem::Shape(0)], ..LbrnProject::default() })
}

/// The one root child of `p` as XML, laid out as a child of the root (its inner lines indented for that level), so it
/// can be written back verbatim there (see [`Element::raw`]).
fn fragment(mut p: LbrnProject) -> String {
    p.format.crlf = false;
    let text = crate::write(&p);
    let start = text.find("?>").map_or(0, |d| d + 2);
    let start = text[start..].find("\n    <").map_or(0, |k| start + k + 5);
    let end = text.rfind("</LightBurnProject>").unwrap_or(text.len());
    text[start..end].trim_end().to_string()
}

// ---------------------------------------------------------------------------------------------------------------
// Layers
// ---------------------------------------------------------------------------------------------------------------

/// OpenEtch fill mode for a LightBurn cut type, if it has a direct equivalent.
fn mode_of(kind: &str) -> Option<FillMode> {
    match kind {
        "Cut" => Some(FillMode::Line),
        "Scan" => Some(FillMode::Fill),
        "Scan+Cut" => Some(FillMode::FillThenLine),
        "Offset Fill" => Some(FillMode::OffsetFill),
        _ => None,
    }
}

/// The mode an imported layer of `kind` gets (also used to decide whether an exported layer keeps its kind).
fn imported_mode(kind: &str) -> FillMode {
    mode_of(kind).unwrap_or(if kind == "Image" { FillMode::Fill } else { FillMode::Line })
}

fn kind_of(mode: &FillMode) -> &'static str {
    match mode {
        FillMode::Line => "Cut",
        FillMode::Fill => "Scan",
        FillMode::FillThenLine => "Scan+Cut",
        FillMode::OffsetFill => "Offset Fill",
    }
}

fn is_tool(index: u32, kind: &str) -> bool {
    index >= COLOR_LAYERS || kind == "Tool"
}

/// LightBurn parameters → OpenEtch settings. Returns the settings and the `tag → value` pairs they do not use.
fn to_settings(kind: &str, p: &CutParams, label: &str, w: &mut Warnings) -> (CutSettings, BTreeMap<String, String>) {
    let d = CutSettings::default();
    let mut rest: BTreeMap<String, String> = p.to_values().into_iter().collect();
    let mapped = ["maxPower", "speed", "frequency", "QPulseWidth", "interval", "angle", "anglePerPass", "crossHatch"];
    for tag in mapped.into_iter().chain(["bidir", "numPasses"]) {
        rest.remove(tag);
    }
    for tag in TYPED_TAGS.iter().filter(|t| p.other.contains_key(**t)) {
        w.push(format!("{label}: {tag} value '{}' could not be read and was ignored", p.other[*tag]));
        rest.remove(*tag);
    }
    let mode = match mode_of(kind) {
        Some(m) => m,
        // An image layer: its images use its image settings; vectors put on it are filled with its interval.
        None if kind == "Image" => FillMode::Fill,
        None => {
            w.push(format!("{label}: cut type '{kind}' is not supported; imported as a line layer"));
            FillMode::Line
        }
    };
    let power_pct = match p.max_power {
        Some(v) if (0.0..=100.0).contains(&v) => v,
        Some(v) => {
            w.push(format!("{label}: power {v} % clamped to 0..=100"));
            v.clamp(0.0, 100.0)
        }
        None => {
            w.push(format!("{label}: no power (maxPower) in the file; using {} %", d.power_pct));
            d.power_pct
        }
    };
    let speed_mm_s = match p.speed {
        Some(v) if v > 0.0 => v,
        other => {
            w.push(format!("{label}: speed {} is not usable; using {} mm/s", show(other), d.speed_mm_s));
            d.speed_mm_s
        }
    };
    let frequency_khz = match p.frequency {
        Some(v) if v > 0.0 => v / 1000.0,
        // LightBurn leaves the frequency out when it is at its default; export leaves it out again (the marker).
        None => d.frequency_khz,
        Some(v) => {
            w.push(format!("{label}: frequency {v} Hz is not usable; using {} kHz", d.frequency_khz));
            d.frequency_khz
        }
    };
    let pulse_width_ns = match p.q_pulse_width {
        Some(v) if v > 0.0 => Some(v),
        Some(v) => {
            w.push(format!("{label}: pulse width {v} ns ignored"));
            None
        }
        None => None,
    };
    let passes = match p.num_passes {
        Some(n) if (1..=MAX_PASSES).contains(&n) => n,
        Some(n) => {
            let c = n.clamp(1, MAX_PASSES);
            w.push(format!("{label}: {n} passes clamped to {c}"));
            c
        }
        None => 1,
    };
    let interval_mm = match p.interval {
        Some(v) if v > 0.0 => v,
        Some(v) => {
            w.push(format!("{label}: hatch interval {v} mm is not usable; using {} mm", d.hatch.interval_mm));
            d.hatch.interval_mm
        }
        None => d.hatch.interval_mm,
    };
    let s = CutSettings {
        mode,
        power_pct,
        speed_mm_s,
        frequency_khz,
        pulse_width_ns,
        passes,
        hatch: doc::Hatch {
            interval_mm,
            angle_deg: p.angle.unwrap_or(0.0),
            angle_increment_deg: p.angle_per_pass.unwrap_or(0.0),
            cross_hatch: p.cross_hatch.unwrap_or(false),
            bidirectional: p.bidir.unwrap_or(true),
        },
        timing: None,
        name: None,
        cleanup: None,
        image: ImageSettings::default(),
        options: doc::CutOptions::default(),
    };
    (s, rest)
}

fn show(v: Option<f64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "(missing)".into())
}

/// Keep a LightBurn value as the extra `{prefix}{tag}`, unless the tag could not be written back as an element name
/// (or would read as bookkeeping).
fn keep(layer: &mut Layer, prefix: &str, tag: &str, value: String, label: &str, w: &mut Warnings) {
    if doc::is_plain_name(tag) && !tag.starts_with('_') {
        layer.extras.insert(format!("{prefix}{tag}"), value);
    } else {
        w.push(format!("{label}: setting {} has a name OpenEtch cannot keep; it was dropped", shown(tag)));
    }
}

/// One LightBurn layer (its vector settings, or image settings when that is all there is) → an OpenEtch layer.
fn layer_from(index: u32, cs: Option<&CutSetting>, id: String, w: &mut Warnings) -> Layer {
    let label = palette::default_name(index);
    let mut layer = Layer::new(id, &label, palette::color(index));
    layer.extras.insert(key("index"), index.to_string());
    let Some(cs) = cs else {
        if index < COLOR_LAYERS {
            w.push(format!("{label}: used by shapes but has no cut settings in the file; default settings used"));
        } else {
            layer.output = false;
            layer.tool = doc::ToolLayer::from_index(index);
        }
        return layer;
    };
    let p = &cs.params;
    layer.name = layer_name(index, p.name.as_deref(), p.subname.as_deref());
    layer.visible = !p.hide.unwrap_or(false);
    layer.output = p.do_output.unwrap_or(true);
    if mode_of(&cs.kind).is_none() {
        layer.extras.insert(key("type"), cs.kind.clone());
    }
    if cs.image {
        layer.extras.insert(key("image"), "1".into());
    }
    if !p.source.is_empty() {
        layer.extras.insert(format!("{EXTRA_PREFIX}{}", keys::SOURCE), attributes_xml("s", &p.source));
    }
    if is_tool(index, &cs.kind) {
        // Tool layers (T1/T2) carry no laser settings and never output.
        layer.output = false;
        layer.tool = doc::ToolLayer::from_index(index);
        for (tag, value) in p.to_values() {
            if !matches!(tag.as_str(), "name" | "priority" | "hide") {
                keep(&mut layer, EXTRA_PREFIX, &tag, value, &label, w);
            }
        }
        return layer;
    }
    let no_frequency = |p: &CutParams| p.frequency.is_none() && !p.other.contains_key("frequency");
    let (mut settings, mut rest) = to_settings(&cs.kind, p, &label, w);
    if cs.image {
        settings.image = crate::image::image_from(p, &mut rest, &ImageSettings::default(), &label, &mut |m| w.push(m));
    }
    layer.settings = settings;
    if no_frequency(p) {
        layer.extras.insert(FREQUENCY_ABSENT_KEY.into(), "1".into());
    }
    for (tag, value) in rest {
        if !matches!(tag.as_str(), "name" | "priority" | "doOutput" | "hide") {
            keep(&mut layer, EXTRA_PREFIX, &tag, value, &label, w);
        }
    }
    for (n, sub) in cs.sub_layers.iter().enumerate() {
        let prefix = format!("{EXTRA_PREFIX}sub{}.", n + 1);
        let sub_label = format!("{label} sub-layer {}", n + 1);
        let (mut s, mut rest) = to_settings(&sub.kind, &sub.params, &sub_label, w);
        sub_layer_fields(&mut s, &mut rest);
        s.image = if cs.image {
            let parent = &layer.settings.image;
            crate::image::image_from(&sub.params, &mut rest, parent, &sub_label, &mut |m| w.push(m))
        } else {
            layer.settings.image.clone()
        };
        layer.sub_layers.push(s);
        if mode_of(&sub.kind).is_none() {
            layer.extras.insert(format!("{prefix}type"), sub.kind.clone());
        }
        if no_frequency(&sub.params) {
            layer.extras.insert(format!("{prefix}{FREQUENCY_ABSENT}"), "1".into());
        }
        if !sub.params.source.is_empty() {
            layer.extras.insert(format!("{prefix}{}", keys::SOURCE), attributes_xml("s", &sub.params.source));
        }
        for (tag, value) in rest {
            keep(&mut layer, &prefix, &tag, value, &sub_label, w);
        }
    }
    layer
}

/// The image settings of a `CutSetting_Img` kept beside a layer's vector settings.
fn image_of_setting(img: &CutSetting, label: &str, w: &mut Warnings) -> ImageSettings {
    let mut rest: BTreeMap<String, String> = img.params.to_values().into_iter().collect();
    crate::image::image_from(&img.params, &mut rest, &ImageSettings::default(), label, &mut |m| w.push(m))
}

/// The `CutSetting_Img` of `xml` (kept beside a layer's vector settings, see [`keys::IMAGE_SETTING`]).
fn cut_setting_from_xml(xml: &str) -> Option<CutSetting> {
    let p = crate::parse(&format!("<LightBurnProject>{xml}</LightBurnProject>")).ok()?;
    p.cut_settings.into_iter().next().filter(|c| c.image)
}

/// A kept `CutSetting_Img` written for `l`: as kept while its image settings still read as `l`'s, else with them
/// written in (everything else as kept).
fn kept_image_xml(xml: &str, l: &Layer, w: &mut Warnings) -> String {
    let Some(mut cs) = cut_setting_from_xml(xml) else { return xml.to_string() };
    if image_of_setting(&cs, &l.name, &mut Warnings::default()) == l.settings.image {
        return xml.to_string();
    }
    let source = cs.params.source.clone();
    let mut values: BTreeMap<String, String> = cs.params.to_values().into_iter().collect();
    if let Some(m) = crate::image::image_values(&l.settings.image, &mut values, Some(&source), &ImageSettings::default()) {
        w.push(format!("layer \"{}\": {m}", l.name));
    }
    cs.params = params_from(&values, Some(source));
    fragment(LbrnProject { cut_settings: vec![cs], order: vec![RootItem::CutSetting(0)], ..LbrnProject::default() })
}

/// A `CutSetting_Img` for a layer that holds images but has no image settings in LightBurn's terms (a layer made in
/// OpenEtch, or a LightBurn vector layer the images were moved to), from its settings.
fn generated_image_xml(l: &Layer, index: u32, priority: Option<&String>, w: &mut Warnings) -> String {
    let mut values = settings_values(&l.settings, BTreeMap::new(), false, None);
    values.remove("crossHatch");
    if let Some(m) = crate::image::image_values(&l.settings.image, &mut values, None, &ImageSettings::default()) {
        w.push(format!("layer \"{}\": {m}", l.name));
    }
    values.insert("name".into(), l.name.clone());
    if let Some(p) = priority {
        values.insert("priority".into(), p.clone());
    }
    let cs = CutSetting { kind: "Image".into(), image: true, index, params: params_from(&values, None), sub_layers: vec![] };
    fragment(LbrnProject { cut_settings: vec![cs], order: vec![RootItem::CutSetting(0)], ..LbrnProject::default() })
}

/// A sub-layer's `subname` and `isCleanup` (taken out of `rest`) → [`CutSettings::name`] and
/// [`CutSettings::cleanup`]. A cleanup sub-layer's `numPasses` is LightBurn's "Clean after # of passes" (in the 3D
/// sliced sample a 250-pass layer has `numPasses` 10 on its cleanup), so it becomes `every_n_passes` and the cleanup
/// itself makes one pass.
fn sub_layer_fields(s: &mut CutSettings, rest: &mut BTreeMap<String, String>) {
    s.name = rest.remove("subname");
    if rest.get("isCleanup").and_then(|v| crate::model::boolean(v)) == Some(true) {
        rest.remove("isCleanup");
        s.cleanup = Some(doc::Cleanup { every_n_passes: s.passes });
        s.passes = 1;
    }
}

/// The export side of [`sub_layer_fields`]: write a sub-layer's name and cleanup into its LightBurn `values`,
/// keeping the source's spelling (`source`) while it still says the same.
fn sub_layer_values(
    s: &CutSettings,
    values: &mut BTreeMap<String, String>,
    source: Option<&[(String, String)]>,
    label: &str,
    w: &mut Warnings,
) {
    let src = |t: &str| source.and_then(|v| v.iter().find(|(k, _)| k == t)).map(|(_, v)| v.as_str());
    if let Some(n) = &s.name {
        values.insert("subname".into(), n.clone());
    }
    if let Some(c) = &s.cleanup {
        let on = src("isCleanup").filter(|v| crate::model::boolean(v) == Some(true)).unwrap_or("1");
        values.insert("isCleanup".into(), on.to_string());
        let n = c.every_n_passes.max(1);
        match src("numPasses") {
            Some(raw) if raw.trim().parse::<u32>().ok() == Some(n) => values.insert("numPasses".into(), raw.into()),
            None if n == 1 => values.remove("numPasses"),
            _ => values.insert("numPasses".into(), n.to_string()),
        };
        if s.passes != 1 {
            w.push(format!(
                "{label}: a cleanup sub-layer makes one pass per cleanup in LightBurn; its {} passes were not exported",
                s.passes
            ));
        }
    }
}

/// The OpenEtch name of a LightBurn layer: its `name`, else its `subname`, else LightBurn's default (`C05`).
fn layer_name(index: u32, name: Option<&str>, subname: Option<&str>) -> String {
    let named = |s: Option<&str>| s.map(clean_name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match named(name).or_else(|| named(subname)) {
        Some(n) => n.chars().take(200).collect(),
        None => palette::default_name(index),
    }
}

/// Convert one LightBurn `CutSetting` (e.g. a `.clb` library entry) to an OpenEtch layer with id `id`.
pub fn cut_setting_to_layer(cs: &CutSetting, id: &str) -> (Layer, Vec<String>) {
    let mut w = Warnings::default();
    let layer = layer_from(cs.index, Some(cs), id.to_string(), &mut w);
    (layer, w.into_vec())
}

fn extra<'a>(map: &'a BTreeMap<String, String>, k: &str) -> Option<&'a str> {
    map.get(&key(k)).map(String::as_str)
}

/// The source tags kept by [`layer_from`] (`lbrn2._src`, or `lbrn2.subN._src`).
fn source_tags(l: &Layer, prefix: &str) -> Option<Vec<(String, String)>> {
    l.extras.get(&format!("{EXTRA_PREFIX}{prefix}{}", keys::SOURCE)).and_then(|x| xml_attributes(x))
}

/// OpenEtch settings → LightBurn `tag → value` pairs, starting from the kept extras. `frequency_absent`: the source
/// had no frequency and it was not changed since, so none is written. For a layer read from LightBurn (`source`), a
/// mapped tag the source left out stays out while its value is still the one assumed for it on import.
fn settings_values(
    s: &CutSettings,
    base: BTreeMap<String, String>,
    frequency_absent: bool,
    source: Option<&[(String, String)]>,
) -> BTreeMap<String, String> {
    let d = CutSettings::default();
    let in_source = |t: &str| source.is_some_and(|v| v.iter().any(|(k, _)| k == t));
    let mut v = base;
    let mut set = |k: &str, x: String, as_if_absent: bool| {
        if source.is_none() || in_source(k) || !as_if_absent {
            v.insert(k.to_string(), x);
        } else {
            v.remove(k);
        }
    };
    set("maxPower", s.power_pct.to_string(), s.power_pct == d.power_pct);
    set("speed", s.speed_mm_s.to_string(), s.speed_mm_s == d.speed_mm_s);
    let hz = s.frequency_khz * 1000.0;
    if !frequency_absent {
        set("frequency", if (hz - hz.round()).abs() < 1e-6 { hz.round() } else { hz }.to_string(), false);
    }
    set("interval", s.hatch.interval_mm.to_string(), s.hatch.interval_mm == d.hatch.interval_mm);
    set("angle", s.hatch.angle_deg.to_string(), s.hatch.angle_deg == 0.0);
    set("crossHatch", if s.hatch.cross_hatch { "1" } else { "0" }.into(), !s.hatch.cross_hatch);
    set("bidir", if s.hatch.bidirectional { "1" } else { "0" }.into(), s.hatch.bidirectional);
    set("numPasses", s.passes.to_string(), s.passes == 1);
    set("anglePerPass", s.hatch.angle_increment_deg.to_string(), s.hatch.angle_increment_deg == 0.0);
    if source.is_none() && s.hatch.angle_increment_deg == 0.0 {
        v.remove("anglePerPass");
    }
    match s.pulse_width_ns {
        Some(pw) => {
            v.insert("QPulseWidth".into(), pw.to_string());
        }
        None => {
            v.remove("QPulseWidth");
        }
    }
    v
}

/// The kind an exported layer is written with: its original LightBurn kind when the mode still matches it.
fn export_kind(original: Option<&str>, mode: &FillMode) -> String {
    match original {
        Some(k) if imported_mode(k) == *mode && k != "Tool" => k.to_string(),
        _ => kind_of(mode).to_string(),
    }
}

/// `values` as cut parameters written in the source's order and spelling (when there is a source).
fn params_from(values: &BTreeMap<String, String>, source: Option<Vec<(String, String)>>) -> CutParams {
    let mut p = CutParams::from_values(values.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    p.source = source.unwrap_or_default();
    p
}

// ---------------------------------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------------------------------

fn leaf_indices(s: &Shape, out: &mut BTreeSet<u32>) {
    match &s.kind {
        ShapeKind::Group(children) => children.iter().for_each(|c| leaf_indices(c, out)),
        ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Path(_) | ShapeKind::Bitmap { .. } => {
            out.insert(s.cut_index.unwrap_or(0));
        }
        ShapeKind::Text { backup: Some(_) } => {
            out.insert(s.cut_index.unwrap_or(0));
        }
        ShapeKind::Text { backup: None } | ShapeKind::Other { .. } => {}
    }
}

/// The type of a base64 image from its first bytes.
fn image_mime(data: &str) -> Option<&'static str> {
    [("iVBORw0KGgo", "image/png"), ("/9j/", "image/jpeg"), ("R0lGOD", "image/gif"), ("Qk", "image/bmp")]
        .into_iter()
        .find(|(magic, _)| data.starts_with(magic))
        .map(|(_, mime)| mime)
        .or_else(|| (data.starts_with("UklGR") && data.get(12..16) == Some("RUJQ")).then_some("image/webp"))
}

/// Convert a parsed project to OpenEtch layers, objects and groups in the field frame. `field_mm` is the galvo field
/// size the project was laid out for: LightBurn coordinates run 0..field_mm from the field's lower-left corner, so
/// every position moves by `-field_mm / 2`.
///
/// Groups become [`Group`]s (objects keep their place in the drawing order); Text becomes an object whose geometry
/// is LightBurn's saved outline and whose extras keep the text, font and size; bitmaps become images. Shapes OpenEtch
/// cannot show (unknown types, text without an outline, unusable sizes or placements) are kept in the extras of the
/// object before them, so they are exported again. Refuses (with [`LbrnError::TooLarge`]) a project that would
/// create more than a few million path elements in total.
pub fn to_document(p: &LbrnProject, field_mm: f64) -> Result<Imported, LbrnError> {
    to_document_in(p, Frame::Galvo { field_mm })
}

/// [`to_document`] for any [`Frame`]: a galvo field, or a bed, whose mirror flags should be the file's own
/// ([`LbrnProject::mirror`]) so the art lands where LightBurn showed it.
pub fn to_document_in(p: &LbrnProject, frame: Frame) -> Result<Imported, LbrnError> {
    to_document_within(p, frame, MAX_ELEMENTS)
}

fn to_document_within(p: &LbrnProject, frame: Frame, max_elements: usize) -> Result<Imported, LbrnError> {
    let frame = frame.checked()?;
    let mut w = Warnings::default();
    if let Frame::Galvo { .. } = frame {
        for k in ["MirrorX", "MirrorY"] {
            if let Some(v) = p.attribute(k).filter(|v| !matches!(*v, "False" | "false" | "0")) {
                w.push(format!("project {k}={v} was not applied"));
            }
        }
    }

    // Layers: every index with settings or used by an imported shape, in LightBurn's run order (priority).
    let mut indices: BTreeSet<u32> = p.cut_settings.iter().map(|c| c.index).collect();
    for s in &p.shapes {
        leaf_indices(s, &mut indices);
    }
    let setting_for = |i: u32| -> Option<&CutSetting> {
        let mut of_index = p.cut_settings.iter().filter(move |c| c.index == i);
        of_index.clone().find(|c| !c.image).or_else(|| of_index.next())
    };
    let mut order: Vec<u32> = indices.into_iter().collect();
    order.sort_by_key(|i| (setting_for(*i).and_then(|c| c.params.priority).unwrap_or(i64::MAX), *i));
    let mut next = 1u64;
    let mut layers = Vec::new();
    let mut layer_ids = HashMap::new();
    // Which cut setting each layer was made from, and the second (image) setting kept with it.
    let mut used: HashSet<usize> = HashSet::new();
    for i in order {
        if i > T1 + 1 {
            w.push(format!(
                "{}: layer index {i} is not a LightBurn layer (0..=31); its shapes were put on a non-output layer",
                palette::default_name(i)
            ));
        }
        let id = format!("layer-{next}");
        next += 1;
        layer_ids.insert(i, id.clone());
        let cs = setting_for(i);
        let mut layer = layer_from(i, cs, id, &mut w);
        if let Some(cs) = cs {
            used.extend(p.cut_settings.iter().position(|c| std::ptr::eq(c, cs)));
            if !cs.image
                && let Some((k, img)) = p.cut_settings.iter().enumerate().find(|(_, c)| c.index == i && c.image)
            {
                used.insert(k);
                let xml = fragment(LbrnProject {
                    cut_settings: vec![img.clone()],
                    order: vec![RootItem::CutSetting(0)],
                    format: p.format.clone(),
                    ..LbrnProject::default()
                });
                layer.extras.insert(key(keys::IMAGE_SETTING), xml);
                let label = layer.name.clone();
                layer.settings.image = image_of_setting(img, &label, &mut w);
                let (v, i) = (&cs.params, &img.params);
                let differs = |a: Option<f64>, b: Option<f64>| a.is_some() && b.is_some() && a != b;
                if differs(v.max_power, i.max_power)
                    || differs(v.speed, i.speed)
                    || differs(v.frequency, i.frequency)
                    || differs(v.q_pulse_width, i.q_pulse_width)
                    || (v.num_passes.is_some() && i.num_passes.is_some() && v.num_passes != i.num_passes)
                {
                    w.push(format!(
                        "{label}: LightBurn's image settings for this layer use their own power, speed, frequency or \
                         passes; OpenEtch engraves the layer's images with the layer's (they are kept for LightBurn)"
                    ));
                }
            }
        }
        layers.push(layer);
    }
    for (_, c) in p.cut_settings.iter().enumerate().filter(|(k, _)| !used.contains(k)) {
        w.push(format!(
            "{}: a second {} with the same index was ignored",
            palette::default_name(c.index),
            if c.image { "CutSetting_Img" } else { "CutSetting" }
        ));
    }
    if layers.is_empty() {
        let id = format!("layer-{next}");
        next += 1;
        layers.push(layer_from(0, None, id, &mut Warnings::default()));
        layer_ids.insert(0, layers[0].id.clone());
    }

    let mut cx = ToDoc {
        shift: frame.from_lightburn(),
        layer_ids,
        fallback: layers[0].id.clone(),
        next,
        objects: Vec::new(),
        groups: Vec::new(),
        lead: Vec::new(),
        w,
        elements: 0,
        max_elements,
        counts: HashMap::new(),
        texts: 0,
    };
    for s in &p.shapes {
        cx.shape(s, Affine::IDENTITY, None)?;
    }
    let ToDoc { objects, groups, lead, mut w, texts, .. } = cx;
    // Groups whose shapes were all left out are dropped.
    let mut tidy = Document { objects, groups, ..Document::default() };
    tidy.prune_groups();
    let Document { objects, groups, .. } = tidy;
    if texts > 0 {
        w.push(format!(
            "{texts} text object(s) are shown and burned as LightBurn's saved outline until they are edited; editing \
             one renders it again with its font, which must be installed"
        ));
    }
    let outside = objects.iter().filter(|o| !frame.contains(o.bounds())).count();
    if outside > 0 {
        let what = match frame {
            Frame::Galvo { .. } => "the field size, so check that field_mm matches the one",
            Frame::Bed { .. } => "the workspace size, so check that it matches the one",
        };
        w.push(format!(
            "{outside} object(s) are partly or wholly outside {}; LightBurn files do not record {what} the project \
             was laid out for",
            frame.describe()
        ));
    }

    // The file itself: root attributes, layout, and the root's children in order.
    let mut extras = BTreeMap::new();
    extras.insert(key(keys::ROOT_ATTRS), attributes_xml("LightBurnProject", &p.attributes));
    if let Some(d) = &p.format.declaration {
        extras.insert(key(keys::DECLARATION), d.clone());
    }
    let flags: Vec<&str> = [
        (p.format.crlf, "crlf"),
        (!p.format.final_newline, "no-final-newline"),
        (p.format.space_before_slash, "space-before-slash"),
    ]
    .into_iter()
    .filter_map(|(on, f)| on.then_some(f))
    .collect();
    if !flags.is_empty() {
        extras.insert(key(keys::FORMAT), flags.join(" "));
    }
    let mut tokens = Vec::new();
    for item in &p.order {
        match *item {
            RootItem::CutSetting(k) => {
                let c = &p.cut_settings[k];
                tokens.push(format!("{}{}", if c.image { 'I' } else { 'C' }, c.index));
            }
            RootItem::Shape(_) => {
                if !tokens.iter().any(|t| t == "S") {
                    tokens.push("S".into());
                }
            }
            RootItem::Element(k) => {
                let e = &p.extra_elements[k];
                let xml = match &e.raw {
                    Some(r) => r.clone(),
                    None => element_xml(e),
                };
                extras.insert(format!("{EXTRA_PREFIX}{}{k}", keys::ROOT_ELEMENT), xml);
                tokens.push(format!("E{k}"));
            }
        }
    }
    if !tokens.is_empty() {
        extras.insert(key(keys::ROOT_ORDER), tokens.join(" "));
    }
    for (k, xml) in lead.into_iter().enumerate() {
        extras.insert(format!("{EXTRA_PREFIX}{}{k}", keys::LEAD), xml);
    }
    let variables = crate::text::variables_from_extras(&extras).unwrap_or_default();
    Ok(Imported { layers, objects, groups, extras, variables, warnings: w.into_vec() })
}

/// An element as XML text (for elements without source text).
fn element_xml(e: &Element) -> String {
    fragment(LbrnProject { extra_elements: vec![e.clone()], order: vec![RootItem::Element(0)], ..LbrnProject::default() })
}

struct ToDoc {
    shift: Affine,
    layer_ids: HashMap<u32, String>,
    fallback: String,
    next: u64,
    objects: Vec<Object>,
    groups: Vec<Group>,
    /// Kept shapes that came before any object.
    lead: Vec<String>,
    w: Warnings,
    /// Path elements created so far, and the most allowed.
    elements: usize,
    max_elements: usize,
    /// Objects made per kind, for names.
    counts: HashMap<&'static str, usize>,
    texts: usize,
}

/// The extras an object or group keeps of its shape: type, attributes in order (all but those in `skip`), `XForm`
/// text and unknown child elements.
fn shape_extras(s: &Shape, skip: &[&str], w: &mut Warnings) -> BTreeMap<String, String> {
    let mut e = BTreeMap::new();
    e.insert(key(keys::TYPE), s.type_name().to_string());
    let mut names = Vec::new();
    for (k, v) in &s.attributes {
        if !doc::is_plain_name(k) || k.starts_with('_') {
            w.push(format!("a shape attribute {} has a name OpenEtch cannot keep; it was dropped", shown(k)));
            continue;
        }
        names.push(k.as_str());
        if !skip.contains(&k.as_str()) {
            e.insert(key(k), v.clone());
        }
    }
    e.insert(key(keys::ATTRS), names.join(" "));
    if let Some(t) = &s.xform_text {
        e.insert(key(keys::XFORM), t.clone());
    }
    for (k, c) in s.children.iter().enumerate() {
        let xml = c.raw.clone().unwrap_or_else(|| element_xml(c));
        e.insert(format!("{EXTRA_PREFIX}{}{k}", keys::CHILD), xml);
    }
    e
}

/// The lists a path carried itself, as extras.
fn list_extras(d: &PathData, e: &mut BTreeMap<String, String>) {
    if let Some(enc) = &d.encoded {
        if let Some(v) = &enc.vert_list {
            e.insert(key(keys::VERT_LIST), v.clone());
        }
        if let Some(p) = &enc.prim_list {
            e.insert(key(keys::PRIM_LIST), p.clone());
        }
    }
}

impl ToDoc {
    fn name(&mut self, kind: &'static str) -> String {
        let n = self.counts.entry(kind).or_default();
        *n += 1;
        format!("{kind} {n}")
    }

    /// Keep a shape OpenEtch cannot show, after the last object made (or before the first).
    fn keep_shape(&mut self, s: &Shape) {
        let xml = shape_to_xml(s);
        match self.objects.last_mut() {
            Some(o) => {
                let k = o.extras.keys().filter(|k| k.starts_with(&key(keys::AFTER))).count();
                o.extras.insert(format!("{EXTRA_PREFIX}{}{k}", keys::AFTER), xml);
            }
            None => self.lead.push(xml),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        s: &Shape,
        name: String,
        geometry: Geometry,
        local: Affine,
        group: &Option<String>,
        extras: BTreeMap<String, String>,
    ) -> Result<bool, LbrnError> {
        let elements = match &geometry {
            Geometry::Path { path } => path.elements().len(),
            Geometry::Text(t) => t.outline.elements().len(),
            Geometry::Polygon { .. } => geometry.local_path().elements().len(),
            Geometry::Rect { .. } | Geometry::Ellipse { .. } | Geometry::Image { .. } => FIXED_SHAPE_ELEMENTS,
        };
        self.elements = self.elements.saturating_add(elements);
        if self.elements > self.max_elements {
            return Err(LbrnError::TooLarge(format!(
                "the shapes would make more than {} path elements (shared lists and groups are expanded for every \
                 use)",
                self.max_elements
            )));
        }
        let index = s.cut_index.unwrap_or(0);
        let layer = self.layer_ids.get(&index).cloned().unwrap_or_else(|| self.fallback.clone());
        let mut o = Object::new(format!("obj-{}", self.next), name, layer, geometry, self.shift * local);
        if let Some(problem) = o.placement_problem() {
            self.w.push(format!(
                "a shape was kept for LightBurn export but not imported: {problem} (check its scale and its groups' \
                 scales)"
            ));
            self.keep_shape(s);
            return Ok(false);
        }
        o.group = group.clone();
        o.extras = extras;
        self.next += 1;
        self.objects.push(o);
        Ok(true)
    }

    fn path_object(
        &mut self,
        s: &Shape,
        name: String,
        d: &PathData,
        xf: Affine,
        group: &Option<String>,
        extras: BTreeMap<String, String>,
    ) -> Result<bool, LbrnError> {
        let path = d.to_bez();
        if path.segments().next().is_none() {
            self.w.push("an empty path was kept for LightBurn export but not imported");
            self.keep_shape(s);
            return Ok(false);
        }
        self.push(s, name, Geometry::Path { path }, xf, group, extras)
    }

    fn shape(&mut self, s: &Shape, parent: Affine, group: Option<String>) -> Result<(), LbrnError> {
        let xf = parent * s.xform;
        match &s.kind {
            ShapeKind::Rect { w, h, cr } => {
                if !(*w > 0.0 && *h > 0.0) {
                    self.w.push("a zero-size rectangle was kept for LightBurn export but not imported");
                    self.keep_shape(s);
                    return Ok(());
                }
                let max = w.min(*h) / 2.0;
                let r = cr.clamp(0.0, max);
                if r != *cr {
                    self.w.push(format!("a rectangle's corner radius {cr} mm was clamped to {r} mm"));
                }
                let e = shape_extras(s, &["CutIndex"], &mut self.w);
                let name = self.name("Rectangle");
                let g = Geometry::Rect { width_mm: *w, height_mm: *h, corner_radius_mm: r };
                self.push(s, name, g, xf, &group, e)?;
            }
            ShapeKind::Ellipse { rx, ry } => {
                if !(*rx > 0.0 && *ry > 0.0) {
                    self.w.push("a zero-size ellipse was kept for LightBurn export but not imported");
                    self.keep_shape(s);
                    return Ok(());
                }
                let e = shape_extras(s, &["CutIndex"], &mut self.w);
                let name = self.name("Ellipse");
                self.push(s, name, Geometry::Ellipse { rx_mm: *rx, ry_mm: *ry }, xf, &group, e)?;
            }
            ShapeKind::Path(d) => {
                let mut e = shape_extras(s, &["CutIndex"], &mut self.w);
                list_extras(d, &mut e);
                let name = self.name("Path");
                self.path_object(s, name, d, xf, &group, e)?;
            }
            ShapeKind::Group(children) => {
                let id = format!("group-{}", self.next);
                self.next += 1;
                let n = self.counts.entry("Group").or_default();
                *n += 1;
                let mut g = Group::new(id.clone(), format!("Group {n}"), group);
                g.extras = shape_extras(s, &[], &mut self.w);
                self.groups.push(g);
                for c in children {
                    self.shape(c, xf, Some(id.clone()))?;
                }
            }
            ShapeKind::Text { backup: Some(b) } => {
                let mut e = shape_extras(s, &["CutIndex"], &mut self.w);
                list_extras(&b.path, &mut e);
                let names: Vec<&str> = b.attributes.iter().map(|(k, _)| k.as_str()).filter(|k| doc::is_plain_name(k)).collect();
                e.insert(key(keys::BACKUP_ATTRS), names.join(" "));
                for (k, v) in b.attributes.iter().filter(|(k, _)| doc::is_plain_name(k) && k != "CutIndex") {
                    e.insert(format!("{EXTRA_PREFIX}{}{k}", keys::BACKUP_ATTR), v.clone());
                }
                if let Some(t) = &b.xform_text {
                    e.insert(key(keys::BACKUP_XFORM), t.clone());
                }
                let text = s.attribute("Str").and_then(|t| t.lines().next()).map(clean_name).unwrap_or_default();
                let text = text.trim();
                let name = if text.is_empty() { self.name("Text") } else { text.chars().take(100).collect() };
                // The Text's own XForm is the live one; the backup's copy can be stale (see crate docs).
                let outline = b.path.to_bez();
                let live = crate::text::from_attributes(&|k| s.attribute(k).map(str::to_string), outline.clone());
                let made = match live {
                    Some(t) if outline.segments().next().is_some() => {
                        self.push(s, name, Geometry::Text(Box::new(t)), xf, &group, e)?
                    }
                    _ => self.path_object(s, name, &b.path, xf, &group, e)?,
                };
                if made {
                    self.texts += 1;
                }
            }
            ShapeKind::Text { backup: None } => {
                let text = s.attribute("Str").and_then(|t| t.lines().next()).map(clean_name).unwrap_or_default();
                let text: String = text.trim().chars().take(60).collect();
                self.w.push(format!(
                    "text \"{text}\" has no saved outline: it was kept for LightBurn export but not imported (convert \
                     it to a path in LightBurn to use it here)"
                ));
                self.keep_shape(s);
            }
            ShapeKind::Bitmap { w, h } => {
                let data = s.attribute("Data").unwrap_or_default();
                let mime = image_mime(data).filter(|_| doc::file::is_base64(data) && *w > 0.0 && *h > 0.0);
                let Some(mime) = mime else {
                    self.w.push("a bitmap with no usable image data was kept for LightBurn export but not imported");
                    self.keep_shape(s);
                    return Ok(());
                };
                let e = shape_extras(s, &["CutIndex", "Data"], &mut self.w);
                let name = match s.attribute("File").and_then(|f| f.rsplit(['/', '\\']).next()).map(clean_name) {
                    Some(f) if !f.trim().is_empty() => f.trim().chars().take(100).collect(),
                    _ => self.name("Image"),
                };
                let (adjust, problems) = crate::image::adjust_from(&|n| s.attribute(n).map(str::to_string));
                for p in problems {
                    self.w.push(format!("image \"{name}\": {p}"));
                }
                let g = Geometry::Image { width_mm: *w, height_mm: *h, mime: mime.into(), data_base64: data.into(), adjust };
                // LightBurn keeps bitmap rows bottom-up (see `BITMAP_ROWS`); OpenEtch's images are top-down files.
                self.push(s, name, g, xf * BITMAP_ROWS, &group, e)?;
            }
            ShapeKind::Other { type_name } => {
                self.w.push(format!(
                    "a '{type_name}' shape was kept for LightBurn export but is not shown or burned (unsupported)"
                ));
                self.keep_shape(s);
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------------------------------

/// Convert a document to a LightBurn project laid out for a `field_mm` field (positions move by `+field_mm / 2`).
/// Layers keep their LightBurn index when they have one (`lbrn2.index`), else take the palette index of their colour,
/// else the lowest free one; more than 30 output layers is an error. Groups become LightBurn groups, images bitmaps,
/// objects imported from Text LightBurn text again. The stock outline goes on tool layer T1 (T2 if a layer already
/// uses T1). What the import kept (see [`keys`]) is written back; numbers, transforms and path lists that did not
/// change keep their exact source text.
pub fn from_document(d: &Document, field_mm: f64) -> Result<(LbrnProject, Vec<String>), LbrnError> {
    from_document_in(d, Frame::Galvo { field_mm })
}

/// [`from_document`] for any [`Frame`]. For a bed the project root's `MirrorX` / `MirrorY` are set to the frame's
/// flags (the target device's origin corner); a galvo export writes back the root attributes the import kept.
pub fn from_document_in(d: &Document, frame: Frame) -> Result<(LbrnProject, Vec<String>), LbrnError> {
    let frame = frame.checked()?;
    let mut w = Warnings::default();

    // Assign indices: kept index, then palette colour, then the lowest free colour index.
    let mut taken = BTreeSet::new();
    let mut index_of: HashMap<&str, u32> = HashMap::new();
    // OpenEtch's tool layers (LAY-042) are LightBurn's T1 / T2.
    for l in &d.layers {
        if let Some(t) = l.tool
            && taken.insert(t.index())
        {
            index_of.insert(&l.id, t.index());
        }
    }
    for l in &d.layers {
        if index_of.contains_key(l.id.as_str()) {
            continue;
        }
        let kept = extra(&l.extras, "index").and_then(|i| i.parse::<u32>().ok());
        if let Some(i) = kept.filter(|i| *i < COLOR_LAYERS || ((*i == T1 || *i == T1 + 1) && !l.output))
            && taken.insert(i)
        {
            index_of.insert(&l.id, i);
        }
    }
    for l in &d.layers {
        if index_of.contains_key(l.id.as_str()) {
            continue;
        }
        let i = palette::index_of_color(&l.color)
            .filter(|i| !taken.contains(i))
            .or_else(|| (0..COLOR_LAYERS).find(|i| !taken.contains(i)))
            .ok_or_else(|| {
                LbrnError::Invalid(format!("the document has more layers than LightBurn's {COLOR_LAYERS}"))
            })?;
        taken.insert(i);
        index_of.insert(&l.id, i);
    }

    // Priorities: the source's while every layer came from LightBurn and the order still agrees with them, else the
    // position in the layer list.
    let sources: Vec<Option<Vec<(String, String)>>> = d.layers.iter().map(|l| source_tags(l, "")).collect();
    let source_priority = |s: &Option<Vec<(String, String)>>| {
        s.as_ref().and_then(|v| v.iter().find(|(k, _)| k == "priority")).and_then(|(_, p)| p.trim().parse::<i64>().ok())
    };
    let kept_priorities = sources.iter().all(Option::is_some) && {
        let p: Vec<i64> = sources.iter().filter_map(source_priority).collect();
        p.windows(2).all(|w| w[0] <= w[1])
    };

    let mut cut_settings: Vec<CutSetting> = Vec::new();
    // Per layer index: the CutSetting_Img kept with it.
    let mut kept_images: BTreeMap<u32, String> = BTreeMap::new();
    for (position, (l, source)) in d.layers.iter().zip(&sources).enumerate() {
        let index = index_of[l.id.as_str()];
        let original = extra(&l.extras, "type");
        let tool = index >= COLOR_LAYERS || original == Some("Tool");
        // Kept LightBurn values, minus the keys this function owns, the sub-layers' (`subN.*`) and bookkeeping
        // (`_*`). Only tags that are plain XML names are written: extras can come from a hand-edited .openetch file.
        let mut base = BTreeMap::new();
        let mut frequency_absent = false;
        for (k, v) in &l.extras {
            let Some(tag) = k.strip_prefix(EXTRA_PREFIX) else { continue };
            if sub_layer_tag(tag).is_some() || matches!(tag, "index" | "type" | "image") || tag.starts_with('_') {
                continue;
            }
            if tag == FREQUENCY_ABSENT {
                frequency_absent = true;
            } else if doc::is_plain_name(tag) {
                base.insert(tag.to_string(), v.clone());
            } else {
                w.push(format!(
                    "layer \"{}\": extra {} is not a LightBurn tag name and was not exported",
                    l.name,
                    shown(k)
                ));
            }
        }
        let src = source.as_deref();
        let in_source = |t: &str| src.is_some_and(|v| v.iter().any(|(k, _)| k == t));
        let mut values = if tool { base } else { settings_values(&l.settings, base, frequency_absent, src) };
        // The source's own spelling while it still gives this name (e.g. an empty name shown as `C00`).
        let src_tag = |t: &str| src.and_then(|v| v.iter().find(|(k, _)| k == t)).map(|(_, v)| v.as_str());
        let name = match src_tag("name") {
            Some(n) if layer_name(index, Some(n), src_tag("subname")) == l.name => n.to_string(),
            _ => l.name.clone(),
        };
        values.insert("name".into(), name);
        if kept_priorities {
            match source_priority(source) {
                Some(p) => {
                    values.insert("priority".into(), p.to_string());
                }
                None => {
                    values.remove("priority");
                }
            }
        } else {
            values.insert("priority".into(), position.to_string());
        }
        if !tool {
            if !l.output {
                values.insert("doOutput".into(), "0".into());
            } else if in_source("doOutput") {
                values.insert("doOutput".into(), "1".into());
            }
        }
        if !l.visible {
            values.insert("hide".into(), "1".into());
        } else if in_source("hide") {
            values.insert("hide".into(), "0".into());
        }
        let kind = if tool { "Tool".to_string() } else { export_kind(original, &l.settings.mode) };
        // A LightBurn image layer (`CutSetting_Img`): its image settings are written into it and its sub-layers.
        let is_image = kind == "Image" && extra(&l.extras, "image") == Some("1");
        let mut sub_layers = Vec::new();
        if !tool {
            for (n, s) in l.sub_layers.iter().enumerate() {
                let mut base = BTreeMap::new();
                let mut sub_kind = None;
                let mut frequency_absent = false;
                for (k, v) in &l.extras {
                    match k.strip_prefix(EXTRA_PREFIX).and_then(sub_layer_tag) {
                        Some((m, _)) if m != n + 1 => {}
                        Some((_, "type")) => sub_kind = Some(v.as_str()),
                        Some((_, FREQUENCY_ABSENT)) => frequency_absent = true,
                        Some((_, tag)) if tag.starts_with('_') => {}
                        Some((_, tag)) if doc::is_plain_name(tag) => {
                            base.insert(tag.to_string(), v.clone());
                        }
                        Some(_) => w.push(format!(
                            "layer \"{}\": extra {} is not a LightBurn tag name and was not exported",
                            l.name,
                            shown(k)
                        )),
                        None => {}
                    }
                }
                let sub_source = source_tags(l, &format!("sub{}.", n + 1));
                let mut values = settings_values(s, base, frequency_absent, sub_source.as_deref());
                let sub_label = format!("layer \"{}\" sub-layer {}", l.name, n + 1);
                sub_layer_values(s, &mut values, sub_source.as_deref(), &sub_label, &mut w);
                if is_image
                    && let Some(m) =
                        crate::image::image_values(&s.image, &mut values, sub_source.as_deref(), &l.settings.image)
                {
                    w.push(format!("{sub_label}: {m}"));
                }
                sub_layers.push(SubLayer {
                    kind: export_kind(sub_kind, &s.mode),
                    index: n as u32 + 1,
                    params: params_from(&values, sub_source),
                });
            }
            // LightBurn's "Enable cleanup pass" on the parent goes with its cleanup sub-layer.
            let enabled = values.get("cleanupPass").and_then(|v| crate::model::boolean(v)) == Some(true);
            match l.sub_layers.iter().any(|s| s.cleanup.is_some()) {
                true if !enabled => {
                    values.insert("cleanupPass".into(), "1".into());
                }
                false if enabled => {
                    values.remove("cleanupPass");
                }
                _ => {}
            }
        } else if !l.sub_layers.is_empty() {
            w.push(format!("layer \"{}\" is a LightBurn tool layer; its sub-layers were not exported", l.name));
        }
        if is_image
            && let Some(m) =
                crate::image::image_values(&l.settings.image, &mut values, source.as_deref(), &ImageSettings::default())
        {
            w.push(format!("layer \"{}\": {m}", l.name));
        }
        let kept_index = extra(&l.extras, "index").and_then(|i| i.parse::<u32>().ok()) == Some(index);
        match extra(&l.extras, keys::IMAGE_SETTING) {
            Some(img) if kept_index => {
                kept_images.insert(index, kept_image_xml(img, l, &mut w));
            }
            kept => {
                let has_images =
                    d.objects.iter().any(|o| o.layer == l.id && matches!(o.geometry, Geometry::Image { .. }));
                if !tool && !is_image && has_images {
                    // Moved to another index (the kept one names the old index), or never had one.
                    kept_images.insert(index, generated_image_xml(l, index, values.get("priority"), &mut w));
                } else if kept.is_some() {
                    w.push(format!(
                        "layer \"{}\" moved to another LightBurn index; its image settings were not exported",
                        l.name
                    ));
                }
            }
        }
        cut_settings.push(CutSetting {
            image: is_image,
            kind,
            index,
            params: params_from(&values, source.clone()),
            sub_layers,
        });
    }

    // Shapes, in drawing order, with groups gathered at their first member.
    let mut fx = FromDoc::new(d, &index_of, frame);
    for k in 0.. {
        let Some(xml) = d.extras.get(&format!("{EXTRA_PREFIX}{}{k}", keys::LEAD)) else { break };
        fx.kept(xml, &mut w);
    }
    let mut done_groups: HashSet<&str> = HashSet::new();
    for o in &d.objects {
        if !index_of.contains_key(o.layer.as_str()) {
            w.push(format!("object {} (\"{}\") is on a missing layer and was not exported", o.id, o.name));
            continue;
        }
        match d.top_group_of(&o.id) {
            None => fx.object(o, Affine::IDENTITY, None, &mut w),
            Some(g) => {
                if done_groups.insert(g.id.as_str()) {
                    fx.group(g, Affine::IDENTITY, None, &mut w);
                }
            }
        }
    }
    let FromDoc { mut shapes, outside, .. } = fx;
    if outside > 0 {
        w.push(format!("{outside} object(s) are partly or wholly outside {}", frame.describe()));
    }

    if let Some(stock) = &d.stock {
        match [T1, T1 + 1].into_iter().find(|i| !taken.contains(i)) {
            Some(index) => {
                let kind = match stock.shape {
                    StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                        ShapeKind::Rect { w: width_mm, h: height_mm, cr: corner_radius_mm }
                    }
                    StockShape::Circle { diameter_mm } => {
                        ShapeKind::Ellipse { rx: diameter_mm / 2.0, ry: diameter_mm / 2.0 }
                    }
                };
                shapes.push(Shape::new(kind, Some(index), frame.to_lightburn() * stock.transform()));
                let mut values = vec![("name", palette::default_name(index))];
                if !kept_priorities {
                    values.push(("priority", cut_settings.len().to_string()));
                }
                cut_settings.push(CutSetting {
                    kind: "Tool".into(),
                    image: false,
                    index,
                    params: CutParams::from_values(values.iter().map(|(k, v)| (*k, v.as_str()))),
                    sub_layers: vec![],
                });
            }
            None => w.push("the stock outline was not exported: both tool layers (T1, T2) are in use"),
        }
    }

    let mut project = root(d, cut_settings, kept_images, shapes);
    if let Frame::Bed { mirror_x, mirror_y, .. } = frame {
        set_mirror(&mut project.attributes, "MirrorX", mirror_x);
        set_mirror(&mut project.attributes, "MirrorY", mirror_y);
    }
    Ok((project, w.into_vec()))
}

/// Set root attribute `name` to `on`, keeping the kept spelling when it already means the same.
fn set_mirror(attributes: &mut Vec<(String, String)>, name: &str, on: bool) {
    let text = if on { "True" } else { "False" };
    match attributes.iter_mut().find(|(k, _)| k == name) {
        Some((_, v)) if truthy(v) == on => {}
        Some((_, v)) => *v = text.to_string(),
        None => attributes.push((name.to_string(), text.to_string())),
    }
}

/// The project around the layers and shapes: the root attributes, elements and child order kept from the source,
/// or LightBurn's defaults for a document that did not come from LightBurn.
fn root(d: &Document, cut_settings: Vec<CutSetting>, mut kept_images: BTreeMap<u32, String>, shapes: Vec<Shape>) -> LbrnProject {
    let attributes = extra(&d.extras, keys::ROOT_ATTRS).and_then(xml_attributes).unwrap_or_else(|| {
        [("AppVersion", "1.4.00"), ("FormatVersion", "1"), ("MaterialHeight", "0"), ("MirrorX", "False"), ("MirrorY", "False")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .to_vec()
    });
    let flags = extra(&d.extras, keys::FORMAT).unwrap_or_default();
    let format = Format {
        declaration: extra(&d.extras, keys::DECLARATION).map(str::to_string),
        crlf: flags.split_whitespace().any(|f| f == "crlf"),
        final_newline: !flags.split_whitespace().any(|f| f == "no-final-newline"),
        space_before_slash: flags.split_whitespace().any(|f| f == "space-before-slash"),
    };
    let n_shapes = shapes.len();
    let mut p = LbrnProject { attributes, shapes, format, ..LbrnProject::default() };

    let Some(tokens) = extra(&d.extras, keys::ROOT_ORDER) else {
        // Not from LightBurn: its usual layout with default variable text and notes.
        let value = |name: &str, v: &str| Element {
            name: name.into(),
            attributes: vec![("Value".into(), v.into())],
            ..Element::default()
        };
        let v = &d.variables;
        p.extra_elements.push(Element {
            name: "VariableText".into(),
            children: vec![
                value("Start", &v.start.to_string()),
                value("End", &v.end.to_string()),
                value("Current", &v.current.to_string()),
                value("Increment", &v.advance_by.to_string()),
                value("AutoAdvance", if v.auto_advance { "1" } else { "0" }),
            ],
            ..Element::default()
        });
        p.extra_elements.push(Element {
            name: "Notes".into(),
            attributes: vec![("ShowOnLoad".into(), "0".into()), ("Notes".into(), String::new())],
            ..Element::default()
        });
        // Each kept image setting follows its layer's settings.
        p.order.push(RootItem::Element(0));
        for c in cut_settings {
            let image = kept_images.remove(&c.index);
            p.order.push(RootItem::CutSetting(p.cut_settings.len()));
            p.cut_settings.push(c);
            if let Some(xml) = image {
                p.order.push(RootItem::Element(p.extra_elements.len()));
                p.extra_elements.push(raw_element(&xml));
            }
        }
        p.order.extend((0..n_shapes).map(RootItem::Shape));
        p.order.push(RootItem::Element(1));
        return p;
    };

    let mut pending: Vec<Option<CutSetting>> = cut_settings.into_iter().map(Some).collect();
    let mut order = Vec::new();
    let mut last_setting = None;
    let push_setting = |p: &mut LbrnProject, order: &mut Vec<RootItem>, c: CutSetting| {
        order.push(RootItem::CutSetting(p.cut_settings.len()));
        p.cut_settings.push(c);
    };
    for t in tokens.split_whitespace() {
        let (tag, rest) = t.split_at(1);
        match tag {
            "E" => {
                if let Some(xml) = d.extras.get(&format!("{EXTRA_PREFIX}{}{rest}", keys::ROOT_ELEMENT)) {
                    order.push(RootItem::Element(p.extra_elements.len()));
                    // The variable text counter is modelled (`Document::variables`): write its current values in.
                    let xml = match crate::text::variables_from_xml(xml) {
                        Some(_) => crate::text::variables_into_xml(xml, &d.variables),
                        None => xml.clone(),
                    };
                    p.extra_elements.push(raw_element(&xml));
                }
            }
            "C" | "I" => {
                let Ok(index) = rest.parse::<u32>() else { continue };
                let image = tag == "I";
                if let Some(slot) = pending.iter_mut().find(|c| c.as_ref().is_some_and(|c| c.index == index && c.image == image))
                    && let Some(c) = slot.take()
                {
                    push_setting(&mut p, &mut order, c);
                    last_setting = Some(order.len());
                } else if image && let Some(xml) = kept_images.remove(&index) {
                    order.push(RootItem::Element(p.extra_elements.len()));
                    p.extra_elements.push(raw_element(&xml));
                    last_setting = Some(order.len());
                }
            }
            "S" => order.extend((0..n_shapes).map(RootItem::Shape)),
            _ => {}
        }
    }
    // Settings the source did not have (new layers, the stock's tool layer): after the last kept one, else before the
    // shapes.
    let mut at = last_setting
        .or_else(|| order.iter().position(|i| matches!(i, RootItem::Shape(_))))
        .unwrap_or(order.len());
    for c in pending.into_iter().flatten() {
        order.insert(at, RootItem::CutSetting(p.cut_settings.len()));
        p.cut_settings.push(c);
        at += 1;
    }
    for (_, xml) in kept_images {
        order.insert(at, RootItem::Element(p.extra_elements.len()));
        p.extra_elements.push(raw_element(&xml));
        at += 1;
    }
    if !order.iter().any(|i| matches!(i, RootItem::Shape(_))) {
        let at = order.iter().rposition(|i| matches!(i, RootItem::CutSetting(_))).map_or(order.len(), |k| k + 1);
        order.splice(at..at, (0..n_shapes).map(RootItem::Shape));
    }
    p.order = order;
    p
}

/// An element written back exactly as kept.
fn raw_element(xml: &str) -> Element {
    let name: String = xml.trim_start_matches('<').chars().take_while(|c| !c.is_whitespace() && !matches!(c, '/' | '>')).collect();
    Element { name, raw: Some(xml.to_string()), ..Element::default() }
}

struct FromDoc<'a> {
    d: &'a Document,
    index_of: &'a HashMap<&'a str, u32>,
    frame: Frame,
    /// Field frame → LightBurn, and back.
    to_lb: Affine,
    from_lb: Affine,
    /// `VertID` / `PrimID` → the list text that defined it, from the objects that carried it.
    vert_defs: HashMap<&'a str, &'a str>,
    prim_defs: HashMap<&'a str, &'a str>,
    /// Ids whose list has been written.
    written_verts: HashSet<String>,
    written_prims: HashSet<String>,
    shapes: Vec<Shape>,
    outside: usize,
}

/// `extras` value of an object or group.
fn ex<'a>(e: &'a BTreeMap<String, String>, k: &str) -> Option<&'a str> {
    e.get(&key(k)).map(String::as_str)
}

impl<'a> FromDoc<'a> {
    fn new(d: &'a Document, index_of: &'a HashMap<&'a str, u32>, frame: Frame) -> Self {
        let mut vert_defs = HashMap::new();
        let mut prim_defs = HashMap::new();
        for o in &d.objects {
            let backup = ex(&o.extras, keys::TYPE) == Some("Text");
            let (vid, pid) = if backup {
                (ex(&o.extras, "_b.VertID"), ex(&o.extras, "_b.PrimID"))
            } else {
                (ex(&o.extras, "VertID"), ex(&o.extras, "PrimID"))
            };
            if let (Some(id), Some(list)) = (vid, ex(&o.extras, keys::VERT_LIST)) {
                vert_defs.entry(id).or_insert(list);
            }
            if let (Some(id), Some(list)) = (pid, ex(&o.extras, keys::PRIM_LIST)) {
                prim_defs.entry(id).or_insert(list);
            }
        }
        FromDoc {
            d,
            index_of,
            frame,
            to_lb: frame.to_lightburn(),
            from_lb: frame.from_lightburn(),
            vert_defs,
            prim_defs,
            written_verts: HashSet::new(),
            written_prims: HashSet::new(),
            shapes: Vec::new(),
            outside: 0,
        }
    }

    /// A kept shape (see [`keys::AFTER`]), into `out` (the top level when `None`).
    fn kept_into(&mut self, xml: &str, out: Option<&mut Vec<Shape>>, w: &mut Warnings) {
        match shape_from_xml(xml) {
            Some(s) => match out {
                Some(v) => v.push(s),
                None => self.shapes.push(s),
            },
            None => w.push("a kept LightBurn shape could not be read back and was not exported"),
        }
    }

    fn kept(&mut self, xml: &str, w: &mut Warnings) {
        self.kept_into(xml, None, w);
    }

    /// The transform to write for an object or group with extras `e` under groups whose LightBurn transform is
    /// `parent`, given its field transform `field_tf`: the kept `XForm` when it still gives the same placement, else
    /// one computed from the current placement. Also its text, for the writer.
    fn local(&self, e: &BTreeMap<String, String>, parent: Affine, field_tf: Affine) -> (Affine, Option<String>) {
        let text = ex(e, keys::XFORM);
        if let Some(t) = text
            && let Ok(x) = crate::parse::parse_xform(t)
            && self.from_lb * (parent * x) == field_tf
        {
            return (x, Some(t.to_string()));
        }
        (parent.inverse() * (self.to_lb * field_tf), text.map(str::to_string))
    }

    /// Attributes in the kept order with the kept values (`Data` and `CutIndex` filled in by the caller / writer).
    fn attributes(e: &BTreeMap<String, String>, names: Option<&str>, prefix: &str) -> Vec<(String, String)> {
        names
            .unwrap_or_default()
            .split_whitespace()
            .map(|k| (k.to_string(), e.get(&format!("{EXTRA_PREFIX}{prefix}{k}")).cloned().unwrap_or_default()))
            .collect()
    }

    /// The path as written: with the kept ids and lists while it is unchanged, else as a new path.
    fn path_data(&mut self, e: &BTreeMap<String, String>, path: &BezPath, backup: bool) -> PathData {
        let (vid, pid) = if backup {
            (ex(e, "_b.VertID"), ex(e, "_b.PrimID"))
        } else {
            (ex(e, "VertID"), ex(e, "PrimID"))
        };
        let vtext = ex(e, keys::VERT_LIST).or_else(|| vid.and_then(|id| self.vert_defs.get(id).copied()));
        let ptext = ex(e, keys::PRIM_LIST).or_else(|| pid.and_then(|id| self.prim_defs.get(id).copied()));
        if let (Some(vt), Some(pt)) = (vtext, ptext)
            && let Ok(vertices) = parse_vertices(vt, usize::MAX)
            && let Ok((prims, _)) = parse_prims(pt, vertices.len(), usize::MAX)
        {
            let mut d = PathData { vertices, prims, encoded: None };
            if d.to_bez() == *path {
                // A list the path carried itself is written again (LightBurn repeats some); one it only referred to
                // is written by the first path that uses it.
                let list = |own: bool, id: Option<&str>, written: &mut HashSet<String>, text: &str| {
                    let first = id.is_none_or(|id| written.insert(id.to_string()));
                    (own || first).then(|| text.to_string())
                };
                let vert_list = list(ex(e, keys::VERT_LIST).is_some(), vid, &mut self.written_verts, vt);
                let prim_list = list(ex(e, keys::PRIM_LIST).is_some(), pid, &mut self.written_prims, pt);
                d.encoded = Some(Encoded {
                    vert_id: vid.map(str::to_string),
                    prim_id: pid.map(str::to_string),
                    vert_list,
                    prim_list,
                });
                return d;
            }
        }
        PathData::from_bez(path)
    }

    /// A LightBurn Text shape's body: its `BackupPath` (`outline`, with the kept ids and lists while unchanged) and
    /// the backup's `XForm` (the kept one while the text's own did not change).
    fn text_shape(
        &mut self,
        e: &BTreeMap<String, String>,
        outline: &BezPath,
        index: u32,
        xform: &Affine,
        xform_text: Option<&str>,
    ) -> ShapeKind {
        let path = self.path_data(e, outline, true);
        let unchanged =
            xform_text.is_some_and(|t| crate::parse::parse_xform(t).is_ok_and(|x| x.as_coeffs() == xform.as_coeffs()));
        let kept = ex(e, keys::BACKUP_XFORM).and_then(|t| crate::parse::parse_xform(t).ok().map(|x| (x, t)));
        let (bxform, btext) = match kept {
            Some((x, t)) if unchanged => (x, Some(t.to_string())),
            _ => (*xform, None),
        };
        let mut battrs = Self::attributes(e, ex(e, keys::BACKUP_ATTRS), keys::BACKUP_ATTR);
        if ex(e, keys::BACKUP_ATTRS).is_none() {
            // A text made in OpenEtch: LightBurn's backup carries only its CutIndex.
            battrs.push(("CutIndex".into(), String::new()));
        }
        let cut_index = battrs.iter().any(|(k, _)| k == "CutIndex").then_some(index);
        ShapeKind::Text { backup: Some(Backup { cut_index, xform: bxform, xform_text: btext, attributes: battrs, path }) }
    }

    /// One object, plus the kept shapes that followed it, into `out` (the top level when `None`).
    fn object(&mut self, o: &Object, parent: Affine, out: Option<&mut Vec<Shape>>, w: &mut Warnings) {
        let mut local_out = Vec::new();
        let Some(&index) = self.index_of.get(o.layer.as_str()) else { return };
        if !self.frame.contains(o.bounds()) {
            self.outside += 1;
        }
        let e = &o.extras;
        let placed = if matches!(o.geometry, Geometry::Image { .. }) { o.transform * BITMAP_ROWS } else { o.transform };
        let (xform, xform_text) = self.local(e, parent, placed);
        let mut attributes = Self::attributes(e, ex(e, keys::ATTRS), "");
        let kind = match &o.geometry {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                ShapeKind::Rect { w: *width_mm, h: *height_mm, cr: *corner_radius_mm }
            }
            Geometry::Ellipse { rx_mm, ry_mm } => ShapeKind::Ellipse { rx: *rx_mm, ry: *ry_mm },
            Geometry::Text(t) => {
                if ex(e, keys::TYPE) != Some("Text") {
                    attributes.clear();
                }
                let lost = crate::text::to_attributes(t, &mut attributes);
                if !lost.is_empty() {
                    w.push(format!(
                        "text \"{}\": {} not written (LightBurn's attribute names for them are unknown); its saved \
                         outline shows them, but LightBurn renders the text again from its attributes when it has the font",
                        o.name,
                        lost.join(", ")
                    ));
                }
                self.text_shape(e, &t.outline, index, &xform, xform_text.as_deref())
            }
            Geometry::Path { path } if ex(e, keys::TYPE) == Some("Text") => self.text_shape(e, path, index, &xform, xform_text.as_deref()),
            // LightBurn has no polygon shape in .lbrn2: a parametric polygon is written as the path it draws.
            Geometry::Polygon { .. } => {
                attributes.clear();
                let data = self.path_data(&BTreeMap::new(), &o.geometry.local_path(), false);
                if data.vertices.is_empty() {
                    w.push(format!("object {} (\"{}\") is an empty polygon and was not exported", o.id, o.name));
                    return;
                }
                ShapeKind::Path(data)
            }
            Geometry::Path { path } => {
                let data = self.path_data(e, path, false);
                if data.vertices.is_empty() {
                    w.push(format!("object {} (\"{}\") is an empty path and was not exported", o.id, o.name));
                    return;
                }
                ShapeKind::Path(data)
            }
            Geometry::Image { width_mm, height_mm, mime, data_base64, adjust } => {
                if ex(e, keys::TYPE) != Some("Bitmap") {
                    if mime != "image/png" {
                        w.push(format!(
                            "image \"{}\" is {mime}; LightBurn may expect PNG bitmap data",
                            o.name
                        ));
                    }
                    attributes = [
                        ("Gamma", "1"),
                        ("Contrast", "0"),
                        ("Brightness", "0"),
                        ("EnhanceAmount", "0"),
                        ("EnhanceRadius", "0"),
                        ("EnhanceDenoise", "0"),
                        ("File", ""),
                        ("SourceHash", "0"),
                        ("Data", ""),
                    ]
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .to_vec();
                    attributes.insert(0, ("CutIndex".into(), String::new()));
                    attributes.insert(1, ("W".into(), String::new()));
                    attributes.insert(2, ("H".into(), String::new()));
                }
                for (k, v) in &mut attributes {
                    if k == "Data" {
                        *v = data_base64.to_string();
                    }
                }
                if !attributes.iter().any(|(k, _)| k == "Data") {
                    attributes.push(("Data".into(), data_base64.to_string()));
                }
                crate::image::adjust_attributes(adjust, &mut attributes);
                ShapeKind::Bitmap { w: *width_mm, h: *height_mm }
            }
        };
        let children = (0..)
            .map_while(|k| e.get(&format!("{EXTRA_PREFIX}{}{k}", keys::CHILD)))
            .map(|x| raw_element(x))
            .collect();
        local_out.push(Shape { kind, cut_index: Some(index), xform, xform_text, attributes, children });
        for k in 0.. {
            let Some(xml) = e.get(&format!("{EXTRA_PREFIX}{}{k}", keys::AFTER)) else { break };
            self.kept_into(xml, Some(&mut local_out), w);
        }
        match out {
            Some(v) => v.extend(local_out),
            None => self.shapes.extend(local_out),
        }
    }

    /// A group with its members (objects directly in it and nested groups, at their first member's place).
    fn group(&mut self, g: &Group, parent: Affine, out: Option<&mut Vec<Shape>>, w: &mut Warnings) {
        let e = &g.extras;
        let gx = ex(e, keys::XFORM).and_then(|t| crate::parse::parse_xform(t).ok()).unwrap_or(Affine::IDENTITY);
        let (gx, gtext) = if gx.determinant().abs() > 0.0 && gx.determinant().is_finite() {
            (gx, ex(e, keys::XFORM).map(str::to_string))
        } else {
            (Affine::IDENTITY, None)
        };
        let world = parent * gx;
        let mut children = Vec::new();
        let mut done: HashSet<&str> = HashSet::new();
        let d = self.d;
        for o in &d.objects {
            let Some(og) = o.group.as_deref() else { continue };
            if og == g.id {
                self.object(o, world, Some(&mut children), w);
            } else if d.group_is_within(og, &g.id) {
                // The child group of `g` that holds `o`.
                let chain = d.group_chain(og);
                if let Some(pos) = chain.iter().position(|c| c.id == g.id)
                    && pos > 0
                    && done.insert(chain[pos - 1].id.as_str())
                {
                    let child = chain[pos - 1];
                    self.group(child, world, Some(&mut children), w);
                }
            }
        }
        let attributes = Self::attributes(e, ex(e, keys::ATTRS), "");
        let cut_index = ex(e, "CutIndex").and_then(|c| c.trim().parse().ok());
        let kept_children =
            (0..).map_while(|k| e.get(&format!("{EXTRA_PREFIX}{}{k}", keys::CHILD))).map(|x| raw_element(x)).collect();
        let shape = Shape {
            kind: ShapeKind::Group(children),
            cut_index,
            xform: gx,
            xform_text: gtext,
            attributes: attributes.into_iter().filter(|(k, _)| k != "CutIndex" || cut_index.is_some()).collect(),
            children: kept_children,
        };
        match out {
            Some(v) => v.push(shape),
            None => self.shapes.push(shape),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_document_caps_the_path_elements_it_creates() {
        let rects: String = (0..5)
            .map(|_| r#"<Shape Type="Rect" CutIndex="0" W="1" H="1"><XForm>1 0 0 1 75 75</XForm></Shape>"#)
            .collect();
        let p = crate::parse(&format!("<LightBurnProject>{rects}</LightBurnProject>")).unwrap();
        assert_eq!(to_document_within(&p, Frame::Galvo { field_mm: 150.0 }, 5 * FIXED_SHAPE_ELEMENTS).unwrap().objects.len(), 5);
        let e = to_document_within(&p, Frame::Galvo { field_mm: 150.0 }, 5 * FIXED_SHAPE_ELEMENTS - 1).unwrap_err();
        assert!(matches!(&e, LbrnError::TooLarge(m) if m.contains("path elements")), "{e}");
    }

    #[test]
    fn warnings_are_counted_and_capped() {
        let mut w = Warnings::default();
        for i in 0..(MAX_WARNINGS + 50) {
            w.push(format!("w{i}"));
            w.push("same");
        }
        let v = w.into_vec();
        assert_eq!(v.len(), MAX_WARNINGS + 1);
        assert_eq!(v[1], format!("same (x{})", MAX_WARNINGS + 50));
        assert_eq!(v[MAX_WARNINGS], "... and 51 more warning(s) not listed");
    }

    #[test]
    fn only_numbered_sub_prefixes_are_sub_layer_tags() {
        assert_eq!(sub_layer_tag("sub1.speed"), Some((1, "speed")));
        assert_eq!(sub_layer_tag("sub12.type"), Some((12, "type")));
        for not_sub in ["subname", "sub.x", "sub1", "subx1.y", "speed"] {
            assert_eq!(sub_layer_tag(not_sub), None, "{not_sub}");
        }
    }

    #[test]
    fn attribute_xml_round_trips_awkward_values() {
        let attrs = vec![("a".to_string(), "x\"<&>\ny\tz".to_string()), ("b".to_string(), String::new())];
        assert_eq!(xml_attributes(&attributes_xml("s", &attrs)), Some(attrs));
    }

    #[test]
    fn image_types_are_recognised_by_their_first_bytes() {
        assert_eq!(image_mime("iVBORw0KGgoAAAANSUhEUg"), Some("image/png"));
        assert_eq!(image_mime("/9j/4AAQ"), Some("image/jpeg"));
        assert_eq!(image_mime("R0lGODlh"), Some("image/gif"));
        assert_eq!(image_mime("UklGRiQAAABXRUJQVlA4"), Some("image/webp"));
        assert_eq!(image_mime(""), None);
        assert_eq!(image_mime("PHN2Zz4="), None);
    }
}
