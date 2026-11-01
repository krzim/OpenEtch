//! Fill regions for display (`GET /api/fill_regions`): for every layer that fills, the closed outlines of each of
//! its objects as an SVG path in field mm. Drawn with `fill-rule: evenodd`, a path shows exactly the area the hatch
//! covers: planning hatches each object's closed subpaths with the even-odd rule, and open subpaths are never
//! filled. Display only; nothing here is used for planning.

use doc::{Document, FillMode, Geometry, LayerId, ObjectId};
use serde::{Deserialize, Serialize};

use crate::geom::{Point, flatten_within};

/// Flattening tolerance for display, coarser than planning's (a tenth of a pixel at high zoom is plenty).
pub const DISPLAY_TOLERANCE_MM: f64 = 0.05;

/// The fill regions of one layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerFill {
    pub layer_id: LayerId,
    pub name: String,
    /// Display colour (CSS).
    pub color: String,
    /// `fill` or `fill_then_line` (only layers that fill are listed).
    pub mode: FillMode,
    pub visible: bool,
    pub output: bool,
    /// One SVG path per object (`M x y L ... Z` per closed subpath, field mm, +Y up), to fill with `evenodd`.
    pub paths: Vec<String>,
    /// The object each entry of `paths` belongs to.
    pub object_ids: Vec<ObjectId>,
}

/// `GET /api/fill_regions`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FillRegions {
    /// Layers whose own settings fill, in document order. Layers with no filled object still appear (empty paths).
    pub layers: Vec<LayerFill>,
}

/// The fill regions of every layer of `doc` whose mode is `fill` or `fill_then_line`. Objects with no closed
/// subpath, and images, are left out.
pub fn fill_regions(doc: &Document) -> FillRegions {
    let layers = doc
        .layers
        .iter()
        .filter(|l| matches!(l.settings.mode, FillMode::Fill | FillMode::FillThenLine))
        .map(|l| {
            let mut paths = Vec::new();
            let mut object_ids = Vec::new();
            for o in doc.objects.iter().filter(|o| o.layer == l.id) {
                if matches!(o.geometry, Geometry::Image { .. }) {
                    continue;
                }
                let d = closed_path_data(&o.field_path());
                if !d.is_empty() {
                    paths.push(d);
                    object_ids.push(o.id.clone());
                }
            }
            LayerFill {
                layer_id: l.id.clone(),
                name: l.name.clone(),
                color: l.color.clone(),
                mode: l.settings.mode.clone(),
                visible: l.visible,
                output: l.output,
                paths,
                object_ids,
            }
        })
        .collect();
    FillRegions { layers }
}

/// SVG path data of the closed subpaths of `path`, flattened at [`DISPLAY_TOLERANCE_MM`].
fn closed_path_data(path: &kurbo::BezPath) -> String {
    let mut d = String::new();
    for sub in flatten_within(path, DISPLAY_TOLERANCE_MM).into_iter().filter(|s| s.closed) {
        for (k, p) in sub.points.iter().enumerate() {
            if !d.is_empty() {
                d.push(' ');
            }
            d.push(if k == 0 { 'M' } else { 'L' });
            push_point(&mut d, *p);
        }
        d.push_str(" Z");
    }
    d
}

/// `x y` with at most 3 decimals (1 µm) and no trailing zeros.
fn push_point(d: &mut String, p: Point) {
    let num = |v: f64| {
        let r = (v * 1000.0).round() / 1000.0;
        let r = if r == 0.0 { 0.0 } else { r };
        format!("{r}")
    };
    d.push_str(&num(p[0]));
    d.push(' ');
    d.push_str(&num(p[1]));
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Layer, Object};
    use kurbo::{Affine, BezPath};

    fn object(id: &str, layer: &str, geometry: Geometry, at: (f64, f64)) -> Object {
        Object::new(id.into(), id, layer.into(), geometry, Affine::translate(at))
    }

    #[test]
    fn only_fill_layers_and_closed_subpaths_are_listed() {
        let mut d = Document::default();
        let mut fill = Layer::new("layer-9".into(), "Engrave", "#00ff00");
        fill.settings.mode = FillMode::Fill;
        d.layers.push(fill);
        let mut open = BezPath::new();
        open.move_to((0.0, 0.0));
        open.line_to((5.0, 5.0));
        d.objects = vec![
            object("obj-a", "layer-9", Geometry::Rect { width_mm: 4.0, height_mm: 2.0, corner_radius_mm: 0.0 }, (10.0, 0.0)),
            object("obj-b", "layer-9", Geometry::Path { path: open }, (0.0, 0.0)),
            object("obj-c", "layer-1", Geometry::Rect { width_mm: 1.0, height_mm: 1.0, corner_radius_mm: 0.0 }, (0.0, 0.0)),
        ];
        let r = fill_regions(&d);
        assert_eq!(r.layers.len(), 1, "the line layer is not listed");
        let l = &r.layers[0];
        assert_eq!(l.layer_id, "layer-9");
        assert_eq!(l.object_ids, ["obj-a"], "open paths are not filled");
        assert_eq!(l.paths[0], "M8 -1 L12 -1 L12 1 L8 1 Z");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["layers"][0]["mode"], "fill");
    }

    #[test]
    fn holes_stay_separate_subpaths_for_even_odd() {
        let mut d = Document::default();
        d.layers[0].settings.mode = FillMode::FillThenLine;
        let mut ring = kurbo::Shape::to_path(&kurbo::Rect::new(-5.0, -5.0, 5.0, 5.0), 0.01);
        ring.extend(kurbo::Shape::to_path(&kurbo::Rect::new(-1.0, -1.0, 1.0, 1.0), 0.01));
        d.objects = vec![object("obj-a", "layer-1", Geometry::Path { path: ring }, (0.0, 0.0))];
        let r = fill_regions(&d);
        assert_eq!(r.layers[0].paths[0].matches('M').count(), 2);
        assert_eq!(r.layers[0].paths[0].matches('Z').count(), 2);
    }
}
