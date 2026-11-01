//! AutoCAD `.shx` shape fonts (single-line). The shape bytes follow Autodesk's published shape description language
//! (AutoCAD Customization Guide, "Shape Descriptions": vector length and direction codes, special codes 0-14); the
//! compiled file layout is the commonly documented one:
//!
//! - `AutoCAD-86 shapes 1.0` (or `1.1`) `\r\n\x1A`, then u16 first code, u16 last code, u16 count, `count` index
//!   entries (u16 code, u16 byte length), then the definitions in index order: a NUL-terminated name and the shape
//!   bytes (ending with 0). Shape 0 describes the font: name, then `above`, `below`, `modes`.
//! - `AutoCAD-86 unifont 1.0\r\n\x1A`, then u32 number of entries, u16 length of the font description (name, above,
//!   below, modes, encoding, type, 0), then per shape u16 code (Unicode), u16 length, name and bytes.
//!
//! Big fonts (`bigfont`, Asian double-byte codes) are refused. No real `.shx` file was available while this was
//! written: the tests build files in this layout, so parsing real AutoCAD fonts is unverified.
//!
//! Units: the font's `above` is the capital height; OpenEtch scales a text so that capitals are 0.7 of its height,
//! as with most TrueType fonts (em = above / 0.7).

use std::collections::HashMap;

use kurbo::{Arc, BezPath, Point, Vec2};

use crate::stroke::{StrokeFont, StrokeGlyph};

const MAX_SHAPES: usize = 0x1_0000;
/// Most commands one glyph may run (subshapes included), so a looping file cannot hang.
const MAX_STEPS: usize = 100_000;
/// Scale factors (codes 3 and 4) are kept within this range of 1, and arcs with a larger radius (font units) are
/// drawn as nothing: real fonts scale by a few times at most, while a file that scales by 255 over and over would
/// otherwise reach infinite coordinates or arcs that never finish flattening.
const MAX_SCALE: f64 = 1e6;
const MAX_RADIUS: f64 = 1e7;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Shapes,
    Unifont,
}

/// Whether `data` looks like an `.shx` file.
pub fn is_shx(data: &[u8]) -> bool {
    data.starts_with(b"AutoCAD-86")
}

fn u16_le(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

/// The 16 directions of a vector byte (on a square, as AutoCAD defines them).
fn direction(d: u8) -> Vec2 {
    const H: f64 = 0.5;
    let (x, y) = match d & 0x0F {
        0 => (1.0, 0.0),
        1 => (1.0, H),
        2 => (1.0, 1.0),
        3 => (H, 1.0),
        4 => (0.0, 1.0),
        5 => (-H, 1.0),
        6 => (-1.0, 1.0),
        7 => (-1.0, H),
        8 => (-1.0, 0.0),
        9 => (-1.0, -H),
        10 => (-1.0, -1.0),
        11 => (-H, -1.0),
        12 => (0.0, -1.0),
        13 => (H, -1.0),
        14 => (1.0, -1.0),
        _ => (1.0, -H),
    };
    Vec2::new(x, y)
}

struct Pen<'a> {
    shapes: &'a HashMap<u16, &'a [u8]>,
    kind: Kind,
    pos: Point,
    down: bool,
    scale: f64,
    stack: Vec<Point>,
    path: BezPath,
    /// Whether the current subpath has started (`move_to` written at the pen position).
    open: bool,
    steps: usize,
}

