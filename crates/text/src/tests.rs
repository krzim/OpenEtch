//! Layout tests on the bundled single-line fonts (always available) and, when one is installed, a system font.

use doc::{AlignH, AlignV, FontKind, FontSpec, Text, TextPath};
use kurbo::{BezPath, Rect, Shape};

use super::*;

fn hershey(text: &str, h: f64) -> Text {
    let mut t = Text::new(text, FontSpec { family: "Hershey Sans".into(), kind: FontKind::Stroke }, h);
    t.welded = false;
    t
}

fn bounds(p: &BezPath) -> Rect {
    geom::bounds(p).expect("non-empty outline")
}

fn render(t: &Text) -> Rendered {
    render_text(t, &t.text, None).unwrap()
}

#[test]
fn height_scales_the_em() {
    // Hershey Simplex capitals are 21 of 32 units: a 32 mm text has 21 mm capitals.
    let mut t = hershey("H", 32.0);
    t.align_v = AlignV::Bottom;
    t.align_h = AlignH::Left;
    let b = bounds(&render(&t).outline);
    assert!((b.y0 - 0.0).abs() < 1e-9 && (b.y1 - 21.0).abs() < 1e-9, "{b:?}");
    t.height_mm = 16.0;
    let b = bounds(&render(&t).outline);
    assert!((b.height() - 10.5).abs() < 1e-9);
}

