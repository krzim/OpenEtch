//! The typed `.lbrn2` model. It holds what OpenEtch reads and everything else it must write back unchanged: unknown
//! cut-setting children, shape attributes in file order, unknown shape children, every root element (the thumbnail
//! included), the order of the root's children, the `VertID`/`PrimID` sharing of path lists, the original spelling of
//! numbers (written back while the value is unchanged) and the file's layout (XML declaration, line endings). So
//! [`crate::write`] of a parsed project reproduces LightBurn's own files byte for byte (see `tests/real_samples.rs`).

use std::collections::BTreeMap;

use kurbo::{Affine, Point};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LbrnProject {
    /// Root attributes in file order, `AppVersion` and `FormatVersion` included.
    pub attributes: Vec<(String, String)>,
    /// `CutSetting` and `CutSetting_Img` elements in file order.
    pub cut_settings: Vec<CutSetting>,
    /// Top-level shapes in file order (= LightBurn's drawing order).
    pub shapes: Vec<Shape>,
    /// Other root elements (`Thumbnail`, `VariableText`, `UIPrefs`, `Notes`, ...), kept verbatim.
    pub extra_elements: Vec<Element>,
    /// The root's children in file order. Empty, or not naming every item exactly once, means LightBurn's usual
    /// layout: the extra elements other than `Notes`, the cut settings, the shapes, then `Notes`.
    pub order: Vec<RootItem>,
    pub format: Format,
}

impl LbrnProject {
    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The root's `MirrorX` / `MirrorY` (`True`/`true`/`1`): the origin corner of the device the project was saved
    /// with (see [`crate::Frame::Bed`]). Missing flags are false.
    pub fn mirror(&self) -> (bool, bool) {
        let flag = |k: &str| self.attribute(k).is_some_and(crate::convert::truthy);
        (flag("MirrorX"), flag("MirrorY"))
    }

    /// The vector (`CutSetting`, not `CutSetting_Img`) settings for a layer index.
    pub fn cut_setting(&self, index: u32) -> Option<&CutSetting> {
        self.cut_settings.iter().find(|c| c.index == index && !c.image)
    }
}

/// One child of the root, by its index in [`LbrnProject::cut_settings`], `shapes` or `extra_elements`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RootItem {
    CutSetting(usize),
    Shape(usize),
    Element(usize),
}

/// How the file is laid out, so a file written back looks like the one read. The default is LightBurn's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    /// The XML declaration as written in the file (`None`: `<?xml version="1.0" encoding="UTF-8"?>`).
    pub declaration: Option<String>,
    /// CRLF line endings (LightBurn 1.x and some 2.2 files) instead of LF.
    pub crlf: bool,
    /// A line break after the closing root tag.
    pub final_newline: bool,
    /// Empty elements written `<a Value="1" />` (some scripts) instead of `<a Value="1"/>` (LightBurn).
    pub space_before_slash: bool,
}

impl Default for Format {
    fn default() -> Self {
        Format { declaration: None, crlf: false, final_newline: true, space_before_slash: false }
    }
}

/// A generic XML element, for parts of the file OpenEtch does not model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    /// Direct text content, trimmed; `None` when empty.
    pub text: Option<String>,
    pub children: Vec<Element>,
    /// The element exactly as it was in the file (line breaks as `\n`), from its `<` to its end, for elements read
    /// from a file. The writer writes this text instead of the fields above, so it must describe the same element.
    pub raw: Option<String>,
}

/// One layer's settings: `<CutSetting type="...">` (or `<CutSetting_Img>`) with `<tag Value="..."/>` children.
#[derive(Debug, Clone, PartialEq)]
pub struct CutSetting {
    /// The `type` attribute: `Cut`, `Scan`, `Scan+Cut`, `Image`, `Tool`, ...
    pub kind: String,
    /// `CutSetting_Img` (bitmap settings) rather than `CutSetting`.
    pub image: bool,
    /// Layer index: 0..=29 are the colour layers C00..C29, 30 and 31 the tool layers T1 and T2.
    pub index: u32,
    pub params: CutParams,
    pub sub_layers: Vec<SubLayer>,
}

