//! EzCad2 `.ezd` import, for galvo users moving from EzCad. Not in LightBurn.
//!
//! The layout follows meerk40t's EZD reader (`meerk40t/extra/ezd.py`, https://github.com/meerk40t/meerk40t, MIT,
//! Copyright (c) meerk40t contributors), re-written here and checked against the owner's EzCad 2 files:
//!
//! - Header: `EZCADUNI` in UTF-16 (16 bytes), two `i32` (0, 2001), three 60-byte UTF-16 strings, 140 bytes; then
//!   seven `i32` absolute offsets (preview, v1, pens, font, v4, vectors, prevectors).
//! - Pens: `i32` count, `i32` offset, then per pen a "struct" (`i32` item count, then `i32` length + bytes per item);
//!   item 0 is the colour (BGR), 1 the name, 4 the loop count, 5 the speed (mm/s), 6 the power (%), 7 the frequency
//!   (Hz) as meerk40t reads them; item 8 is read as the MOPA pulse width in ns (EzCad's default 200 is there in the
//!   owner's files; meerk40t does not name it). Each pen that marks something becomes a layer (line mode).
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
//! Coordinates are mm with the origin at the field centre and +Y up, as OpenEtch's field frame. Unknown objects are
//! skipped by scanning for the next object.
//!
//! - **Hatch** (0x20): its children are the outlines; after them a 54-item settings struct (meerk40t's layout:
//!   item 0 mark contours, then per hatch 1/2/3 enabled, pen, type flags, edge offset, line space, start/end
//!   offsets, angle at items 1-8, 9-16 and 20-27). The outlines go on a fill layer: the first enabled hatch with its
//!   pen's settings (interval = line space, angle), the others as sub-layers; marked contours make it fill + line.
//!   The type flags (bidirectional, loops, edge follow, ...) are not decoded: hatches are bidirectional. The hatch
//!   lines EzCad generated are dropped (the fill layer hatches the outlines again).
//! - **Text** (0x800): a 126-item struct (item 1 the height, 7 the start point, 10 the text, 44 the TrueType font,
//!   18 its other font name) and then, after an `i32`, a child list. In the owner's file the children are the glyph
//!   outlines (a hatch of one group per character), which are imported. A text with no outlines is returned in
//!   [`Art::texts`] for the caller to set in the font (or mark the place of).
//! - **Image** (0x40): a struct (item 4 height, 5 width, mm) then a Windows bitmap (`BM`, `i32` size, the rest),
//!   converted to PNG and centred on the object's position (header item 13). **Unverified**: none of the owner's
//!   files has an image; the layout is meerk40t's.

use std::collections::HashMap;

use kurbo::{Affine, BezPath, Point, Shape};

use crate::{Art, ArtLayer, ArtText, FileError, Result, invalid};
use doc::{CutSettings, FillMode};

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

/// One EzCad pen: name, colour and its settings as a line layer.
#[derive(Debug, Clone, PartialEq)]
struct Pen {
    name: String,
    color: [u8; 3],
    settings: CutSettings,
}

/// The object tree being read.
struct Parse<'a> {
    art: Art,
    pens: &'a [Pen],
    objects: usize,
    skipped: HashMap<&'static str, usize>,
    /// The layer (index into `art.layers`) of each pen used so far.
    pen_layers: HashMap<usize, usize>,
}

