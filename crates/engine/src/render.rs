//! Preview rendering: the field square, stock, objects and toolpath as SVG (`GET /api/preview.svg`), and that SVG
//! rasterised to PNG with resvg (MCP `render_preview`).
//!
//! The SVG is drawn in the field frame (mm, +Y up) inside a `scale(1 -1)` group, so it looks like the UI's view
//! from above with the operator at the bottom. It contains no text, so resvg needs no fonts.

use std::fmt::Write as _;

use doc::Document;
use plan::{Preview, SegmentKind};

use crate::EngineError;

/// Largest PNG side `svg_to_png` produces.
pub const MAX_PNG_PX: u32 = 2048;

/// What to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOptions {
    /// Draw travel (laser-off) moves as dashed lines.
    pub show_jumps: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { show_jumps: true }
    }
}

/// Escape text for an XML attribute value.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// A layer colour if it looks like a plain CSS colour (`#hex` or a name), else a neutral grey. Keeps arbitrary
/// document strings out of the SVG markup.
fn safe_color(c: &str) -> String {
    let ok = (c.starts_with('#') && c.len() <= 9 && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
        || (!c.is_empty() && c.len() <= 20 && c.chars().all(|ch| ch.is_ascii_alphabetic()));
    if ok { esc(c) } else { "#888888".into() }
}

/// Render `doc` (and `preview`'s toolpath, when given) on a `field_mm` square.
pub fn preview_svg(doc: &Document, field_mm: f64, preview: Option<&Preview>, opts: RenderOptions) -> String {
    let h = field_mm / 2.0;
    let pad = field_mm * 0.02;
    let w = field_mm / 700.0; // base stroke width
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}mm" height="{}mm">"#,
        -h - pad,
        -h - pad,
        field_mm + 2.0 * pad,
        field_mm + 2.0 * pad,
        field_mm + 2.0 * pad,
        field_mm + 2.0 * pad
    );
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#ffffff"/>"##,
        -h - pad,
        -h - pad,
        field_mm + 2.0 * pad,
        field_mm + 2.0 * pad
    );
    s.push_str(r#"<g transform="scale(1 -1)">"#);
    // Field square and centre cross.
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{field_mm}" height="{field_mm}" fill="#f4f6f8" stroke="#9aa5b1" stroke-width="{}"/>"##,
        -h,
        -h,
        w * 2.0
    );
    let c = field_mm * 0.02;
    let _ = write!(
        s,
        r##"<path d="M {} 0 L {c} 0 M 0 {} L 0 {c}" stroke="#9aa5b1" stroke-width="{w}" fill="none"/>"##,
        -c, -c
    );
    // Operator side marker: a short bar under the field's -Y edge.
    let _ = write!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#9aa5b1"/>"##,
        -h * 0.2,
        -h - pad * 0.8,
        h * 0.4,
        pad * 0.4
    );
    if let Some(stock) = &doc.stock {
        let _ = write!(
            s,
            r##"<path d="{}" fill="#e6dcc8" fill-opacity="0.6" stroke="#8a7350" stroke-width="{}"/>"##,
            stock.outline().to_svg(),
            w * 2.0
        );
    }
    for o in &doc.objects {
        let layer = doc.layer(&o.layer);
        if layer.is_some_and(|l| !l.visible) {
            continue;
        }
        let color = layer.map(|l| safe_color(&l.color)).unwrap_or_else(|| "#888888".into());
        let _ = write!(
            s,
            r#"<path d="{}" fill="none" stroke="{color}" stroke-width="{}" stroke-opacity="0.55"/>"#,
            o.field_path().to_svg(),
            w * 2.0
        );
    }
    if let Some(p) = preview {
        let color_of = |layer: &Option<String>| {
            layer
                .as_deref()
                .and_then(|id| doc.layer(id))
                .map(|l| safe_color(&l.color))
                .unwrap_or_else(|| "#d62728".into())
        };
        if opts.show_jumps {
            let mut d = String::new();
            for seg in p.segments.iter().filter(|g| g.kind == SegmentKind::Jump) {
                let _ = write!(d, "M{} {}L{} {}", seg.from[0], seg.from[1], seg.to[0], seg.to[1]);
            }
            if !d.is_empty() {
                let _ = write!(
                    s,
                    r##"<path d="{d}" fill="none" stroke="#7f8c99" stroke-width="{w}" stroke-dasharray="{} {}"/>"##,
                    w * 4.0,
                    w * 4.0
                );
            }
        }
        // Marks grouped by colour so the SVG stays small.
        let mut groups: Vec<(String, String)> = Vec::new();
        for seg in p.segments.iter().filter(|g| g.kind == SegmentKind::Mark) {
            let color = color_of(&seg.layer);
            let idx = match groups.iter().position(|(c, _)| *c == color) {
                Some(i) => i,
                None => {
                    groups.push((color, String::new()));
                    groups.len() - 1
                }
            };
            let _ = write!(groups[idx].1, "M{} {}L{} {}", seg.from[0], seg.from[1], seg.to[0], seg.to[1]);
        }
        for (color, d) in groups {
            let _ = write!(
                s,
                r#"<path d="{d}" fill="none" stroke="{color}" stroke-width="{}" stroke-linecap="round"/>"#,
                w * 1.5
            );
        }
    }
    s.push_str("</g></svg>");
    s
}

/// Rasterise an SVG to a square-ish PNG `width_px` wide (clamped to 16..=[`MAX_PNG_PX`]).
pub fn svg_to_png(svg: &str, width_px: u32) -> Result<Vec<u8>, EngineError> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default())
        .map_err(|e| EngineError::Internal(format!("preview SVG did not parse: {e}")))?;
    let size = tree.size();
    let width = width_px.clamp(16, MAX_PNG_PX);
    let scale = width as f32 / size.width();
    let height = ((size.height() * scale).ceil() as u32).clamp(1, MAX_PNG_PX);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| EngineError::Internal("could not allocate the preview image".into()))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| EngineError::Internal(format!("PNG encoding failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Geometry, Object};
    use kurbo::Affine;
    use plan::{PreviewSegment, Stats};

    fn doc() -> Document {
        let mut d = Document::default();
        d.layers[0].color = "\"/><script>".into();
        d.objects.push(Object {
            id: "obj-2".into(),
            name: "r".into(),
            layer: "layer-1".into(),
            geometry: Geometry::Rect { width_mm: 10.0, height_mm: 10.0, corner_radius_mm: 0.0 },
            transform: Affine::IDENTITY,
            group: None,
            extras: Default::default(),
        });
        d
    }

    #[test]
    fn svg_contains_objects_and_toolpath_and_no_injected_markup() {
        let preview = Preview {
            segments: vec![PreviewSegment {
                kind: SegmentKind::Mark,
                from: [0.0, 0.0],
                to: [5.0, 0.0],
                layer: Some("layer-1".into()),
                pass: Some(0),
                emits: true,
            }],
            stats: Stats::default(),
            ..Preview::default()
        };
        let svg = preview_svg(&doc(), 150.0, Some(&preview), RenderOptions::default());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("M0 0L5 0"));
        assert!(!svg.contains("<script"));
    }

    #[test]
    fn png_renders() {
        let svg = preview_svg(&doc(), 150.0, None, RenderOptions::default());
        let png = svg_to_png(&svg, 256).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}
