//! Fill Region (a paint bucket): the closed area around a point that the boundary lines enclose, as a new shape.
//!
//! The boundary lines are polylines (curves flattened at [`TOLERANCE`](crate::TOLERANCE) by the caller), open or
//! closed, in any number and in any overlap. Together they cut the plane into faces (the planar arrangement:
//! crossings, T-junctions, shared and duplicated edges all split or merge as they should). The face that contains a
//! point is what the bucket fills: its outer contour plus, as holes, every island inside it (a pupil drawn inside an
//! eye outline leaves a hole in the eye white). Lines that end inside a face without closing anything (a dead end, a
//! stray stroke) do not split it.
//!
//! **How.** The arrangement is not built as a whole. A square window around the point is sliced by every line that
//! reaches into it (`i_overlay`'s slice, which resolves crossings and overlaps on its integer grid); the piece that
//! holds the point is the face when it stays clear of the window's border, because lines that never enter the window
//! cannot cut or touch a piece inside it. Otherwise the window grows (x4) until the piece fits or the window holds
//! every line (a window as wide as the drawing is the last one), and a piece that still reaches the border then is
//! open to the outside: the point is not enclosed. Lines are kept in chunks of at most [`CHUNK`] segments in a uniform
//! grid ([`Boundaries`]), so a window only visits the lines near it and a click in a small area of a large document
//! stays cheap.
//!
//! **Open areas fail fast.** A point outside every closed area would grow the window until it holds every line,
//! which on a large document slices all of it. So once a piece reaches its window's border, a coarse map of the
//! free space (`Reach`: grid cells no line comes near, joined to the outside of the drawing through such cells) is
//! built, once per call: a piece that touches one of those cells is open, and the answer is [`Miss::NotEnclosed`]
//! straight away. The map is conservative (a cell any line comes near is not free), so it never calls a closed area
//! open; it only saves work.
//!
//! **Gaps.** With a gap tolerance `g > 0`, openings narrower than `g` are closed: every line is thickened to a band
//! `g` wide, the free space around the point is taken (so it cannot leak through a narrower gap), grown back by `g/2`
//! with round joins, and clipped to the exact face. Only lines within `g/2` of the exact face can thicken into it, so
//! the lines are clipped to that neighbourhood before they are thickened (the inside of every island is left out,
//! which on a large drawing is most of the work). The result follows the lines to within the flattening tolerance
//! except at the closed gaps (a round bulge at most `g/2` deep) and in corners sharper than the band, which come
//! back rounded with radius about `g/2`.
//!
//! Results are polygons ([`Region`] shapes: one outer contour, counter-clockwise, and its holes, clockwise).

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::clip::FloatClip;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::float::slice::FloatSlice;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, StrokeStyle};
use i_overlay::string::clip::ClipRule;
use kurbo::{ParamCurveNearest, Point, Rect};
use std::cell::OnceCell;

use crate::flatten::{Polyline, point_in_ring};
use crate::offset::{Join, offset_region};
use crate::region::{Contour, Region, check_range};
use crate::{GeomError, Result, guard, invalid};

/// Most segments in one chunk of a boundary line (the unit the grid indexes and a window takes or leaves).
pub const CHUNK: usize = 32;

/// Most points one call may fill.
pub const MAX_POINTS_PER_CALL: usize = 256;

/// Largest gap tolerance, mm.
pub const MAX_GAP_MM: f64 = 10.0;

/// Gap tolerances below this (mm) are 0: a band that thin vanishes on the polygon engine's integer grid and would leave
/// every area open.
pub const MIN_GAP_MM: f64 = 1e-3;

/// Half-size of the first window around a point, mm.
const FIRST_HALF: f64 = 8.0;

/// Faces and holes smaller than this (mm²) are slivers: dropped from results.
const MIN_AREA: f64 = 1e-6;

/// Options of [`fill_regions`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillOptions {
    /// Close openings narrower than this, mm (0: lines must meet exactly). At most [`MAX_GAP_MM`]; below
    /// [`MIN_GAP_MM`] it is 0.
    pub gap_mm: f64,
    /// Islands inside the face become holes (true) or are filled over (false: the outer contour only).
    pub holes: bool,
}

impl Default for FillOptions {
    fn default() -> Self {
        FillOptions { gap_mm: 0.0, holes: true }
    }
}

/// Why a point has no region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// The area around the point is open to the outside (no closed boundary surrounds it).
    NotEnclosed,
    /// The point lies on a boundary line (or, with a gap tolerance, within half the gap of one).
    OnLine,
}

impl Miss {
    /// A sentence for the caller.
    pub fn message(self) -> &'static str {
        match self {
            Miss::NotEnclosed => "no closed area around this point (the lines around it leave an opening; a gap tolerance can close small ones)",
            Miss::OnLine => "the point is on a line; pick a point inside an area",
        }
    }
}

/// What one point gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointResult {
    /// The index of its face in [`FillResult::faces`] (several points in one face share it).
    Face(usize),
    Miss(Miss),
}

/// The regions for a list of points: distinct faces in order of first appearance, and what each point gave.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FillResult {
    /// Each face as a region of one shape (outer contour, then holes when [`FillOptions::holes`]).
    pub faces: Vec<Region>,
    pub points: Vec<PointResult>,
}

#[derive(Debug, Clone)]
struct Chunk {
    points: Contour,
    bbox: Rect,
}

/// Boundary lines prepared for repeated queries: chunked and indexed in a uniform grid.
#[derive(Debug, Clone)]
pub struct Boundaries {
    chunks: Vec<Chunk>,
    bounds: Option<Rect>,
    grid: Grid,
}

#[derive(Debug, Clone, Default)]
struct Grid {
    origin: Point,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
    /// Chunks spanning too many cells to register in each.
    big: Vec<u32>,
}

/// Most cells one chunk is registered in; larger ones go to [`Grid::big`].
const MAX_CELLS_PER_CHUNK: usize = 64;

