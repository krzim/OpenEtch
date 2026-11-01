//! TrueType / OpenType fonts: shaping with harfrust (a HarfBuzz port: kerning, ligatures, marks) and outlines with
//! skrifa. Quadratic TrueType curves are raised to cubics, as LightBurn's saved outlines have them.

use std::sync::Arc;

use kurbo::{BezPath, Point};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// A loaded font face. Units are the font's own (see [`OutlineFont::upem`]).
#[derive(Clone)]
pub struct OutlineFont {
    data: Arc<[u8]>,
    index: u32,
    pub family: String,
    pub upem: f64,
    /// Above the baseline, below it (positive), and the line gap, as the font's horizontal header gives them.
    pub ascent: f64,
    pub descent: f64,
    pub line_gap: f64,
    pub cap_height: f64,
    /// The face's own weight (400 regular, 700 bold) and slant.
    pub weight: f64,
    pub italic: bool,
}

impl std::fmt::Debug for OutlineFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutlineFont").field("family", &self.family).field("upem", &self.upem).finish_non_exhaustive()
    }
}

/// One shaped glyph, in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shaped {
    pub glyph: u32,
    pub x_advance: f64,
    pub x_offset: f64,
    pub y_offset: f64,
    /// Byte offset of the character it came from.
    pub cluster: u32,
}

impl OutlineFont {
    /// Load face `index` of a font file (`.ttf`, `.otf`, `.ttc`).
    pub fn from_data(data: Arc<[u8]>, index: u32) -> Result<OutlineFont, String> {
        let font = FontRef::from_index(&data, index).map_err(|e| format!("not a usable font file ({e})"))?;
        let m = font.metrics(Size::unscaled(), LocationRef::default());
        if m.units_per_em == 0 {
            return Err("the font has no units per em".into());
        }
        let family = font
            .localized_strings(skrifa::string::StringId::FAMILY_NAME)
            .english_or_first()
            .map(|s| s.chars().collect::<String>())
            .unwrap_or_default();
        let upem = f64::from(m.units_per_em);
        let attrs = font.attributes();
        let cap = m.cap_height.map(f64::from).filter(|c| *c > 0.0);
        let mut f = OutlineFont {
            family,
            upem,
            ascent: f64::from(m.ascent),
            descent: f64::from(m.descent).abs(),
            line_gap: f64::from(m.leading).max(0.0),
            cap_height: cap.unwrap_or(upem * 0.7),
            weight: f64::from(attrs.weight.value()),
            italic: attrs.style != skrifa::attribute::Style::Normal,
            data: data.clone(),
            index,
        };
        if cap.is_none() {
            // Older fonts lack the OS/2 cap height: measure H.
            let gid = font.charmap().map('H').map(|g| g.to_u32());
            if let Some(b) = gid.and_then(|g| geom::bounds(&f.glyph_path(g))) {
                f.cap_height = b.y1;
            }
        }
        Ok(f)
    }

    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.index).expect("checked when loaded")
    }

    /// Whether the font has a glyph for `c`.
    pub fn has_char(&self, c: char) -> bool {
        self.font().charmap().map(c).is_some()
    }

    /// Shape one line of text.
    pub fn shape(&self, text: &str) -> Vec<Shaped> {
        let font = self.font();
        let data = harfrust::ShaperData::new(&font);
        let shaper = data.shaper(&font).build();
        let mut buffer = harfrust::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let out = shaper.shape(buffer, harfrust::ShapeOptions::new());
        out.glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(i, p)| Shaped {
                glyph: i.glyph_id,
                x_advance: f64::from(p.x_advance),
                x_offset: f64::from(p.x_offset),
                y_offset: f64::from(p.y_offset),
                cluster: i.cluster,
            })
            .collect()
    }

    /// The outline of glyph `gid` in font units (y up, origin on the baseline at the pen position).
    pub fn glyph_path(&self, gid: u32) -> BezPath {
        let font = self.font();
        let mut pen = Pen::default();
        if let Some(g) = font.outline_glyphs().get(GlyphId::new(gid)) {
            let _ = g.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
        }
        pen.path
    }
}

#[derive(Default)]
struct Pen {
    path: BezPath,
    at: Point,
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.at = Point::new(f64::from(x), f64::from(y));
        self.path.move_to(self.at);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.at = Point::new(f64::from(x), f64::from(y));
        self.path.line_to(self.at);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let c = Point::new(f64::from(cx0), f64::from(cy0));
        let p = Point::new(f64::from(x), f64::from(y));
        let a = self.at;
        self.path.curve_to(a + (c - a) * (2.0 / 3.0), p + (c - p) * (2.0 / 3.0), p);
        self.at = p;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.at = Point::new(f64::from(x), f64::from(y));
        self.path.curve_to((f64::from(cx0), f64::from(cy0)), (f64::from(cx1), f64::from(cy1)), self.at);
    }

    fn close(&mut self) {
        self.path.close_path();
    }
}