impl Parse<'_> {
    fn pen(&self, pen: usize) -> Pen {
        self.pens.get(pen).cloned().unwrap_or_else(|| Pen {
            name: format!("Pen {pen}"),
            color: [0, 0, 0],
            settings: CutSettings { mode: FillMode::Line, ..CutSettings::default() },
        })
    }

    /// The line layer of `pen`, made on first use.
    fn pen_layer(&mut self, pen: usize) -> usize {
        if let Some(l) = self.pen_layers.get(&pen) {
            return *l;
        }
        let p = self.pen(pen);
        self.art.layers.push(ArtLayer { name: p.name, color: p.color, settings: p.settings, sub_layers: Vec::new() });
        let l = self.art.layers.len() - 1;
        self.pen_layers.insert(pen, l);
        l
    }

    fn push(&mut self, path: BezPath, pen: usize) {
        let p = self.pen(pen);
        let before = self.art.paths.len();
        self.art.push(path, p.name, Some(p.color));
        if self.art.paths.len() > before {
            let l = self.pen_layer(pen);
            self.art.paths[before].layer = Some(l);
        }
    }

    /// The fill layer of a hatch from its settings struct (see the module docs); `None` when no hatch is enabled.
    fn hatch_layer(&mut self, set: &[&[u8]], contour_pen: usize) -> Option<usize> {
        let int = |i: usize| set.get(i).and_then(|b| as_i32(b));
        let num = |i: usize| set.get(i).and_then(|b| as_f64(b)).filter(|v| v.is_finite());
        // (enabled, pen, line space, angle) of hatches 1, 2 and 3.
        let hatches: Vec<(usize, f64, f64)> = [(1, 2, 5, 8), (9, 10, 13, 16), (20, 21, 24, 27)]
            .into_iter()
            .filter(|(e, ..)| int(*e).is_some_and(|v| v != 0))
            .filter_map(|(_, p, s, a)| {
                let space = num(s).filter(|v| *v >= 0.001 && *v <= 100.0)?;
                Some((int(p).unwrap_or(0).max(0) as usize, space, num(a).unwrap_or(0.0)))
            })
            .collect();
        let (&(pen, space, angle), rest) = hatches.split_first()?;
        let fill = |this: &Self, pen: usize, space: f64, angle: f64| {
            let mut s = this.pen(pen).settings;
            s.mode = FillMode::Fill;
            s.hatch.interval_mm = space;
            s.hatch.angle_deg = angle;
            s.hatch.bidirectional = true;
            s
        };
        let mut settings = fill(self, pen, space, angle);
        if int(0).is_some_and(|v| v != 0) {
            settings.mode = FillMode::FillThenLine;
        }
        let sub_layers: Vec<CutSettings> = rest.iter().map(|&(p, s, a)| fill(self, p, s, a)).collect();
        let base = self.pen(contour_pen);
        let name = format!("{} hatch {} mm", self.pen(pen).name, show(space));
        let layer = ArtLayer { name, color: base.color, settings, sub_layers };
        if let Some(i) = self.art.layers.iter().position(|l| *l == layer) {
            return Some(i);
        }
        self.art.layers.push(layer);
        Some(self.art.layers.len() - 1)
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
                let children_end = self.art.paths.len();
                if kind == HATCH {
                    // Some files keep the generated hatch lines among the children: many open two-point lines.
                    let line = |p: &BezPath| p.elements().len() == 2 && matches!(p.elements()[1], kurbo::PathEl::LineTo(_));
                    if self.art.paths[first..].iter().filter(|a| line(&a.path)).count() > 20 {
                        let kept: Vec<crate::ArtPath> = self.art.paths.drain(first..).filter(|a| !line(&a.path)).collect();
                        self.art.paths.extend(kept);
                    }
                }
                let settings = r.items()?;
                if kind == HATCH {
                    match self.hatch_layer(&settings, pen) {
                        Some(l) => {
                            let end = children_end.min(self.art.paths.len());
                            for a in &mut self.art.paths[first..end] {
                                a.layer = Some(l);
                            }
                        }
                        None => *self.skipped.entry("hatch settings (no hatch enabled)").or_default() += 1,
                    }
                }
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
            }
            TEXT => {
                let args = r.items()?;
                let _ = r.i32()?;
                let first = self.art.paths.len();
                container(self, r)?;
                let text = args.get(10).map(|b| utf16(b)).unwrap_or_default();
                if self.art.paths.len() > first {
                    // The glyph outlines EzCad keeps with the text.
                    let label = format!("text \"{}\"", text.chars().take(40).collect::<String>());
                    for a in &mut self.art.paths[first..] {
                        a.label = label.clone();
                    }
                } else if !hidden && !text.trim().is_empty() {
                    let font = [44, 18]
                        .iter()
                        .filter_map(|i| args.get(*i).map(|b| utf16(b)))
                        .find(|f| !f.trim().is_empty())
                        .unwrap_or_default();
                    let height = args.get(1).and_then(|b| as_f64(b)).filter(|h| h.is_finite() && *h > 0.0 && *h <= 1000.0).unwrap_or(5.0);
                    let at = args.get(7).and_then(|b| as_point(b)).unwrap_or(Point::ZERO);
                    let m = args.get(9).and_then(|b| as_matrix(b)).unwrap_or(Affine::IDENTITY);
                    let at = m * at;
                    let angle = args.first().and_then(|b| as_f64(b)).filter(|a| a.is_finite()).unwrap_or(0.0);
                    if at.x.is_finite() && at.y.is_finite() && at.x.abs() <= MAX_SHAPE_MM && at.y.abs() <= MAX_SHAPE_MM {
                        let layer = Some(self.pen_layer(pen));
                        self.art.texts.push(ArtText { text, font, height_mm: height, at, angle_deg: angle.to_degrees(), layer });
                    }
                }
            }
            IMAGE => {
                let args = r.items()?;
                let magic = r.take(2)?;
                let size = r.i32()?;
                if magic != b"BM" || !(6..=64 << 20).contains(&size) {
                    *self.skipped.entry("images not read").or_default() += 1;
                    return None;
                }
                let rest = r.take(size as usize - 6)?;
                let mut bmp = Vec::with_capacity(size as usize);
                bmp.extend_from_slice(b"BM");
                bmp.extend_from_slice(&size.to_le_bytes());
                bmp.extend_from_slice(rest);
                let w = args.get(5).and_then(|b| as_f64(b)).filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_SHAPE_MM);
                let h = args.get(4).and_then(|b| as_f64(b)).filter(|v| v.is_finite() && *v > 0.0 && *v <= MAX_SHAPE_MM);
                let centre = header.get(13).and_then(|b| as_point(b)).filter(|p| p.x.is_finite() && p.y.is_finite()).unwrap_or(Point::ZERO);
                let png = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok().and_then(|img| {
                    let mut out = Vec::new();
                    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok().map(|_| out)
                });
                match (png, w, h) {
                    (Some(data), Some(w), Some(h)) if !hidden => self.art.images.push(crate::ArtImage {
                        data,
                        mime: "image/png",
                        placement: Affine::new([w, 0.0, 0.0, h, centre.x - w / 2.0, centre.y - h / 2.0]),
                        label: "image".into(),
                    }),
                    (_, _, _) if hidden => {}
                    _ => *self.skipped.entry("images not read").or_default() += 1,
                }
            }
            _ => return None,
        }
        Some(())
    }
}