impl Grid {
    fn new(chunks: &[Chunk], bounds: Rect) -> Grid {
        let n = chunks.len().max(1);
        let side = ((n as f64).sqrt().ceil() as usize).clamp(1, 512);
        let span = bounds.width().max(bounds.height()).max(1e-9);
        let cell = span / side as f64;
        let nx = ((bounds.width() / cell).floor() as usize + 1).min(side + 1);
        let ny = ((bounds.height() / cell).floor() as usize + 1).min(side + 1);
        let mut g = Grid { origin: Point::new(bounds.x0, bounds.y0), cell, nx, ny, cells: vec![Vec::new(); nx * ny], big: Vec::new() };
        for (i, c) in chunks.iter().enumerate() {
            let (x0, y0, x1, y1) = g.range(c.bbox);
            if (x1 - x0 + 1) * (y1 - y0 + 1) > MAX_CELLS_PER_CHUNK {
                g.big.push(i as u32);
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    g.cells[y * nx + x].push(i as u32);
                }
            }
        }
        g
    }

    /// Cell index range covered by `r` (clamped to the grid).
    fn range(&self, r: Rect) -> (usize, usize, usize, usize) {
        let ix = |v: f64, n: usize| (((v) / self.cell).floor().max(0.0) as usize).min(n - 1);
        (
            ix(r.x0 - self.origin.x, self.nx),
            ix(r.y0 - self.origin.y, self.ny),
            ix(r.x1 - self.origin.x, self.nx),
            ix(r.y1 - self.origin.y, self.ny),
        )
    }

    /// Indices of the chunks whose boxes may meet `r`, each once.
    fn query(&self, r: Rect) -> Vec<u32> {
        if self.nx == 0 {
            return Vec::new();
        }
        let (x0, y0, x1, y1) = self.range(r);
        let mut out: Vec<u32> = self.big.clone();
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&self.cells[y * self.nx + x]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

fn arr(p: Point) -> [f64; 2] {
    [p.x, p.y]
}

fn pt(a: [f64; 2]) -> Point {
    Point::new(a[0], a[1])
}

fn ring_pts(c: &Contour) -> Vec<Point> {
    c.iter().copied().map(pt).collect()
}

fn contour_bbox(c: &Contour) -> Rect {
    let mut it = c.iter();
    let f = it.next().copied().unwrap_or([0.0, 0.0]);
    it.fold(Rect::new(f[0], f[1], f[0], f[1]), |r, a| r.union_pt(pt(*a)))
}

fn ring_area(c: &Contour) -> f64 {
    crate::flatten::polygon_area(&ring_pts(c))
}

/// Whether `p` is inside a shape (inside its outer contour and outside every hole).
pub fn shape_contains(shape: &[Contour], p: Point) -> bool {
    let Some((outer, holes)) = shape.split_first() else { return false };
    point_in_ring(p, &ring_pts(outer)) && !holes.iter().any(|h| point_in_ring(p, &ring_pts(h)))
}

impl Boundaries {
    /// Index boundary polylines (closed ones are closed back to their first point). Fails on coordinates beyond
    /// [`MAX_COORD`](crate::MAX_COORD).
    pub fn new(lines: &[Polyline]) -> Result<Boundaries> {
        let mut chunks = Vec::new();
        for l in lines {
            let drawn: Contour = l.drawn().into_iter().map(arr).collect();
            if drawn.len() < 2 {
                continue;
            }
            check_range(&drawn)?;
            let mut start = 0;
            while start + 1 < drawn.len() {
                let end = (start + CHUNK).min(drawn.len() - 1);
                let points: Contour = drawn[start..=end].to_vec();
                let bbox = contour_bbox(&points);
                chunks.push(Chunk { points, bbox });
                start = end;
            }
        }
        let bounds = chunks.iter().map(|c| c.bbox).reduce(|a, b| a.union(b));
        let grid = bounds.map(|b| Grid::new(&chunks, b)).unwrap_or_default();
        Ok(Boundaries { chunks, bounds, grid })
    }

    /// Bounds of every line, `None` when there are none.
    pub fn bounds(&self) -> Option<Rect> {
        self.bounds
    }

    /// Number of indexed chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    fn lines_in(&self, w: Rect) -> Vec<&Contour> {
        self.grid.query(w).into_iter().map(|i| &self.chunks[i as usize]).filter(|c| overlaps(c.bbox, w)).map(|c| &c.points).collect()
    }

    /// The face around `p`, with every hole, or why there is none.
    pub fn face_at(&self, p: Point, opts: &FillOptions) -> Result<std::result::Result<Vec<Contour>, Miss>> {
        self.face_near(p, opts, &OnceCell::new()).map(|(r, _)| r)
    }

    /// [`Boundaries::face_at`] with the free-space map shared between points (built on first need, for this gap),
    /// and how many windows it sliced.
    fn face_near(&self, p: Point, opts: &FillOptions, reach: &OnceCell<Option<Reach>>) -> Result<(Answer, usize)> {
        check_options(opts)?;
        if !(p.x.is_finite() && p.y.is_finite()) {
            return invalid("the point must have finite coordinates");
        }
        check_range(&[arr(p)])?;
        let Some(all) = self.bounds else { return Ok((Err(Miss::NotEnclosed), 0)) };
        if !all.inflate(ON_LINE_MM, ON_LINE_MM).contains(p) {
            return Ok((Err(Miss::NotEnclosed), 0));
        }
        let gap = effective_gap(opts);
        let margin = 1.0 + 2.0 * gap;
        let mut half = FIRST_HALF.max(8.0 * gap);
        let mut windows = 0;
        loop {
            let mut w = Rect::new(p.x - half, p.y - half, p.x + half, p.y + half);
            // A window as wide as the whole drawing costs as much as the drawing itself: take that instead.
            let last = 2.0 * half >= all.width().max(all.height()) + 2.0 * margin
                || (w.contains(Point::new(all.x0 - margin, all.y0 - margin)) && w.contains(Point::new(all.x1 + margin, all.y1 + margin)));
            if last {
                w = all.inflate(margin, margin).union_pt(p);
            }
            let lines = self.lines_in(w);
            windows += 1;
            match face_in_window(p, w, &lines, gap)? {
                Window::Face(face) => return Ok((Ok(face), windows)),
                Window::OnLine => return Ok((Err(Miss::OnLine), windows)),
                Window::Border(_) if last => return Ok((Err(Miss::NotEnclosed), windows)),
                Window::Border(piece) => {
                    let map = reach.get_or_init(|| Reach::new(self, gap));
                    if map.as_ref().is_some_and(|m| m.is_out(p) || piece.is_some_and(|c| m.touches(&c))) {
                        return Ok((Err(Miss::NotEnclosed), windows));
                    }
                    half *= 4.0;
                }
            }
        }
    }
}

