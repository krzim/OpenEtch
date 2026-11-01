//! The Hershey fonts (James Hurt's `.jhf` format), bundled as OpenEtch's built-in single-line fonts. Licence and
//! acknowledgements: `fonts/hershey/README.md` and ATTRIBUTION.md.
//!
//! Format: one glyph per line for the characters 32..=127 in order: a 5-column glyph number, a 3-column count of
//! coordinate pairs (the bounds pair included), then pairs of characters, each a coordinate `c - 'R'`. The first
//! pair is the glyph's left and right bounds; `" R"` lifts the pen. Coordinates are y down with the baseline at
//! y = 9 and capitals reaching y = -12; a line can wrap onto the next when a glyph is long.

use std::collections::HashMap;

use crate::stroke::{StrokeFont, StrokeGlyph, polyline};

/// Bundled fonts: (family name, file stem, data).
pub const FONTS: &[(&str, &str, &str)] = &[
    ("Hershey Sans", "futural", include_str!("../fonts/hershey/futural.jhf")),
    ("Hershey Sans Medium", "futuram", include_str!("../fonts/hershey/futuram.jhf")),
    ("Hershey Roman Simplex", "rowmans", include_str!("../fonts/hershey/rowmans.jhf")),
    ("Hershey Serif Medium", "rowmand", include_str!("../fonts/hershey/rowmand.jhf")),
    ("Hershey Serif Bold", "rowmant", include_str!("../fonts/hershey/rowmant.jhf")),
    ("Hershey Script", "scripts", include_str!("../fonts/hershey/scripts.jhf")),
    ("Hershey Script Medium", "scriptc", include_str!("../fonts/hershey/scriptc.jhf")),
    ("Hershey Times", "timesr", include_str!("../fonts/hershey/timesr.jhf")),
    ("Hershey Times Italic", "timesi", include_str!("../fonts/hershey/timesi.jhf")),
    ("Hershey Times Bold", "timesrb", include_str!("../fonts/hershey/timesrb.jhf")),
    ("Hershey Gothic English", "gothiceng", include_str!("../fonts/hershey/gothiceng.jhf")),
    ("Hershey Cursive", "cursive", include_str!("../fonts/hershey/cursive.jhf")),
];

/// The default single-line font.
pub const DEFAULT: &str = "Hershey Sans";

const BASELINE: f64 = 9.0;

fn coord(c: u8) -> f64 {
    f64::from(c) - f64::from(b'R')
}

/// Parse a `.jhf` file whose glyphs are the characters from 32 on, in order.
pub fn parse(name: &str, data: &str) -> Result<StrokeFont, String> {
    let mut glyphs = HashMap::new();
    let mut lines = data.lines().filter(|l| !l.trim().is_empty());
    let mut code = 32u32;
    while let Some(first) = lines.next() {
        let bytes = first.as_bytes();
        if bytes.len() < 8 {
            return Err(format!("{name}: glyph {code}: line too short"));
        }
        let count: usize = std::str::from_utf8(&bytes[5..8])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| format!("{name}: glyph {code}: bad coordinate count"))?;
        let mut body: Vec<u8> = bytes[8..].to_vec();
        while body.len() < count * 2 {
            match lines.next() {
                Some(more) => body.extend_from_slice(more.as_bytes()),
                None => return Err(format!("{name}: glyph {code}: truncated")),
            }
        }
        if count == 0 {
            return Err(format!("{name}: glyph {code}: no bounds"));
        }
        let (left, right) = (coord(body[0]), coord(body[1]));
        let mut path = kurbo::BezPath::new();
        let mut stroke: Vec<(f64, f64)> = Vec::new();
        for pair in body[2..count * 2].as_chunks::<2>().0 {
            if pair == b" R" {
                path.extend(polyline(&stroke));
                stroke.clear();
            } else {
                stroke.push((coord(pair[0]) - left, BASELINE - coord(pair[1])));
            }
        }
        path.extend(polyline(&stroke));
        if let Some(c) = char::from_u32(code) {
            glyphs.insert(c, StrokeGlyph { path, advance: right - left });
        }
        code += 1;
    }
    if glyphs.is_empty() {
        return Err(format!("{name}: no glyphs"));
    }
    let space = glyphs.get(&' ').map_or(16.0, |g| g.advance);
    let mut f = StrokeFont {
        name: name.to_string(),
        em: 32.0,
        ascent: 21.0,
        descent: 7.0,
        line_gap: 4.0,
        cap_height: 21.0,
        space,
        letter_gap: 0.0,
        glyphs,
    };
    f.measure_caps();
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_font_parses() {
        for (name, _, data) in FONTS {
            let f = parse(name, data).unwrap();
            assert!(f.glyphs.len() >= 95, "{name}: {}", f.glyphs.len());
            let a = f.glyph('A').unwrap();
            let b = geom::bounds(&a.path).unwrap();
            assert!(b.y0 > -1.0 && b.y1 > 15.0 && b.y1 < 30.0, "{name}: A spans {b:?}");
            assert!(a.advance > 0.0);
            assert!((f.cap_height - b.y1).abs() < 6.0, "{name}: caps {}", f.cap_height);
        }
    }

    #[test]
    fn simplex_capitals_stand_on_the_baseline() {
        let f = parse("s", FONTS[0].2).unwrap();
        let h = geom::bounds(&f.glyph('H').unwrap().path).unwrap();
        assert_eq!((h.y0, h.y1), (0.0, 21.0), "Simplex H runs from the baseline to y 21");
        assert!(f.glyph(' ').unwrap().path.elements().is_empty());
        let l = f.glyph('l').unwrap();
        assert!(geom::bounds(&l.path).unwrap().x0 >= 0.0);
    }

    #[test]
    fn broken_files_are_refused() {
        assert!(parse("x", "12345").is_err());
        assert!(parse("x", "12345 9MW").is_err());
        assert!(parse("x", "").is_err());
    }
}