/// `<SubLayer type="..." index="n">`: an extra pass with its own parameters, run after its parent.
#[derive(Debug, Clone, PartialEq)]
pub struct SubLayer {
    pub kind: String,
    pub index: u32,
    pub params: CutParams,
}

/// Cut parameters in LightBurn's own units. Typed fields are the ones OpenEtch maps; every other child, and any
/// typed child whose value does not parse, is in `other` verbatim. `source` keeps the children as read, so
/// [`CutParams::to_values`] writes them back in their order and, while a typed value is unchanged, in their
/// original spelling (`15.8750` stays `15.8750`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CutParams {
    pub name: Option<String>,
    pub subname: Option<String>,
    /// %.
    pub min_power: Option<f64>,
    /// %.
    pub max_power: Option<f64>,
    /// mm/s.
    pub speed: Option<f64>,
    /// Hz.
    pub frequency: Option<f64>,
    /// MOPA pulse width, ns.
    pub q_pulse_width: Option<f64>,
    /// Hatch line spacing, mm.
    pub interval: Option<f64>,
    /// Hatch angle, degrees.
    pub angle: Option<f64>,
    /// Added to the hatch angle on every pass, degrees.
    pub angle_per_pass: Option<f64>,
    pub cross_hatch: Option<bool>,
    pub bidir: Option<bool>,
    pub num_passes: Option<u32>,
    /// Run order among layers (lower first).
    pub priority: Option<i64>,
    pub do_output: Option<bool>,
    pub hide: Option<bool>,
    /// Every other child: tag name → `Value`.
    pub other: BTreeMap<String, String>,
    /// The `tag → Value` pairs this was built from, in their order (see [`CutParams::from_values`]).
    pub source: Vec<(String, String)>,
}

/// Tags in the order LightBurn writes them (merged from the 1.3–2.2 samples). The writer sorts tags that have no
/// source position by this; unknown tags follow in name order.
pub(crate) const TAG_ORDER: &[&str] = &[
    "index",
    "name",
    "LinkPath",
    "minPower",
    "maxPower",
    "minPower2",
    "maxPower2",
    "speed",
    "frequency",
    "PPI",
    "QPulseWidth",
    "JumpSpeed",
    "wobbleEnable",
    "perforate",
    "numPasses",
    "anglePerPass",
    "perfLen",
    "perfSkip",
    "dotTime",
    "dotSpacing",
    "bidir",
    "floodFill",
    "autoRotate",
    "crossHatch",
    "overscan",
    "overscanPercent",
    "dotWidth",
    "useDotCorrection",
    "interval",
    "angle",
    "subname",
    "priority",
    "doOutput",
    "hide",
    "tabsEnabled",
    "tabCount",
    "tabCountMax",
    "tabSpacing",
    "tabSize",
    "halftoneAngle",
    "negative",
    "ditherMode",
    "dpi",
];

/// The tags [`CutParams`] types (all but `index`, which lives on [`CutSetting`]).
pub(crate) const TYPED_TAGS: &[&str] = &[
    "name",
    "subname",
    "minPower",
    "maxPower",
    "speed",
    "frequency",
    "QPulseWidth",
    "interval",
    "angle",
    "anglePerPass",
    "crossHatch",
    "bidir",
    "numPasses",
    "priority",
    "doOutput",
    "hide",
];