type Answer = std::result::Result<Vec<Contour>, Miss>;

/// The gap the geometry uses: [`FillOptions::gap_mm`], or 0 below [`MIN_GAP_MM`].
fn effective_gap(opts: &FillOptions) -> f64 {
    if opts.gap_mm < MIN_GAP_MM { 0.0 } else { opts.gap_mm }
}

/// Most cells along each side of the free-space map.
const REACH_SIDE: f64 = 1024.0;

/// Smallest cell of the free-space map, mm.
const REACH_MIN_CELL: f64 = 0.05;

/// Most line samples the free-space map rasterizes; a drawing with more gets no map (windows grow as without it).
const REACH_MAX_SAMPLES: f64 = 8.0e6;

/// A coarse map of the free space that is joined to the outside of the drawing (see the module docs): `out` marks the
/// cells that no line (thickened by half the gap, plus a margin) comes near and that connect, through such cells, to
/// the ring of empty cells around every line. Any point in such a cell is open to the outside.
struct Reach {
    origin: Point,
    cell: f64,
    nx: usize,
    ny: usize,
    out: Vec<bool>,
}

impl Reach {
    fn new(b: &Boundaries, gap: f64) -> Option<Reach> {
        let all = b.bounds?;
        let cell = (all.width().max(all.height()) / REACH_SIDE).max(REACH_MIN_CELL);
        let pad = gap / 2.0 + 2.0 * cell;
        let g = all.inflate(pad, pad);
        let nx = (g.width() / cell).ceil() as usize + 1;
        let ny = (g.height() / cell).ceil() as usize + 1;
        // Samples every half cell along each segment: every point of a line is within a quarter cell of one.
        let step = cell / 2.0;
        let segs = || b.chunks.iter().flat_map(|c| c.points.windows(2)).map(|s| (pt(s[0]), pt(s[1])));
        let samples: f64 = segs().map(|(a, z)| (a.distance(z) / step).ceil() + 1.0).sum();
        if samples > REACH_MAX_SAMPLES {
            return None;
        }
        let mut occ = vec![false; nx * ny];
        // A quarter cell to cover the line between samples, and a margin of a twentieth of a cell (well above the
        // polygon engine's grid) so a free cell keeps clear of every line.
        let h = cell * 0.3 + 1e-6;
        let ix = |v: f64, n: usize| ((v / cell).floor().max(0.0) as usize).min(n - 1);
        for (a, z) in segs() {
            let n = ((a.distance(z) / step).ceil() as usize).max(1);
            for k in 0..=n {
                let q = a.lerp(z, k as f64 / n as f64);
                let (x0, x1) = (ix(q.x - h - g.x0, nx), ix(q.x + h - g.x0, nx));
                let (y0, y1) = (ix(q.y - h - g.y0, ny), ix(q.y + h - g.y0, ny));
                for y in y0..=y1 {
                    occ[y * nx + x0..=y * nx + x1].fill(true);
                }
            }
        }
        if gap > 0.0 {
            dilate(&mut occ, nx, ny, (gap / 2.0 / cell).floor() as usize + 1);
        }
        let mut out = vec![false; nx * ny];
        let mut stack: Vec<usize> = Vec::new();
        let seed = |i: usize, out: &mut Vec<bool>, stack: &mut Vec<usize>| {
            if !occ[i] && !out[i] {
                out[i] = true;
                stack.push(i);
            }
        };
        for x in 0..nx {
            seed(x, &mut out, &mut stack);
            seed((ny - 1) * nx + x, &mut out, &mut stack);
        }
        for y in 0..ny {
            seed(y * nx, &mut out, &mut stack);
            seed(y * nx + nx - 1, &mut out, &mut stack);
        }
        while let Some(i) = stack.pop() {
            let (x, y) = (i % nx, i / nx);
            if x > 0 {
                seed(i - 1, &mut out, &mut stack);
            }
            if x + 1 < nx {
                seed(i + 1, &mut out, &mut stack);
            }
            if y > 0 {
                seed(i - nx, &mut out, &mut stack);
            }
            if y + 1 < ny {
                seed(i + nx, &mut out, &mut stack);
            }
        }
        Some(Reach { origin: Point::new(g.x0, g.y0), cell, nx, ny, out })
    }

    /// Whether `q` lies in a free cell joined to the outside (beyond the map counts: no line is there).
    fn is_out(&self, q: Point) -> bool {
        let (fx, fy) = ((q.x - self.origin.x) / self.cell, (q.y - self.origin.y) / self.cell);
        if !(fx >= 0.0 && fy >= 0.0 && fx < self.nx as f64 && fy < self.ny as f64) {
            return true;
        }
        self.out[fy as usize * self.nx + fx as usize]
    }

    /// Whether the outline of a piece passes through a free cell joined to the outside. Its points on a line are in
    /// cells the line marked, so a hit is where the piece meets its window's border in open space.
    fn touches(&self, outer: &Contour) -> bool {
        let n = outer.len();
        (0..n).any(|i| {
            let (a, z) = (pt(outer[i]), pt(outer[(i + 1) % n]));
            let k = ((a.distance(z) / (self.cell / 2.0)).ceil() as usize).clamp(1, 1 << 20);
            (0..=k).any(|j| self.is_out(a.lerp(z, j as f64 / k as f64)))
        })
    }
}

/// Mark every cell within `k` cells (along each axis) of a marked one.
fn dilate(occ: &mut [bool], nx: usize, ny: usize, k: usize) {
    fn line(get: impl Fn(usize) -> bool, n: usize, k: usize, mut set: impl FnMut(usize, bool)) {
        let mut near = vec![false; n];
        let mut last: Option<usize> = None;
        for (i, v) in near.iter_mut().enumerate() {
            if get(i) {
                last = Some(i);
            }
            *v = last.is_some_and(|l| i - l <= k);
        }
        let mut next: Option<usize> = None;
        for i in (0..n).rev() {
            if get(i) {
                next = Some(i);
            }
            set(i, near[i] || next.is_some_and(|m| m - i <= k));
        }
    }
    let mut tmp = vec![false; occ.len()];
    for y in 0..ny {
        line(|x| occ[y * nx + x], nx, k, |x, v| tmp[y * nx + x] = v);
    }
    for x in 0..nx {
        line(|y| tmp[y * nx + x], ny, k, |y, v| occ[y * nx + x] = v);
    }
}

