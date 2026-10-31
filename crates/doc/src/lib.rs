//! The document model. Everything is in millimetres and degrees, in the field frame (origin at the galvo field
//! centre, +X right, +Y away from the operator). See ARCHITECTURE.md.
//!
//! This crate is plain data plus geometry helpers. It never changes itself: all edits go through `commands`.

use kurbo::{Affine, BezPath, Rect, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type ObjectId = String;
pub type LayerId = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Bumped by every successful command; clients use it to detect changes.
    pub revision: u64,
    pub name: String,
    pub stock: Option<Stock>,
    pub layers: Vec<Layer>,
    pub objects: Vec<Object>,
    /// Counter for stable ids ("obj-1", "layer-1").
    pub next_id: u64,
}

impl Default for Document {
    fn default() -> Self {
        let mut doc = Document { revision: 0, name: "Untitled".into(), stock: None, layers: vec![], objects: vec![], next_id: 1 };
        let id = doc.alloc_id("layer");
        doc.layers.push(Layer::new(id, "Layer 1", "#1f77b4"));
        doc
    }
}

impl Document {
    pub fn alloc_id(&mut self, prefix: &str) -> String {
        let id = format!("{prefix}-{}", self.next_id);
        self.next_id += 1;
        id
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
}

impl Layer {
    pub fn new(id: LayerId, name: &str, color: &str) -> Self {
        Layer { id, name: name.into(), color: color.into(), visible: true, output: true, settings: CutSettings::default(), sub_layers: vec![] }
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
        let doc = Document {
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
        let json = serde_json::to_string(&doc).unwrap();
        let back: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc, back);
    }
}