#[test]
fn horizontal_alignment_uses_each_lines_ink() {
    let mut t = hershey("HI
H", 32.0);
    t.align_v = AlignV::Bottom;
    for a in [AlignH::Left, AlignH::Middle, AlignH::Right] {
        t.align_h = a;
        let b = bounds(&render(&t).outline);
        let at = match a {
            AlignH::Left => b.x0,
            AlignH::Middle => (b.x0 + b.x1) / 2.0,
            AlignH::Right => b.x1,
        };
        assert!(at.abs() < 1e-9, "{a:?}: {b:?}");
    }
    // The short second line is centred on its own.
    t.align_h = AlignH::Middle;
    t.text = "H".into();
    let one = bounds(&render(&t).outline);
    assert!((one.x0 + one.x1).abs() < 1e-9);
}

#[test]
fn vertical_alignment_and_line_spacing() {
    let mut t = hershey("H\nH", 32.0);
    t.align_h = AlignH::Left;
    // Hershey: line = 21 + 7 + 4 = 32 units, so 32 mm here.
    t.align_v = AlignV::Bottom;
    let b = bounds(&render(&t).outline);
    assert!((b.y0 - 0.0).abs() < 1e-9 && (b.y1 - (32.0 + 21.0)).abs() < 1e-9, "{b:?}");
    t.align_v = AlignV::Top;
    let b = bounds(&render(&t).outline);
    assert!((b.y1 - 0.0).abs() < 1e-9, "caps of line 1 at the origin: {b:?}");
    t.align_v = AlignV::Middle;
    let b = bounds(&render(&t).outline);
    assert!(b.y0.abs() < 1e-9 + 32.0 && (b.y1 - 21.0).abs() < 1e-9, "two lines: first baseline at 0: {b:?}");
    t.text = "H".into();
    let b = bounds(&render(&t).outline);
    assert!((b.y0 + 16.0).abs() < 1e-9, "one line: baseline half a line down: {b:?}");
    t.text = "H\nH".into();
    t.align_v = AlignV::Bottom;
    t.v_spacing = 50.0;
    let b = bounds(&render(&t).outline);
    assert!((b.y1 - (48.0 + 21.0)).abs() < 1e-9, "VSpace 50 % adds half a line: {b:?}");
}

#[test]
fn letter_spacing_upper_case_and_max_width() {
    let mut t = hershey("ab", 10.0);
    t.align_h = AlignH::Left;
    let w0 = bounds(&render(&t).outline).width();
    t.h_spacing = 20.0;
    let w1 = bounds(&render(&t).outline).width();
    assert!((w1 - w0 - 2.0).abs() < 1e-9, "20 % of 10 mm between the two letters");
    t.h_spacing = 0.0;
    t.upper = true;
    let r = render(&t);
    assert_eq!(r.shown, "ab", "the stored text keeps its case");
    let up = hershey("AB", 10.0);
    let up = Text { align_h: AlignH::Left, ..up };
    assert_eq!(bounds(&r.outline), bounds(&render(&up).outline));
    let mut t = hershey("WIDE TEXT", 10.0);
    let full = bounds(&render(&t).outline);
    t.max_width_mm = full.width() / 2.0;
    let half = bounds(&render(&t).outline);
    assert!(half.width() <= full.width() / 2.0 + 1e-9, "{half:?}");
    let k = half.width() / full.width();
    assert!((half.height() - full.height() * k).abs() < 1e-6, "proportional without squeeze");
    t.squeeze = true;
    let sq = bounds(&render(&t).outline);
    assert!((sq.height() - full.height()).abs() < 1e-9, "squeeze keeps the height");
}

#[test]
fn italic_slants_single_line_fonts() {
    let mut t = hershey("I", 32.0);
    let straight = bounds(&render(&t).outline);
    t.italic = true;
    let r = render(&t);
    assert!(bounds(&r.outline).width() > straight.width() + 3.0);
}

#[test]
fn bend_wraps_round_a_circle() {
    let mut t = hershey("OPENETCH OPENETCH", 5.0);
    let flat = bounds(&render(&t).outline);
    t.bend_deg = 180.0;
    let arched = bounds(&render(&t).outline);
    // Half a circle whose circumference half is the text width: about width / π high.
    assert!(arched.height() > flat.width() / std::f64::consts::PI * 0.8, "{arched:?}");
    assert!(arched.width() < flat.width(), "{arched:?}");
    assert!(arched.y1 < flat.y1 + 0.5, "positive bend arches upward from the origin");
    t.distort = true;
    let distorted = bounds(&render(&t).outline);
    assert!((distorted.height() - arched.height()).abs() < 1.5);
    t.bend_deg = -180.0;
    let down = bounds(&render(&t).outline);
    assert!(down.y0 > -flat.height() - 1.0 && down.y1 > 2.0, "negative bend hangs the ends up: {down:?}");
}

#[test]
fn text_follows_a_path() {
    let mut t = hershey("AB", 10.0);
    t.path = Some(TextPath { object: "p".into(), offset_mm: 0.0 });
    t.align_h = AlignH::Left;
    t.align_v = AlignV::Bottom;
    // A vertical line going up: the text reads upward, its baseline on the line (x = 0), glyphs to its left.
    let mut line = BezPath::new();
    line.move_to((0.0, 0.0));
    line.line_to((0.0, 100.0));
    let r = render_text(&t, "AB", Some(&line)).unwrap();
    let b = bounds(&r.outline);
    assert!(b.x1 <= 1e-6 && b.x0 < -5.0 && b.y0 >= -1e-6 && b.y1 < 30.0, "{b:?}");
    t.align_h = AlignH::Right;
    let b = bounds(&render_text(&t, "AB", Some(&line)).unwrap().outline);
    assert!(b.y1 > 95.0 && b.y1 <= 100.0 + 1e-6, "{b:?}");
    t.path.as_mut().unwrap().offset_mm = -10.0;
    let b2 = bounds(&render_text(&t, "AB", Some(&line)).unwrap().outline);
    assert!((b.y1 - b2.y1 - 10.0).abs() < 1e-6);
    let mut circle = kurbo::Circle::new((0.0, 0.0), 20.0).to_path(0.01);
    circle = circle.reverse_subpaths();
    assert!(render_text(&t, "AB", Some(&circle)).is_ok());
}

#[test]
fn missing_fonts_and_glyphs() {
    let t = Text::new("x", FontSpec { family: "No Such Font".into(), kind: FontKind::Stroke }, 5.0);
    assert!(render(&Text { ..hershey("x", 5.0) }).warnings.is_empty());
    assert!(render_text(&t, "x", None).unwrap_err().contains("not installed"));
    let r = render(&hershey("x€", 5.0));
    assert!(r.warnings.iter().any(|w| w.contains('€')), "{:?}", r.warnings);
    let mut bad = hershey("x", 5.0);
    bad.height_mm = -1.0;
    assert!(render_text(&bad, "x", None).is_err());
}

#[test]
fn ignore_empty_vars_drops_blank_lines() {
    let mut t = hershey("%0\n%1\n%2", 10.0);
    t.variable = Some(doc::VarText { mode: doc::VarMode::Csv, offset: 0 });
    t.align_v = AlignV::Bottom;
    let shown = "A\n\nC";
    let tall = bounds(&render_text(&t, shown, None).unwrap().outline);
    t.ignore_empty_vars = true;
    let short = bounds(&render_text(&t, shown, None).unwrap().outline);
    assert!((tall.height() - short.height() - 10.0).abs() < 1e-6, "one line less: {tall:?} {short:?}");
}

/// A system TrueType font, when one is installed where these tests look.
fn system_font() -> Option<FontSpec> {
    let candidates = [
        ("C:\\Windows\\Fonts\\arial.ttf", "Arial"),
        ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", "DejaVu Sans"),
        ("/System/Library/Fonts/Supplemental/Arial.ttf", "Arial"),
    ];
    for (path, family) in candidates {
        if let Ok(data) = std::fs::read(path) {
            registry::add_font_data(family, data).ok()?;
            return Some(FontSpec::system(family));
        }
    }
    None
}

#[test]
fn truetype_fonts_render_filled_outlines() {
    let Some(font) = system_font() else { return };
    let mut t = Text::new("Hello", font.clone(), 10.0);
    t.welded = false;
    let r = render(&t);
    let b = bounds(&r.outline);
    assert!(b.height() > 6.0 && b.height() < 10.0, "{b:?}");
    assert!(r.outline.elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(..))), "curves kept as cubics");
    assert!(r.outline.elements().iter().any(|e| matches!(e, kurbo::PathEl::ClosePath)));
    // Weld merges overlapping letters only.
    t.h_spacing = -40.0;
    t.welded = true;
    let welded = render(&t);
    let subpaths = |p: &BezPath| p.elements().iter().filter(|e| matches!(e, kurbo::PathEl::MoveTo(_))).count();
    t.welded = false;
    let loose = render(&t);
    assert!(subpaths(&welded.outline) < subpaths(&loose.outline));
    // Bold needs a bold face; a missing italic face is slanted with a warning.
    let mut b = Text::new("H", font, 10.0);
    b.bold = true;
    assert!(render(&b).outline.elements().len() > 4);
}

#[test]
fn an_added_truetype_font_lays_out_without_system_fonts() {
    // The generated box font: one box per non-space character, 0.7 em tall.
    let data = include_bytes!("../tests/fixtures/ttf/openetch-box.ttf").to_vec();
    assert_eq!(registry::add_font_data("box", data).unwrap(), ["OpenEtch Box"]);
    let mut t = Text::new("OE 12", FontSpec::system("OpenEtch Box"), 10.0);
    t.welded = false;
    let r = render(&t);
    let boxes = r.outline.elements().iter().filter(|e| matches!(e, kurbo::PathEl::MoveTo(_))).count();
    assert_eq!(boxes, 4);
    assert!((bounds(&r.outline).height() - 7.0).abs() < 1e-6, "{:?}", bounds(&r.outline));
}
