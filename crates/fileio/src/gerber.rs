//! Gerber RS-274X and Excellon drill import (LightBurn 2.2's Gerber PCB import), for PCB isolation and solder-mask
//! work: the copper image becomes closed outlines in mm (fill them on a fill layer to burn the copper area, or
//! `invert` to burn everything else inside the board's bounds).
//!
//! Gerber (written from the Ucamco Gerber Layer Format Specification): `FS` (leading/trailing zero omission,
//! absolute/incremental, integer and decimal digits), `MO` (mm/inch; also the old `G70`/`G71`), standard apertures
//! `C`, `R`, `O`, `P` (with an optional round hole on flashes), `LP` dark/clear polarity (clear areas are cut out of
//! what came before), `G01` lines, `G02`/`G03` arcs in multi-quadrant (`G75`) and single-quadrant (`G74`) mode,
//! `D01` draw, `D02` move, `D03` flash, `Dnn` aperture select (also `G54Dnn`), regions (`G36`/`G37`). Draws are
//! stroked with the aperture's width and round ends (a rectangular aperture draws as round, with a warning).
//! Aperture macros (`AM`), step-and-repeat (`SR`) and block apertures (`AB`) are skipped with a warning.
//!
//! Excellon: `METRIC`/`INCH` (with `LZ`/`TZ`), tool definitions `TnCd`, tool selects and `X..Y..` hits become circles
//! of the tool diameter. Routed slots (`G85`, `G00`/`G01` routing) are skipped with a warning.

use std::collections::BTreeMap;

use geom::{BoolOp, Fill, Region};
use kurbo::{BezPath, Point, Shape, Vec2};

use crate::{Art, FileError, Result, invalid};

/// Largest file read.
pub const MAX_BYTES: usize = 50 * 1024 * 1024;
/// Most drawing operations read (draws, flashes, region segments).
pub const MAX_OPERATIONS: usize = 500_000;
/// Largest coordinate (mm, either sign) and aperture or drill size (mm) read: a board is never metres wide, and
/// outlines of huge or non-finite sizes made the curve flattening and the boolean operations run without end.
pub const MAX_COORD_MM: f64 = 100_000.0;
pub const MAX_APERTURE_MM: f64 = 10_000.0;

fn coord_ok(v: f64) -> Result<f64> {
    if v.is_finite() && v.abs() <= MAX_COORD_MM {
        Ok(v)
    } else {
        invalid(format!("a coordinate ({v}) is outside the {MAX_COORD_MM} mm the import reads"))
    }
}