fn finite(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(crate) fn boolean(s: &str) -> Option<bool> {
    match s.trim() {
        "1" | "True" | "true" => Some(true),
        "0" | "False" | "false" => Some(false),
        _ => None,
    }
}

/// Shortest text that parses back to exactly `v`. Very small and very large magnitudes use exponent form with a
/// signed exponent of at least two digits, as LightBurn writes in `XForm` (e.g. `1.98952e-13`, `4.8876205e-07`).
pub(crate) fn num(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v.abs() < 1e-4 || v.abs() >= 1e15 {
        let s = format!("{v:e}");
        match s.split_once('e') {
            Some((m, e)) => {
                let (sign, digits) = e.strip_prefix('-').map_or(("+", e), |d| ("-", d));
                format!("{m}e{sign}{digits:0>2}")
            }
            None => s,
        }
    } else {
        format!("{v}")
    }
}

/// `raw` when it reads as exactly `v`, else [`num`]`(v)`: unchanged numbers keep the file's own spelling.
pub(crate) fn num_like(raw: Option<&str>, v: f64) -> String {
    match raw {
        Some(r) if r.trim().parse::<f64>().ok() == Some(v) => r.to_string(),
        _ => num(v),
    }
}

fn flag(b: bool) -> String {
    if b { "1".into() } else { "0".into() }
}

impl CutParams {
    /// Build from `tag → Value` pairs. A typed tag whose value does not parse is kept in `other`. The pairs are also
    /// kept as [`CutParams::source`], which sets the order and spelling [`CutParams::to_values`] writes.
    pub fn from_values<'a>(values: impl IntoIterator<Item = (&'a str, &'a str)>) -> CutParams {
        let mut p = CutParams::default();
        for (tag, value) in values {
            p.source.push((tag.to_string(), value.to_string()));
            let ok = match tag {
                "name" => {
                    p.name = Some(value.to_string());
                    true
                }
                "subname" => {
                    p.subname = Some(value.to_string());
                    true
                }
                "minPower" => set(&mut p.min_power, finite(value)),
                "maxPower" => set(&mut p.max_power, finite(value)),
                "speed" => set(&mut p.speed, finite(value)),
                "frequency" => set(&mut p.frequency, finite(value)),
                "QPulseWidth" => set(&mut p.q_pulse_width, finite(value)),
                "interval" => set(&mut p.interval, finite(value)),
                "angle" => set(&mut p.angle, finite(value)),
                "anglePerPass" => set(&mut p.angle_per_pass, finite(value)),
                "crossHatch" => set(&mut p.cross_hatch, boolean(value)),
                "bidir" => set(&mut p.bidir, boolean(value)),
                "numPasses" => set(&mut p.num_passes, value.trim().parse().ok()),
                "priority" => set(&mut p.priority, value.trim().parse().ok()),
                "doOutput" => set(&mut p.do_output, boolean(value)),
                "hide" => set(&mut p.hide, boolean(value)),
                _ => false,
            };
            if !ok {
                p.other.insert(tag.to_string(), value.to_string());
            }
        }
        p
    }

    /// `tag → Value` pairs: in [`CutParams::source`] order, then the others in LightBurn's order (see
    /// [`TAG_ORDER`]). A typed value keeps its source spelling while it still reads as the same value.
    pub fn to_values(&self) -> Vec<(String, String)> {
        let source = |tag: &str| self.source.iter().find(|(t, _)| t == tag).map(|(_, v)| v.as_str());
        let number = |tag: &str, v: Option<f64>| v.map(|v| num_like(source(tag), v));
        let keep = |tag: &str, v: Option<String>, same: &dyn Fn(&str) -> bool| match source(tag) {
            Some(raw) if v.is_some() && same(raw) => Some(raw.to_string()),
            _ => v,
        };
        let flag_of = |tag: &str, b: Option<bool>| keep(tag, b.map(flag), &|raw| boolean(raw) == b);
        let int_of = |tag: &str, n: Option<i64>| {
            keep(tag, n.map(|n| n.to_string()), &|raw| raw.trim().parse::<i64>().ok() == n)
        };
        let mut v: Vec<(String, String)> = Vec::new();
        let mut push = |tag: &str, value: Option<String>| {
            if let Some(value) = value {
                v.push((tag.to_string(), value));
            }
        };
        push("name", self.name.clone());
        push("subname", self.subname.clone());
        push("minPower", number("minPower", self.min_power));
        push("maxPower", number("maxPower", self.max_power));
        push("speed", number("speed", self.speed));
        push("frequency", number("frequency", self.frequency));
        push("QPulseWidth", number("QPulseWidth", self.q_pulse_width));
        push("interval", number("interval", self.interval));
        push("angle", number("angle", self.angle));
        push("anglePerPass", number("anglePerPass", self.angle_per_pass));
        push("crossHatch", flag_of("crossHatch", self.cross_hatch));
        push("bidir", flag_of("bidir", self.bidir));
        push("numPasses", int_of("numPasses", self.num_passes.map(i64::from)));
        push("priority", int_of("priority", self.priority));
        push("doOutput", flag_of("doOutput", self.do_output));
        push("hide", flag_of("hide", self.hide));
        for (tag, value) in &self.other {
            // A typed field wins over a stray duplicate in `other`.
            if !v.iter().any(|(t, _)| t == tag) {
                v.push((tag.clone(), value.clone()));
            }
        }
        let in_source = |t: &str| self.source.iter().position(|(s, _)| s == t);
        let rank = |t: &str| TAG_ORDER.iter().position(|o| *o == t).unwrap_or(TAG_ORDER.len());
        v.sort_by(|a, b| match (in_source(&a.0), in_source(&b.0)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0)),
        });
        v
    }
}

