//! EzCad2 `.ezd` import (vectors only), for galvo users moving from EzCad. Not in LightBurn.
//!
//! The layout follows meerk40t's EZD reader (`meerk40t/extra/ezd.py`, https://github.com/meerk40t/meerk40t, MIT,
//! Copyright (c) meerk40t contributors), re-written here and checked against the owner's EzCad 2 files:
//!
//! - Header: `EZCADUNI` in UTF-16 (16 bytes), two `i32` (0, 2001), three 60-byte UTF-16 strings, 140 bytes; then
//!   seven `i32` absolute offsets (preview, v1, pens, font, v4, vectors, prevectors).
//! - Pens: `i32` count, `i32` offset, then per pen a "struct" (`i32` item count, then `i32` length + bytes per item);
//!   item 0 is the colour (BGR), 1 the name.
//! - Vectors: five `u32` (the first the uncompressed length), a `u16` Huffman table size and 7-byte entries
//!   (`u8` symbol, `u32` code, `u16` code length: the low bits of the code, most significant first), then the
//!   bit stream, most significant bit first.
//! - The decoded stream is a list of objects: `i32` type (0 ends a list), a 15-item header struct (item 0 the pen,
//!   item 1 flags: 0x01 hidden), then per type: curves (`u32` segment count, `u32` closed, per segment `u8`, `u8` kind
//!   (0: 40 bytes skipped; 1 lines, 2 quadratic, 3 cubic), two `u16`, `i32` point count and the points as `f64`
//!   pairs), containers (group 0x10, combine 0x30, vector file 0x50, hatch 0x20, spiral 0x60: an `i32` child count
//!   and the children), and struct-described shapes (rectangle 0x03, circle 0x04, ellipse 0x05, polygon 0x06)
//!   whose items are typed by size (8 bytes `f64`, 16 a point, 72 a 3 x 3 matrix).
//!
//! Coordinates are mm with the origin at the field centre and +Y up, as OpenEtch's field frame. Text (0x800) and
//! images (0x40) are skipped with a warning (EzCad text has no outline in the file); hatch settings are not read
//! (the outlines a hatch fills are imported). Unknown objects are skipped by scanning for the next object.

use std::collections::HashMap;

use kurbo::{Affine, BezPath, Point, Shape};

use crate::{Art, FileError, Result, invalid};

/// Largest file read.
pub const MAX_BYTES: usize = 200 * 1024 * 1024;
/// Largest decoded vector stream.
const MAX_DECODED: usize = 256 * 1024 * 1024;
/// Most objects read.
const MAX_OBJECTS: usize = 200_000;
/// Largest coordinate or radius of a rectangle, circle, ellipse or polygon read (mm).
const MAX_SHAPE_MM: f64 = 100_000.0;

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.i32().map(|v| v as u32)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take(8).map(|s| f64::from_le_bytes(s.try_into().unwrap_or([0; 8])))
    }
    /// A struct: `i32` count, then (`i32` length, bytes) per item.
    fn items(&mut self) -> Option<Vec<&'a [u8]>> {
        let n = self.i32()?;
        if !(0..=10_000).contains(&n) {
            return None;
        }
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let len = self.i32()?;
            if !(0..=1 << 20).contains(&len) {
                return None;
            }
            out.push(self.take(len as usize)?);
        }
        Some(out)
    }
}

fn utf16(b: &[u8]) -> String {
    let u: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&u).trim_end_matches('\0').to_string()
}

fn as_f64(b: &[u8]) -> Option<f64> {
    (b.len() == 8).then(|| f64::from_le_bytes(b.try_into().unwrap_or([0; 8])))
}

fn as_i32(b: &[u8]) -> Option<i32> {
    (b.len() == 4).then(|| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn as_point(b: &[u8]) -> Option<Point> {
    (b.len() == 16).then(|| Point::new(f64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8])), f64::from_le_bytes(b[8..].try_into().unwrap_or([0; 8]))))
}

/// A 3 x 3 matrix (row-major, translation in the last row) as an affine map.
fn as_matrix(b: &[u8]) -> Option<Affine> {
    if b.len() != 72 {
        return None;
    }
    let m: Vec<f64> = b.as_chunks::<8>().0.iter().map(|c| f64::from_le_bytes(*c)).collect();
    let a = Affine::new([m[0], m[1], m[3], m[4], m[6], m[7]]);
    (a.as_coeffs().iter().all(|v| v.is_finite()) && a.determinant().abs() > 1e-12).then_some(a)
}