fn check_options(opts: &FillOptions) -> Result<()> {
    if !(opts.gap_mm.is_finite() && (0.0..=MAX_GAP_MM).contains(&opts.gap_mm)) {
        return invalid(format!("the gap tolerance must be between 0 and {MAX_GAP_MM} mm (got {})", opts.gap_mm));
    }
    Ok(())
}

enum Window {
    Face(Vec<Contour>),
    OnLine,
    /// The piece around the point reaches the window's border: grow the window, or it is open. With the piece's
    /// outer contour when that is what reached it.
    Border(Option<Contour>),
}

fn rect_contour(w: Rect) -> Contour {
    vec![[w.x0, w.y0], [w.x1, w.y0], [w.x1, w.y1], [w.x0, w.y1]]
}

/// Whether `b`, grown by `pad`, stays strictly inside `w`.
fn clear_of_border(b: Rect, w: Rect, pad: f64) -> bool {
    let eps = 1e-9 * (1.0 + w.width().max(w.height()));
    b.x0 - pad > w.x0 + eps && b.y0 - pad > w.y0 + eps && b.x1 + pad < w.x1 - eps && b.y1 + pad < w.y1 - eps
}

/// Drop sliver contours (and shapes whose outer contour is one); outer contours stay first.
fn tidy(shapes: Vec<Vec<Contour>>) -> Vec<Vec<Contour>> {
    shapes
        .into_iter()
        .filter_map(|s| {
            let mut it = s.into_iter();
            let outer = it.next().filter(|c| c.len() >= 3 && ring_area(c).abs() > MIN_AREA)?;
            Some(std::iter::once(outer).chain(it.filter(|c| c.len() >= 3 && ring_area(c).abs() > MIN_AREA)).collect())
        })
        .collect()
}

/// The exact face around `p` among the pieces of `w` cut by `lines`: `None` when `p` is on a line.
fn exact_piece(p: Point, w: Rect, lines: &[&Contour]) -> Result<Option<Vec<Contour>>> {
    let domain = rect_contour(w);
    let strings: Vec<Contour> = lines.iter().map(|c| (*c).clone()).collect();
    let pieces = guard("fill region", || domain.slice_by(&strings, FillRule::EvenOdd))?;
    let Some(face) = pieces.into_iter().find(|s| shape_contains(s, p)) else { return Ok(None) };
    // A line that ends inside the face can leave an out-and-back spike in its contour; a non-zero simplify removes it.
    let clean = guard("fill region", || vec![face].simplify_shape(FillRule::NonZero))?;
    Ok(tidy(clean).into_iter().find(|s| shape_contains(s, p)))
}

/// Points this close to a line (mm) are on it.
const ON_LINE_MM: f64 = 1e-6;

/// Whether `p` lies within `d` of any segment of `lines`.
fn near_line(p: Point, lines: &[&Contour], d: f64) -> bool {
    lines.iter().any(|c| {
        c.windows(2).any(|s| {
            let (a, b) = (pt(s[0]), pt(s[1]));
            kurbo::Line::new(a, b).nearest(p, 1e-9).distance_sq <= d * d
        })
    })
}

fn face_in_window(p: Point, w: Rect, lines: &[&Contour], gap: f64) -> Result<Window> {
    face_in_window_with(p, w, lines, gap, true)
}

/// [`face_in_window`]; `clip` false strokes every line (the tests compare).
fn face_in_window_with(p: Point, w: Rect, lines: &[&Contour], gap: f64, clip: bool) -> Result<Window> {
    if near_line(p, lines, ON_LINE_MM.max(gap / 2.0)) {
        return Ok(Window::OnLine);
    }
    let exact = exact_piece(p, w, lines)?;
    if gap <= 0.0 {
        return Ok(match exact {
            None => Window::OnLine,
            Some(mut f) if !clear_of_border(contour_bbox(&f[0]), w, 0.0) => Window::Border(Some(f.swap_remove(0))),
            Some(f) => Window::Face(f),
        });
    }
    // Thicken the lines to bands `gap` wide and take the free space around the point. That free space stays inside
    // the exact piece (its own lines are banded), so only lines within half the gap of the piece can matter: clip the
    // lines to that neighbourhood first, which spares stroking the inside of every island (on a large drawing the
    // stroke is the costly step). The neighbourhood is taken a little wider, as its round corners are chords.
    let mut strings: Vec<Contour> = lines.iter().map(|c| (*c).clone()).collect();
    if let Some(f) = exact.as_ref().filter(|_| clip) {
        let wide = gap / 2.0 + (0.05 * gap).max(2.0 * crate::TOLERANCE);
        let near = offset_region(&Region { shapes: vec![f.clone()] }, wide, Join::Round)?;
        let rule = ClipRule { invert: false, boundary_included: true };
        strings = guard("fill region", || strings.clip_by(&near.shapes, FillRule::NonZero, rule))?;
    }
    let step = (8.0 * crate::TOLERANCE / (gap / 2.0).max(crate::TOLERANCE)).sqrt().clamp(0.01 * std::f64::consts::PI, 0.25 * std::f64::consts::PI);
    let style = StrokeStyle::new(gap).line_join(LineJoin::Round(step)).start_cap(LineCap::Round(step)).end_cap(LineCap::Round(step));
    let bands = if strings.is_empty() { Vec::new() } else { guard("fill region", || strings.stroke(style, false))? };
    let domain = vec![rect_contour(w)];
    let free = if bands.is_empty() {
        vec![domain]
    } else {
        guard("fill region", || vec![domain].overlay(&bands, OverlayRule::Difference, FillRule::NonZero))?
    };
    let Some(mut core) = tidy(free).into_iter().find(|s| shape_contains(s, p)) else { return Ok(Window::OnLine) };
    if !clear_of_border(contour_bbox(&core[0]), w, 0.0) {
        return Ok(Window::Border(Some(core.swap_remove(0))));
    }
    let grown = offset_region(&Region { shapes: vec![core] }, gap / 2.0, Join::Round)?;
    let clipped = match exact {
        Some(f) => {
            let shapes = guard("fill region", || grown.shapes.overlay(&vec![f], OverlayRule::Intersect, FillRule::NonZero))?;
            tidy(shapes)
        }
        None => tidy(grown.shapes),
    };
    let pick = clipped.iter().position(|s| shape_contains(s, p)).or_else(|| {
        // The point sat in a sliver the clip removed: take the largest piece.
        (0..clipped.len()).max_by(|&a, &b| ring_area(&clipped[a][0]).abs().total_cmp(&ring_area(&clipped[b][0]).abs()))
    });
    let Some(i) = pick else { return Ok(Window::OnLine) };
    let face = clipped[i].clone();
    if !clear_of_border(contour_bbox(&face[0]), w, gap) {
        return Ok(Window::Border(None));
    }
    Ok(Window::Face(face))
}

