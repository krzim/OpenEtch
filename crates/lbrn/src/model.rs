//! The typed `.lbrn2` model. It holds what OpenEtch reads and everything else it must write back unchanged:
//! unknown cut-setting children, unknown shape attributes and children, and unknown root elements. Only the
//! thumbnail and the `VertID`/`PrimID` sharing are not kept (shared lists are resolved into every path that uses
//! them, and the writer emits fresh ids).

use std::collections::BTreeMap;

use kurbo::{Affine, Point};

#[derive(Debug, Clone, PartialEq)]
pub struct LbrnProject {
    /// Root attributes in file order, `AppVersion` and `FormatVersion` included.
    pub attributes: Vec<(String, String)>,
    /// `CutSetting` and `CutSetting_Img` elements in file order.
    pub cut_settings: Vec<CutSetting>,
    /// Top-level shapes in file order (= LightBurn's drawing order).
    pub shapes: Vec<Shape>,
    /// Other root elements (`VariableText`, `UIPrefs`, `Notes`, ...), kept verbatim. `Thumbnail` is dropped.
    pub extra_elements: Vec<Element>,
}

impl LbrnProject {
    pub fn attribute(&self, key: &str) -> Option<&str> {
        self.attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The vector (`CutSetting`, not `CutSetting_Img`) settings for a layer index.
    pub fn cut_setting(&self, index: u32) -> Option<&CutSetting> {
        self.cut_settings.iter().find(|c| c.index == index && !c.image)
    }
}

/// A generic XML element, for parts of the file OpenEtch does not model.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    /// Direct text content, trimmed; `None` when empty.
    pub text: Option<String>,
    pub children: Vec<Element>,
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
/// typed child whose value does not parse, is in `other` verbatim.
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
}

/// Tags in the order LightBurn writes them (merged from the 1.3–2.1 samples). The writer sorts by this; unknown tags
/// follow in name order.
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

/// Shortest text that parses back to exactly `v`. Very small and very large magnitudes use exponent form, as
/// LightBurn itself writes in `XForm` (e.g. `1.98952e-13`).
pub(crate) fn num(v: f64) -> String {
    if v == 0.0 {
        "0".into()
    } else if v.abs() < 1e-6 || v.abs() >= 1e15 {
        format!("{v:e}")
    } else {
        format!("{v}")
    }
}

fn flag(b: bool) -> String {
    if b { "1".into() } else { "0".into() }
}

impl CutParams {
    /// Build from `tag → Value` pairs. A typed tag whose value does not parse is kept in `other`.
    pub fn from_values<'a>(values: impl IntoIterator<Item = (&'a str, &'a str)>) -> CutParams {
        let mut p = CutParams::default();
        for (tag, value) in values {
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

    /// `tag → Value` pairs in LightBurn's order (see [`TAG_ORDER`]).
    pub fn to_values(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::new();
        let mut push = |tag: &str, value: Option<String>| {
            if let Some(value) = value {
                v.push((tag.to_string(), value));
            }
        };
        push("name", self.name.clone());
        push("subname", self.subname.clone());
        push("minPower", self.min_power.map(num));
        push("maxPower", self.max_power.map(num));
        push("speed", self.speed.map(num));
        push("frequency", self.frequency.map(num));
        push("QPulseWidth", self.q_pulse_width.map(num));
        push("interval", self.interval.map(num));
        push("angle", self.angle.map(num));
        push("anglePerPass", self.angle_per_pass.map(num));
        push("crossHatch", self.cross_hatch.map(flag));
        push("bidir", self.bidir.map(flag));
        push("numPasses", self.num_passes.map(|n| n.to_string()));
        push("priority", self.priority.map(|n| n.to_string()));
        push("doOutput", self.do_output.map(flag));
        push("hide", self.hide.map(flag));
        for (tag, value) in &self.other {
            // A typed field wins over a stray duplicate in `other`.
            if !v.iter().any(|(t, _)| t == tag) {
                v.push((tag.clone(), value.clone()));
            }
        }
        let rank = |t: &str| TAG_ORDER.iter().position(|o| *o == t).unwrap_or(TAG_ORDER.len());
        v.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then_with(|| a.0.cmp(&b.0)));
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
    /// Attributes not modelled by `kind` (e.g. Text's `Str`/`Font`, Bitmap's `Data`), in file order.
    pub attributes: Vec<(String, String)>,
    /// Child elements not modelled by `kind`, kept verbatim.
    pub children: Vec<Element>,
}

impl Shape {
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
    pub path: PathData,
}

/// A path's vertices and the primitives that join them.
#[derive(Debug, Clone, PartialEq)]
pub struct PathData {
    pub vertices: Vec<Vertex>,
    pub prims: Prims,
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
        for v in [0.0, -0.0, 1.0, 0.03, 100000.0, -135.0, 4.7608781, 1.98952e-13, -3.12639e-13, 1e20, 0.1 + 0.2] {
            let s = num(v);
            assert_eq!(s.parse::<f64>().unwrap(), v, "{s}");
        }
        assert_eq!(num(100000.0), "100000");
        assert_eq!(num(-0.0), "0");
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
        assert_eq!(tags, ["maxPower", "speed", "frequency", "bidir", "crossHatch", "tabCount"]);
        let back = CutParams::from_values(p.to_values().iter().map(|(t, v)| (t.as_str(), v.as_str())));
        assert_eq!(back, p);
    }
}