/// Decode the Huffman-coded vector stream at `at`.
fn decode_vectors(b: &[u8], at: usize) -> Result<Vec<u8>> {
    let mut r = Reader { b, at };
    let bad = || FileError::Invalid("the EZD vector section is damaged".into());
    let total = r.u32().ok_or_else(bad)? as usize;
    r.take(16).ok_or_else(bad)?;
    if total > MAX_DECODED {
        return Err(FileError::TooLarge("the EZD drawing is too large".into()));
    }
    let n = r.u16().ok_or_else(bad)?;
    // Decode tree: nodes of [child 0, child 1], leaves as symbol + 1 << 16.
    let mut tree: Vec<[u32; 2]> = vec![[0, 0]];
    for _ in 0..n {
        let sym = r.u8().ok_or_else(bad)?;
        let code = r.u32().ok_or_else(bad)?;
        let len = r.u16().ok_or_else(bad)?;
        if len == 0 || len > 32 {
            return Err(bad());
        }
        let mut node = 0usize;
        for k in (0..len).rev() {
            let bit = ((code >> k) & 1) as usize;
            if k == 0 {
                tree[node][bit] = (1 << 16) | u32::from(sym);
            } else {
                let next = tree[node][bit];
                if next == 0 {
                    tree.push([0, 0]);
                    let id = (tree.len() - 1) as u32;
                    tree[node][bit] = id;
                    node = id as usize;
                } else if next >= 1 << 16 {
                    return Err(bad());
                } else {
                    node = next as usize;
                }
            }
        }
    }
    // The declared size is only a hint: the data can hold at most 8 symbols per byte.
    let mut out = Vec::with_capacity(total.min(b.len().saturating_sub(r.at).saturating_mul(8)));
    let mut node = 0usize;
    'bytes: for &byte in &b[r.at..] {
        for k in (0..8).rev() {
            let next = tree[node][usize::from((byte >> k) & 1)];
            if next >= 1 << 16 {
                out.push((next & 0xFF) as u8);
                node = 0;
                if out.len() >= total {
                    break 'bytes;
                }
            } else if next == 0 {
                return Err(bad());
            } else {
                node = next as usize;
            }
        }
    }
    Ok(out)
}

const CURVE: i32 = 0x01;
const RECT: i32 = 0x03;
const CIRCLE: i32 = 0x04;
const ELLIPSE: i32 = 0x05;
const POLYGON: i32 = 0x06;
const GROUP: i32 = 0x10;
const HATCH: i32 = 0x20;
const COMBINE: i32 = 0x30;
const IMAGE: i32 = 0x40;
const VECTOR_FILE: i32 = 0x50;
const SPIRAL: i32 = 0x60;
const TEXT: i32 = 0x800;
const KNOWN: [i32; 12] = [CURVE, RECT, CIRCLE, ELLIPSE, POLYGON, GROUP, HATCH, COMBINE, IMAGE, VECTOR_FILE, SPIRAL, TEXT];

struct Parse<'a> {
    art: Art,
    pens: &'a [(String, [u8; 3])],
    objects: usize,
    skipped: HashMap<&'static str, usize>,
}

