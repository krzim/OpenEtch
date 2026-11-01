//! Laying text out like LightBurn: lines, spacing, alignment, max width, bend, text on a path, welding.
//!
//! Measured against LightBurn's own saved outlines (`BackupPath`, see `tests/lightburn.rs`): a glyph is scaled by
//! `H / units-per-em` (so `H` is the em size, not the cap height), each line is aligned by its ink (LightBurn 2.1:
//! both lines of a centred two-line text have symmetric ink; LightBurn 1.4 centred a single "1" by its advance
//! instead, up to a few hundredths of a mm away), and lines are `ascent + descent + line gap` apart (the font's
//! horizontal header). For Align Y Middle the block of `n` lines is centred so that the first baseline sits at
//! `((n - 1) · pitch + line) / 2 - line`: one line has its baseline half a line below the origin, two lines put the
//! first baseline on it. Top puts the capitals' tops of the first line on the origin, Bottom the last baseline
//! (LightBurn's docs: "Top aligns to the tops of capitals"). HSpace adds `LS` % of the height after every character
//! but the last; VSpace adds `LnS` % of the line height between lines.

use kurbo::{Affine, BezPath, PathEl, Point, Vec2};

use doc::{AlignH, AlignV, Text};

use crate::registry::{Font, Resolved};

/// Italic slant for fonts without an italic face (about 12°, as most obliques).
const SLANT: f64 = 0.21;

/// A glyph placed on a straight baseline: its outline in mm relative to its own pen position, where that pen sits
/// in the line, and its advance.
#[derive(Debug, Clone)]
struct Placed {
    path: BezPath,
    /// Pen position (left end of the advance) on the baseline, mm.
    x: f64,
    baseline: f64,
    advance: f64,
}

/// What a font gives the layout, in mm for a given height.
struct Metrics {
    line: f64,
    cap: f64,
}

/// Lay out the lines of `t` with `font`: the outline in the text's own frame (before bend or path), plus warnings.
pub(crate) struct Laid {
    glyphs: Vec<Placed>,
    /// Width of each line in mm (after max-width scaling) and the widest.
    widest: f64,
    cap: f64,
    line_widths: Vec<f64>,
    pub warnings: Vec<String>,
    pub stroke: bool,
}

fn metrics(font: &Font, h: f64) -> Metrics {
    match font {
        Font::Outline(f) => {
            let s = h / f.upem;
            Metrics { line: (f.ascent + f.descent + f.line_gap) * s, cap: f.cap_height * s }
        }
        Font::Stroke(f) => {
            let s = h / f.em;
            Metrics { line: (f.ascent + f.descent + f.line_gap) * s, cap: f.cap_height * s }
        }
    }
}

