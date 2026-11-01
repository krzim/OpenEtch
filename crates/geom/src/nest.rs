//! Bounding-box nesting (like LightBurn's Quick Nest): a greedy shelf packer. Parts are rectangles (their bounding
//! boxes, optionally in several rotations); they never interlock, so irregular parts pack less tightly than a true
//! nest would.

use kurbo::{Point, Rect};

/// One part to place: its bounding-box size in each allowed orientation.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// `(width, height)` per orientation, in the caller's orientation order.
    pub sizes: Vec<(f64, f64)>,
}

/// Where a part went: the orientation index and the lower-left corner of its box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub orientation: usize,
    pub min: Point,
}

struct Shelf {
    y: f64,
    height: f64,
    x: f64,
}

/// Pack `parts` into `region` with at least `spacing` mm between boxes, filling shelves from the bottom left (lower
/// Y first, +Y up). Tallest parts go first; each takes its lowest orientation that fits, preferring an existing
/// shelf. Parts that fit nowhere get `None`.
pub fn shelf_pack(parts: &[Part], region: Rect, spacing: f64) -> Vec<Option<Placement>> {
    let spacing = if spacing.is_finite() { spacing.max(0.0) } else { 0.0 };
    let mut order: Vec<usize> = (0..parts.len()).collect();
    let tallest = |p: &Part| p.sizes.iter().map(|s| s.1.min(s.0)).fold(0.0, f64::max);
    order.sort_by(|&a, &b| tallest(&parts[b]).total_cmp(&tallest(&parts[a])).then(a.cmp(&b)));
    let mut shelves: Vec<Shelf> = Vec::new();
    let mut out = vec![None; parts.len()];
    let eps = 1e-9;
    for i in order {
        let mut orients: Vec<usize> = (0..parts[i].sizes.len()).collect();
        orients.sort_by(|&a, &b| parts[i].sizes[a].1.total_cmp(&parts[i].sizes[b].1).then(a.cmp(&b)));
        let mut placed = None;
        'search: for &o in &orients {
            let (w, h) = parts[i].sizes[o];
            if !(w.is_finite() && h.is_finite()) || w > region.width() + eps || h > region.height() + eps {
                continue;
            }
            for s in &mut shelves {
                if h <= s.height + eps && s.x + w <= region.x1 + eps {
                    placed = Some(Placement { orientation: o, min: Point::new(s.x, s.y) });
                    s.x += w + spacing;
                    break 'search;
                }
            }
        }
        if placed.is_none() {
            for &o in &orients {
                let (w, h) = parts[i].sizes[o];
                if !(w.is_finite() && h.is_finite()) || w > region.width() + eps {
                    continue;
                }
                let y = shelves.last().map_or(region.y0, |s| s.y + s.height + spacing);
                if y + h <= region.y1 + eps {
                    shelves.push(Shelf { y, height: h, x: region.x0 + w + spacing });
                    placed = Some(Placement { orientation: o, min: Point::new(region.x0, y) });
                    break;
                }
            }
        }
        out[i] = placed;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_without_overlap_and_reports_misfits() {
        let parts: Vec<Part> = (0..20).map(|i| Part { sizes: vec![(10.0 + i as f64 % 3.0, 5.0), (5.0, 10.0 + i as f64 % 3.0)] }).collect();
        let region = Rect::new(0.0, 0.0, 50.0, 50.0);
        let out = shelf_pack(&parts, region, 1.0);
        let boxes: Vec<Rect> = out
            .iter()
            .zip(&parts)
            .filter_map(|(p, part)| p.map(|p| {
                let (w, h) = part.sizes[p.orientation];
                Rect::new(p.min.x, p.min.y, p.min.x + w, p.min.y + h)
            }))
            .collect();
        assert!(boxes.len() >= 15, "{}", boxes.len());
        for (i, a) in boxes.iter().enumerate() {
            assert!(a.x0 >= -1e-9 && a.y0 >= -1e-9 && a.x1 <= 50.0 + 1e-9 && a.y1 <= 50.0 + 1e-9);
            for b in &boxes[i + 1..] {
                let o = a.intersect(*b);
                assert!(o.width() <= 1e-9 || o.height() <= 1e-9, "{a:?} overlaps {b:?}");
            }
        }
        let big = shelf_pack(&[Part { sizes: vec![(100.0, 1.0)] }], region, 0.0);
        assert_eq!(big, vec![None]);
        assert!(shelf_pack(&[], region, 0.0).is_empty());
    }
}

