//! `font.preview`: a sample of each font drawn by the engine (LightBurn 2.1's Font Manager shows every font in its
//! own face). The browser cannot show the engine's fonts itself: single-line Hershey, SHX and JSF fonts are not web
//! fonts, and an installed font on the engine computer (the Pi) is usually not installed on the iPad that shows the
//! picker. The picker asks for the rows it shows, a page at a time.

use doc::{AlignH, AlignV, Document, FontSpec, Text};
use kurbo::{BezPath, PathEl, Point};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::text::FontInput;
use super::util::{self, invalid};
use crate::{Command, Result, Risk};

/// Most fonts one call draws.
pub const MAX_FONTS: usize = 64;
/// Longest sample, characters.
pub const MAX_SAMPLE: usize = 40;
/// The sample when none is given.
pub const DEFAULT_SAMPLE: &str = "AaBbCc 123";

/// An SVG path `d` of `path` with y flipped (SVG's y points down) and coordinates rounded to `digits` decimals.
pub(crate) fn svg_d(path: &BezPath, digits: usize) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let num = |out: &mut String, v: f64| {
        let s = format!("{v:.digits$}");
        let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { &s };
        out.push_str(if s == "-0" { "0" } else { s });
    };
    let pt = |out: &mut String, p: Point| {
        num(out, p.x);
        out.push(' ');
        num(out, -p.y);
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.push('M');
                pt(&mut out, p);
            }
            PathEl::LineTo(p) => {
                out.push('L');
                pt(&mut out, p);
            }
            PathEl::QuadTo(a, p) => {
                out.push('Q');
                pt(&mut out, a);
                out.push(' ');
                pt(&mut out, p);
            }
            PathEl::CurveTo(a, b, p) => {
                out.push('C');
                pt(&mut out, a);
                out.push(' ');
                pt(&mut out, b);
                out.push(' ');
                pt(&mut out, p);
            }
            PathEl::ClosePath => {
                let _ = write!(out, "Z");
            }
        }
    }
    out
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontPreviewInput {
    /// The fonts to draw (1-64), as `font.list` lists them.
    pub fonts: Vec<FontInput>,
    /// The sample text (one line, at most 40 characters). Default "AaBbCc 123".
    pub text: Option<String>,
}

/// `font.preview`.
pub struct FontPreview;

impl Command for FontPreview {
    type Input = FontPreviewInput;
    const ID: &'static str = "font.preview";
    const TITLE: &'static str = "Font samples";
    const DESCRIPTION: &'static str = "Draw a one-line sample in each of up to 64 fonts (`fonts: [{family, kind}]`, \
        as font.list lists them; `text` default \"AaBbCc 123\", at most 40 characters), 10 mm high, for a font \
        picker. Returns `[{family, kind, d, view_box: [x, y, w, h]}]`: `d` an SVG path (y down, mm) and the box \
        that holds it; a font that cannot be drawn has `error` instead. Changes nothing.";
    const RISK: Risk = Risk::Query;

    fn apply(_doc: &mut Document, i: FontPreviewInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        if i.fonts.is_empty() || i.fonts.len() > MAX_FONTS {
            return Err(invalid(cmd, format!("fonts must list 1 to {MAX_FONTS} fonts")));
        }
        let sample: String = i.text.as_deref().unwrap_or(DEFAULT_SAMPLE).replace(['\n', '\r', '\t'], " ");
        if sample.chars().count() > MAX_SAMPLE {
            return Err(invalid(cmd, format!("text is longer than {MAX_SAMPLE} characters")));
        }
        if sample.chars().any(char::is_control) {
            return Err(invalid(cmd, "text must not contain control characters"));
        }
        let sample = if sample.trim().is_empty() { DEFAULT_SAMPLE.to_string() } else { sample };
        let mut out = Vec::with_capacity(i.fonts.len());
        for f in &i.fonts {
            let family = util::name(cmd, "fonts.family", &f.family)?;
            let mut t = Text::new(sample.clone(), FontSpec { family: family.clone(), kind: f.kind }, 10.0);
            t.align_h = AlignH::Left;
            t.align_v = AlignV::Bottom;
            // Unwelded: faster, and the sample only shows the face.
            t.welded = false;
            match text::render_text(&t, &sample, None) {
                Ok(r) => {
                    let b = geom::bounds(&r.outline).unwrap_or_default();
                    // The box in SVG coordinates (y flipped), with a little room round the ink.
                    let pad = 0.5;
                    out.push(json!({
                        "family": family, "kind": f.kind, "d": svg_d(&r.outline, 2),
                        "view_box": [util::r4(b.x0 - pad), util::r4(-b.y1 - pad), util::r4(b.width() + 2.0 * pad), util::r4(b.height() + 2.0 * pad)],
                    }));
                }
                Err(e) => out.push(json!({ "family": family, "kind": f.kind, "error": e })),
            }
        }
        let n = out.len();
        Ok((json!(out), format!("{n} font samples")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::FontKind;

    #[test]
    fn samples_bundled_fonts_and_reports_missing_ones() {
        let mut d = Document::default();
        let fonts = vec![
            FontInput { family: "Hershey Sans".into(), kind: FontKind::Stroke },
            FontInput { family: "No Such Font 4711".into(), kind: FontKind::System },
        ];
        let (r, _) = FontPreview::apply(&mut d, FontPreviewInput { fonts, text: Some("Ab1".into()) }).unwrap();
        let a = &r[0];
        assert_eq!(a["family"], "Hershey Sans");
        assert!(a["d"].as_str().unwrap().starts_with('M'));
        let vb: Vec<f64> = a["view_box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!(vb[2] > 5.0 && vb[3] > 5.0 && vb[1] < 0.0, "ink above the baseline is at negative y: {vb:?}");
        assert!(r[1]["error"].is_string());
        assert!(FontPreview::apply(&mut d, FontPreviewInput { fonts: vec![], text: None }).is_err());
        let long = "x".repeat(MAX_SAMPLE + 1);
        let one = vec![FontInput { family: "Hershey Sans".into(), kind: FontKind::Stroke }];
        assert!(FontPreview::apply(&mut d, FontPreviewInput { fonts: one, text: Some(long) }).is_err());
    }

    #[test]
    fn svg_d_flips_and_rounds() {
        let mut p = BezPath::new();
        p.move_to((1.0, 2.0));
        p.line_to((3.333_333, -0.000_1));
        p.close_path();
        assert_eq!(svg_d(&p, 2), "M1 -2L3.33 0Z");
    }
}