impl Parse<'_> {
    fn push(&mut self, path: BezPath, pen: usize) {
        let (name, color) = self.pens.get(pen).cloned().unwrap_or_else(|| (format!("Pen {pen}"), [0, 0, 0]));
        self.art.push(path, name, Some(color));
    }

    /// Parse one list of objects (until a 0 type or the end). `top` allows resyncing after unknown data.
    fn list(&mut self, r: &mut Reader, top: bool, depth: usize) {
        // After a hatch, its generated lines may follow as plain two-point curves (seen in the owner's files):
        // they are dropped, since a fill layer hatches the outlines again.
        let mut after_hatch = false;
        let line = |p: &BezPath| p.elements().len() == 2 && matches!(p.elements()[1], kurbo::PathEl::LineTo(_));
        while r.at < r.b.len() && self.objects < MAX_OBJECTS {
            let start = r.at;
            let Some(kind) = r.i32() else { return };
            if kind == 0 {
                if top {
                    continue;
                }
                return;
            }
            let before = self.art.paths.len();
            let ok = KNOWN.contains(&kind) && self.object(r, kind, depth).is_some();
            if ok && after_hatch && kind == CURVE && self.art.paths.len() == before + 1 && line(&self.art.paths[before].path) {
                self.art.paths.pop();
                *self.skipped.entry("hatch lines").or_default() += 1;
            } else {
                after_hatch = ok && (kind == HATCH || (after_hatch && kind == CURVE));
            }
            if !ok {
                if !top {
                    return;
                }
                // Resync: the next known type followed by a plausible header.
                r.at = start + 1;
                if !self.resync(r) {
                    return;
                }
            }
        }
    }

    fn resync(&self, r: &mut Reader) -> bool {
        let limit = (r.at + 100_000).min(r.b.len().saturating_sub(8));
        while r.at < limit {
            let k = i32::from_le_bytes(r.b[r.at..r.at + 4].try_into().unwrap_or([0; 4]));
            let n = i32::from_le_bytes(r.b[r.at + 4..r.at + 8].try_into().unwrap_or([0; 4]));
            if KNOWN.contains(&k) && n == 15 {
                return true;
            }
            r.at += 1;
        }
        false
    }

    fn object(&mut self, r: &mut Reader, kind: i32, depth: usize) -> Option<()> {
        if depth > 64 {
            return None;
        }
        self.objects += 1;
        let header = r.items()?;
        let pen = header.first().and_then(|b| as_i32(b)).unwrap_or(0).max(0) as usize;
        let hidden = header.get(1).and_then(|b| as_i32(b)).is_some_and(|f| f & 1 != 0);
        let container = |this: &mut Self, r: &mut Reader| -> Option<()> {
            let n = r.i32()?;
            let n = if (0..=10_000).contains(&n) { n } else { 0 };
            for _ in 0..n {
                let k = r.i32()?;
                if !KNOWN.contains(&k) {
                    return None;
                }
                this.object(r, k, depth + 1)?;
            }
            Some(())
        };
        match kind {
            CURVE => {
                let count = r.u32()?;
                let closed = r.u32()? != 0;
                let mut path = BezPath::new();
                let mut last: Option<Point> = None;
                for _ in 0..count.min(1_000_000) {
                    let _ = r.u8()?;
                    let seg = r.u8()?;
                    let _ = r.u16()?;
                    let _ = r.u16()?;
                    if seg == 0 {
                        r.take(40)?;
                        continue;
                    }
                    let n = r.i32()?;
                    if !(0..=1_000_000).contains(&n) {
                        return None;
                    }
                    let mut pts = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        let (x, y) = (r.f64()?, r.f64()?);
                        pts.push(Point::new(x, y));
                    }
                    if !pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()) || pts.is_empty() {
                        continue;
                    }
                    let first = pts[0];
                    if last.is_none_or(|l| (l - first).hypot() > 1e-9) {
                        path.move_to(first);
                    }
                    match seg {
                        2 => {
                            for [a, p] in pts[1..].as_chunks::<2>().0 {
                                path.quad_to(*a, *p);
                            }
                        }
                        3 => {
                            for [a, b, p] in pts[1..].as_chunks::<3>().0 {
                                path.curve_to(*a, *b, *p);
                            }
                        }
                        _ => {
                            for p in &pts[1..] {
                                path.line_to(*p);
                            }
                        }
                    }
                    last = pts.last().copied();
                }
                if closed {
                    path.close_path();
                }
                if !hidden {
                    self.push(path, pen);
                }
            }
            RECT | CIRCLE | ELLIPSE | POLYGON => {
                let args = r.items()?;
                let m = args.iter().rev().find_map(|b| as_matrix(b)).unwrap_or(Affine::IDENTITY);
                let pts: Vec<Point> = args.iter().filter_map(|b| as_point(b)).collect();
                // Shapes are flattened at 0.001 mm: a huge (or non-finite) size made that run without end.
                let usable = |v: f64| v.is_finite() && v.abs() <= MAX_SHAPE_MM;
                if !pts.iter().all(|p| usable(p.x) && usable(p.y)) {
                    *self.skipped.entry("shapes of unusable size").or_default() += 1;
                    return Some(());
                }
                let path = match kind {
                    RECT if pts.len() >= 2 => Some(kurbo::Rect::from_points(pts[0], pts[1]).to_path(0.001)),
                    CIRCLE => {
                        let c = pts.first().copied()?;
                        let rad = args.get(1).and_then(|b| as_f64(b))?;
                        if !usable(rad) {
                            *self.skipped.entry("shapes of unusable size").or_default() += 1;
                            return Some(());
                        }
                        Some(kurbo::Circle::new(c, rad.abs()).to_path(0.001))
                    }
                    ELLIPSE if pts.len() >= 2 => {
                        let rect = kurbo::Rect::from_points(pts[0], pts[1]);
                        Some(kurbo::Ellipse::from_rect(rect).to_path(0.001))
                    }
                    POLYGON if pts.len() >= 2 => {
                        let sides = args.get(7).and_then(|b| as_i32(b)).unwrap_or(6).clamp(3, 1000);
                        let rect = kurbo::Rect::from_points(pts[0], pts[1]);
                        let (c, rx, ry) = (rect.center(), rect.width() / 2.0, rect.height() / 2.0);
                        let mut p = BezPath::new();
                        for k in 0..sides {
                            let a = std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * f64::from(k) / f64::from(sides);
                            let q = Point::new(c.x + rx * a.cos(), c.y + ry * a.sin());
                            if k == 0 { p.move_to(q) } else { p.line_to(q) }
                        }
                        p.close_path();
                        Some(p)
                    }
                    _ => None,
                };
                if let Some(p) = path
                    && !hidden
                {
                    self.push(m * p, pen);
                }
            }
            GROUP | COMBINE | VECTOR_FILE => container(self, r)?,
            HATCH | SPIRAL => {
                let first = self.art.paths.len();
                container(self, r)?;
                if kind == HATCH {
                    // Some files keep the generated hatch lines among the children: many open two-point lines.
                    let line = |p: &BezPath| p.elements().len() == 2 && matches!(p.elements()[1], kurbo::PathEl::LineTo(_));
                    if self.art.paths[first..].iter().filter(|a| line(&a.path)).count() > 20 {
                        let kept: Vec<crate::ArtPath> = self.art.paths.drain(first..).filter(|a| !line(&a.path)).collect();
                        self.art.paths.extend(kept);
                    }
                }
                let _ = r.items()?;
                // The generated hatch lines may follow as a group body without its type word (its 15-item header
                // comes first); they are skipped: the outlines carry the art and a fill layer hatches them again.
                let save = r.at;
                if r.i32() == Some(15) {
                    r.at = save;
                    let before = self.art.paths.len();
                    if self.object(r, GROUP, depth + 1).is_none() {
                        r.at = save;
                    }
                    self.art.paths.truncate(before);
                } else {
                    r.at = save;
                }
                *self.skipped.entry("hatch settings").or_default() += usize::from(kind == HATCH);
            }
            TEXT => {
                *self.skipped.entry("text").or_default() += 1;
                return None;
            }
            IMAGE => {
                *self.skipped.entry("image").or_default() += 1;
                return None;
            }
            _ => return None,
        }
        Some(())
    }
}

