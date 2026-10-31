//! The document model. Everything is in millimetres and degrees, in the field frame (origin at the galvo field
//! centre, +X right, +Y away from the operator). See ARCHITECTURE.md.
//!
//! This crate is plain data plus geometry helpers. It never changes itself: all edits go through `commands`.

use std::collections::BTreeMap;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod file;

pub type ObjectId = String;
pub type LayerId = String;

/// Numbered ids ("obj-12") stay below 2^53, so every JSON reader (including ones that hold numbers as doubles) sees
/// them exactly, and the counter can never wrap.
pub const MAX_ID: u64 = 1 << 53;

/// [`Document::alloc_id`] ran out of ids (only reachable from a crafted file with a huge `next_id`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("the document has used up its object ids (next id {0}); save it and start a new document")]
pub struct IdsExhausted(pub u64);

/// `^[A-Za-z_][A-Za-z0-9_.-]*$`: a name that is also a valid XML element name, so exporters can write it as a tag.
pub fn is_plain_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Whether `key` can be a [`Layer::extras`] key: `<format>.<name>` (e.g. `lbrn2.minPower`, `lbrn2.sub1.speed`), both
/// parts [plain names](is_plain_name), at most 128 bytes.
pub fn is_extras_key(key: &str) -> bool {
    key.len() <= 128 && key.split_once('.').is_some_and(|(format, name)| is_plain_name(format) && is_plain_name(name))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Bumped by every successful command; clients use it to detect changes.
    pub revision: u64,
    pub name: String,
    /// Galvo field size in mm the document was laid out for. The engine fills it in from the machine profile for
    /// new and opened documents that have none; LightBurn import/export uses it to move between frames.
    #[serde(default)]
    pub field_mm: Option<f64>,
    pub stock: Option<Stock>,
    pub layers: Vec<Layer>,
    pub objects: Vec<Object>,
    /// Counter for stable ids ("obj-1", "layer-1").
    pub next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        let mut doc = Document {
            revision: 0,
            name: "Untitled".into(),
            field_mm: None,
            stock: None,
            layers: vec![],
            objects: vec![],
            next_id: 1,
        };
        let id = doc.alloc_id("layer").expect("a new document has ids to spare");
        doc.layers.push(Layer::new(id, "Layer 1", "#1f77b4"));
        doc
    }
}

impl Document {
    /// A fresh `"{prefix}-{n}"` id. Ids stay below [`MAX_ID`]; past that this fails instead of wrapping.
    pub fn alloc_id(&mut self, prefix: &str) -> Result<String, IdsExhausted> {
        let n = self.next_id;
        let next = n.checked_add(1).filter(|v| *v <= MAX_ID).ok_or(IdsExhausted(n))?;
        self.next_id = next;
        Ok(format!("{prefix}-{n}"))
    }

    /// The first unusable number in the document: NaN or infinity anywhere (field size, stock, layer settings, object
    /// geometry, transforms, and object outlines in the field frame), or an object transform that cannot be inverted.
    /// `None` when every number is usable. JSON cannot store such values and planning cannot use them, so commands
    /// refuse to leave a document in this state.
    pub fn numeric_problem(&self) -> Option<String> {
        if let Some(f) = self.field_mm
            && !f.is_finite()
        {
            return Some(format!("field_mm is {f}"));
        }
        if let Some(s) = &self.stock {
            let dims = match s.shape {
                StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                    vec![width_mm, height_mm, corner_radius_mm]
                }
                StockShape::Circle { diameter_mm } => vec![diameter_mm],
            };
            let placement = [s.thickness_mm, s.center[0], s.center[1], s.rotation_deg];
            if dims.into_iter().chain(placement).any(|v| !v.is_finite()) {
                return Some("the stock has a number that is not finite".into());
            }
        }
        for l in &self.layers {
            for s in std::iter::once(&l.settings).chain(&l.sub_layers) {
                if let Some(field) = s.non_finite_field() {
                    return Some(format!("layer {} (\"{}\"): {field} is not a finite number", l.id, l.name));
                }
            }
        }
        self.objects
            .iter()
            .find_map(|o| o.placement_problem().map(|p| format!("object {} (\"{}\"): {p}", o.id, o.name)))
    }

    pub fn object(&self, id: &str) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn object_mut(&mut self, id: &str) -> Option<&mut Object> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn layer(&self, id: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: &str) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Union of the field-frame bounding boxes of the given objects.
    pub fn bounds_of(&self, ids: &[ObjectId]) -> Option<Rect> {
        ids.iter().filter_map(|id| self.object(id)).map(Object::bounds).reduce(|a, b| a.union(b))
    }
}

