//! EzCad `.jsf` single-line fonts ("JCZ SINGLE FONT"). The layout is not published; it was worked out from the 162
//! fonts EzCad2 ships (all parse with these rules, every glyph ending exactly where the next begins):
//!
//! - `0x00`: the ASCII magic `JCZ SINGLE FONT\0`, then `0xCC` filler; at `0x32` a NUL-terminated description
//!   (`JSF Font -- ARCHD`).
//! - `0x96` u32 `0x200`, `0x9A` u32 1 (constant in every file).
//! - `0x9E`, `0xA2`, `0xA6`, `0xAA`: four f32, equal in every file: the design size (units per em; 32 in ARCHD, 100
//!   in HELVETIC).
//! - `0xAE` u32: number of entries in the character table; `0xB2` u32: its offset (always `0x11A`); `0xC6` u32: the
//!   table's end (offset + 4 × entries).
//! - `0xB6`, `0xBA` f32: ascent and descent in % of the size (96.875 + 3.125, 80 + 20, ...).
//! - `0xBE`, `0xC2` f32: a spacing in font units, 10 % of the size in most files (used here as the gap between
//!   characters).
//! - Character table: one u32 file offset per character code (0 = no glyph). Codes below 256 are the Windows ANSI
//!   code (code page 1252, read as Latin-1 here plus the 0x80-0x9F punctuation); larger tables hold either UTF-16
//!   code units with their bytes swapped (`0x1004` is U+0410, Cyrillic A; Greek and Latin Extended fonts) or GB2312
//!   double-byte codes (`0xA1A1`.., the Chinese fonts; not mapped here).
//! - Glyph: u16 stroke count, then per stroke u8 kind, u16 point count and that many (f32 x, f32 y) pairs in font
//!   units, y up, baseline 0. Kind 2 is a polyline; kind 3 (only in two Chinese fonts) a chain of cubic Béziers
//!   (3n + 1 points). Most glyphs end with 34 more bytes: their bounds (4 × f32), two f32 spacings, 10 bytes not
//!   needed here.
//!
//! Advances: a glyph's ink width plus the spacing (EzCad spaces characters by their bounds); a space is half the size.
//! The ink is moved to start at x = 0.

use std::collections::HashMap;

use kurbo::BezPath;

use crate::stroke::{StrokeFont, StrokeGlyph, polyline};

const MAGIC: &[u8] = b"JCZ SINGLE FONT";
/// Glyph data beyond this many points per stroke is refused (EzCad's largest have a few hundred).
const MAX_POINTS: usize = 100_000;

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn f32_at(d: &[u8], o: usize) -> Option<f64> {
    let v = f64::from(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?));
    v.is_finite().then_some(v)
}