fn size_ok(v: f64) -> Result<f64> {
    if v.is_finite() && (0.0..=MAX_APERTURE_MM).contains(&v) {
        Ok(v)
    } else {
        invalid(format!("an aperture or drill size ({v}) must be 0..={MAX_APERTURE_MM} mm"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Gerber,
    Excellon,
}

/// Whether `text` is an Excellon drill file rather than Gerber.
pub fn detect(text: &str) -> Kind {
    let head: String = text.chars().take(4000).collect();
    if head.contains("%FS") || head.contains("%MO") || head.contains("%ADD") {
        Kind::Gerber
    } else if head.lines().any(|l| l.trim() == "M48") || head.lines().any(|l| l.trim_start().starts_with("METRIC") || l.trim_start().starts_with("INCH")) {
        Kind::Excellon
    } else {
        Kind::Gerber
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Aperture {
    Circle { d: f64, hole: f64 },
    Rect { w: f64, h: f64, hole: f64 },
    Obround { w: f64, h: f64, hole: f64 },
    Polygon { d: f64, n: u32, rot: f64, hole: f64 },
}

impl Aperture {
    /// The flash outline centred on `c`.
    fn flash(&self, c: Point) -> BezPath {
        let mut p = match *self {
            Aperture::Circle { d, .. } => kurbo::Circle::new(c, d / 2.0).to_path(0.001),
            Aperture::Rect { w, h, .. } => kurbo::Rect::from_center_size(c, (w, h)).to_path(0.001),
            Aperture::Obround { w, h, .. } => {
                kurbo::Rect::from_center_size(c, (w, h)).to_rounded_rect(w.min(h) / 2.0).to_path(0.001)
            }
            Aperture::Polygon { d, n, rot, .. } => {
                let mut p = BezPath::new();
                for i in 0..n.max(3) {
                    let a = (rot + 360.0 * f64::from(i) / f64::from(n.max(3))).to_radians();
                    let q = c + Vec2::new(d / 2.0 * a.cos(), d / 2.0 * a.sin());
                    if i == 0 { p.move_to(q) } else { p.line_to(q) }
                }
                p.close_path();
                p
            }
        };
        let hole = match *self {
            Aperture::Circle { hole, .. } | Aperture::Rect { hole, .. } | Aperture::Obround { hole, .. } | Aperture::Polygon { hole, .. } => hole,
        };
        if hole > 0.0 {
            p.extend(kurbo::Circle::new(c, hole / 2.0).to_path(0.001));
        }
        p
    }

    /// Stroke width of a draw.
    fn width(&self) -> f64 {
        match *self {
            Aperture::Circle { d, .. } | Aperture::Polygon { d, .. } => d,
            Aperture::Rect { w, h, .. } | Aperture::Obround { w, h, .. } => w.min(h),
        }
    }
}

/// One drawn element, in file order.
enum Item {
    Dark(Region),
    Clear(Region),
}

fn geom_err(e: geom::GeomError) -> FileError {
    FileError::Invalid(format!("the copper outline could not be computed ({e})"))
}

/// Coordinate format from `FS`.
#[derive(Debug, Clone, Copy)]
struct Format {
    decimals: u32,
    integers: u32,
    trailing: bool,
    incremental: bool,
}

impl Format {
    fn value(&self, s: &str) -> Option<f64> {
        if s.contains('.') {
            return s.parse().ok();
        }
        let (neg, digits) = match s.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let total = (self.integers + self.decimals) as usize;
        let padded = if self.trailing && digits.len() < total { format!("{digits:0<total$}") } else { digits.to_string() };
        let v: f64 = padded.parse::<f64>().ok()? / 10f64.powi(self.decimals as i32);
        Some(if neg { -v } else { v })
    }
}

/// Points of an arc from `a` to `b` around `c`, `ccw` or clockwise; a full circle when `a == b`.
fn arc(a: Point, b: Point, c: Point, ccw: bool) -> Vec<Point> {
    let r = (a - c).hypot();
    let a0 = (a.y - c.y).atan2(a.x - c.x);
    let a1 = (b.y - c.y).atan2(b.x - c.x);
    let mut sweep = a1 - a0;
    let tau = std::f64::consts::TAU;
    if ccw {
        while sweep <= 1e-9 {
            sweep += tau;
        }
    } else {
        while sweep >= -1e-9 {
            sweep -= tau;
        }
    }
    let n = ((sweep.abs() * r.max(0.01) / 0.02).ceil() as usize).clamp(4, 2000);
    (1..=n)
        .map(|i| {
            let t = a0 + sweep * i as f64 / n as f64;
            c + Vec2::new(r * t.cos(), r * t.sin())
        })
        .collect()
}

/// Parse a Gerber file into copper outlines.
pub fn parse_gerber(text: &str, invert: bool) -> Result<Art> {
    if text.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the Gerber file is larger than 50 MB".into()));
    }
    let mut art = Art::default();
    let mut fmt = Format { decimals: 4, integers: 2, trailing: false, incremental: false };
    let mut unit = 1.0; // mm per file unit
    let mut apertures: BTreeMap<u32, Aperture> = BTreeMap::new();
    let mut current: Option<u32> = None;
    let mut pos = Point::ZERO;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let mut interp = 1u8; // 1 linear, 2 cw, 3 ccw
    let mut multi_quadrant = true;
    let mut dark = true;
    let mut in_region = false;
    let mut region: Vec<Vec<Point>> = Vec::new();
    let mut contour: Vec<Point> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    // Consecutive dark draws and flashes, unioned in one go.
    let mut strokes: Vec<Region> = Vec::new();
    let mut flashes: Vec<BezPath> = Vec::new();
    let mut ops = 0usize;
    let mut rect_draw = false;
    let mut found_any = false;

    // Close the pending shapes of the current polarity into an item.
    let flush = |strokes: &mut Vec<Region>, flashes: &mut Vec<BezPath>, items: &mut Vec<Item>, dark: bool| -> Result<()> {
        if strokes.is_empty() && flashes.is_empty() {
            return Ok(());
        }
        let mut parts = std::mem::take(strokes);
        if !flashes.is_empty() {
            parts.push(Region::from_paths(flashes, Fill::EvenOdd).map_err(geom_err)?);
            flashes.clear();
        }
        let r = Region::union_all(parts).map_err(geom_err)?;
        items.push(if dark { Item::Dark(r) } else { Item::Clear(r) });
        Ok(())
    };

    // Split into extended commands (%...%) and data blocks (...*).
    let mut rest = text;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after) = rest.strip_prefix('%') {
            let end = after.find('%').ok_or_else(|| FileError::Invalid("an extended command (%) is not closed".into()))?;
            let body = &after[..end];
            rest = &after[end + 1..];
            for cmd in body.split('*').map(str::trim).filter(|c| !c.is_empty()) {
                found_any = true;
                if let Some(f) = cmd.strip_prefix("FS") {
                    fmt.trailing = f.starts_with('T');
                    fmt.incremental = f.get(1..2) == Some("I");
                    if let Some(xi) = f.find('X') {
                        let d = f.as_bytes();
                        if let (Some(i), Some(dd)) = (d.get(xi + 1), d.get(xi + 2)) {
                            fmt.integers = u32::from(i.saturating_sub(b'0'));
                            fmt.decimals = u32::from(dd.saturating_sub(b'0')).min(9);
                        }
                    }
                } else if let Some(m) = cmd.strip_prefix("MO") {
                    unit = if m.starts_with("IN") { 25.4 } else { 1.0 };
                } else if let Some(ad) = cmd.strip_prefix("ADD") {
                    let num_end = ad.find(|c: char| !c.is_ascii_digit()).unwrap_or(ad.len());
                    let Ok(code) = ad[..num_end].parse::<u32>() else { continue };
                    let def = &ad[num_end..];
                    let (shape, params) = def.split_once(',').unwrap_or((def, ""));
                    let p: Vec<f64> = params.split('X').filter_map(|v| v.trim().parse::<f64>().ok()).map(|v| v * unit).collect();
                    let get = |i: usize| p.get(i).copied().unwrap_or(0.0);
                    // Sizes (not the polygon's vertex count and rotation) must be usable.
                    let sizes: &[usize] = match shape {
                        "P" => &[0, 3],
                        "C" | "R" | "O" => &[0, 1, 2],
                        _ => &[],
                    };
                    for &i in sizes {
                        size_ok(get(i))?;
                    }
                    let ap = match shape {
                        "C" => Some(Aperture::Circle { d: get(0), hole: get(1) }),
                        "R" => Some(Aperture::Rect { w: get(0), h: get(1), hole: get(2) }),
                        "O" => Some(Aperture::Obround { w: get(0), h: get(1), hole: get(2) }),
                        "P" => {
                            // The specification allows 3 to 12 vertices.
                            let n = get(1) / unit;
                            if !(3.0..=12.0).contains(&n) {
                                return invalid(format!("polygon aperture D{code} has {n} vertices (3 to 12 allowed)"));
                            }
                            let rot = get(2) / unit;
                            Some(Aperture::Polygon { d: get(0), n: n as u32, rot: if rot.is_finite() { rot } else { 0.0 }, hole: get(3) })
                        }
                        _ => {
                            art.warn(format!("aperture macros are not supported: D{code} ({shape}) is drawn as a 0.1 mm circle"));
                            Some(Aperture::Circle { d: 0.1, hole: 0.0 })
                        }
                    };
                    if let Some(a) = ap {
                        apertures.insert(code, a);
                    }
                } else if let Some(lp) = cmd.strip_prefix("LP") {
                    let next_dark = !lp.starts_with('C');
                    if next_dark != dark {
                        flush(&mut strokes, &mut flashes, &mut items, dark)?;
                        dark = next_dark;
                    }
                } else if cmd.starts_with("AM") {
                    // Macro bodies follow on the same %...% block; they are skipped with it.
                    break;
                } else if cmd.starts_with("SR") && cmd.len() > 2 {
                    art.warn("step and repeat (SR) is not supported: one copy imported");
                } else if cmd.starts_with("AB") {
                    art.warn("block apertures (AB) are not supported");
                }
            }
            continue;
        }
        let end = rest.find('*').unwrap_or(rest.len());
        let block = rest[..end].trim();
        rest = rest.get(end + 1..).unwrap_or("");
        if block.is_empty() {
            continue;
        }
        if block.starts_with("G04") || block.starts_with("G4 ") {
            continue;
        }
        found_any = true;
        // Words: a letter then a number.
        let mut d_code: Option<u32> = None;
        let (mut i_off, mut j_off) = (0.0f64, 0.0f64);
        let mut has_xy = false;
        let b = block.as_bytes();
        let mut k = 0;
        while k < b.len() {
            let letter = b[k];
            let s = k + 1;
            let mut e = s;
            while e < b.len() && (b[e].is_ascii_digit() || matches!(b[e], b'-' | b'+' | b'.')) {
                e += 1;
            }
            let word = &block[s..e];
            k = e.max(k + 1);
            match letter {
                b'G' => match word.parse::<u32>().unwrap_or(999) {
                    1 => interp = 1,
                    2 => interp = 2,
                    3 => interp = 3,
                    36 => {
                        in_region = true;
                        region.clear();
                        contour.clear();
                    }
                    37 => {
                        in_region = false;
                        if contour.len() >= 3 {
                            region.push(std::mem::take(&mut contour));
                        }
                        let mut p = BezPath::new();
                        for c in region.drain(..) {
                            p.move_to(c[0]);
                            for q in &c[1..] {
                                p.line_to(*q);
                            }
                            p.close_path();
                        }
                        if !p.elements().is_empty() {
                            flashes.push(p);
                        }
                        contour.clear();
                    }
                    70 => unit = 25.4,
                    71 => unit = 1.0,
                    74 => multi_quadrant = false,
                    75 => multi_quadrant = true,
                    90 => fmt.incremental = false,
                    91 => fmt.incremental = true,
                    _ => {}
                },
                b'X' | b'Y' | b'I' | b'J' => {
                    let v = coord_ok(fmt.value(word).ok_or_else(|| FileError::Invalid(format!("bad coordinate {}{word}", letter as char)))? * unit)?;
                    match letter {
                        b'X' => {
                            x = coord_ok(if fmt.incremental { x + v } else { v })?;
                            has_xy = true;
                        }
                        b'Y' => {
                            y = coord_ok(if fmt.incremental { y + v } else { v })?;
                            has_xy = true;
                        }
                        b'I' => i_off = v,
                        _ => j_off = v,
                    }
                }
                b'D' => d_code = word.parse().ok(),
                b'M' => {
                    if matches!(word, "02" | "2" | "00" | "0") {
                        rest = "";
                    }
                }
                _ => {}
            }
        }
        let target = Point::new(x, y);
        let code = match d_code {
            Some(c) if c >= 10 => {
                current = Some(c);
                continue;
            }
            Some(c) => c,
            None if has_xy => 1, // deprecated modal D01
            None => continue,
        };
        ops += 1;
        if ops > MAX_OPERATIONS {
            return Err(FileError::TooLarge(format!("more than {MAX_OPERATIONS} drawing operations")));
        }
        match code {
            2 => {
                if in_region && contour.len() >= 3 {
                    region.push(std::mem::take(&mut contour));
                }
                contour.clear();
                pos = target;
            }
            1 => {
                let pts: Vec<Point> = if interp == 1 {
                    vec![target]
                } else {
                    let ccw = interp == 3;
                    let center = if multi_quadrant {
                        pos + Vec2::new(i_off, j_off)
                    } else {
                        // Single quadrant: unsigned offsets; the sign pair with equal radii and a sweep of 90° or less.
                        let mut best = (f64::INFINITY, pos);
                        for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                            let c = pos + Vec2::new(sx * i_off.abs(), sy * j_off.abs());
                            let err = ((pos - c).hypot() - (target - c).hypot()).abs();
                            let a0 = (pos.y - c.y).atan2(pos.x - c.x);
                            let a1 = (target.y - c.y).atan2(target.x - c.x);
                            let mut sw = if ccw { a1 - a0 } else { a0 - a1 };
                            sw = sw.rem_euclid(std::f64::consts::TAU);
                            if sw <= std::f64::consts::FRAC_PI_2 + 1e-6 && err < best.0 {
                                best = (err, c);
                            }
                        }
                        best.1
                    };
                    if !multi_quadrant && (pos - target).hypot() < 1e-9 {
                        vec![target]
                    } else {
                        arc(pos, target, center, ccw)
                    }
                };
                if in_region {
                    if contour.is_empty() {
                        contour.push(pos);
                    }
                    contour.extend(pts);
                } else {
                    let ap = current.and_then(|c| apertures.get(&c)).cloned().ok_or_else(|| FileError::Invalid("a draw uses no aperture".into()))?;
                    if matches!(ap, Aperture::Rect { .. }) {
                        rect_draw = true;
                    }
                    let mut line = vec![pos];
                    line.extend(pts);
                    let w = ap.width();
                    if w > 0.0 {
                        let r = geom::offset::offset_polyline(&line, w / 2.0, geom::offset::Join::Round).map_err(geom_err)?;
                        strokes.push(r);
                    }
                }
                pos = target;
            }
            3 => {
                let ap = current.and_then(|c| apertures.get(&c)).ok_or_else(|| FileError::Invalid("a flash uses no aperture".into()))?;
                flashes.push(ap.flash(target));
                pos = target;
            }
            _ => {}
        }
    }
    if !found_any {
        return invalid("this is not a Gerber file");
    }
    flush(&mut strokes, &mut flashes, &mut items, dark)?;
    if rect_draw {
        art.warn("draws with rectangular apertures are drawn with round ends");
    }
    let mut copper = Region::default();
    for it in items {
        copper = match it {
            Item::Dark(r) => copper.boolean(&r, BoolOp::Union).map_err(geom_err)?,
            Item::Clear(r) => copper.boolean(&r, BoolOp::Difference).map_err(geom_err)?,
        };
    }
    let result = if invert {
        let Some(b) = copper.bounds() else { return invalid("the Gerber file has no copper") };
        let board = Region::from_path(&b.inflate(1.0, 1.0).to_path(0.001), Fill::EvenOdd).map_err(geom_err)?;
        board.boolean(&copper, BoolOp::Difference).map_err(geom_err)?
    } else {
        copper
    };
    for island in result.islands() {
        art.push(island, if invert { "isolation" } else { "copper" }, None);
    }
    if art.paths.is_empty() {
        return invalid("the Gerber file draws no copper");
    }
    Ok(art)
}

/// Parse an Excellon drill file: one circle per hole.
pub fn parse_excellon(text: &str) -> Result<Art> {
    if text.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the drill file is larger than 50 MB".into()));
    }
    let mut art = Art::default();
    let mut unit = 25.4;
    let mut trailing_zeros_kept = false; // "LZ": leading zeros kept, trailing omitted
    let mut tools: BTreeMap<u32, f64> = BTreeMap::new();
    let mut tool: Option<u32> = None;
    let (mut x, mut y) = (0.0, 0.0);
    let mut holes: BTreeMap<u32, BezPath> = BTreeMap::new();
    let mut routed = false;
    let mut n = 0usize;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with(';') {
            continue;
        }
        if l.starts_with("METRIC") {
            unit = 1.0;
            trailing_zeros_kept = l.contains("LZ");
            continue;
        }
        if l.starts_with("INCH") {
            unit = 25.4;
            trailing_zeros_kept = l.contains("LZ");
            continue;
        }
        if l == "M71" {
            unit = 1.0;
            continue;
        }
        if l == "M72" {
            unit = 25.4;
            continue;
        }
        if l.starts_with("G85") || l.starts_with("G00") || l.starts_with("G01") || l.starts_with("M15") {
            routed = true;
            continue;
        }
        if let Some(t) = l.strip_prefix('T') {
            let num_end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            let Ok(code) = t[..num_end].parse::<u32>() else { continue };
            if let Some(ci) = t[num_end..].find('C') {
                let d: String = t[num_end + ci + 1..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
                if let Ok(d) = d.parse::<f64>() {
                    tools.insert(code, size_ok(d * unit)?);
                }
            } else {
                tool = (code != 0).then_some(code);
            }
            continue;
        }
        if l.starts_with('X') || l.starts_with('Y') {
            let coord = |s: &str| -> Option<f64> {
                if s.contains('.') {
                    return s.parse::<f64>().ok();
                }
                // No decimal point: inch 2.4, mm 3.3 digits; LZ keeps leading zeros (pad on the right).
                let (int_digits, dec_digits) = if unit == 1.0 { (3usize, 3i32) } else { (2usize, 4i32) };
                let (neg, d) = match s.strip_prefix('-') {
                    Some(d) => (true, d),
                    None => (false, s.strip_prefix('+').unwrap_or(s)),
                };
                let total = int_digits + dec_digits as usize;
                let padded = if trailing_zeros_kept { format!("{d:0<total$}") } else { d.to_string() };
                let v = padded.parse::<f64>().ok()? / 10f64.powi(dec_digits);
                Some(if neg { -v } else { v })
            };
            let xi = l.find('X');
            let yi = l.find('Y');
            if let Some(xi) = xi {
                let end = yi.filter(|&y| y > xi).unwrap_or(l.len());
                if let Some(v) = coord(&l[xi + 1..end]) {
                    x = coord_ok(v * unit)?;
                }
            }
            if let Some(yi) = yi {
                let s: String = l[yi + 1..].chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')).collect();
                if let Some(v) = coord(&s) {
                    y = coord_ok(v * unit)?;
                }
            }
            let Some(t) = tool else { continue };
            let d = tools.get(&t).copied().unwrap_or(0.8);
            n += 1;
            if n > MAX_OPERATIONS {
                return Err(FileError::TooLarge(format!("more than {MAX_OPERATIONS} holes")));
            }
            holes.entry(t).or_default().extend(kurbo::Circle::new((x, y), d / 2.0).to_path(0.001));
        }
    }
    if routed {
        art.warn("routed slots (G85, G00/G01) are not imported");
    }
    for (t, p) in holes {
        art.push(p, format!("drill T{t}"), None);
    }
    if art.paths.is_empty() {
        return invalid("the drill file has no holes");
    }
    Ok(art)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = "G04 test*\n%FSLAX24Y24*%\n%MOMM*%\n%ADD10C,1.0*%\n%ADD11R,2.0X2.0*%\nD10*\nX0Y0D02*\nX100000Y0D01*\nD11*\nX200000Y0D03*\nG36*\nX0Y50000D02*\nX50000Y50000D01*\nX50000Y100000D01*\nX0Y100000D01*\nX0Y50000D01*\nG37*\nM02*\n";

    #[test]
    fn tracks_pads_and_regions_become_copper_islands() {
        let art = parse_gerber(BOARD, false).unwrap();
        // The track and the pad touch (track ends at x 10.5, pad from 19): two separate islands plus the region.
        assert_eq!(art.paths.len(), 3, "{:?}", art.paths.iter().map(|p| p.path.bounding_box()).collect::<Vec<_>>());
        let areas: f64 = art.paths.iter().map(|p| p.path.area().abs()).sum();
        // Track 10 x 1 + two half discs (π/4), pad 4, region 25.
        assert!((areas - (10.0 + std::f64::consts::FRAC_PI_4 + 4.0 + 25.0)).abs() < 0.05, "{areas}");
    }

    #[test]
    fn clear_polarity_cuts_and_invert_burns_the_rest() {
        let text = "%FSLAX24Y24*%%MOMM*%%ADD10R,4X4*%%ADD11C,1*%D10*X0Y0D03*%LPC*%D11*X0Y0D03*%LPD*%M02*";
        let art = parse_gerber(text, false).unwrap();
        let area: f64 = art.paths.iter().map(|p| p.path.area().abs()).sum();
        let expected = 16.0 - std::f64::consts::FRAC_PI_4;
        // A pad with a hole: one island whose outline holds the hole (area is the signed sum).
        assert!((area - expected).abs() < 0.05 || (art.paths[0].path.area().abs() - expected).abs() < 0.05, "{area}");
        let inv = parse_gerber(text, true).unwrap();
        assert!(inv.paths.iter().all(|p| p.label == "isolation"));
    }

    #[test]
    fn inch_units_trailing_zero_format_and_arcs() {
        // A full circle of radius 0.1 in, drawn with a hairline.
        let text = "%FSLAX24Y24*%%MOIN*%%ADD10C,0.001*%D10*G75*X1000Y0D02*G03X1000Y0I-1000J0D01*M02*";
        let art = parse_gerber(text, false).unwrap();
        let b = art.paths[0].path.bounding_box();
        assert!((b.width() - 2.0 * 2.54 - 0.0254).abs() < 0.01, "{b:?}");
        let f = Format { decimals: 4, integers: 2, trailing: true, incremental: false };
        assert_eq!(f.value("12"), Some(12.0));
        assert_eq!(f.value("-0015"), Some(-0.15));
        assert!(parse_gerber("hello world", false).is_err());
    }

    /// Huge, non-finite or absurd sizes are refused before they reach curve flattening (a circle of radius 1e300
    /// flattened to 0.001 mm never ends) and a 4-billion-vertex polygon is never built.
    #[test]
    fn hostile_sizes_are_refused_quickly() {
        let t = std::time::Instant::now();
        for bad in [
            "%FSLAX24Y24*%%MOMM*%%ADD10C,1e300*%D10*X0Y0D03*M02*",
            "%FSLAX24Y24*%%MOMM*%%ADD10C,inf*%D10*X0Y0D03*M02*",
            "%FSLAX24Y24*%%MOMM*%%ADD10C,NaN*%D10*X0Y0D03*M02*",
            "%FSLAX24Y24*%%MOMM*%%ADD10P,1X4000000000*%D10*X0Y0D03*M02*",
            "%FSLAX24Y24*%%MOMM*%%ADD10O,1X1e200*%D10*X0Y0D03*M02*",
        ] {
            assert!(parse_gerber(bad, false).is_err(), "{bad}");
        }
        let far = format!("%FSLAX24Y24*%%MOMM*%%ADD10C,1*%D10*X{}Y0D03*M02*", "9".repeat(400));
        assert!(parse_gerber(&far, false).is_err(), "an infinite coordinate");
        let incremental = format!("%FSLIX24Y24*%%MOMM*%%ADD10C,1*%D10*{}M02*", "X999999999Y0D03*".repeat(200));
        assert!(parse_gerber(&incremental, false).is_err(), "incremental coordinates add up past the limit");
        assert!(parse_excellon(&format!("M48\nMETRIC\nT1C{}\n%\nT1\nX0Y0\n", "9".repeat(300))).is_err());
        assert!(parse_excellon(&format!("M48\nMETRIC\nT1C1.0\n%\nT1\nX{}.0Y0\n", "9".repeat(320))).is_err());
        assert!(t.elapsed().as_secs() < 10, "{:?}", t.elapsed());
        // A polygon aperture keeps working.
        assert!(parse_gerber("%FSLAX24Y24*%%MOMM*%%ADD10P,2X6X15*%D10*X0Y0D03*M02*", false).is_ok());
    }

    #[test]
    fn excellon_holes() {
        let text = "M48\nMETRIC,TZ\nT1C0.800\nT2C1.000\n%\nT1\nX10.0Y5.0\nX20.0Y5.0\nT2\nX0Y0\nG85X1Y1\nM30\n";
        assert_eq!(detect(text), Kind::Excellon);
        let art = parse_excellon(text).unwrap();
        assert_eq!(art.paths.len(), 2);
        assert_eq!(art.paths[0].label, "drill T1");
        let b = art.paths[0].path.bounding_box();
        assert!((b.x0 - 9.6).abs() < 1e-6 && (b.x1 - 20.4).abs() < 1e-6, "{b:?}");
        assert!(!art.warnings.is_empty());
        assert_eq!(detect(BOARD), Kind::Gerber);
    }
}
