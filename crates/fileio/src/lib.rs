//! File formats and image tools beyond the native and LightBurn projects (ARCHITECTURE.md "Files and image tools").
//! Pure and wasm-ok, except PDF reading ([`pdf_in`], native only: its parser needs a random source).
//!
//! - Exports of a [`doc::Document`] (or a selection of it): [`svg_out`] (SVG, also print-ready), [`dxf_out`] (DXF
//!   R12), [`pdf_out`] (PDF, also print-ready and the PDF-compatible Adobe Illustrator `.ai`).
//! - Imports into [`Art`] (paths in mm, +Y up): [`pdf_in`] (PDF and PDF-based `.ai`), [`postscript`] (EPS and
//!   PostScript-based `.ai`), [`hpgl`] (HPGL/PLT), [`gcode_in`] (G-code toolpaths), [`gerber`] (Gerber RS-274X and
//!   Excellon drill), [`mesh`] (3D models to a depth map image), [`bitmap`] (TIFF/TGA to PNG), [`rd_in`] (RDWorks
//!   `.rd` Ruida jobs).
//! - Image tools: [`trace`] (Trace Image, outline and centre line), [`raster`] (Convert to Bitmap), [`bitmap`] (masks).
//! - [`zip`]: a small zip reader and writer (3MF, user bundles, LightBurn `.lbzip` bundles).
//! - [`lbart`]: LightBurn Art Library files (`.lbart`), the binary container around `<LightBurnShapes>` XML.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use doc::{Document, Geometry, Object, ObjectId};
use kurbo::BezPath;

pub mod bitmap;
pub mod colorsep;
/// The image crate as fileio uses it (with TIFF and TGA), for callers handling decoded images.
pub use image;
pub mod dxf_out;
pub mod ezd;
pub mod gcode_in;
pub mod gerber;
pub mod hpgl;
pub mod lbart;
pub mod lbrn_legacy;
pub mod mesh;
pub mod pdf_out;
#[cfg(not(target_arch = "wasm32"))]
pub mod pdf_in;
pub mod postscript;
pub mod rd_in;
pub mod raster;
pub mod svg_out;
pub mod trace;
pub mod zip;

/// Why a file could not be read or written. Messages are short sentences for the user.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FileError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    TooLarge(String),
}

pub type Result<T, E = FileError> = std::result::Result<T, E>;

pub(crate) fn invalid<T>(m: impl Into<String>) -> Result<T> {
    Err(FileError::Invalid(m.into()))
}

/// One imported path: millimetres, +Y up, in the file's own position (the importer's caller centres it).
#[derive(Debug, Clone, PartialEq)]
pub struct ArtPath {
    pub path: BezPath,
    /// What it came from, for object names (e.g. "path", "pen 2", "D10 flash").
    pub label: String,
    /// The stroke or fill colour the file gave it, if any.
    pub color: Option<[u8; 3]>,
    /// The layer the file gives it (an index into [`Art::layers`]), for formats that carry settings (EzCad pens
    /// and hatches); `None`: the import's target layer.
    pub layer: Option<usize>,
}

/// Laser settings an imported file carries (an EzCad pen, or a hatch with its pens), to become a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtLayer {
    pub name: String,
    pub color: [u8; 3],
    pub settings: doc::CutSettings,
    /// Further passes with their own settings (EzCad's second and third hatch).
    pub sub_layers: Vec<doc::CutSettings>,
}

/// A text the file holds without its outline (EzCad keeps glyph outlines only for some texts): the importer's
/// caller sets it in the font if it can, else marks its place.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtText {
    pub text: String,
    /// The font family the file names.
    pub font: String,
    pub height_mm: f64,
    /// Where the text starts (its left end at the baseline), art mm.
    pub at: kurbo::Point,
    /// Rotation, degrees counter-clockwise.
    pub angle_deg: f64,
    pub layer: Option<usize>,
}

/// One imported raster image (embedded in a PDF or an EzCad file).
#[derive(Debug, Clone, PartialEq)]
pub struct ArtImage {
    /// The image file (`mime`).
    pub data: Vec<u8>,
    /// `image/png` or `image/jpeg`.
    pub mime: &'static str,
    /// Maps the image's unit square (`(0, 0)` its lower-left corner as shown, `(1, 1)` its upper-right; the first
    /// pixel row is at the top) to art millimetres, +Y up.
    pub placement: kurbo::Affine,
    /// What it came from, for object names.
    pub label: String,
}

impl ArtImage {
    /// The image's corners in art mm.
    pub fn bounds(&self) -> kurbo::Rect {
        self.placement.transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, 1.0, 1.0))
    }
}

/// Imported vector art (and images) and what the importer left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Art {
    pub paths: Vec<ArtPath>,
    pub images: Vec<ArtImage>,
    /// Layers the paths name ([`ArtPath::layer`]).
    pub layers: Vec<ArtLayer>,
    /// Texts without outlines.
    pub texts: Vec<ArtText>,
    pub warnings: Vec<String>,
}

impl Art {
    pub(crate) fn push(&mut self, path: BezPath, label: impl Into<String>, color: Option<[u8; 3]>) {
        if path.elements().iter().any(|e| !matches!(e, kurbo::PathEl::MoveTo(_) | kurbo::PathEl::ClosePath)) {
            self.paths.push(ArtPath { path, label: label.into(), color, layer: None });
        }
    }