/// Whether `data` looks like a `.jsf` font.
pub fn is_jsf(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

/// The character a table index stands for.
fn char_of(index: u32, table_len: u32) -> Option<char> {
    match index {
        0..=0x7F => char::from_u32(index),
        0x80..=0x9F => {
            // Windows-1252's printable characters in the C1 range.
            const CP1252: [u32; 32] = [
                0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0,
                0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
                0x0153, 0, 0x017E, 0x0178,
            ];
            char::from_u32(CP1252[(index - 0x80) as usize]).filter(|c| *c != '\0')
        }
        0xA0..=0xFF => char::from_u32(index),
        _ if table_len > 0xA1A1 && (index >> 8) >= 0xA1 && (index & 0xFF) >= 0xA1 => None, // GB2312, unmapped
        _ => char::from_u32((index as u16).swap_bytes() as u32).filter(|c| u32::from(*c) >= 0x100),
    }
}

/// Parse one glyph at `o`: its strokes in font units.
fn glyph(d: &[u8], o: usize) -> Option<BezPath> {
    let n = u16_at(d, o)?;
    let mut at = o + 2;
    let mut path = BezPath::new();
    for _ in 0..n {
        let kind = *d.get(at)?;
        let count = usize::from(u16_at(d, at + 1)?);
        at += 3;
        if count > MAX_POINTS {
            return None;
        }
        let mut pts = Vec::with_capacity(count);
        for k in 0..count {
            pts.push((f32_at(d, at + 8 * k)?, f32_at(d, at + 8 * k + 4)?));
        }
        at += 8 * count;
        match kind {
            3 if count >= 4 && (count - 1) % 3 == 0 => {
                path.move_to(pts[0]);
                for c in pts[1..].as_chunks::<3>().0 {
                    path.curve_to(c[0], c[1], c[2]);
                }
            }
            _ => path.extend(polyline(&pts)),
        }
    }
    Some(path)
}

/// Parse a `.jsf` font. `name` is the family name to give it (the file stem).
pub fn parse(name: &str, d: &[u8]) -> Result<StrokeFont, String> {
    if !is_jsf(d) {
        return Err(format!("{name}: not an EzCad JSF font (no JCZ SINGLE FONT header)"));
    }
    let bad = || format!("{name}: damaged JSF header");
    let size = f32_at(d, 0x9E).filter(|s| *s > 0.0).ok_or_else(bad)?;
    let entries = u32_at(d, 0xAE).ok_or_else(bad)?;
    let table = u32_at(d, 0xB2).ok_or_else(bad)? as usize;
    let ascent_pct = f32_at(d, 0xB6).unwrap_or(80.0);
    let descent_pct = f32_at(d, 0xBA).unwrap_or(20.0);
    let gap = f32_at(d, 0xBE).filter(|g| *g >= 0.0 && *g < size).unwrap_or(size * 0.1);
    if entries > 0x1_0000 || table.saturating_add(entries as usize * 4) > d.len() {
        return Err(bad());
    }
    let mut glyphs = HashMap::new();
    for i in 0..entries {
        let off = u32_at(d, table + 4 * i as usize).ok_or_else(bad)? as usize;
        if off == 0 {
            continue;
        }
        let Some(c) = char_of(i, entries) else { continue };
        let Some(path) = glyph(d, off) else {
            return Err(format!("{name}: glyph {i:#x} is damaged"));
        };
        let (path, advance) = match geom::bounds(&path) {
            Some(b) if !path.elements().is_empty() => {
                (kurbo::Affine::translate((-b.x0, 0.0)) * path, b.width() + gap)
            }
            _ => (path, size * 0.5),
        };
        glyphs.insert(c, StrokeGlyph { path, advance });
    }
    if glyphs.is_empty() {
        return Err(format!("{name}: the font has no glyphs OpenEtch can map"));
    }
    let mut f = StrokeFont {
        name: name.to_string(),
        em: size,
        ascent: size * ascent_pct / 100.0,
        descent: size * descent_pct / 100.0,
        line_gap: size * 0.2,
        cap_height: 0.0,
        space: size * 0.5,
        letter_gap: 0.0,
        glyphs,
    };
    f.glyphs.remove(&' ');
    f.measure_caps();
    Ok(f)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny JSF in the layout above: size 32, a table of 0x42 entries with `!` and `A`.
    pub fn sample() -> Vec<u8> {
        let mut d = vec![0xCCu8; 0x11A];
        d[..MAGIC.len()].copy_from_slice(MAGIC);
        d[MAGIC.len()] = 0;
        let put_f = |d: &mut Vec<u8>, o: usize, v: f32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let put_u = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        for o in [0x9E, 0xA2, 0xA6, 0xAA] {
            put_f(&mut d, o, 32.0);
        }
        let entries = 0x42u32;
        put_u(&mut d, 0xAE, entries);
        put_u(&mut d, 0xB2, 0x11A);
        put_f(&mut d, 0xB6, 80.0);
        put_f(&mut d, 0xBA, 20.0);
        put_f(&mut d, 0xBE, 3.2);
        put_f(&mut d, 0xC2, 3.2);
        put_u(&mut d, 0xC6, 0x11A + 4 * entries);
        d.resize(0x11A + 4 * entries as usize, 0);
        let glyph = |d: &mut Vec<u8>, code: u32, strokes: &[(u8, &[(f32, f32)])]| {
            let at = d.len() as u32;
            d[0x11A + 4 * code as usize..0x11A + 4 * code as usize + 4].copy_from_slice(&at.to_le_bytes());
            d.extend((strokes.len() as u16).to_le_bytes());
            for (kind, pts) in strokes {
                d.push(*kind);
                d.extend((pts.len() as u16).to_le_bytes());
                for (x, y) in *pts {
                    d.extend(x.to_le_bytes());
                    d.extend(y.to_le_bytes());
                }
            }
            d.extend([0u8; 34]);
        };
        glyph(&mut d, 0x21, &[(2, &[(4.0, 1.0), (5.0, 0.0), (6.0, 1.0)]), (2, &[(5.0, 7.0), (5.0, 27.0)])]);
        glyph(&mut d, 0x41, &[(2, &[(2.0, 0.0), (12.0, 26.0), (22.0, 0.0)]), (3, &[(6.0, 10.0), (9.0, 12.0), (15.0, 12.0), (18.0, 10.0)])]);
        d
    }

    #[test]
    fn synthetic_font_parses() {
        let f = parse("t", &sample()).unwrap();
        assert_eq!(f.em, 32.0);
        assert_eq!(f.glyphs.len(), 2);
        let a = f.glyph('A').unwrap();
        let b = geom::bounds(&a.path).unwrap();
        assert_eq!((b.x0, b.y0, b.y1), (0.0, 0.0, 26.0), "moved to x 0");
        assert!((a.advance - (20.0 + 3.2)).abs() < 1e-5, "{}", a.advance);
        assert!(a.path.elements().iter().any(|e| matches!(e, kurbo::PathEl::CurveTo(..))), "kind 3 is a cubic");
        assert!((f.cap_height - 25.6 * 0.7).abs() < 1e-9, "no H or X: 70 % of the ascent");
        assert!((f.ascent - 25.6).abs() < 1e-9);
    }

    #[test]
    fn damaged_files_are_refused() {
        assert!(parse("x", b"not a font").is_err());
        let mut d = sample();
        d.truncate(0x200);
        assert!(parse("x", &d).is_err());
        let mut d = sample();
        let at = u32_at(&d, 0x11A + 4 * 0x41).unwrap() as usize;
        d.truncate(at + 10);
        assert!(parse("x", &d).is_err(), "glyph runs past the end");
    }

    #[test]
    fn codes_map_to_characters() {
        assert_eq!(char_of(0x41, 256), Some('A'));
        assert_eq!(char_of(0x80, 256), Some('€'));
        assert_eq!(char_of(0x81, 256), None);
        assert_eq!(char_of(0xE9, 256), Some('é'));
        assert_eq!(char_of(0x1004, 42785), Some('А'), "byte-swapped UTF-16");
        assert_eq!(char_of(0x9103, 54532), Some('Α'));
        assert_eq!(char_of(0xA1A2, 63487), None, "GB2312 is not mapped");
    }

    /// The owner's EzCad fonts, read in place (`OPENETCH_JSF_DIR`); none is copied into the repository.
    #[test]
    fn real_fonts_parse_when_available() {
        let Ok(dir) = std::env::var("OPENETCH_JSF_DIR") else { return };
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let p = e.unwrap().path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jsf")) {
                continue;
            }
            let data = std::fs::read(&p).unwrap();
            let name = p.file_stem().unwrap().to_string_lossy().to_string();
            match parse(&name, &data) {
                Ok(f) => {
                    assert!(f.em > 0.0, "{name}");
                    n += 1;
                }
                Err(e) => assert!(e.contains("no glyphs OpenEtch can map"), "{name}: {e}"),
            }
        }
        assert!(n > 100, "parsed {n} fonts");
    }
}