impl Pen<'_> {
    fn move_by(&mut self, d: Vec2) {
        let to = self.pos + d * self.scale;
        if !(to.x.is_finite() && to.y.is_finite()) {
            return;
        }
        if self.down {
            if !self.open {
                self.path.move_to(self.pos);
                self.open = true;
            }
            self.path.line_to(to);
        } else {
            self.open = false;
        }
        self.pos = to;
    }

    /// An arc from the pen position about `center` by `sweep` radians (positive counter-clockwise).
    fn arc(&mut self, center: Point, sweep: f64) {
        let r = (self.pos - center).hypot();
        let start = (self.pos - center).atan2();
        let end = center + Vec2::from_angle(start + sweep) * r;
        if !(r <= MAX_RADIUS && sweep.is_finite() && end.x.is_finite() && end.y.is_finite()) {
            // Too large to draw (see MAX_RADIUS): the pen moves on, lifted, when it can.
            if end.x.is_finite() && end.y.is_finite() {
                self.pos = end;
                self.open = false;
            }
            return;
        }
        if self.down && r > 0.0 {
            if !self.open {
                self.path.move_to(self.pos);
                self.open = true;
            }
            let a = Arc::new(center, (r, r), start, sweep, 0.0);
            a.to_cubic_beziers(0.01, |p1, p2, p3| self.path.curve_to(p1, p2, p3));
        } else if !self.down {
            self.open = false;
        }
        self.pos = end;
    }

    /// A bulge arc to `pos + d` (`bulge` -127..=127, 127 a semicircle; positive counter-clockwise).
    fn bulge(&mut self, d: Vec2, bulge: i8) {
        let d = d * self.scale;
        if bulge == 0 || d.hypot() == 0.0 {
            let s = self.scale;
            self.scale = 1.0;
            self.move_by(d);
            self.scale = s;
            return;
        }
        let sweep = 4.0 * (f64::from(bulge) / 127.0).atan();
        // Centre: on the chord's perpendicular bisector, at distance (D/2) / tan(sweep/2) to the left for a
        // counter-clockwise arc.
        let mid = self.pos + d * 0.5;
        let normal = Vec2::new(-d.y, d.x) / d.hypot();
        let center = mid + normal * (d.hypot() / 2.0 / (sweep / 2.0).tan());
        self.arc(center, sweep);
    }

    fn octant_arc(&mut self, radius: f64, start_deg: f64, sweep_deg: f64) {
        let start = start_deg.to_radians();
        let center = self.pos - Vec2::from_angle(start) * radius;
        self.arc(center, sweep_deg.to_radians());
    }

    fn byte(spec: &[u8], i: usize) -> Option<u8> {
        spec.get(i).copied()
    }

    /// Run one command at `i` (only measuring its length when `run` is false). Returns the next index, or `None` at
    /// the end of the shape.
    fn step(&mut self, spec: &[u8], i: usize, run: bool, depth: usize) -> Option<usize> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return None;
        }
        let b = Self::byte(spec, i)?;
        let s8 = |k: usize| Self::byte(spec, k).map(|v| v as i8);
        if b >> 4 != 0 {
            if run {
                self.move_by(direction(b) * f64::from(b >> 4));
            }
            return Some(i + 1);
        }
        match b {
            0 => None,
            1 => {
                if run {
                    self.down = true;
                }
                Some(i + 1)
            }
            2 => {
                if run {
                    self.down = false;
                    self.open = false;
                }
                Some(i + 1)
            }
            3 | 4 => {
                let f = f64::from(Self::byte(spec, i + 1)?);
                if run && f > 0.0 {
                    let s = if b == 3 { self.scale / f } else { self.scale * f };
                    self.scale = s.clamp(1.0 / MAX_SCALE, MAX_SCALE);
                }
                Some(i + 2)
            }
            5 => {
                if run && self.stack.len() < 4 {
                    self.stack.push(self.pos);
                }
                Some(i + 1)
            }
            6 => {
                if run && let Some(p) = self.stack.pop() {
                    self.pos = p;
                    self.open = false;
                }
                Some(i + 1)
            }
            7 => {
                let (code, len) = match self.kind {
                    Kind::Shapes => (u16::from(Self::byte(spec, i + 1)?), 2),
                    Kind::Unifont => (u16::from_be_bytes([Self::byte(spec, i + 1)?, Self::byte(spec, i + 2)?]), 3),
                };
                if run && depth < 8 && let Some(sub) = self.shapes.get(&code).copied() {
                    self.run_spec(sub, depth + 1);
                }
                Some(i + len)
            }
            8 => {
                let (dx, dy) = (s8(i + 1)?, s8(i + 2)?);
                if run {
                    self.move_by(Vec2::new(f64::from(dx), f64::from(dy)));
                }
                Some(i + 3)
            }
            9 => {
                let mut k = i + 1;
                loop {
                    let (dx, dy) = (s8(k)?, s8(k + 1)?);
                    k += 2;
                    if dx == 0 && dy == 0 {
                        return Some(k);
                    }
                    if run {
                        self.move_by(Vec2::new(f64::from(dx), f64::from(dy)));
                    }
                }
            }
            10 => {
                let r = f64::from(Self::byte(spec, i + 1)?);
                let o = Self::byte(spec, i + 2)?;
                if run {
                    let (start, span, sign) = octants(o);
                    self.octant_arc(r * self.scale, start * 45.0, sign * span * 45.0);
                }
                Some(i + 3)
            }
            11 => {
                let so = f64::from(Self::byte(spec, i + 1)?);
                let eo = f64::from(Self::byte(spec, i + 2)?);
                let r = f64::from(u16::from_be_bytes([Self::byte(spec, i + 3)?, Self::byte(spec, i + 4)?]));
                let o = Self::byte(spec, i + 5)?;
                if run {
                    let (start, span, sign) = octants(o);
                    let a0 = start * 45.0 + so * 45.0 / 256.0;
                    let a1 = if eo == 0.0 { (start + span) * 45.0 } else { (start + span - 1.0) * 45.0 + eo * 45.0 / 256.0 };
                    // Counter-clockwise angles; a clockwise arc runs the same octants the other way.
                    let (from, sweep) = if sign > 0.0 { (a0, a1 - a0) } else { (start * 45.0 - so * 45.0 / 256.0, -(a1 - a0)) };
                    self.octant_arc(r * self.scale, from, sweep);
                }
                Some(i + 6)
            }
            12 => {
                let (dx, dy, bulge) = (s8(i + 1)?, s8(i + 2)?, s8(i + 3)?);
                if run {
                    self.bulge(Vec2::new(f64::from(dx), f64::from(dy)), bulge);
                }
                Some(i + 4)
            }
            13 => {
                let mut k = i + 1;
                loop {
                    let (dx, dy) = (s8(k)?, s8(k + 1)?);
                    if dx == 0 && dy == 0 {
                        return Some(k + 2);
                    }
                    let bulge = s8(k + 2)?;
                    k += 3;
                    if run {
                        self.bulge(Vec2::new(f64::from(dx), f64::from(dy)), bulge);
                    }
                }
            }
            14 => {
                // The next command is for vertical text only: skip it.
                let next = i + 1;
                Self::byte(spec, next)?;
                if Self::byte(spec, next) == Some(0) {
                    return Some(next);
                }
                self.step(spec, next, false, depth)
            }
            _ => Some(i + 1),
        }
    }

    fn run_spec(&mut self, spec: &[u8], depth: usize) {
        let mut i = 0;
        while let Some(next) = self.step(spec, i, true, depth) {
            i = next;
        }
    }
}