fn set<T>(slot: &mut Option<T>, v: Option<T>) -> bool {
    match v {
        Some(v) => {
            *slot = Some(v);
            true
        }
        None => false,
    }
}

/// `<Shape Type="...">`.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    /// `CutIndex`: the layer. Groups have none.
    pub cut_index: Option<u32>,
    /// `XForm` (`a b c d e f`: x' = a·x + c·y + e, y' = b·x + d·y + f), identity when absent.
    pub xform: Affine,
    /// The `XForm` text as read; written instead of `xform` while it still reads as exactly `xform`.
    pub xform_text: Option<String>,
    /// Every attribute but `Type`, in file order (e.g. `CutIndex`, `W`, Text's `Str`/`Font`, Bitmap's `Data`). For
    /// the ones `kind` and `cut_index` model (`CutIndex W H Cr Rx Ry VertID PrimID`) the writer writes the model's
    /// value at this position, in this spelling while it still reads as that value; modelled ones missing here
    /// follow `Type`.
    pub attributes: Vec<(String, String)>,
    /// Child elements not modelled by `kind`, kept verbatim, written after the modelled ones.
    pub children: Vec<Element>,
}

impl Shape {
    /// A shape with no source attributes or children.
    pub fn new(kind: ShapeKind, cut_index: Option<u32>, xform: Affine) -> Shape {
        Shape { kind, cut_index, xform, xform_text: None, attributes: vec![], children: vec![] }
    }

    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The `Type` attribute value.
    pub fn type_name(&self) -> &str {
        match &self.kind {
            ShapeKind::Rect { .. } => "Rect",
            ShapeKind::Ellipse { .. } => "Ellipse",
            ShapeKind::Path(_) => "Path",
            ShapeKind::Group(_) => "Group",
            ShapeKind::Text { .. } => "Text",
            ShapeKind::Bitmap { .. } => "Bitmap",
            ShapeKind::Other { type_name } => type_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    /// Centred on the local origin; `cr` is the corner radius.
    Rect { w: f64, h: f64, cr: f64 },
    /// Centred on the local origin.
    Ellipse { rx: f64, ry: f64 },
    Path(PathData),
    /// `<Children>`; each child's XForm is relative to the group's.
    Group(Vec<Shape>),
    /// Live text. `Str`, `Font`, `H` (text height) etc. stay in [`Shape::attributes`]; `backup` is LightBurn's
    /// cached outline (`HasBackupPath="1"`).
    Text { backup: Option<Backup> },
    /// Embedded image; `W`/`H` in mm before the XForm, centred. `Data` (base64 PNG) stays in the attributes.
    Bitmap { w: f64, h: f64 },
    /// Any other type (Polygon, Star, Barcode, ...), kept for writing back but never converted.
    Other { type_name: String },
}

/// `<BackupPath Type="Path">` inside a Text shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub cut_index: Option<u32>,
    pub xform: Affine,
    /// As [`Shape::xform_text`].
    pub xform_text: Option<String>,
    /// Every attribute but `Type`, in file order, as [`Shape::attributes`].
    pub attributes: Vec<(String, String)>,
    pub path: PathData,
}

/// A path's vertices and the primitives that join them.
#[derive(Debug, Clone, PartialEq)]
pub struct PathData {
    pub vertices: Vec<Vertex>,
    pub prims: Prims,
    /// How the file wrote this path. `None` (a new path): the writer gives it fresh ids and writes both lists.
    pub encoded: Option<Encoded>,
}

/// A path's `VertID`/`PrimID` and the lists it wrote itself. A list given only by id is defined by an earlier path
/// with that id (`None` here). Must describe `vertices` and `prims` exactly.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Encoded {
    pub vert_id: Option<String>,
    pub prim_id: Option<String>,
    /// The `VertList` text, when this path carries it.
    pub vert_list: Option<String>,
    /// The `PrimList` text, when this path carries it.
    pub prim_list: Option<String>,
}