/// The region around each point (see the module docs). Points in the same face share it; faces come back in the
/// order of their first point. Fails on bad options, non-finite points or more than [`MAX_POINTS_PER_CALL`] points;
/// a point with no region is a [`PointResult::Miss`], not an error.
pub fn fill_regions(bounds: &Boundaries, points: &[Point], opts: &FillOptions) -> Result<FillResult> {
    check_options(opts)?;
    if points.len() > MAX_POINTS_PER_CALL {
        return invalid(format!("at most {MAX_POINTS_PER_CALL} points at once (got {})", points.len()));
    }
    let mut out = FillResult::default();
    // Each face with its holes, for telling whether a later point falls in a face already found.
    let mut full: Vec<Vec<Contour>> = Vec::new();
    let reach = OnceCell::new();
    for &p in points {
        if let Some(k) = full.iter().position(|f| shape_contains(f, p)) {
            out.points.push(PointResult::Face(k));
            continue;
        }
        match bounds.face_near(p, opts, &reach)?.0 {
            Ok(face) => {
                let shown = if opts.holes { face.clone() } else { vec![face[0].clone()] };
                full.push(face);
                out.faces.push(Region { shapes: vec![shown] });
                out.points.push(PointResult::Face(out.faces.len() - 1));
            }
            Err(m) => out.points.push(PointResult::Miss(m)),
        }
    }
    Ok(out)
}

/// [`fill_regions`] straight from polylines.
pub fn fill_regions_in(lines: &[Polyline], points: &[Point], opts: &FillOptions) -> Result<FillResult> {
    let b = Boundaries::new(lines)?;
    fill_regions(&b, points, opts)
}

