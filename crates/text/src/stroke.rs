//! Single-line (stroke) fonts: glyphs made of open polylines and curves, never filled. The Hershey fonts, AutoCAD
//! `.shx` and EzCad `.jsf` files all load into a [`StrokeFont`].

use std::collections::HashMap;

use kurbo::BezPath;

/// One glyph in font units: y up, baseline at 0, its left edge at x = 0 (the pen starts there).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrokeGlyph {
    pub path: BezPath,
    pub advance: f64,
}

/// A single-line font in its own units.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeFont {
    pub name: String,
    /// Units per em: a text of height `H` mm scales the font by `H / em`.
    pub em: f64,
    /// Above the baseline (positive) and below it (positive), and the gap between lines, in font units.
    pub ascent: f64,
    pub descent: f64,
    pub line_gap: f64,
    /// Height of the capitals above the baseline.
    pub cap_height: f64,
    /// Advance of a space (and of characters the font lacks are drawn as nothing with this advance).
    pub space: f64,
    /// Extra gap EzCad and AutoCAD put between characters, in font units (0 for Hershey).
    pub letter_gap: f64,
    pub glyphs: HashMap<char, StrokeGlyph>,
}

impl StrokeFont {
    pub fn glyph(&self, c: char) -> Option<&StrokeGlyph> {
        self.glyphs.get(&c)
    }

    /// Fill in `cap_height` from the glyph `H` (or `X`, else 70 % of the ascent), as stroke formats rarely say.
    pub(crate) fn measure_caps(&mut self) {
        let from = |c| self.glyphs.get(&c).and_then(|g: &StrokeGlyph| geom::bounds(&g.path)).map(|b| b.y1);
        self.cap_height = from('H').or_else(|| from('X')).filter(|h| *h > 0.0).unwrap_or(self.ascent * 0.7);
    }
}

/// A polyline as an open path (a closed one when first == last is still drawn as written).
pub(crate) fn polyline(points: &[(f64, f64)]) -> BezPath {
    let mut p = BezPath::new();
    if points.len() < 2 {
        // A single point is a dot: a zero-length line, so it still burns.
        if let Some(&(x, y)) = points.first() {
            p.move_to((x, y));
            p.line_to((x, y));
        }
        return p;
    }
    p.move_to(points[0]);
    for &q in &points[1..] {
        p.line_to(q);
    }
    p
}