/// One line's glyphs at pen positions from 0, with the letter spacing; returns them and the line width.
fn line_glyphs(font: &Font, line: &str, h: f64, spacing: f64, slant: bool, warnings: &mut Vec<String>) -> (Vec<Placed>, f64) {
    let mut out = Vec::new();
    let mut x = 0.0;
    let skew = if slant { Affine::new([1.0, 0.0, SLANT, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
    match font {
        Font::Outline(f) => {
            let s = h / f.upem;
            let shaped = f.shape(line);
            let n = shaped.len();
            for (k, g) in shaped.iter().enumerate() {
                if g.glyph == 0 {
                    let c = line.get(g.cluster as usize..).and_then(|r| r.chars().next()).unwrap_or('?');
                    if !c.is_whitespace() {
                        warnings.push(format!("\"{}\" has no glyph for '{c}'", f.family));
                    }
                }
                let path = Affine::translate((g.x_offset * s, g.y_offset * s)) * skew * Affine::scale(s) * f.glyph_path(g.glyph);
                let advance = g.x_advance * s;
                out.push(Placed { path, x, baseline: 0.0, advance });
                x += advance;
                if k + 1 < n {
                    x += spacing;
                }
            }
        }
        Font::Stroke(f) => {
            let s = h / f.em;
            let n = line.chars().count();
            for (k, c) in line.chars().enumerate() {
                let (path, adv) = match f.glyph(c) {
                    Some(g) => (skew * Affine::scale(s) * g.path.clone(), (g.advance + f.letter_gap) * s),
                    None => {
                        if !c.is_whitespace() {
                            warnings.push(format!("\"{}\" has no glyph for '{c}'", f.name));
                        }
                        (BezPath::new(), f.space * s)
                    }
                };
                out.push(Placed { path, x, baseline: 0.0, advance: adv });
                x += adv;
                if k + 1 < n {
                    x += spacing;
                }
            }
        }
    }
    (out, x.max(0.0))
}

/// The lines of an evaluated text: split on newlines (a final empty line is kept, as typed).
pub(crate) fn lines(text: &str, upper: bool) -> Vec<String> {
    let t = if upper { text.to_uppercase() } else { text.to_string() };
    t.replace("\r\n", "\n").split('\n').map(str::to_string).collect()
}

pub(crate) fn lay_out(t: &Text, shown: &[String], r: &Resolved) -> Laid {
    let h = t.height_mm;
    let m = metrics(&r.font, h);
    let spacing = t.h_spacing / 100.0 * h;
    let pitch = m.line * (1.0 + t.v_spacing / 100.0);
    let mut warnings = r.warnings.clone();
    let mut lines: Vec<(Vec<Placed>, f64)> =
        shown.iter().map(|l| line_glyphs(&r.font, l, h, spacing, r.slant, &mut warnings)).collect();
    warnings.sort();
    warnings.dedup();
    let widest = lines.iter().map(|(_, w)| *w).fold(0.0, f64::max);
    // Max width: scale every line (and, without squeeze, the height too) about the origin.
    let (sx, sy) = if t.max_width_mm > 0.0 && widest > t.max_width_mm {
        let k = t.max_width_mm / widest;
        (k, if t.squeeze { 1.0 } else { k })
    } else {
        (1.0, 1.0)
    };
    let n = lines.len().max(1) as f64;
    let first = match t.align_v {
        AlignV::Top => -m.cap,
        AlignV::Middle => ((n - 1.0) * pitch + m.line) / 2.0 - m.line,
        AlignV::Bottom => (n - 1.0) * pitch,
    };
    let mut glyphs = Vec::new();
    let mut line_widths = Vec::new();
    for (k, (line, w)) in lines.iter_mut().enumerate() {
        // Each line is aligned by its ink (LightBurn 2.x: both lines of a centred two-line text have symmetric ink
        // bounds); a line without ink by its advance.
        let ink = line
            .iter()
            .filter_map(|g| geom::bounds(&g.path).map(|b| (b.x0 + g.x, b.x1 + g.x)))
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
        let (left, right) = ink.unwrap_or((0.0, *w));
        let x0 = match t.align_h {
            AlignH::Left => -left,
            AlignH::Middle => -(left + right) / 2.0,
            AlignH::Right => -right,
        };
        let baseline = (first - k as f64 * pitch) * sy;
        for g in line.drain(..) {
            glyphs.push(Placed {
                path: Affine::scale_non_uniform(sx, sy) * g.path,
                x: (x0 + g.x) * sx,
                baseline,
                advance: g.advance * sx,
            });
        }
        line_widths.push(*w * sx);
    }
    Laid {
        glyphs,
        widest: widest * sx,
        cap: m.cap * sy,
        line_widths,
        warnings,
        stroke: matches!(r.font, Font::Stroke(_)),
    }
}

/// Split long segments so that no piece is longer than `max` (lines only; curves are flattened first).
fn subdivide(path: &BezPath, max: f64) -> BezPath {
    let mut out = BezPath::new();
    let mut at = Point::ORIGIN;
    let mut start = Point::ORIGIN;
    let push_line = |out: &mut BezPath, a: Point, b: Point| {
        let n = ((a - b).hypot() / max).ceil().clamp(1.0, 10_000.0) as usize;
        for i in 1..=n {
            out.line_to(a.lerp(b, i as f64 / n as f64));
        }
    };
    let mut flat = BezPath::new();
    kurbo::flatten(path.elements().iter().copied(), 0.002, |el| flat.push(el));
    for el in flat.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                out.move_to(p);
                at = p;
                start = p;
            }
            PathEl::LineTo(p) => {
                push_line(&mut out, at, p);
                at = p;
            }
            PathEl::ClosePath => {
                if (at - start).hypot() > 1e-12 {
                    push_line(&mut out, at, start);
                }
                out.close_path();
                at = start;
            }
            PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                out.line_to(p);
                at = p;
            }
        }
    }
    out
}