    pub(crate) fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }
}

/// The objects an export writes: `ids` (objects or groups, expanded; unknown ids are an error) in document order,
/// or every object when `ids` is `None` or empty (LightBurn: "exports the selected graphics; if nothing is
/// selected, the whole project").
pub fn export_objects<'a>(doc: &'a Document, ids: Option<&[ObjectId]>) -> Result<Vec<&'a Object>> {
    let Some(ids) = ids.filter(|i| !i.is_empty()) else {
        return Ok(doc.objects.iter().collect());
    };
    if let Some(bad) = ids.iter().find(|id| doc.object(id).is_none() && doc.group(id).is_none()) {
        return invalid(format!("no object or group {bad}"));
    }
    let wanted: BTreeSet<ObjectId> = doc.expand_group_ids(ids).into_iter().collect();
    Ok(doc.objects.iter().filter(|o| wanted.contains(&o.id)).collect())
}

/// `#rrggbb` (or `#rgb`) → RGB; anything else is black.
pub fn parse_color(s: &str) -> [u8; 3] {
    let h = s.trim().trim_start_matches('#');
    let hex = |s: &str| u8::from_str_radix(s, 16).ok();
    match h.len() {
        6 => [hex(&h[0..2]), hex(&h[2..4]), hex(&h[4..6])].map(|v| v.unwrap_or(0)),
        3 => [&h[0..1], &h[1..2], &h[2..3]].map(|c| hex(c).map_or(0, |v| v * 17)),
        _ => [0, 0, 0],
    }
}

/// An object's outline in the field frame as exports write it: a sharp rectangle as its four lines (the generic
/// rounded-rectangle outline adds zero-length corner curves), everything else as [`Object::field_path`].
pub fn clean_field_path(o: &Object) -> BezPath {
    match o.geometry {
        Geometry::Rect { width_mm, height_mm, corner_radius_mm: 0.0 } => {
            o.transform * kurbo::Shape::to_path(&kurbo::Rect::from_center_size((0.0, 0.0), (width_mm, height_mm)), 0.01)
        }
        _ => o.field_path(),
    }
}

/// An object's vector outline in the field frame, or `None` for images.
pub fn field_outline(o: &Object) -> Option<BezPath> {
    (!matches!(o.geometry, Geometry::Image { .. })).then(|| clean_field_path(o))
}

/// Escape text for XML content and attribute values.
pub(crate) fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t') => {}
            c => out.push(c),
        }
    }
    out
}

/// A number for text formats: at most 4 decimals, no trailing zeros, no `-0`.
pub(crate) fn num(v: f64) -> String {
    let r = (v * 1e4).round() / 1e4;
    let r = if r == 0.0 { 0.0 } else { r };
    let s = format!("{r:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" { "0".into() } else { s.to_string() }
}

/// CRC-32 (IEEE 802.3), as zip and PNG use.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = t[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
pub(crate) mod testutil {
    use doc::{Document, Geometry, Layer, Object};
    use kurbo::{Affine, BezPath};

    /// Two layers (line red, fill blue), a rectangle, a circle and a triangle path.
    pub fn sample() -> Document {
        let mut d = Document { field_mm: Some(100.0), ..Document::default() };
        d.layers[0].color = "#ff0000".into();
        let mut fill = Layer::new("layer-9".into(), "Fill", "#0000ff");
        fill.settings.mode = doc::FillMode::Fill;
        d.layers.push(fill);
        d.objects.push(Object::new(
            "obj-1".into(),
            "rect",
            "layer-1".into(),
            Geometry::Rect { width_mm: 20.0, height_mm: 10.0, corner_radius_mm: 0.0 },
            Affine::translate((10.0, 5.0)),
        ));
        d.objects.push(Object::new(
            "obj-2".into(),
            "circle",
            "layer-9".into(),
            Geometry::Ellipse { rx_mm: 5.0, ry_mm: 5.0 },
            Affine::translate((-20.0, 0.0)),
        ));
        let mut tri = BezPath::new();
        tri.move_to((0.0, 0.0));
        tri.line_to((10.0, 0.0));
        tri.line_to((0.0, 10.0));
        tri.close_path();
        d.objects.push(Object::new("obj-3".into(), "tri", "layer-1".into(), Geometry::Path { path: tri }, Affine::translate((0.0, -30.0))));
        d.next_id = 10;
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_expands_groups_and_refuses_unknown_ids() {
        let mut d = testutil::sample();
        d.groups.push(doc::Group::new("group-5".into(), "g", None));
        d.objects[2].group = Some("group-5".into());
        let ids = |v: Vec<&Object>| v.iter().map(|o| o.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(export_objects(&d, None).unwrap()).len(), 3);
        assert_eq!(ids(export_objects(&d, Some(&["group-5".into()])).unwrap()), ["obj-3"]);
        assert!(export_objects(&d, Some(&["obj-77".into()])).is_err());
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_color("#ff8000"), [255, 128, 0]);
        assert_eq!(parse_color("#f80"), [255, 136, 0]);
        assert_eq!(num(1.23456), "1.2346");
        assert_eq!(num(-0.00001), "0");
        assert_eq!(num(10.0), "10");
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(xml_escape("a<&\"b\u{1}"), "a&lt;&amp;&quot;b");
    }
}