/// The physical piece being marked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stock {
    pub name: String,
    pub shape: StockShape,
    /// Material thickness. The B6's Z lift is not readable over USB, so this is always user-entered.
    pub thickness_mm: f64,
    /// Free text or a material-library key, e.g. "stainless 304".
    pub material: Option<String>,
    /// Centre of the stock in the field frame.
    pub center: [f64; 2],
    pub rotation_deg: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StockShape {
    Rect { width_mm: f64, height_mm: f64, corner_radius_mm: f64 },
    Circle { diameter_mm: f64 },
}

impl Stock {
    /// Outline in the field frame.
    pub fn outline(&self) -> BezPath {
        let local = match self.shape {
            StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                let r = Rect::from_center_size((0.0, 0.0), (width_mm, height_mm));
                r.to_rounded_rect(corner_radius_mm).to_path(0.01)
            }
            StockShape::Circle { diameter_mm } => kurbo::Circle::new((0.0, 0.0), diameter_mm / 2.0).to_path(0.01),
        };
        self.transform() * local
    }

    pub fn transform(&self) -> Affine {
        Affine::translate((self.center[0], self.center[1])) * Affine::rotate(self.rotation_deg.to_radians())
    }

    pub fn bounds(&self) -> Rect {
        self.outline().bounding_box()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    /// CSS colour used for display.
    pub color: String,
    pub visible: bool,
    /// Excluded from jobs when false.
    pub output: bool,
    pub settings: CutSettings,
    /// Extra passes with their own settings (LightBurn sub-layers), run after `settings`.
    pub sub_layers: Vec<CutSettings>,
    /// Settings from other software that OpenEtch does not model (e.g. `lbrn2.minPower`), kept so exports
    /// round-trip. Keys are prefixed with the format they came from. Never used for planning.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
}