fn map_points(path: &BezPath, f: impl Fn(Point) -> Point) -> BezPath {
    path.elements()
        .iter()
        .map(|el| match *el {
            PathEl::MoveTo(p) => PathEl::MoveTo(f(p)),
            PathEl::LineTo(p) => PathEl::LineTo(f(p)),
            PathEl::QuadTo(a, p) => PathEl::QuadTo(f(a), f(p)),
            PathEl::CurveTo(a, b, p) => PathEl::CurveTo(f(a), f(b), f(p)),
            PathEl::ClosePath => PathEl::ClosePath,
        })
        .collect()
}

impl Laid {
    /// Glyph outlines on straight lines.
    fn straight(&self) -> Vec<BezPath> {
        self.glyphs.iter().map(|g| Affine::translate((g.x, g.baseline)) * g.path.clone()).collect()
    }

    /// Glyphs bent round a circle (see [`doc::Text::bend_deg`]).
    fn bent(&self, bend_deg: f64, distort: bool) -> Vec<BezPath> {
        let span = bend_deg.to_radians();
        if span.abs() < 1e-9 || self.widest <= 0.0 {
            return self.straight();
        }
        // Radius such that the widest line spans `span` radians; the circle's centre is at (0, -r).
        let r = self.widest / span;
        let place = move |p: Point| {
            let theta = p.x / r;
            let rr = r + p.y;
            Point::new(rr * theta.sin(), rr * theta.cos() - r)
        };
        self.glyphs
            .iter()
            .map(|g| {
                if distort {
                    let p = Affine::translate((g.x, g.baseline)) * g.path.clone();
                    map_points(&subdivide(&p, (r.abs() * 0.02).clamp(0.01, 1.0)), place)
                } else {
                    let cx = g.x + g.advance / 2.0;
                    let theta = cx / r;
                    let rr = r + g.baseline;
                    let at = Point::new(rr * theta.sin(), rr * theta.cos() - r);
                    Affine::translate(at.to_vec2()) * Affine::rotate(-theta) * Affine::translate((g.x - cx, 0.0)) * g.path.clone()
                }
            })
            .collect()
    }