/// `V x y` with optional control points. `c0` is the control leaving the vertex (used by a `B` primitive that
/// starts here), `c1` the control arriving at it (used by a `B` that ends here). `None` is written `c0x1`/`c1x1`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub p: Point,
    pub c0: Option<Point>,
    pub c1: Option<Point>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Prims {
    List(Vec<Prim>),
    /// `LineClosed`: every vertex joined by lines, then closed.
    LineClosed,
    /// `LineOpen`: every vertex joined by lines.
    LineOpen,
}

/// `L from to` (line) or `B from to` (cubic using `from.c0` and `to.c1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prim {
    pub bezier: bool,
    pub from: usize,
    pub to: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip_exactly() {
        for v in [0.0, -0.0, 1.0, 0.03, 100000.0, -135.0, 4.7608781, 1.98952e-13, -3.12639e-13, 1e20, 0.1 + 0.2, 5e-5] {
            let s = num(v);
            assert_eq!(s.parse::<f64>().unwrap(), v, "{s}");
        }
        assert_eq!(num(100000.0), "100000");
        assert_eq!(num(-0.0), "0");
        // LightBurn's (C printf) exponent style.
        assert_eq!(num(4.8876205e-07), "4.8876205e-07");
        assert_eq!(num(-3.12639e-13), "-3.12639e-13");
        assert_eq!(num(1e20), "1e+20");
        assert_eq!(num(0.0001), "0.0001");
        assert_eq!(num_like(Some("15.8750"), 15.875), "15.8750");
        assert_eq!(num_like(Some("15.8750"), 15.9), "15.9");
        assert_eq!(num_like(None, 2.5), "2.5");
    }

    #[test]
    fn params_keep_unknown_and_unparsable_values() {
        let p = CutParams::from_values([
            ("maxPower", "30"),
            ("frequency", "100000"),
            ("bidir", "0"),
            ("crossHatch", "True"),
            ("speed", "fast"),
            ("tabCount", "1"),
        ]);
        assert_eq!(p.max_power, Some(30.0));
        assert_eq!(p.bidir, Some(false));
        assert_eq!(p.cross_hatch, Some(true));
        assert_eq!(p.speed, None);
        assert_eq!(p.other.get("speed").map(String::as_str), Some("fast"));
        assert_eq!(p.other.get("tabCount").map(String::as_str), Some("1"));
        let tags: Vec<String> = p.to_values().into_iter().map(|(t, _)| t).collect();
        assert_eq!(tags, ["maxPower", "frequency", "bidir", "crossHatch", "speed", "tabCount"], "source order");
        let back = CutParams::from_values(p.to_values().iter().map(|(t, v)| (t.as_str(), v.as_str())));
        assert_eq!(back, p);
        // Without a source the order is LightBurn's.
        let fresh = CutParams { source: vec![], ..p.clone() };
        let tags: Vec<String> = fresh.to_values().into_iter().map(|(t, _)| t).collect();
        assert_eq!(tags, ["maxPower", "speed", "frequency", "bidir", "crossHatch", "tabCount"]);
    }

    #[test]
    fn params_keep_their_source_spelling_while_unchanged() {
        let mut p = CutParams::from_values([("maxPower", "15.8750"), ("bidir", "False"), ("numPasses", "04")]);
        let text = |p: &CutParams| p.to_values().into_iter().map(|(t, v)| format!("{t}={v}")).collect::<Vec<_>>();
        assert_eq!(text(&p), ["maxPower=15.8750", "bidir=False", "numPasses=04"]);
        p.max_power = Some(20.0);
        p.bidir = Some(true);
        p.num_passes = Some(5);
        assert_eq!(text(&p), ["maxPower=20", "bidir=1", "numPasses=5"]);
    }
}