/// True-shape nesting (a raster bottom-left fill, in the spirit of LightBurn's TrueNest): parts are placed by their
/// actual outlines, so they interlock and use each other's holes, at least `spacing` mm apart, inside a region of
/// any shape. Everything is rasterised on one grid of square cells; every test is conservative (a part's cells cover
/// it grown by half a cell diagonal, placed parts block the cells within `spacing` plus that of them), so placed parts
/// never come closer than `spacing` to each other and never leave the region, however coarse the grid. Work is
/// bounded by [`shapes::SHAPE_NEST_BUDGET`] cell-word tests; parts left when it runs out are not placed.
pub mod shapes {
    use kurbo::{Point, Rect, Vec2};

    use crate::flatten::Polyline;

    /// Most cells along the longer side of the region's grid.
    pub const MAX_GRID: usize = 512;

    /// Most 64-cell word comparisons one nest may make (about a second of work).
    pub const SHAPE_NEST_BUDGET: u64 = 600_000_000;

    /// One part: its outline (closed rings fill even-odd, open polylines are lines) in each allowed orientation, in
    /// the caller's orientation order and frame.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ShapePart {
        pub orientations: Vec<Vec<Polyline>>,
    }

    /// Where a part went: its orientation and the translation to apply to that orientation's outline.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ShapePlacement {
        pub orientation: usize,
        pub offset: Vec2,
    }

    /// The result of [`nest`]: a placement per part (`None` when it did not fit), the cell size used, and whether
    /// the work budget ran out (parts after that point were not tried).
    #[derive(Debug, Clone, PartialEq)]
    pub struct ShapeNest {
        pub placements: Vec<Option<ShapePlacement>>,
        pub cell_mm: f64,
        pub budget_exhausted: bool,
    }

    /// A bit grid, `w` x `h` cells, rows of 64-cell words.
    #[derive(Clone)]
    struct Grid {
        w: usize,
        h: usize,
        words: usize,
        bits: Vec<u64>,
    }

    impl Grid {
        fn new(w: usize, h: usize) -> Grid {
            let words = w.div_ceil(64).max(1);
            Grid { w, h, words, bits: vec![0; words * h] }
        }

        fn set(&mut self, x: usize, y: usize) {
            if x < self.w && y < self.h {
                self.bits[y * self.words + x / 64] |= 1 << (x % 64);
            }
        }

        fn get(&self, x: usize, y: usize) -> bool {
            x < self.w && y < self.h && self.bits[y * self.words + x / 64] & (1 << (x % 64)) != 0
        }

        /// Set cells `x0..=x1` of row `y` (clipped to the grid), a word at a time.
        fn set_span(&mut self, x0: usize, x1: usize, y: usize) {
            if y >= self.h || x0 >= self.w {
                return;
            }
            let x1 = x1.min(self.w - 1);
            if x1 < x0 {
                return;
            }
            let row = &mut self.bits[y * self.words..(y + 1) * self.words];
            let (w0, w1) = (x0 / 64, x1 / 64);
            for (k, word) in row.iter_mut().enumerate().take(w1 + 1).skip(w0) {
                let lo = if k == w0 { x0 % 64 } else { 0 };
                let hi = if k == w1 { x1 % 64 } else { 63 };
                let mask = (u64::MAX >> (63 - hi)) & (u64::MAX << lo);
                *word |= mask;
            }
        }

        /// The extent of the set cells, `(first row, last row, first column, last column)`, if any is set.
        fn extent(&self) -> Option<(usize, usize, usize, usize)> {
            let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0, usize::MAX, 0);
            for y in 0..self.h {
                let row = &self.bits[y * self.words..(y + 1) * self.words];
                for (k, &w) in row.iter().enumerate() {
                    if w != 0 {
                        r0 = r0.min(y);
                        r1 = r1.max(y);
                        c0 = c0.min(k * 64 + w.trailing_zeros() as usize);
                        c1 = c1.max(k * 64 + 63 - w.leading_zeros() as usize);
                    }
                }
            }
            (r0 != usize::MAX).then_some((r0, r1, c0, c1))
        }

        /// 64 cells of row `y` starting at cell `x` (cells past the right edge read as set).
        fn word_at(&self, x: usize, y: usize) -> u64 {
            let row = &self.bits[y * self.words..(y + 1) * self.words];
            let (i, s) = (x / 64, x % 64);
            let lo = row.get(i).copied().unwrap_or(u64::MAX) >> s;
            let hi = if s == 0 { 0 } else { row.get(i + 1).copied().unwrap_or(u64::MAX) << (64 - s) };
            let mut v = lo | hi;
            let valid = self.w.saturating_sub(x);
            if valid < 64 {
                v |= u64::MAX << valid;
            }
            v
        }
    }

    /// Mark every cell of `g` (cell (0, 0) starting at `origin`, cells `cell` mm) whose centre lies inside the
    /// even-odd fill of the closed `lines`, or within `radius` of any of `lines` (closed ones including their
    /// closing edge).
    fn rasterise(g: &mut Grid, origin: Point, cell: f64, lines: &[Polyline], radius: f64) {
        let rings: Vec<&Polyline> = lines.iter().filter(|l| l.closed && l.points.len() >= 3).collect();
        if !rings.is_empty() {
            // Only the rows the rings span.
            let (lo, hi) = rings
                .iter()
                .flat_map(|r| r.points.iter())
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p.y), hi.max(p.y)));
            let j0 = ((lo - origin.y) / cell - 0.5).floor().max(0.0);
            let j1 = ((hi - origin.y) / cell + 0.5).ceil().min(g.h as f64);
            let rows = if j0.is_finite() && j1.is_finite() && j0 < j1 { j0 as usize..j1 as usize } else { 0..0 };
            for j in rows {
                let y = origin.y + (j as f64 + 0.5) * cell;
                let mut xs: Vec<f64> = Vec::new();
                for r in &rings {
                    let n = r.points.len();
                    for k in 0..n {
                        let (a, b) = (r.points[k], r.points[(k + 1) % n]);
                        if (a.y <= y) != (b.y <= y) {
                            xs.push(a.x + (y - a.y) / (b.y - a.y) * (b.x - a.x));
                        }
                    }
                }
                xs.sort_by(f64::total_cmp);
                for pair in xs.as_chunks::<2>().0 {
                    let i0 = ((pair[0] - origin.x) / cell - 0.5).ceil().max(0.0);
                    let i1 = ((pair[1] - origin.x) / cell - 0.5).floor().min(g.w as f64 - 1.0);
                    if i1 < i0 {
                        continue;
                    }
                    g.set_span(i0 as usize, i1 as usize, j);
                }
            }
        }
        // Outlines grown by `radius`: discs stamped along every edge, sampled finely enough that the discs (grown
        // by half a step) cover the whole band.
        // Each disc is set row by row as one span of cells (clipped to the grid), so a large radius costs rows x
        // words per sample, never radius² cells.
        let step = cell * 0.5;
        let rr = radius + step * 0.5;
        if !(rr.is_finite() && cell > 0.0) || g.w == 0 || g.h == 0 {
            return;
        }
        // A hair more than the radius, so rounding never leaves out a cell the exact test would set.
        let rr2 = (rr * (1.0 + 1e-12)).powi(2);
        for l in lines {
            let pts = l.drawn();
            let segs: Vec<(Point, Point)> =
                if pts.len() == 1 { vec![(pts[0], pts[0])] } else { pts.windows(2).map(|w| (w[0], w[1])).collect() };
            for (a, b) in segs {
                let n = ((a.distance(b) / step).ceil().min(1e9) as usize).max(1);
                for k in 0..=n {
                    let p = a.lerp(b, k as f64 / n as f64);
                    // Rows whose centre is within rr of p, clipped to the grid.
                    let j0 = ((p.y - rr - origin.y) / cell - 0.5).ceil().max(0.0);
                    let j1 = ((p.y + rr - origin.y) / cell - 0.5).floor().min(g.h as f64 - 1.0);
                    if !(j0.is_finite() && j1.is_finite()) || j1 < j0 {
                        continue;
                    }
                    for j in j0 as usize..=j1 as usize {
                        let dy = origin.y + (j as f64 + 0.5) * cell - p.y;
                        let dx2 = rr2 - dy * dy;
                        if dx2 < 0.0 {
                            continue;
                        }
                        let dx = dx2.sqrt();
                        let i0 = ((p.x - dx - origin.x) / cell - 0.5).ceil().max(0.0);
                        let i1 = ((p.x + dx - origin.x) / cell - 0.5).floor().min(g.w as f64 - 1.0);
                        if i0 <= i1 {
                            g.set_span(i0 as usize, i1 as usize, j);
                        }
                    }
                }
            }
        }
    }

    fn bounds(lines: &[Polyline]) -> Option<Rect> {
        lines
            .iter()
            .flat_map(|l| l.points.iter())
            .fold(None, |r: Option<Rect>, p| Some(r.map_or(Rect::from_points(*p, *p), |r| r.union_pt(*p))))
    }

    /// A part orientation's own cells: they cover it grown by `grow`, on a local grid starting at `local_origin`.
    struct Mask {
        h: usize,
        grid: Grid,
        local_origin: Point,
    }

    fn mask(lines: &[Polyline], cell: f64, grow: f64) -> Option<Mask> {
        let b = bounds(lines)?;
        let pad = (grow / cell).ceil() + 1.0;
        let local_origin = Point::new(b.x0 - pad * cell, b.y0 - pad * cell);
        let w = ((b.width() / cell) + 2.0 * pad).ceil() as usize + 1;
        let h = ((b.height() / cell) + 2.0 * pad).ceil() as usize + 1;
        if w > 4 * MAX_GRID || h > 4 * MAX_GRID {
            return None;
        }
        let mut grid = Grid::new(w, h);
        rasterise(&mut grid, local_origin, cell, lines, grow);
        Some(Mask { h, grid, local_origin })
    }

    /// Whether `m` with its cell (0, 0) on cell (gx, gy) of `blocked` meets a set cell (cells off the grid count as
    /// set). Adds the words compared to `work`.
    fn hits(blocked: &Grid, m: &Mask, gx: i64, gy: i64, work: &mut u64) -> bool {
        for r in 0..m.h {
            let row = &m.grid.bits[r * m.grid.words..(r + 1) * m.grid.words];
            if row.iter().all(|w| *w == 0) {
                continue;
            }
            let y = gy + r as i64;
            if y < 0 || y as usize >= blocked.h {
                return true;
            }
            for (k, &mw) in row.iter().enumerate() {
                if mw == 0 {
                    continue;
                }
                *work += 1;
                let x = gx + (k * 64) as i64;
                if x < 0 {
                    return true;
                }
                if blocked.word_at(x as usize, y as usize) & mw != 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Nest `parts` into the even-odd fill of the closed `region` rings, keeping them at least `edge` mm inside its
    /// outline and `spacing` mm apart. Larger parts (by bounding-box area) go first; each takes, over its
    /// orientations, the placement whose top is lowest (then leftmost). The grid has at most [`MAX_GRID`] cells on
    /// its longer side and cells no smaller than `min_cell` mm.
    pub fn nest(parts: &[ShapePart], region: &[Polyline], edge: f64, spacing: f64, min_cell: f64) -> ShapeNest {
        let none = |cell| ShapeNest { placements: vec![None; parts.len()], cell_mm: cell, budget_exhausted: false };
        let closed: Vec<Polyline> = region.iter().filter(|l| l.closed && l.points.len() >= 3).cloned().collect();
        let Some(rb) = bounds(&closed) else { return none(0.0) };
        let spacing = if spacing.is_finite() { spacing.max(0.0) } else { 0.0 };
        let edge = if edge.is_finite() { edge.max(0.0) } else { 0.0 };
        let min_cell = if min_cell.is_finite() { min_cell.max(0.0) } else { 0.0 };
        let cell = (rb.width().max(rb.height()) / MAX_GRID as f64).max(min_cell);
        if !(cell.is_finite() && cell > 0.0) {
            return none(0.0);
        }
        // No two points of the region are further apart than its diagonal, so larger gaps change nothing (and are
        // capped, so a huge gap costs no more than one that blocks everything).
        let diag = rb.width().hypot(rb.height());
        let (spacing, edge) = (spacing.min(diag), edge.min(diag));
        let half_diag = cell * std::f64::consts::FRAC_1_SQRT_2;
        let gw = (rb.width() / cell).ceil() as usize + 1;
        let gh = (rb.height() / cell).ceil() as usize + 1;
        let origin = Point::new(rb.x0, rb.y0);
        // Blocked: outside the region, or within `edge` (plus half a cell diagonal) of its outline.
        let mut inside = Grid::new(gw, gh);
        rasterise(&mut inside, origin, cell, &closed, 0.0);
        let mut band = Grid::new(gw, gh);
        let outlines: Vec<Polyline> = closed.iter().map(|l| Polyline { points: l.drawn(), closed: false }).collect();
        rasterise(&mut band, origin, cell, &outlines, edge + half_diag);
        let mut blocked = Grid::new(gw, gh);
        for j in 0..gh {
            for i in 0..gw {
                if !inside.get(i, j) || band.get(i, j) {
                    blocked.set(i, j);
                }
            }
        }
        let area = |p: &ShapePart| p.orientations.first().and_then(|o| bounds(o)).map_or(0.0, |b| b.area());
        let mut order: Vec<usize> = (0..parts.len()).collect();
        order.sort_by(|&a, &b| area(&parts[b]).total_cmp(&area(&parts[a])).then(a.cmp(&b)));
        let mut out = vec![None; parts.len()];
        let mut work = 0u64;
        let mut exhausted = false;
        'parts: for i in order {
            // (top row, column, orientation, row) of the best placement so far.
            let mut best: Option<(i64, i64, usize, i64)> = None;
            for (o, lines) in parts[i].orientations.iter().enumerate() {
                // A part must lie inside the region, so one larger than its bounds is never rasterised.
                if bounds(lines).is_none_or(|b| b.width() > rb.width() || b.height() > rb.height()) {
                    continue;
                }
                let Some(m) = mask(lines, cell, half_diag) else { continue };
                let mh = m.h as i64;
                // Only offsets that keep every set cell on the grid can pass `hits` (off the grid counts as blocked,
                // and a word starting left of the grid is refused whole): searching just those keeps the scan within
                // the work budget, which counts the words compared.
                let Some((r0, r1, c0, c1)) = m.grid.extent() else { continue };
                let (gy0, gy1) = (-(r0 as i64), gh as i64 - 1 - r1 as i64);
                let (gx0, gx1) = (-((c0 / 64 * 64) as i64), gw as i64 - 1 - c1 as i64);
                'search: for gy in gy0..=gy1 {
                    if best.is_some_and(|b| gy + mh >= b.0) {
                        break;
                    }
                    for gx in gx0..=gx1 {
                        if work > SHAPE_NEST_BUDGET {
                            exhausted = true;
                            break 'parts;
                        }
                        if !hits(&blocked, &m, gx, gy, &mut work) {
                            let top = gy + mh;
                            if best.is_none_or(|b| (top, gx) < (b.0, b.1)) {
                                best = Some((top, gx, o, gy));
                            }
                            break 'search;
                        }
                    }
                }
            }
            let Some((_, gx, o, gy)) = best else { continue };
            let lines = &parts[i].orientations[o];
            let Some(m) = mask(lines, cell, half_diag) else { continue };
            let target = Point::new(origin.x + gx as f64 * cell, origin.y + gy as f64 * cell);
            let offset = target - m.local_origin;
            // Block the placed part grown by the spacing.
            let moved: Vec<Polyline> =
                lines.iter().map(|l| Polyline { points: l.points.iter().map(|p| *p + offset).collect(), closed: l.closed }).collect();
            rasterise(&mut blocked, origin, cell, &moved, spacing + half_diag);
            out[i] = Some(ShapePlacement { orientation: o, offset });
        }
        ShapeNest { placements: out, cell_mm: cell, budget_exhausted: exhausted }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn ring(pts: &[(f64, f64)]) -> Polyline {
            Polyline { points: pts.iter().map(|&(x, y)| Point::new(x, y)).collect(), closed: true }
        }

        fn min_gap(a: &[Polyline], b: &[Polyline]) -> f64 {
            let mut d = f64::MAX;
            for la in a {
                for lb in b {
                    for p in la.drawn() {
                        for w in lb.drawn().windows(2) {
                            let (s, e) = (w[0], w[1]);
                            let ab = e - s;
                            let t = if ab.hypot2() > 0.0 { ((p - s).dot(ab) / ab.hypot2()).clamp(0.0, 1.0) } else { 0.0 };
                            d = d.min(p.distance(s + ab * t));
                        }
                    }
                }
            }
            d
        }

        fn placed(n: &ShapeNest, parts: &[ShapePart]) -> Vec<Option<Vec<Polyline>>> {
            n.placements
                .iter()
                .zip(parts)
                .map(|(p, part)| {
                    p.map(|p| {
                        part.orientations[p.orientation]
                            .iter()
                            .map(|l| Polyline { points: l.points.iter().map(|q| *q + p.offset).collect(), closed: l.closed })
                            .collect()
                    })
                })
                .collect()
        }

        #[test]
        fn l_shapes_interlock_where_their_boxes_cannot() {
            // An L: a 20 x 20 box with a 15 x 15 notch out of the top right; and the same L turned half round.
            let l = ring(&[(0.0, 0.0), (20.0, 0.0), (20.0, 5.0), (5.0, 5.0), (5.0, 20.0), (0.0, 20.0)]);
            let turned = ring(&[(20.0, 20.0), (0.0, 20.0), (0.0, 15.0), (15.0, 15.0), (15.0, 0.0), (20.0, 0.0)]);
            let parts = vec![ShapePart { orientations: vec![vec![l]] }, ShapePart { orientations: vec![vec![turned]] }];
            // 23 x 30: two 20 x 20 boxes cannot both fit (they need 40 in one direction); interlocked Ls can.
            let region = vec![ring(&[(0.0, 0.0), (23.0, 0.0), (23.0, 30.0), (0.0, 30.0)])];
            let n = nest(&parts, &region, 0.0, 0.5, 0.0);
            assert!(n.placements.iter().all(Option::is_some), "{n:?}");
            assert!(!n.budget_exhausted);
            let p = placed(&n, &parts);
            let (a, b) = (p[0].as_ref().unwrap(), p[1].as_ref().unwrap());
            assert!(min_gap(a, b) >= 0.5 - 1e-9 && min_gap(b, a) >= 0.5 - 1e-9, "{}", min_gap(a, b));
            for part in [a, b] {
                for q in &part[0].points {
                    assert!(q.x >= -1e-9 && q.x <= 23.0 + 1e-9 && q.y >= -1e-9 && q.y <= 30.0 + 1e-9, "{q:?}");
                }
            }
        }

        #[test]
        fn parts_go_into_holes_and_misfits_are_reported() {
            // A frame with a 20 x 20 hole; a 5 x 5 square fits in the hole.
            let outer = ring(&[(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)]);
            let hole = ring(&[(5.0, 5.0), (25.0, 5.0), (25.0, 25.0), (5.0, 25.0)]);
            let small = ring(&[(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (0.0, 5.0)]);
            let huge = ring(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
            let parts = vec![
                ShapePart { orientations: vec![vec![outer, hole]] },
                ShapePart { orientations: vec![vec![small]] },
                ShapePart { orientations: vec![vec![huge]] },
            ];
            let region = vec![ring(&[(0.0, 0.0), (31.0, 0.0), (31.0, 31.0), (0.0, 31.0)])];
            let n = nest(&parts, &region, 0.0, 0.5, 0.0);
            assert!(n.placements[0].is_some() && n.placements[1].is_some(), "{n:?}");
            assert!(n.placements[2].is_none());
            let p = placed(&n, &parts);
            let s = &p[1].as_ref().unwrap()[0].points;
            assert!(s.iter().all(|q| q.x > 5.0 && q.x < 25.0 && q.y > 5.0 && q.y < 25.0), "the small part sits in the hole: {s:?}");
            assert!(min_gap(p[0].as_ref().unwrap(), p[1].as_ref().unwrap()) >= 0.5 - 1e-9);
            // A round region keeps parts inside its outline (with the edge gap).
            let circle: Vec<Point> = (0..64).map(|k| {
                let a = std::f64::consts::TAU * k as f64 / 64.0;
                Point::new(20.0 * a.cos(), 20.0 * a.sin())
            }).collect();
            let disc = vec![Polyline { points: circle, closed: true }];
            let squares: Vec<ShapePart> = (0..6).map(|_| ShapePart { orientations: vec![vec![ring(&[(0.0, 0.0), (8.0, 0.0), (8.0, 8.0), (0.0, 8.0)])]] }).collect();
            let n = nest(&squares, &disc, 1.0, 1.0, 0.0);
            let p = placed(&n, &squares);
            assert!(p.iter().filter(|x| x.is_some()).count() >= 4);
            for part in p.iter().flatten() {
                for q in &part[0].points {
                    assert!(q.to_vec2().hypot() <= 20.0 * (std::f64::consts::PI / 64.0).cos() - 1.0 + 1e-6, "{q:?}");
                }
            }
        }

        /// The disc stamping as it was first written (every cell of the disc's box tested), for comparison.
        fn stamp_naive(g: &mut Grid, origin: Point, cell: f64, lines: &[Polyline], radius: f64) {
            let step = cell * 0.5;
            let rr = radius + step * 0.5;
            let reach = (rr / cell).ceil() as i64 + 1;
            for l in lines {
                let pts = l.drawn();
                for w in pts.windows(2) {
                    let n = ((w[0].distance(w[1]) / step).ceil() as usize).max(1);
                    for k in 0..=n {
                        let p = w[0].lerp(w[1], k as f64 / n as f64);
                        let ci = ((p.x - origin.x) / cell).floor() as i64;
                        let cj = ((p.y - origin.y) / cell).floor() as i64;
                        for dj in -reach..=reach {
                            for di in -reach..=reach {
                                let (i, j) = (ci + di, cj + dj);
                                if i < 0 || j < 0 || i as usize >= g.w || j as usize >= g.h {
                                    continue;
                                }
                                let c = Point::new(origin.x + (i as f64 + 0.5) * cell, origin.y + (j as f64 + 0.5) * cell);
                                if c.distance(p) <= rr {
                                    g.set(i as usize, j as usize);
                                }
                            }
                        }
                    }
                }
            }
        }

        #[test]
        fn span_stamping_covers_every_cell_the_disc_test_does() {
            let wavy = Polyline {
                points: (0..40).map(|k| Point::new(k as f64 * 0.7 - 3.0, (k as f64 * 0.9).sin() * 4.0 + 5.0)).collect(),
                closed: false,
            };
            for (radius, w) in [(0.0, 70usize), (0.33, 70), (2.7, 130), (40.0, 64)] {
                let mut a = Grid::new(w, 50);
                let mut b = Grid::new(w, 50);
                let origin = Point::new(-5.0, -2.0);
                rasterise(&mut a, origin, 0.31, std::slice::from_ref(&wavy), radius);
                stamp_naive(&mut b, origin, 0.31, std::slice::from_ref(&wavy), radius);
                for j in 0..50 {
                    for i in 0..w {
                        assert!(!b.get(i, j) || a.get(i, j), "radius {radius}: cell ({i}, {j}) left out");
                    }
                }
                // And nothing further than a hair beyond the disc test.
                let extra = (0..50).flat_map(|j| (0..w).map(move |i| (i, j))).filter(|&(i, j)| a.get(i, j) && !b.get(i, j)).count();
                assert_eq!(extra, 0, "radius {radius}");
            }
        }

        #[test]
        fn huge_gaps_and_hopeless_parts_stay_cheap() {
            let square = |s: f64| ring(&[(0.0, 0.0), (s, 0.0), (s, s), (0.0, s)]);
            let region = vec![square(100.0)];
            let parts: Vec<ShapePart> = (0..3).map(|_| ShapePart { orientations: vec![vec![square(10.0)]] }).collect();
            let t = std::time::Instant::now();
            // A gap far larger than the region: one part fits, nothing else, and the stamping does not explode.
            let n = nest(&parts, &region, 0.0, 1e12, 0.0);
            assert_eq!(n.placements.iter().filter(|p| p.is_some()).count(), 1, "{n:?}");
            let n = nest(&parts, &region, 1e300, 0.0, 0.0);
            assert!(n.placements.iter().all(Option::is_none));
            let n = nest(&parts, &region, f64::MAX, f64::MAX, 0.0);
            assert!(n.placements.iter().all(Option::is_none));
            // Many parts larger than the region: each is refused without scanning every offset off the grid.
            let big: Vec<ShapePart> =
                (0..300).map(|_| ShapePart { orientations: (0..8).map(|_| vec![square(300.0)]).collect() }).collect();
            let n = nest(&big, &region, 0.0, 1.0, 0.0);
            assert!(n.placements.iter().all(Option::is_none) && !n.budget_exhausted);
            assert!(t.elapsed() < std::time::Duration::from_secs(60), "{:?}", t.elapsed());
        }
    }
}