    /// Glyphs along `path` (in the text's frame), following LightBurn's Apply Path to Text: Align X places the text
    /// at the path's start (left), centred between its ends (middle) or ending at its end (right); Align Y puts the
    /// baseline (bottom), the middle of the capitals (middle) or their tops (top) on the path.
    fn on_path(&self, t: &Text, path: &BezPath, offset: f64) -> Result<Vec<BezPath>, String> {
        let walker = geom::measure::Walker::longest(path).map_err(|e| format!("the text's path: {e}"))?;
        let len = walker.length();
        let shift = match t.align_v {
            AlignV::Bottom => 0.0,
            AlignV::Middle => -self.cap / 2.0,
            AlignV::Top => -self.cap,
        };
        // Lines stack below the first one; the straight layout already spaced them, relative to its first line.
        let first = self.glyphs.first().map_or(0.0, |g| g.baseline);
        let mut out = Vec::with_capacity(self.glyphs.len());
        let mut line = 0usize;
        let mut prev_baseline = first;
        let mut line_start = self.glyphs.first().map_or(0.0, |g| g.x);
        for g in &self.glyphs {
            if (g.baseline - prev_baseline).abs() > 1e-9 {
                line += 1;
                prev_baseline = g.baseline;
                line_start = g.x;
            }
            let w = self.line_widths.get(line).copied().unwrap_or(0.0);
            let start = match t.align_h {
                AlignH::Left => 0.0,
                AlignH::Middle => (len - w) / 2.0,
                AlignH::Right => len - w,
            };
            let cx = g.x - line_start + g.advance / 2.0;
            let (p, angle) = walker.at(start + offset + cx);
            let b = g.baseline - first + shift;
            out.push(
                Affine::translate(p.to_vec2())
                    * Affine::rotate(angle)
                    * Affine::translate(Vec2::new(-(g.x + g.advance / 2.0), b - g.baseline))
                    * Affine::translate((g.x, g.baseline))
                    * g.path.clone(),
            );
        }
        Ok(out)
    }
}

/// The final outline of a laid-out text: bent or on `path` when asked, welded when asked (filled fonts only).
pub(crate) fn finish(t: &Text, laid: &Laid, path: Option<&BezPath>) -> Result<BezPath, String> {
    let glyphs = match (path, &t.path) {
        (Some(p), Some(tp)) => laid.on_path(t, p, tp.offset_mm)?,
        _ => laid.bent(t.bend_deg, t.distort),
    };
    let mut out = BezPath::new();
    if t.welded && !laid.stroke && glyphs.len() > 1 {
        // Only glyphs that touch are merged (as polygons, within 0.01 mm); the others keep their exact curves.
        for cluster in touching(&glyphs) {
            if cluster.len() == 1 {
                out.extend(glyphs[cluster[0]].iter());
                continue;
            }
            let group: Vec<BezPath> = cluster.iter().map(|&i| glyphs[i].clone()).collect();
            let region = geom::Region::from_paths(&group, geom::Fill::NonZero).map_err(|e| format!("weld: {e}"))?;
            out.extend(region.to_path().iter());
        }
        return Ok(out);
    }
    for g in glyphs {
        out.extend(g);
    }
    Ok(out)
}

/// Groups of glyph indices whose bounding boxes touch (transitively), in order of their first glyph.
fn touching(glyphs: &[BezPath]) -> Vec<Vec<usize>> {
    let boxes: Vec<Option<kurbo::Rect>> = glyphs.iter().map(geom::bounds).collect();
    let mut parent: Vec<usize> = (0..glyphs.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut regions: Vec<Option<geom::Region>> = vec![None; glyphs.len()];
    let mut region = |i: usize| -> geom::Region {
        regions[i]
            .get_or_insert_with(|| geom::Region::from_path(&glyphs[i], geom::Fill::NonZero).unwrap_or_default())
            .clone()
    };
    for i in 0..glyphs.len() {
        for j in i + 1..glyphs.len() {
            // Glyphs already in one group need no contact test (the costly part: stacked glyphs would test every
            // pair).
            if let (Some(a), Some(b)) = (boxes[i], boxes[j])
                && root(&mut parent, i) != root(&mut parent, j)
                && a.x0 <= b.x1
                && b.x0 <= a.x1
                && a.y0 <= b.y1
                && b.y0 <= a.y1
                && region(i).boolean(&region(j), geom::BoolOp::Intersect).is_ok_and(|r| !r.is_empty())
            {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                parent[ri.max(rj)] = ri.min(rj);
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (i, b) in boxes.iter().enumerate() {
        if b.is_none() {
            continue;
        }
        let r = root(&mut parent, i);
        let k = *slot.entry(r).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[k].push(i);
    }
    groups
}