/// A number for a name: at most 4 decimals, trailing zeros dropped.
fn show(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A pen's settings as a line layer (see the module docs); values out of range keep OpenEtch's defaults.
fn pen_settings(items: &[&[u8]]) -> CutSettings {
    let num = |i: usize| items.get(i).and_then(|b| as_f64(b)).filter(|v| v.is_finite());
    let int = |i: usize| items.get(i).and_then(|b| as_i32(b));
    let mut s = CutSettings { mode: FillMode::Line, ..CutSettings::default() };
    if let Some(v) = num(5).filter(|v| *v > 0.0 && *v <= 100_000.0) {
        s.speed_mm_s = v;
    }
    if let Some(v) = num(6).filter(|v| (0.0..=100.0).contains(v)) {
        s.power_pct = v;
    }
    if let Some(v) = int(7).filter(|v| (1_000..=10_000_000).contains(v)) {
        s.frequency_khz = f64::from(v) / 1000.0;
    }
    if let Some(v) = int(4).filter(|v| (1..=1000).contains(v)) {
        s.passes = v as u32;
    }
    if let Some(v) = int(8).filter(|v| (1..=1000).contains(v)) {
        s.pulse_width_ns = Some(f64::from(v));
    }
    s
}

/// Read pens: name, RGB colour and settings.
fn pens(b: &[u8], at: usize) -> Vec<Pen> {
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
        out.push(Pen { name, color, settings: pen_settings(&items) });
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
    let mut p = Parse { art: Art::default(), pens: &pens, objects: 0, skipped: HashMap::new(), pen_layers: HashMap::new() };
    let mut r = Reader { b: &data, at: 0 };
    p.list(&mut r, true, 0);
    let mut skipped: Vec<String> = p.skipped.iter().filter(|(_, n)| **n > 0).map(|(k, n)| format!("{n} {k}")).collect();
    skipped.sort();
    if !skipped.is_empty() {
        p.art.warn(format!("left out: {}", skipped.join(", ")));
    }
    if !p.art.layers.is_empty() {
        p.art.warn("EzCad hatch types (bidirectional, loops, edge follow) are not read: hatches are bidirectional fills");
    }
    if p.art.paths.is_empty() && p.art.texts.is_empty() && p.art.images.is_empty() {
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
            let mut p = Parse { art: Art::default(), pens: &[], objects: 0, skipped: HashMap::new(), pen_layers: HashMap::new() };
            p.object(&mut Reader { b: &data, at: 0 }, CIRCLE, 0).unwrap();
            assert_eq!(p.art.paths.len(), paths, "radius {r}");
        }
        assert!(t.elapsed().as_secs() < 5);
    }

    /// An image object as meerk40t describes it (no sample file has one): a struct (item 4 height, 5 width), then a
    /// Windows bitmap; it is placed centred on the header's position.
    #[test]
    fn images_are_read_from_their_bitmap() {
        let items = |list: &[Vec<u8>]| {
            let mut b = (list.len() as i32).to_le_bytes().to_vec();
            for i in list {
                b.extend_from_slice(&(i.len() as i32).to_le_bytes());
                b.extend_from_slice(i);
            }
            b
        };
        let mut header: Vec<Vec<u8>> = (0..15).map(|_| 0i32.to_le_bytes().to_vec()).collect();
        header[13] = [5f64, -3f64].iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut args: Vec<Vec<u8>> = (0..10).map(|_| 0i32.to_le_bytes().to_vec()).collect();
        args[4] = 10f64.to_le_bytes().to_vec();
        args[5] = 20f64.to_le_bytes().to_vec();
        let mut bmp = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 2, image::Rgb([200, 10, 10])))
            .write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        let mut data = items(&header);
        data.extend(items(&args));
        data.extend_from_slice(&bmp);
        let mut p = Parse { art: Art::default(), pens: &[], objects: 0, skipped: HashMap::new(), pen_layers: HashMap::new() };
        let mut r = Reader { b: &data, at: 0 };
        p.object(&mut r, IMAGE, 0).unwrap();
        assert_eq!(r.at, data.len(), "the whole bitmap is read");
        assert_eq!(p.art.images.len(), 1);
        let im = &p.art.images[0];
        let b = im.bounds();
        assert!((b.x0 + 5.0).abs() < 1e-9 && (b.x1 - 15.0).abs() < 1e-9 && (b.y0 + 8.0).abs() < 1e-9 && (b.y1 - 2.0).abs() < 1e-9, "{b:?}");
        let png = image::load_from_memory(&im.data).unwrap().to_rgb8();
        assert_eq!((png.width(), png.get_pixel(0, 0).0), (4, [200, 10, 10]));
        // A broken bitmap leaves the object out (and the reader resyncs on the next object).
        let mut bad = items(&header);
        bad.extend(items(&args));
        bad.extend_from_slice(&[b'X', b'X', 16, 0, 0, 0]);
        let mut p = Parse { art: Art::default(), pens: &[], objects: 0, skipped: HashMap::new(), pen_layers: HashMap::new() };
        assert!(p.object(&mut Reader { b: &bad, at: 0 }, IMAGE, 0).is_none());
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
                assert!(art.paths.iter().all(|a| a.layer.is_some_and(|l| l < art.layers.len())), "every path has its layer");
                for l in &art.layers {
                    let s = &l.settings;
                    eprintln!("  layer {:?} {:?} {} mm/s {} % {} kHz {:?} ns, interval {} angle {}, {} sub-layer(s)", l.name, s.mode, s.speed_mm_s, s.power_pct, s.frequency_khz, s.pulse_width_ns, s.hatch.interval_mm, s.hatch.angle_deg, l.sub_layers.len());
                }
                if name == "AUTOSAVE.ezd" {
                    // EzCad's default "TEXT" in Arial 10 mm, hatched: its glyph outlines on a 0.03 mm fill at -90 degrees.
                    assert!(art.paths.iter().all(|a| a.label == "text \"TEXT\""), "{:?}", art.paths.iter().map(|a| &a.label).collect::<Vec<_>>());
                    assert!((b.height() - 7.15).abs() < 0.01, "cap height of 10 mm Arial: {b:?}");
                    let l = &art.layers[art.paths[0].layer.unwrap()];
                    assert_eq!((l.settings.mode.clone(), l.settings.hatch.interval_mm, l.settings.hatch.angle_deg), (FillMode::Fill, 0.03, -90.0));
                    assert_eq!((l.settings.speed_mm_s, l.settings.power_pct, l.settings.frequency_khz, l.settings.pulse_width_ns), (1000.0, 75.0, 30.0, Some(200.0)));
                }
                if name == "Test file-Dog.ezd" {
                    // Three hatches (0.05, 0.2 and 1 mm): one fill layer with two sub-layers.
                    let l = art.layers.iter().find(|l| l.settings.mode == FillMode::Fill).expect("a fill layer");
                    assert_eq!((l.settings.hatch.interval_mm, l.sub_layers.len()), (0.05, 2));
                    assert_eq!((l.sub_layers[0].hatch.interval_mm, l.sub_layers[0].hatch.angle_deg, l.sub_layers[1].hatch.interval_mm), (0.2, 45.0, 1.0));
                }
                n += 1;
            }
        }
        eprintln!("{n} EZD sample(s) imported");
    }
}
