//! `art.import_rd`: RDWorks `.rd` files (Ruida controller jobs; LightBurn's Import of third-party projects, UX-037),
//! read by `fileio::rd_in`.

use doc::Document;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::files::{Placement, base64_bytes, file_err, place_art};
use super::util::invalid;
use crate::{Command, Result, Risk};

/// The corner at the machine's (0, 0), as the `.rd` file's coordinates count from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RdOrigin {
    /// Rear right (MeerK40t's `.rd` writer implies it; the default).
    #[default]
    TopRight,
    TopLeft,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRdInput {
    /// The .rd file in base64.
    pub data_base64: String,
    /// The machine's origin corner the file counts from (default top_right; the engine passes the active Ruida
    /// device's).
    #[serde(default)]
    pub origin: RdOrigin,
    /// The bed width and height in mm, with `keep_position`: the file's machine coordinates are then placed on the bed
    /// (the engine passes the active Ruida device's bed).
    pub bed_width_mm: Option<f64>,
    /// The bed height in mm (with `bed_width_mm`).
    pub bed_height_mm: Option<f64>,
    /// Layer id for everything when `rd_layers` is false (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own position (on the bed when its size is given) instead of centring on the origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per polyline in a group (default true); false makes one object per layer.
    pub split: Option<bool>,
    /// Put the polylines on layers made from the file's layers, with their speed, power and colour (default true);
    /// false puts everything on `layer`.
    pub rd_layers: Option<bool>,
}

/// `art.import_rd`.
pub struct ImportRd;

impl Command for ImportRd {
    type Input = ImportRdInput;
    const ID: &'static str = "art.import_rd";
    const TITLE: &'static str = "Import RDWorks .rd";
    const DESCRIPTION: &'static str = "Import an RDWorks .rd file (a Ruida controller job, base64): each layer's cut \
         polylines on a line layer with the file's speed, laser 1 maximum power and colour (rd_layers: false puts \
         everything on `layer`). Curves arrive as polylines, fills as scan lines; what is not imported is warned. \
         origin: the machine's origin corner (default top_right); bed_width_mm/bed_height_mm with keep_position keep \
         the art's place on the bed. Placement: layer, name, width_mm, keep_position, split. Returns {created, \
         group, bounds, warnings, size_mm}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportRdInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, fileio::rd_in::MAX_BYTES)?;
        let (mirror_x, mirror_y) = match i.origin {
            RdOrigin::TopRight => (true, true),
            RdOrigin::TopLeft => (false, true),
            RdOrigin::BottomLeft => (false, false),
            RdOrigin::BottomRight => (true, false),
        };
        let bed_mm = match (i.bed_width_mm, i.bed_height_mm) {
            (Some(w), Some(h)) if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 && w <= 5000.0 && h <= 5000.0 => Some([w, h]),
            (None, None) => None,
            _ => return Err(invalid(id, "bed_width_mm and bed_height_mm go together, each within 0..=5000 mm")),
        };
        let mut art = fileio::rd_in::parse(&bytes, fileio::rd_in::RdFrame { mirror_x, mirror_y, bed_mm }).map_err(file_err(id))?;
        let rd_layers = i.rd_layers.unwrap_or(true);
        if !rd_layers {
            art.layers.clear();
            for a in &mut art.paths {
                a.layer = None;
            }
        }
        let p = Placement {
            layer: i.layer.as_deref(),
            name: i.name.as_deref(),
            width_mm: i.width_mm,
            keep_position: i.keep_position,
            split: i.split.unwrap_or(true),
            file_layers: rd_layers,
        };
        place_art(id, doc, art, &p, "Imported RD")
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use ruida::rd::{RdJob, RdLayer, WriteOptions, write_rd};
    use serde_json::json;

    use crate::{Invocation, Origin, Session};

    #[test]
    fn imports_layers_with_their_settings() {
        let job = RdJob {
            layers: vec![RdLayer {
                color: [0, 255, 0],
                speed_mm_s: 80.0,
                min_power: [40.0, 0.0],
                max_power: [40.0, 0.0],
                frequency: None,
                polylines: vec![vec![[0.0, 0.0], [10.0, 0.0]], vec![[0.0, 5.0], [10.0, 5.0]]],
            }],
        };
        let data = STANDARD.encode(write_rd(&job, &WriteOptions::default()).unwrap());
        let mut s = Session::new(doc::Document::default());
        let layers_before = s.document().layers.len();
        let r = s
            .execute(Invocation { id: "art.import_rd".into(), params: json!({ "data_base64": data, "origin": "bottom_left", "keep_position": true }) }, Origin::Ui)
            .unwrap();
        assert_eq!(r.result["created"].as_array().unwrap().len(), 2, "{}", r.result);
        let doc = s.document();
        assert_eq!(doc.layers.len(), layers_before + 1);
        let l = doc.layers.last().unwrap();
        assert_eq!(l.color, "#00ff00");
        assert_eq!(l.settings.speed_mm_s, 80.0);
        assert!((l.settings.power_pct - 40.0).abs() < 0.01);
        let e = s.execute(Invocation { id: "art.import_rd".into(), params: json!({ "data_base64": "AAAA" }) }, Origin::Ui);
        assert!(e.is_err());
        let e = s.execute(
            Invocation { id: "art.import_rd".into(), params: json!({ "data_base64": data, "bed_width_mm": 100 }) },
            Origin::Ui,
        );
        assert!(e.is_err());
    }
}