impl Layer {
    pub fn new(id: LayerId, name: &str, color: &str) -> Self {
        Layer {
            id,
            name: name.into(),
            color: color.into(),
            visible: true,
            output: true,
            settings: CutSettings::default(),
            sub_layers: vec![],
            extras: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillMode {
    /// Trace outlines.
    Line,
    /// Hatch the inside of closed shapes.
    Fill,
    /// Hatch, then trace outlines.
    FillThenLine,
}

/// Per-layer laser parameters. Mirrors LightBurn's galvo cut settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CutSettings {
    pub mode: FillMode,
    /// 0..=100.
    pub power_pct: f64,
    pub speed_mm_s: f64,
    pub frequency_khz: f64,
    /// MOPA pulse width. `None` leaves the source at its current setting.
    pub pulse_width_ns: Option<f64>,
    pub passes: u32,
    pub hatch: Hatch,
    /// `None` uses the machine profile's defaults.
    pub timing: Option<GalvoTiming>,
}

impl CutSettings {
    /// The name of the first field holding NaN or infinity, if any.
    pub fn non_finite_field(&self) -> Option<&'static str> {
        let mut fields = vec![
            ("power_pct", self.power_pct),
            ("speed_mm_s", self.speed_mm_s),
            ("frequency_khz", self.frequency_khz),
            ("pulse_width_ns", self.pulse_width_ns.unwrap_or(0.0)),
            ("hatch.interval_mm", self.hatch.interval_mm),
            ("hatch.angle_deg", self.hatch.angle_deg),
            ("hatch.angle_increment_deg", self.hatch.angle_increment_deg),
        ];
        if let Some(t) = &self.timing {
            fields.extend([
                ("timing.laser_on_delay_us", t.laser_on_delay_us),
                ("timing.laser_off_delay_us", t.laser_off_delay_us),
                ("timing.polygon_delay_us", t.polygon_delay_us),
                ("timing.end_delay_us", t.end_delay_us),
                ("timing.jump_speed_mm_s", t.jump_speed_mm_s),
                ("timing.min_jump_delay_us", t.min_jump_delay_us),
                ("timing.max_jump_delay_us", t.max_jump_delay_us),
                ("timing.jump_distance_limit_mm", t.jump_distance_limit_mm),
            ]);
        }
        fields.into_iter().find(|(_, v)| !v.is_finite()).map(|(f, _)| f)
    }
}

impl Default for CutSettings {
    fn default() -> Self {
        CutSettings {
            mode: FillMode::Line,
            power_pct: 20.0,
            speed_mm_s: 1000.0,
            frequency_khz: 30.0,
            pulse_width_ns: None,
            passes: 1,
            hatch: Hatch::default(),
            timing: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Hatch {
    /// Line spacing.
    pub interval_mm: f64,
    pub angle_deg: f64,
    /// Added to the angle on every pass (EzCad "auto-rotate hatch angle").
    pub angle_increment_deg: f64,
    /// Second hatch at +90°.
    pub cross_hatch: bool,
    /// Alternate direction on every line instead of always marking the same way.
    pub bidirectional: bool,
}

impl Default for Hatch {
    fn default() -> Self {
        Hatch { interval_mm: 0.05, angle_deg: 0.0, angle_increment_deg: 0.0, cross_hatch: false, bidirectional: true }
    }
}

/// Galvo timing constants in microseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GalvoTiming {
    pub laser_on_delay_us: f64,
    pub laser_off_delay_us: f64,
    pub polygon_delay_us: f64,
    pub end_delay_us: f64,
    pub jump_speed_mm_s: f64,
    pub min_jump_delay_us: f64,
    pub max_jump_delay_us: f64,
    pub jump_distance_limit_mm: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    pub layer: LayerId,
    /// Geometry in object-local coordinates.
    pub geometry: Geometry,
    /// Local → field frame.
    pub transform: Affine,
}

impl Object {
    /// Geometry in the field frame.
    pub fn field_path(&self) -> BezPath {
        self.transform * self.geometry.local_path()
    }

    pub fn bounds(&self) -> Rect {
        self.field_path().bounding_box()
    }

    /// Why this object cannot be placed in the field: a non-finite number in its geometry or transform, a transform
    /// that cannot be inverted, or an outline that overflows to infinity in the field frame. `None` when it is sound.
    /// Linear in the path size and allocation-free.
    pub fn placement_problem(&self) -> Option<&'static str> {
        let t = self.transform;
        if t.as_coeffs().iter().any(|v| !v.is_finite()) {
            return Some("its transform has a number that is not finite");
        }
        let det = t.determinant();
        if !(det.is_finite() && det != 0.0) {
            return Some("its transform cannot be inverted (zero or overflowing scale)");
        }
        let ok = |p: Point| p.x.is_finite() && p.y.is_finite() && {
            let q = t * p;
            q.x.is_finite() && q.y.is_finite()
        };
        // The outline lies within the hull of these points, so they bound it.
        let corners = |x: f64, y: f64| [(-x, -y), (x, -y), (x, y), (-x, y)].map(Point::from);
        let sound = match &self.geometry {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                corner_radius_mm.is_finite() && corners(width_mm / 2.0, height_mm / 2.0).into_iter().all(ok)
            }
            Geometry::Ellipse { rx_mm, ry_mm } => corners(*rx_mm, *ry_mm).into_iter().all(ok),
            Geometry::Path { path } => path.elements().iter().all(|el| match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => ok(p),
                PathEl::QuadTo(a, p) => ok(a) && ok(p),
                PathEl::CurveTo(a, b, p) => ok(a) && ok(b) && ok(p),
                PathEl::ClosePath => true,
            }),
        };
        (!sound).then_some("its size or outline is not finite in the field frame")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    /// Centred on the local origin.
    Rect { width_mm: f64, height_mm: f64, corner_radius_mm: f64 },
    /// Centred on the local origin.
    Ellipse { rx_mm: f64, ry_mm: f64 },
    /// Arbitrary path (imported vector art, text outlines).
    Path { path: BezPath },
}

impl Geometry {
    pub fn local_path(&self) -> BezPath {
        match *self {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)).to_rounded_rect(corner_radius_mm).to_path(0.01)
            }
            Geometry::Ellipse { rx_mm, ry_mm } => kurbo::Ellipse::new((0.0, 0.0), (rx_mm, ry_mm), 0.0).to_path(0.01),
            Geometry::Path { ref path } => path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_has_one_layer() {
        let doc = Document::default();
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].id, "layer-1");
        assert_eq!(doc.next_id, 2);
    }

    #[test]
    fn object_bounds_follow_transform() {
        let obj = Object {
            id: "obj-1".into(),
            name: "r".into(),
            layer: "layer-1".into(),
            geometry: Geometry::Rect { width_mm: 10.0, height_mm: 4.0, corner_radius_mm: 0.0 },
            transform: Affine::translate((5.0, -2.0)),
        };
        let b = obj.bounds();
        assert!((b.x0 - 0.0).abs() < 1e-9 && (b.x1 - 10.0).abs() < 1e-9);
        assert!((b.y0 + 4.0).abs() < 1e-9 && (b.y1 - 0.0).abs() < 1e-9);
    }

    #[test]
    fn document_round_trips_through_json() {
        let mut doc = Document {
            field_mm: Some(153.75),
            stock: Some(Stock {
                name: "tag".into(),
                shape: StockShape::Circle { diameter_mm: 30.0 },
                thickness_mm: 1.0,
                material: Some("stainless 304".into()),
                center: [0.0, 0.0],
                rotation_deg: 0.0,
            }),
            ..Document::default()
        };
        doc.layers[0].extras.insert("lbrn2.minPower".into(), "12".into());
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn alloc_id_stops_below_max_id_instead_of_overflowing() {
        let mut doc = Document { next_id: MAX_ID - 1, ..Document::default() };
        assert_eq!(doc.alloc_id("obj").unwrap(), format!("obj-{}", MAX_ID - 1));
        assert_eq!(doc.alloc_id("obj"), Err(IdsExhausted(MAX_ID)));
        let mut doc = Document { next_id: u64::MAX, ..Document::default() };
        assert_eq!(doc.alloc_id("obj"), Err(IdsExhausted(u64::MAX)), "no add-with-overflow panic");
        assert_eq!(doc.next_id, u64::MAX, "unchanged on error");
    }

    #[test]
    fn numeric_problem_finds_non_finite_and_singular_geometry() {
        let mut doc = Document::default();
        assert_eq!(doc.numeric_problem(), None);
        let obj = |geometry, transform| Object {
            id: "obj-9".into(),
            name: "o".into(),
            layer: "layer-1".into(),
            geometry,
            transform,
        };
        let rect = Geometry::Rect { width_mm: 2.0, height_mm: 2.0, corner_radius_mm: 0.0 };
        for (g, t) in [
            (rect.clone(), Affine::scale(1e200) * Affine::scale(1e200)),
            (rect.clone(), Affine::scale_non_uniform(1.0, 0.0)),
            (rect.clone(), Affine::new([1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0])),
            (Geometry::Rect { width_mm: f64::INFINITY, height_mm: 1.0, corner_radius_mm: 0.0 }, Affine::IDENTITY),
            (Geometry::Ellipse { rx_mm: 1e300, ry_mm: 1.0 }, Affine::scale(1e10)),
        ] {
            doc.objects = vec![obj(g, t)];
            assert!(doc.numeric_problem().is_some_and(|p| p.contains("obj-9")), "{:?}", doc.objects[0]);
        }
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1e308, 0.0));
        doc.objects = vec![obj(Geometry::Path { path }, Affine::scale(10.0))];
        assert!(doc.numeric_problem().is_some());
        doc.objects.clear();
        doc.layers[0].settings.hatch.angle_deg = f64::NAN;
        assert!(doc.numeric_problem().is_some_and(|p| p.contains("hatch.angle_deg")));
    }

    #[test]
    fn extras_keys_are_plain_names() {
        for good in ["lbrn2.minPower", "lbrn2.sub1.speed", "lbrn2.frequency_absent", "x._a-b"] {
            assert!(is_extras_key(good), "{good}");
        }
        for bad in ["lbrn2.", "lbrn2", "lbrn2.evil a=\"1\"/><x", "lbrn2.1x", ".a", "lbrn2.größe", "lbrn2.a\u{1}"] {
            assert!(!is_extras_key(bad), "{bad}");
        }
    }

    #[test]
    fn older_json_without_new_fields_still_loads() {
        let mut v = serde_json::to_value(Document::default()).unwrap();
        v.as_object_mut().unwrap().remove("field_mm");
        assert!(v["layers"][0].get("extras").is_none(), "empty extras are not serialized");
        let back: Document = serde_json::from_value(v).unwrap();
        assert_eq!(back.field_mm, None);
        assert!(back.layers[0].extras.is_empty());
    }
}