/// Read pens: (name, RGB colour).
fn pens(b: &[u8], at: usize) -> Vec<(String, [u8; 3])> {
    let mut r = Reader { b, at };
    let (Some(n), Some(ptr)) = (r.i32(), r.i32()) else { return vec![] };
    if !(0..=256).contains(&n) {
        return vec![];
    }
    r.at = ptr.max(0) as usize;
    let mut out = Vec::new();
    for _ in 0..n {
        let Some(items) = r.items() else { break };
        let bgr = items.first().and_then(|i| as_i32(i)).unwrap_or(0) as u32;
        let color = [(bgr & 0xFF) as u8, ((bgr >> 8) & 0xFF) as u8, ((bgr >> 16) & 0xFF) as u8];
        let name = items.get(1).map(|i| utf16(i)).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Pen {}", out.len()));
        out.push((name, color));
    }
    out
}

/// Import the vector objects of an EzCad2 `.ezd` file.
pub fn parse(bytes: &[u8]) -> Result<Art> {
    if bytes.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the EZD file is larger than 200 MB".into()));
    }
    if bytes.len() < 400 || utf16(&bytes[..16]) != "EZCADUNI" {
        return invalid("not an EzCad .ezd file (no EZCADUNI header)");
    }
    let mut r = Reader { b: bytes, at: 0x158 };
    let offsets: Vec<usize> = (0..7).map(|_| r.i32().unwrap_or(0).max(0) as usize).collect();
    let (pen_at, vec_at) = (offsets[2], offsets[5]);
    if vec_at == 0 || vec_at >= bytes.len() {
        return invalid("the EZD file has no vector section");
    }
    let pens = if pen_at > 0 && pen_at < bytes.len() { pens(bytes, pen_at) } else { vec![] };
    let data = decode_vectors(bytes, vec_at)?;
    let mut p = Parse { art: Art::default(), pens: &pens, objects: 0, skipped: HashMap::new() };
    let mut r = Reader { b: &data, at: 0 };
    p.list(&mut r, true, 0);
    let mut skipped: Vec<String> = p.skipped.iter().filter(|(_, n)| **n > 0).map(|(k, n)| format!("{n} {k}")).collect();
    skipped.sort();
    if !skipped.is_empty() {
        p.art.warn(format!("left out: {} (EzCad text has no outline in the file; images and hatch settings are not read)", skipped.join(", ")));
    }
    if p.art.paths.is_empty() {
        return invalid("no vector objects found in the EZD file");
    }
    Ok(p.art)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_other_files() {
        assert!(parse(b"hello").is_err());
        let mut fake = vec![0u8; 500];
        for (i, c) in "EZCADUNI".encode_utf16().enumerate() {
            fake[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        assert!(parse(&fake).is_err(), "no vector section");
    }

    #[test]
    fn huffman_stream_decodes() {
        // Table: 'a' = 0, 'b' = 10, 'c' = 11; stream "abcab" = 0 10 11 0 10 -> 01011010 (padded).
        let mut b = vec![0u8; 8];
        b.extend_from_slice(&5u32.to_le_bytes());
        b.extend_from_slice(&[0; 16]);
        b.extend_from_slice(&3u16.to_le_bytes());
        for (sym, code, len) in [(b'a', 0u32, 1u16), (b'b', 2, 2), (b'c', 3, 2)] {
            b.push(sym);
            b.extend_from_slice(&code.to_le_bytes());
            b.extend_from_slice(&len.to_le_bytes());
        }
        b.push(0b0101_1010);
        assert_eq!(decode_vectors(&b, 8).unwrap(), b"abcab");
    }

    /// A circle of radius 1e300 (flattened at 0.001 mm, it never ended) is left out; a normal one is read.
    #[test]
    fn huge_circles_are_left_out() {
        let items = |list: &[Vec<u8>]| {
            let mut b = (list.len() as i32).to_le_bytes().to_vec();
            for i in list {
                b.extend_from_slice(&(i.len() as i32).to_le_bytes());
                b.extend_from_slice(i);
            }
            b
        };
        let circle = |r: f64| {
            let mut b = items(&[0i32.to_le_bytes().to_vec()]);
            let centre: Vec<u8> = [0f64, 0f64].iter().flat_map(|v| v.to_le_bytes()).collect();
            b.extend(items(&[centre, r.to_le_bytes().to_vec()]));
            b
        };
        let t = std::time::Instant::now();
        for (r, paths) in [(1e300, 0), (f64::INFINITY, 0), (5.0, 1)] {
            let data = circle(r);
            let mut p = Parse { art: Art::default(), pens: &[], objects: 0, skipped: HashMap::new() };
            p.object(&mut Reader { b: &data, at: 0 }, CIRCLE, 0).unwrap();
            assert_eq!(p.art.paths.len(), paths, "radius {r}");
        }
        assert!(t.elapsed().as_secs() < 5);
    }

    /// The owner's EzCad files (OPENETCH_LBRN_SAMPLES), read in place.
    #[test]
    fn real_ezd_files_import() {
        let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else { return };
        let mut n = 0;
        let mut stack: Vec<std::path::PathBuf> = std::env::split_paths(&dirs).collect();
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() {
                    if !(name.starts_with('.') || name == "target" || p.join("Cargo.toml").exists()) {
                        stack.push(p);
                    }
                    continue;
                }
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ezd")) {
                    continue;
                }
                let bytes = std::fs::read(&p).unwrap();
                let art = parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                let b = art.paths.iter().map(|a| a.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
                eprintln!("{}: {} paths, bounds {b:?}, warnings {:?}", p.display(), art.paths.len(), art.warnings);
                assert!(b.width() < 1000.0 && b.height() < 1000.0, "{}: {b:?}", p.display());
                n += 1;
            }
        }
        eprintln!("{n} EZD sample(s) imported");
    }
}