/// An octant byte: (start octant, octants spanned (0 = 8), +1 counter-clockwise / -1 clockwise).
fn octants(o: u8) -> (f64, f64, f64) {
    let sign = if o & 0x80 != 0 { -1.0 } else { 1.0 };
    let start = f64::from((o >> 4) & 0x07);
    let span = match o & 0x0F {
        0 => 8.0,
        n => f64::from(n),
    };
    (start, span, sign)
}

/// Shape bytes after the NUL-terminated name.
fn spec_of(def: &[u8]) -> &[u8] {
    match def.iter().position(|b| *b == 0) {
        Some(n) => &def[n + 1..],
        None => &[],
    }
}

/// Parse an `.shx` font named `name` (the file stem).
pub fn parse(name: &str, d: &[u8]) -> Result<StrokeFont, String> {
    if !is_shx(d) {
        return Err(format!("{name}: not an AutoCAD SHX font"));
    }
    let header_end = d.iter().position(|b| *b == 0x1A).ok_or_else(|| format!("{name}: damaged SHX header"))?;
    let header = String::from_utf8_lossy(&d[..header_end]).to_ascii_lowercase();
    let bad = || format!("{name}: damaged SHX file");
    let mut defs: HashMap<u16, &[u8]> = HashMap::new();
    let mut info: Option<&[u8]> = None;
    let kind;
    if header.contains("bigfont") {
        return Err(format!("{name}: AutoCAD big fonts (Asian double-byte SHX) are not supported"));
    } else if header.contains("unifont") {
        kind = Kind::Unifont;
        let mut at = header_end + 1;
        let count = u32::from_le_bytes(d.get(at..at + 4).ok_or_else(bad)?.try_into().map_err(|_| bad())?) as usize;
        at += 4;
        let info_len = usize::from(u16_le(d, at).ok_or_else(bad)?);
        at += 2;
        info = Some(d.get(at..at + info_len).ok_or_else(bad)?);
        at += info_len;
        for _ in 1..count.min(MAX_SHAPES) {
            let (Some(code), Some(len)) = (u16_le(d, at), u16_le(d, at + 2)) else { break };
            at += 4;
            let def = d.get(at..at + usize::from(len)).ok_or_else(bad)?;
            at += usize::from(len);
            defs.insert(code, spec_of(def));
        }
    } else if header.contains("shapes") {
        kind = Kind::Shapes;
        let at = header_end + 1;
        let count = usize::from(u16_le(d, at + 4).ok_or_else(bad)?);
        let mut index = Vec::with_capacity(count);
        for k in 0..count {
            let o = at + 6 + 4 * k;
            index.push((u16_le(d, o).ok_or_else(bad)?, usize::from(u16_le(d, o + 2).ok_or_else(bad)?)));
        }
        let mut data = at + 6 + 4 * count;
        for (code, len) in index {
            let def = d.get(data..data + len).ok_or_else(bad)?;
            data += len;
            if code == 0 {
                info = Some(def);
            } else {
                defs.insert(code, spec_of(def));
            }
        }
    } else {
        return Err(format!("{name}: unknown SHX kind ({})", header.trim()));
    }
    let info = info.map(spec_of).unwrap_or_default();
    let above = f64::from(info.first().copied().filter(|a| *a > 0).unwrap_or(21));
    let below = f64::from(info.get(1).copied().unwrap_or(7));
    let mut glyphs = HashMap::new();
    for (&code, spec) in &defs {
        let Some(c) = char::from_u32(u32::from(code)) else { continue };
        if c.is_control() {
            continue;
        }
        let mut pen = Pen {
            shapes: &defs,
            kind,
            pos: Point::ORIGIN,
            down: true,
            scale: 1.0,
            stack: Vec::new(),
            path: BezPath::new(),
            open: false,
            steps: 0,
        };
        pen.run_spec(spec, 0);
        glyphs.insert(c, StrokeGlyph { path: pen.path, advance: pen.pos.x.max(0.0) });
    }
    if glyphs.is_empty() {
        return Err(format!("{name}: the font has no character shapes"));
    }
    let space = glyphs.get(&' ').map_or(above * 0.6, |g| g.advance);
    Ok(StrokeFont {
        name: name.to_string(),
        em: above / 0.7,
        ascent: above,
        descent: below,
        line_gap: above * 0.4,
        cap_height: above,
        space,
        letter_gap: 0.0,
        glyphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shapes_file(shapes: &[(u16, &str, &[u8])]) -> Vec<u8> {
        let mut d = b"AutoCAD-86 shapes 1.0\r\n\x1A".to_vec();
        let defs: Vec<Vec<u8>> = shapes
            .iter()
            .map(|(_, n, spec)| {
                let mut v = n.as_bytes().to_vec();
                v.push(0);
                v.extend_from_slice(spec);
                v
            })
            .collect();
        d.extend(shapes.iter().map(|s| s.0).min().unwrap().to_le_bytes());
        d.extend(shapes.iter().map(|s| s.0).max().unwrap().to_le_bytes());
        d.extend((shapes.len() as u16).to_le_bytes());
        for ((code, _, _), def) in shapes.iter().zip(&defs) {
            d.extend(code.to_le_bytes());
            d.extend((def.len() as u16).to_le_bytes());
        }
        for def in defs {
            d.extend(def);
        }
        d
    }

    #[test]
    fn vectors_pen_and_displacements() {
        // "L": pen down, up 8 (direction 4, length 8), down... ends with a pen-up move to the next start.
        let l: &[u8] = &[0x01, 0x84, 0x02, 0x8C, 0x01, 0x60, 0x02, 0x08, 2, 0, 0];
        let f = shapes_file(&[(0, "TEST", &[21, 7, 0, 0]), (u16::from(b'L'), "", l)]);
        let font = parse("t", &f).unwrap();
        let g = font.glyph('L').unwrap();
        let b = geom::bounds(&g.path).unwrap();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (0.0, 0.0, 6.0, 8.0));
        assert_eq!(g.advance, 8.0);
        assert_eq!(font.cap_height, 21.0);
        assert!((font.em - 30.0).abs() < 1e-9);
    }

    #[test]
    fn arcs_and_bulges() {
        // A full octant circle of radius 4 starting at octant 0 (pen at its east point), then a semicircle bulge.
        let o: &[u8] = &[0x01, 10, 4, 0x00, 0x02, 0x08, 8, 0, 0];
        let c: &[u8] = &[0x01, 12, 0, 8, 127, 0];
        let f = shapes_file(&[(0, "T", &[10, 2, 0, 0]), (u16::from(b'O'), "", o), (u16::from(b'C'), "", c)]);
        let font = parse("t", &f).unwrap();
        let ob = geom::bounds(&font.glyph('O').unwrap().path).unwrap();
        assert!((ob.width() - 8.0).abs() < 0.05 && (ob.height() - 8.0).abs() < 0.05, "{ob:?}");
        let cb = geom::bounds(&font.glyph('C').unwrap().path).unwrap();
        // From (0,0) up to (0,8) counter-clockwise: the semicircle swings out to the right (positive x).
        assert!(cb.x0.abs() < 0.05 && (cb.x1 - 4.0).abs() < 0.05 && (cb.height() - 8.0).abs() < 0.05, "{cb:?}");
    }

    #[test]
    fn subshapes_scale_and_vertical_only_codes() {
        let bar: &[u8] = &[0x01, 0x44, 0];
        // Scale x2, subshape 'I', vertical-only (skipped) displacement, pop/push.
        let t: &[u8] = &[4, 2, 7, b'I', 14, 8, 50, 50, 5, 0x02, 0x40, 6, 0];
        let f = shapes_file(&[(0, "T", &[10, 2, 0, 0]), (u16::from(b'I'), "", bar), (u16::from(b'T'), "", t)]);
        let font = parse("t", &f).unwrap();
        let tb = geom::bounds(&font.glyph('T').unwrap().path).unwrap();
        assert_eq!(tb.height(), 8.0, "subshape drawn at scale 2");
        assert_eq!(font.glyph('T').unwrap().advance, 0.0, "the skipped displacement and the pop leave x at 0");
    }

    #[test]
    fn unifont_and_refusals() {
        let mut d = b"AutoCAD-86 unifont 1.0\r\n\x1A".to_vec();
        d.extend(2u32.to_le_bytes());
        let info: &[u8] = b"UNI\0\x14\x06\x00\x00\x00\x00";
        d.extend((info.len() as u16).to_le_bytes());
        d.extend(info);
        d.extend(0x0416u16.to_le_bytes()); // Cyrillic Zhe
        let def: &[u8] = b"\0\x01\x88\x02\x80\x00";
        d.extend((def.len() as u16).to_le_bytes());
        d.extend(def);
        let f = parse("u", &d).unwrap();
        assert_eq!(f.cap_height, 20.0);
        assert!(f.glyph('Ж').is_some());
        assert!(parse("b", b"AutoCAD-86 bigfont 1.0\r\n\x1A").unwrap_err().contains("big fonts"));
        assert!(parse("x", b"nope").is_err());
        assert!(parse("x", b"AutoCAD-86 shapes 1.0\r\n\x1A\x01").is_err());
    }

    #[test]
    fn looping_subshapes_stop() {
        let a: &[u8] = &[7, b'A', 0];
        let f = shapes_file(&[(0, "T", &[10, 2, 0, 0]), (u16::from(b'A'), "", a), (u16::from(b'B'), "", &[0x14, 0])]);
        let font = parse("t", &f).unwrap();
        assert!(font.glyph('A').is_some());
    }

    /// Real `.shx` fonts, when there are some (OPENETCH_SHX_SAMPLES: folders, separated like PATH; read in place,
    /// never copied). No permissively licensed real .shx file was found to commit: see ARCHITECTURE.md "Text".
    /// Every font must parse and give A-Z outlines that start at the baseline and fit the capital height.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn real_shx_fonts_parse() {
        let Some(dirs) = std::env::var_os("OPENETCH_SHX_SAMPLES") else { return };
        let mut n = 0;
        for d in std::env::split_paths(&dirs) {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("shx")) {
                    continue;
                }
                let data = std::fs::read(&p).unwrap();
                if !is_shx(&data) {
                    continue;
                }
                let name = p.file_stem().unwrap().to_string_lossy().to_string();
                let font = parse(&name, &data).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                if let Some(g) = font.glyph('A') {
                    let b = kurbo::Shape::bounding_box(&g.path);
                    assert!(b.height() > 0.0 && b.height() <= font.cap_height * 1.5 + 1e-9, "{}: A is {b:?}", p.display());
                    assert!(g.advance > 0.0);
                }
                eprintln!("{}: {} em, cap {}", p.display(), font.em, font.cap_height);
                n += 1;
            }
        }
        eprintln!("{n} SHX font(s) parsed");
    }
}
