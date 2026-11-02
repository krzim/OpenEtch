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
//! every line, and a piece that still reaches the border then is open to the outside: the point is not enclosed. Lines
//! are kept in chunks of at most [`CHUNK`] segments in a uniform grid ([`Boundaries`]), so a window only visits the
//! lines near it and a click in a small area of a large document stays cheap.
//!
//! **Gaps.** With a gap tolerance `g > 0`, openings narrower than `g` are closed: every line is thickened to a band
//! `g` wide, the free space around the point is taken (so it cannot leak through a narrower gap), grown back by `g/2`
//! with round joins, and clipped to the exact face. The result follows the lines to within the flattening tolerance
//! except at the closed gaps (a round bulge at most `g/2` deep) and in corners sharper than the band, which come
//! back rounded with radius about `g/2`.
//!
//! Results are polygons ([`Region`] shapes: one outer contour, counter-clockwise, and its holes, clockwise).

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::float::slice::FloatSlice;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, StrokeStyle};
use kurbo::{ParamCurveNearest, Point, Rect};

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

/// Half-size of the first window around a point, mm.
const FIRST_HALF: f64 = 8.0;

/// Faces and holes smaller than this (mm²) are slivers: dropped from results.
const MIN_AREA: f64 = 1e-6;

/// Options of [`fill_regions`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillOptions {
    /// Close openings narrower than this, mm (0: lines must meet exactly). At most [`MAX_GAP_MM`].
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
        check_options(opts)?;
        if !(p.x.is_finite() && p.y.is_finite()) {
            return invalid("the point must have finite coordinates");
        }
        check_range(&[arr(p)])?;
        let Some(all) = self.bounds else { return Ok(Err(Miss::NotEnclosed)) };
        if !all.inflate(ON_LINE_MM, ON_LINE_MM).contains(p) {
            return Ok(Err(Miss::NotEnclosed));
        }
        let gap = opts.gap_mm;
        let margin = 1.0 + 2.0 * gap;
        let mut half = FIRST_HALF.max(8.0 * gap);
        loop {
            let mut w = Rect::new(p.x - half, p.y - half, p.x + half, p.y + half);
            let last = w.contains(Point::new(all.x0 - margin, all.y0 - margin)) && w.contains(Point::new(all.x1 + margin, all.y1 + margin));
            if last {
                w = all.inflate(margin, margin).union_pt(p);
            }
            let lines = self.lines_in(w);
            match face_in_window(p, w, &lines, gap)? {
                Window::Face(face) => return Ok(Ok(face)),
                Window::OnLine => return Ok(Err(Miss::OnLine)),
                Window::Border if last => return Ok(Err(Miss::NotEnclosed)),
                Window::Border => half *= 4.0,
            }
        }
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
    /// The piece around the point reaches the window's border: grow the window, or it is open.
    Border,
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
    if near_line(p, lines, ON_LINE_MM.max(gap / 2.0)) {
        return Ok(Window::OnLine);
    }
    let exact = exact_piece(p, w, lines)?;
    if gap <= 0.0 {
        return Ok(match exact {
            None => Window::OnLine,
            Some(f) if !clear_of_border(contour_bbox(&f[0]), w, 0.0) => Window::Border,
            Some(f) => Window::Face(f),
        });
    }
    // Thicken every line to a band `gap` wide and take the free space around the point.
    let strings: Vec<Contour> = lines.iter().map(|c| (*c).clone()).collect();
    let step = (8.0 * crate::TOLERANCE / (gap / 2.0).max(crate::TOLERANCE)).sqrt().clamp(0.01 * std::f64::consts::PI, 0.25 * std::f64::consts::PI);
    let style = StrokeStyle::new(gap).line_join(LineJoin::Round(step)).start_cap(LineCap::Round(step)).end_cap(LineCap::Round(step));
    let bands = if strings.is_empty() { Vec::new() } else { guard("fill region", || strings.stroke(style, false))? };
    let domain = vec![rect_contour(w)];
    let free = if bands.is_empty() {
        vec![domain]
    } else {
        guard("fill region", || vec![domain].overlay(&bands, OverlayRule::Difference, FillRule::NonZero))?
    };
    let Some(core) = tidy(free).into_iter().find(|s| shape_contains(s, p)) else { return Ok(Window::OnLine) };
    if !clear_of_border(contour_bbox(&core[0]), w, 0.0) {
        return Ok(Window::Border);
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
        return Ok(Window::Border);
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
    for &p in points {
        if let Some(k) = full.iter().position(|f| shape_contains(f, p)) {
            out.points.push(PointResult::Face(k));
            continue;
        }
        match bounds.face_at(p, opts)? {
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
}