impl From<Miss> for GeomError {
    fn from(m: Miss) -> Self {
        GeomError::Invalid(m.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatten::flatten;
    use kurbo::{BezPath, Circle, Shape};

    fn lines_of(paths: &[BezPath]) -> Vec<Polyline> {
        paths.iter().flat_map(|p| flatten(p, crate::TOLERANCE).unwrap()).collect()
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.01)
    }

    fn line(pts: &[(f64, f64)]) -> BezPath {
        let mut p = BezPath::new();
        p.move_to(pts[0]);
        for &q in &pts[1..] {
            p.line_to(q);
        }
        p
    }

    fn circle(c: (f64, f64), r: f64) -> BezPath {
        Circle::new(c, r).to_path(0.001)
    }

    fn one(paths: &[BezPath], at: (f64, f64), opts: FillOptions) -> std::result::Result<Region, Miss> {
        let r = fill_regions_in(&lines_of(paths), &[Point::new(at.0, at.1)], &opts).unwrap();
        match r.points[0] {
            PointResult::Face(i) => Ok(r.faces[i].clone()),
            PointResult::Miss(m) => Err(m),
        }
    }

    fn area_at(paths: &[BezPath], at: (f64, f64)) -> f64 {
        one(paths, at, FillOptions::default()).unwrap().area()
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn square_inside_and_outside() {
        let sq = [rect(0.0, 0.0, 10.0, 10.0)];
        let f = one(&sq, (5.0, 5.0), FillOptions::default()).unwrap();
        assert!(close(f.area(), 100.0, 1e-6), "{}", f.area());
        assert_eq!(f.shapes.len(), 1);
        assert_eq!(f.shapes[0].len(), 1, "no holes");
        assert_eq!(one(&sq, (20.0, 5.0), FillOptions::default()), Err(Miss::NotEnclosed));
        assert_eq!(one(&sq, (-0.5, 5.0), FillOptions::default()), Err(Miss::NotEnclosed));
        assert_eq!(one(&sq, (10.0, 5.0), FillOptions::default()), Err(Miss::OnLine));
        assert_eq!(one(&[], (0.0, 0.0), FillOptions::default()), Err(Miss::NotEnclosed));
    }

    #[test]
    fn nested_shapes_leave_holes_or_fill_over_them() {
        let paths = [rect(0.0, 0.0, 10.0, 10.0), rect(3.0, 3.0, 7.0, 7.0)];
        let ring = one(&paths, (1.0, 1.0), FillOptions::default()).unwrap();
        assert!(close(ring.area(), 84.0, 1e-6), "{}", ring.area());
        assert_eq!(ring.shapes[0].len(), 2, "outer and one hole");
        let full = one(&paths, (1.0, 1.0), FillOptions { holes: false, ..Default::default() }).unwrap();
        assert!(close(full.area(), 100.0, 1e-6), "{}", full.area());
        assert!(close(area_at(&paths, (5.0, 5.0)), 16.0, 1e-6));
    }

    #[test]
    fn shared_edges_and_duplicates() {
        // Two squares sharing an edge, one drawn twice: three faces' worth of lines, each square its own face.
        let paths = [rect(0.0, 0.0, 10.0, 10.0), rect(10.0, 0.0, 20.0, 10.0), rect(10.0, 0.0, 20.0, 10.0)];
        assert!(close(area_at(&paths, (5.0, 5.0)), 100.0, 1e-6));
        assert!(close(area_at(&paths, (15.0, 5.0)), 100.0, 1e-6));
        // Overlapping squares: the lens in the middle is its own face.
        let ov = [rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 5.0, 15.0, 15.0)];
        assert!(close(area_at(&ov, (7.0, 7.0)), 25.0, 1e-6));
        assert!(close(area_at(&ov, (2.0, 2.0)), 75.0, 1e-6));
    }

    #[test]
    fn t_junctions_and_open_lines_divide() {
        // A square cut by a line from edge to edge (T-junctions at both ends), and a line through and beyond.
        let paths = [rect(0.0, 0.0, 10.0, 10.0), line(&[(4.0, 0.0), (4.0, 10.0)]), line(&[(-5.0, 6.0), (15.0, 6.0)])];
        assert!(close(area_at(&paths, (1.0, 1.0)), 24.0, 1e-6));
        assert!(close(area_at(&paths, (8.0, 8.0)), 24.0, 1e-6));
        assert!(close(area_at(&paths, (8.0, 1.0)), 36.0, 1e-6));
        // Four open lines crossing like a #: the middle cell is closed, the arms are not.
        let hash = [
            line(&[(0.0, 3.0), (10.0, 3.0)]),
            line(&[(0.0, 7.0), (10.0, 7.0)]),
            line(&[(3.0, 0.0), (3.0, 10.0)]),
            line(&[(7.0, 0.0), (7.0, 10.0)]),
        ];
        assert!(close(area_at(&hash, (5.0, 5.0)), 16.0, 1e-6));
        assert_eq!(one(&hash, (5.0, 1.0), FillOptions::default()), Err(Miss::NotEnclosed));
    }

    #[test]
    fn dead_ends_do_not_split_or_spike() {
        // A stroke that enters the square and stops, and a loose one inside.
        let paths = [rect(0.0, 0.0, 10.0, 10.0), line(&[(0.0, 5.0), (6.0, 5.0)]), line(&[(2.0, 2.0), (3.0, 3.0)])];
        let f = one(&paths, (8.0, 8.0), FillOptions::default()).unwrap();
        assert!(close(f.area(), 100.0, 1e-6), "{}", f.area());
        // The outline is the square itself, without a slit out and back along the dead end.
        let outer = &f.shapes[0][0];
        assert!(outer.len() <= 5, "{outer:?}");
    }

    #[test]
    fn gaps_close_only_with_a_tolerance() {
        // A square whose right edge stops 0.2 mm short of the top.
        let open = [line(&[(10.0, 9.8), (10.0, 0.0), (0.0, 0.0), (0.0, 10.0), (10.0, 10.0)])];
        assert_eq!(one(&open, (5.0, 5.0), FillOptions::default()), Err(Miss::NotEnclosed));
        let small = FillOptions { gap_mm: 0.1, ..Default::default() };
        assert_eq!(one(&open, (5.0, 5.0), small), Err(Miss::NotEnclosed), "0.1 mm does not close 0.2 mm");
        let f = one(&open, (5.0, 5.0), FillOptions { gap_mm: 0.3, ..Default::default() }).unwrap();
        assert!(close(f.area(), 100.0, 0.1), "{}", f.area());
        let b = f.bounds().unwrap();
        assert!(b.x0 > -0.02 && b.y0 > -0.02 && b.x1 < 10.02 && b.y1 < 10.02, "{b:?}");
        // Two closed shapes that nearly touch: the gap between them is closed off too.
        let pair = [
            rect(0.0, 0.0, 10.0, 10.0),
            line(&[(10.0, 0.0), (20.0, 0.0), (20.0, 10.0), (10.0, 10.0), (10.0, 6.0)]),
            line(&[(10.0, 5.9), (10.0, 0.0)]),
        ];
        assert!(close(area_at(&pair, (15.0, 5.0)), 100.0, 1e-6), "a 0.1 mm gap into the left square is still a wall");
        let opts = FillOptions { gap_mm: 0.5, ..Default::default() };
        assert!(one(&pair, (15.0, 5.0), opts).is_ok());
        assert!(fill_regions_in(&[], &[], &FillOptions { gap_mm: 11.0, ..Default::default() }).is_err());
        assert!(fill_regions_in(&[], &[], &FillOptions { gap_mm: f64::NAN, ..Default::default() }).is_err());
    }

    #[test]
    fn gap_with_an_island_keeps_the_hole() {
        let paths = [line(&[(10.0, 9.9), (10.0, 0.0), (0.0, 0.0), (0.0, 10.0), (10.0, 10.0)]), circle((5.0, 5.0), 2.0)];
        let f = one(&paths, (1.0, 1.0), FillOptions { gap_mm: 0.3, ..Default::default() }).unwrap();
        assert_eq!(f.shapes[0].len(), 2, "outer and the circle's hole");
        let expect = 100.0 - std::f64::consts::PI * 4.0;
        assert!(close(f.area(), expect, 0.2), "{} vs {expect}", f.area());
    }

    #[test]
    fn curves_are_followed_within_tolerance() {
        let f = one(&[circle((0.0, 0.0), 5.0)], (1.0, 1.0), FillOptions::default()).unwrap();
        // Flattened at 0.01 mm the circle loses about 2/3 * 0.01 mm of depth along its outline.
        assert!(close(f.area(), std::f64::consts::PI * 25.0, 0.3), "{}", f.area());
        // Two crossing circles: the lens.
        let two = [circle((0.0, 0.0), 5.0), circle((6.0, 0.0), 5.0)];
        let lens = area_at(&two, (3.0, 0.0));
        let d = 6.0f64;
        let r = 5.0f64;
        let expect = 2.0 * r * r * (d / (2.0 * r)).acos() - d / 2.0 * (4.0 * r * r - d * d).sqrt();
        assert!(close(lens, expect, 0.3), "{lens} vs {expect}");
    }

    #[test]
    fn self_intersecting_path_splits_into_lobes() {
        // A bow tie: one closed path crossing itself at (5, 5).
        let bow = [line(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0), (0.0, 0.0)])];
        assert!(close(area_at(&bow, (2.0, 5.0)), 25.0, 1e-6));
        assert!(close(area_at(&bow, (8.0, 5.0)), 25.0, 1e-6));
        assert_eq!(one(&bow, (5.0, 8.0), FillOptions::default()), Err(Miss::NotEnclosed));
    }

    #[test]
    fn tangent_circles_and_slivers() {
        // A circle touching the square's edge from inside: the square's face keeps the circle as a hole that touches.
        let paths = [rect(0.0, 0.0, 10.0, 10.0), circle((5.0, 3.0), 3.0)];
        let outer = area_at(&paths, (8.0, 8.0));
        let expect = 100.0 - std::f64::consts::PI * 9.0;
        assert!(close(outer, expect, 0.1), "{outer} vs {expect}");
        // Two almost identical squares (1 µm apart): the sliver between them does not stop a click inside.
        let near = [rect(0.0, 0.0, 10.0, 10.0), rect(0.0, 0.0, 10.0, 10.001)];
        assert!(close(area_at(&near, (5.0, 5.0)), 100.0, 1e-6));
    }

    #[test]
    fn ghost_eye_with_pupil() {
        // An eye outline (an ellipse) with a pupil (a circle) inside, on a ghost body; the eye white is the ring.
        let body = line(&[(-20.0, -20.0), (20.0, -20.0), (20.0, 10.0), (0.0, 25.0), (-20.0, 10.0), (-20.0, -20.0)]);
        let eye = kurbo::Ellipse::new((-6.0, 5.0), (4.0, 3.0), 0.0).to_path(0.001);
        let pupil = circle((-5.0, 5.0), 1.2);
        let paths = [body, eye, pupil];
        let white = one(&paths, (-8.5, 5.0), FillOptions::default()).unwrap();
        let expect = std::f64::consts::PI * (12.0 - 1.44);
        assert!(close(white.area(), expect, 0.3), "{} vs {expect}", white.area());
        assert_eq!(white.shapes[0].len(), 2, "the pupil is a hole");
        let solid = one(&paths, (-8.5, 5.0), FillOptions { holes: false, ..Default::default() }).unwrap();
        assert!(close(solid.area(), std::f64::consts::PI * 12.0, 0.3), "{}", solid.area());
        // The pupil itself, and the body around the eye (with the eye as a hole).
        assert!(close(area_at(&paths, (-5.0, 5.0)), std::f64::consts::PI * 1.44, 0.1));
        let b = one(&paths, (10.0, -10.0), FillOptions::default()).unwrap();
        assert_eq!(b.shapes[0].len(), 2);
    }

    #[test]
    fn several_points_dedupe_faces() {
        let paths = [rect(0.0, 0.0, 10.0, 10.0), rect(20.0, 0.0, 30.0, 10.0)];
        let pts = [Point::new(1.0, 1.0), Point::new(25.0, 5.0), Point::new(9.0, 9.0), Point::new(50.0, 50.0)];
        let r = fill_regions_in(&lines_of(&paths), &pts, &FillOptions::default()).unwrap();
        assert_eq!(r.faces.len(), 2);
        assert_eq!(r.points, vec![PointResult::Face(0), PointResult::Face(1), PointResult::Face(0), PointResult::Miss(Miss::NotEnclosed)]);
        let many = vec![Point::new(1.0, 1.0); MAX_POINTS_PER_CALL + 1];
        assert!(fill_regions_in(&lines_of(&paths), &many, &FillOptions::default()).is_err());
        assert!(fill_regions_in(&lines_of(&paths), &[Point::new(f64::NAN, 0.0)], &FillOptions::default()).is_err());
    }

    #[test]
    fn far_geometry_needs_a_larger_window() {
        // A big frame (300 mm) around a small square: a click between them must find the frame's face with the square
        // as a hole, which the first small window cannot see whole.
        let paths = [rect(-150.0, -150.0, 150.0, 150.0), rect(0.0, 0.0, 2.0, 2.0)];
        let f = one(&paths, (5.0, 5.0), FillOptions::default()).unwrap();
        assert!(close(f.area(), 90000.0 - 4.0, 1e-3), "{}", f.area());
    }

    #[test]
    fn large_grid_of_cells_stays_local() {
        // 200 x 200 small squares (40,000 closed outlines): a click finds its own cell.
        let mut paths = Vec::new();
        for i in 0..200 {
            for j in 0..200 {
                let (x, y) = (i as f64 * 2.0, j as f64 * 2.0);
                paths.push(rect(x, y, x + 1.5, y + 1.5));
            }
        }
        let b = Boundaries::new(&lines_of(&paths)).unwrap();
        let pts = [Point::new(200.5, 200.5), Point::new(201.75, 201.75), Point::new(201.0, 201.0)];
        let r = fill_regions(&b, &pts, &FillOptions::default()).unwrap();
        assert_eq!(r.points, vec![PointResult::Face(0), PointResult::Miss(Miss::NotEnclosed), PointResult::Face(0)]);
        assert!(close(r.faces[0].area(), 2.25, 1e-6));
    }

    #[test]
    fn a_tiny_gap_tolerance_is_none() {
        // A positive gap far below the engine's grid used to thin every band to nothing and leave each area open.
        let sq = [rect(0.0, 0.0, 10.0, 10.0)];
        for gap_mm in [1e-12, 1e-6, MIN_GAP_MM / 2.0] {
            let f = one(&sq, (5.0, 5.0), FillOptions { gap_mm, ..Default::default() }).unwrap();
            assert!(close(f.area(), 100.0, 1e-6), "gap {gap_mm}: {}", f.area());
        }
    }

    /// The face (or miss) at `p` with the free-space map and without it, and the windows each sliced.
    fn with_and_without_map(b: &Boundaries, p: Point, opts: &FillOptions) -> ((Answer, usize), (Answer, usize)) {
        let with = b.face_near(p, opts, &OnceCell::new()).unwrap();
        let without = b.face_near(p, opts, &OnceCell::from(None)).unwrap();
        (with, without)
    }

    fn same_answer(a: &Answer, b: &Answer) -> bool {
        match (a, b) {
            (Ok(x), Ok(y)) => {
                let (x, y) = (Region { shapes: vec![x.clone()] }, Region { shapes: vec![y.clone()] });
                close(x.area(), y.area(), 1e-9) && x.shapes[0].len() == y.shapes[0].len()
            }
            (Err(x), Err(y)) => x == y,
            _ => false,
        }
    }

    #[test]
    fn open_points_fail_fast_in_a_large_drawing() {
        // Six detailed "coins" with open space between them: a point between them (or just outside one) is open,
        // and the free-space map says so after the first window instead of slicing every line.
        let mut paths = Vec::new();
        for c in 0..6 {
            let (cx, cy) = (60.0 * (c % 3) as f64, 60.0 * (c / 3) as f64);
            paths.push(circle((cx, cy), 20.0));
            for k in 0..120 {
                let a = k as f64 * 2.399963;
                let r = 18.0 * ((k as f64 + 0.5) / 120.0).sqrt();
                paths.push(circle((cx + r * a.cos(), cy + r * a.sin()), 0.3));
            }
        }
        let b = Boundaries::new(&lines_of(&paths)).unwrap();
        for gap_mm in [0.0, 0.5] {
            let opts = FillOptions { gap_mm, ..Default::default() };
            for p in [Point::new(30.0, 30.0), Point::new(20.6, 0.0), Point::new(-40.0, 50.0)] {
                let ((a, n), (b0, _)) = with_and_without_map(&b, p, &opts);
                assert_eq!(a, Err(Miss::NotEnclosed), "{p:?} gap {gap_mm}");
                assert!(same_answer(&a, &b0));
                assert!(n <= 1, "{p:?} gap {gap_mm}: {n} windows");
            }
            // Inside a coin, between the dots: the coin's face, the dots as holes, whatever the map says.
            let ((a, _), (b0, _)) = with_and_without_map(&b, Point::new(0.0, 19.5), &opts);
            assert!(a.is_ok() && same_answer(&a, &b0), "gap {gap_mm}");
        }
    }

    #[test]
    fn the_free_space_map_never_opens_a_closed_area() {
        // An empty 200 mm frame: the free cells inside it are not joined to the outside, so a point in it still finds
        // the frame. A 0.001 mm slit in a second frame opens it with no gap tolerance (narrower than any map cell).
        let frame = [rect(-100.0, -100.0, 100.0, 100.0)];
        let b = Boundaries::new(&lines_of(&frame)).unwrap();
        let ((a, n), _) = with_and_without_map(&b, Point::new(1.0, 1.0), &FillOptions::default());
        assert!(close(a.map(|f| Region { shapes: vec![f] }.area()).unwrap(), 40000.0, 1e-6));
        assert!(n > 1);
        let slit = [line(&[(100.0, 0.0005), (100.0, 100.0), (-100.0, 100.0), (-100.0, -100.0), (100.0, -100.0), (100.0, -0.0005)])];
        let b = Boundaries::new(&lines_of(&slit)).unwrap();
        let ((a, _), _) = with_and_without_map(&b, Point::new(1.0, 1.0), &FillOptions::default());
        assert_eq!(a, Err(Miss::NotEnclosed));
        let closed = with_and_without_map(&b, Point::new(1.0, 1.0), &FillOptions { gap_mm: 0.01, ..Default::default() }).0.0;
        assert!(closed.is_ok(), "a 0.01 mm gap closes the slit");

        // Random scribbles (open and closed, crossing, nested): the map changes how much is sliced, never the answer.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let (mut faces, mut open) = (0, 0);
        for round in 0..4 {
            let mut paths = Vec::new();
            for _ in 0..40 {
                let (cx, cy) = (rnd() * 120.0, rnd() * 120.0);
                match (rnd() * 3.0) as u32 {
                    0 => paths.push(circle((cx, cy), 1.0 + rnd() * 25.0)),
                    1 => paths.push(rect(cx, cy, cx + 1.0 + rnd() * 40.0, cy + 1.0 + rnd() * 40.0)),
                    _ => {
                        let pts: Vec<(f64, f64)> = (0..4).map(|_| (cx + rnd() * 50.0 - 25.0, cy + rnd() * 50.0 - 25.0)).collect();
                        paths.push(line(&pts));
                    }
                }
            }
            let b = Boundaries::new(&lines_of(&paths)).unwrap();
            for gap_mm in [0.0, 0.8] {
                let opts = FillOptions { gap_mm, ..Default::default() };
                for _ in 0..12 {
                    let p = Point::new(rnd() * 160.0 - 20.0, rnd() * 160.0 - 20.0);
                    let ((a, _), (b0, _)) = with_and_without_map(&b, p, &opts);
                    assert!(same_answer(&a, &b0), "round {round} gap {gap_mm} at {p:?}: {:?} vs {:?}", a.as_ref().map(|f| f.len()), b0.as_ref().map(|f| f.len()));
                    faces += usize::from(a.is_ok());
                    open += usize::from(a == Err(Miss::NotEnclosed));
                }
            }
        }
        assert!(faces >= 10 && open >= 10, "both kinds of point were tried: {faces} faces, {open} open");
    }

    #[test]
    fn gap_lines_far_from_the_area_are_left_out_without_changing_it() {
        // A frame with detailed islands (their insides never reach the frame's area), lines just outside and just
        // inside one island (within half the gap of the area, so they count) and a near-closed notch: stroking only
        // the lines near the area gives what stroking all of them gives.
        let mut paths = vec![
            rect(-30.0, -30.0, 70.0, 40.0),
            line(&[(30.1, -5.0), (30.1, 25.0)]),
            line(&[(29.9, -5.0), (29.9, 15.0)]),
            line(&[(-30.0, 10.0), (-10.0, 10.0), (-10.0, 20.0), (-29.7, 20.0)]),
        ];
        for c in 0..3 {
            let cx = 30.0 * c as f64;
            paths.push(rect(cx - 10.0, -10.0, cx, 20.0));
            for k in 0..40 {
                paths.push(circle((cx - 5.0 + (k % 5) as f64 - 2.0, (k / 5) as f64 * 3.0 - 6.0), 0.4));
            }
        }
        let lines = lines_of(&paths);
        let b = Boundaries::new(&lines).unwrap();
        let w = b.bounds().unwrap().inflate(3.0, 3.0);
        let all: Vec<&Contour> = b.chunks.iter().map(|c| &c.points).collect();
        for gap in [0.3, 1.0] {
            for p in [Point::new(-20.0, 0.0), Point::new(65.0, 35.0), Point::new(-25.0, 15.0)] {
                let get = |clip| match face_in_window_with(p, w, &all, gap, clip).unwrap() {
                    Window::Face(f) => Region { shapes: vec![f] },
                    _ => panic!("no face at {p:?}"),
                };
                let (fast, full) = (get(true), get(false));
                assert!(close(fast.area(), full.area(), 1e-6), "gap {gap} at {p:?}: {} vs {}", fast.area(), full.area());
                assert_eq!(fast.shapes[0].len(), full.shapes[0].len(), "gap {gap} at {p:?}");
            }
        }
    }
}
