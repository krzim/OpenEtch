//! Travel ordering: a greedy nearest-neighbour over chunks to cut jump distance.
//!
//! A chunk is something marked without interruption from our side: an open polyline (may run backwards), a closed
//! loop (may start at any vertex, direction kept), or one hatch set of one object (its scanline order is kept; the
//! whole set may run from its last line back to its first). Hatch sets are chunks rather than single lines so the
//! search stays small and the zig-zag the hatcher made is kept.
//!
//! [`order_with`] applies LightBurn's Optimization Settings ([`OrderOpts`], from [`doc::Optimization`]): output in
//! drawn order when optimization is off, inner shapes first, direction order, nearest-first travel with an optional
//! penalty for turning (reduce direction changes), best start point (or the first point), sharp corners as start
//! points, and best direction (or always forwards). The direction-change penalty is OpenEtch's own heuristic: a
//! candidate's cost is its jump plus up to half its jump (and 0.5 mm) more the further its first segment turns away
//! from the current heading.

use doc::{CutDirection, Optimization};

use crate::geom::{Point, dist};
use crate::hatch::HatchLine;

/// One unit of the travel search.
#[derive(Debug, Clone, PartialEq)]
pub enum Chunk {
    /// Marked in either direction.
    Open(Vec<Point>),
    /// A loop without its repeated last point; it may start at any vertex.
    Closed(Vec<Point>),
    /// Hatch lines in scanline order. Reversing the set reverses the line order and, when `bidirectional`, each
    /// line too (so the zig-zag survives); a one-way hatch keeps every line's direction.
    Hatch { lines: Vec<HatchLine>, bidirectional: bool },
}

/// How a chosen chunk is entered.
#[derive(Debug, Clone, Copy)]
enum Entry {
    Forward,
    Reverse,
    /// Closed loop starting at this vertex.
    At(usize),
}

impl Chunk {
    /// The cheapest way to enter this chunk from `from`, and the jump distance it costs. `bound` is the best
    /// distance found so far; a loop whose bounding box is already further away is not searched vertex by vertex.
    fn best_entry(&self, from: Point, bound: f64) -> Option<(Entry, f64)> {
        match self {
            Chunk::Open(p) => {
                let (first, last) = (*p.first()?, *p.last()?);
                let (f, r) = (dist(from, first), dist(from, last));
                Some(if r < f { (Entry::Reverse, r) } else { (Entry::Forward, f) })
            }
            Chunk::Closed(p) => {
                if bbox_distance(p, from) >= bound {
                    return None;
                }
                p.iter().enumerate().map(|(i, &q)| (Entry::At(i), dist(from, q))).min_by(|a, b| a.1.total_cmp(&b.1))
            }
            Chunk::Hatch { lines, bidirectional } => {
                let first = lines.first()?[0];
                let last = lines.last()?;
                let rev_start = if *bidirectional { last[1] } else { last[0] };
                let (f, r) = (dist(from, first), dist(from, rev_start));
                Some(if r < f { (Entry::Reverse, r) } else { (Entry::Forward, f) })
            }
        }
    }

    /// The chunk as polylines in marking order, entered as `entry` says.
    fn into_polylines(self, entry: Entry) -> Vec<Vec<Point>> {
        match (self, entry) {
            (Chunk::Open(mut p), Entry::Reverse) => {
                p.reverse();
                vec![p]
            }
            (Chunk::Open(p), _) => vec![p],
            (Chunk::Closed(p), entry) => {
                let start = if let Entry::At(i) = entry { i } else { 0 };
                let mut out: Vec<Point> = p[start..].iter().chain(&p[..start]).copied().collect();
                if let Some(&first) = out.first() {
                    out.push(first);
                }
                vec![out]
            }
            (Chunk::Hatch { mut lines, bidirectional }, Entry::Reverse) => {
                lines.reverse();
                if bidirectional {
                    for l in &mut lines {
                        l.swap(0, 1);
                    }
                }
                lines.into_iter().map(|l| l.to_vec()).collect()
            }
            (Chunk::Hatch { lines, .. }, _) => lines.into_iter().map(|l| l.to_vec()).collect(),
        }
    }
}

/// Distance from `p` to the bounding box of `pts` (0 inside); a lower bound on the distance to any vertex.
fn bbox_distance(pts: &[Point], p: Point) -> f64 {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for q in pts {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    let dx = (x0 - p[0]).max(p[0] - x1).max(0.0);
    let dy = (y0 - p[1]).max(p[1] - y1).max(0.0);
    dx.hypot(dy)
}

/// The ordering rules of one pass (LightBurn's Optimization Settings without the Order By list, which the caller
/// applies by ordering groups of chunks).
#[derive(Debug, Clone, PartialEq)]
pub struct OrderOpts {
    pub enabled: bool,
    pub reduce_travel: bool,
    pub best_start: bool,
    pub corners: bool,
    pub best_direction: bool,
    pub reduce_direction_changes: bool,
    pub inner_first: bool,
    pub direction: Option<CutDirection>,
}

impl Default for OrderOpts {
    /// OpenEtch's behaviour before the settings existed: nearest-first, best start, best direction.
    fn default() -> Self {
        OrderOpts::from(&Optimization::default())
    }
}

impl From<&Optimization> for OrderOpts {
    fn from(o: &Optimization) -> Self {
        OrderOpts {
            enabled: o.enabled,
            reduce_travel: o.reduce_travel,
            best_start: o.best_start,
            corners: o.corners,
            best_direction: o.best_direction,
            reduce_direction_changes: o.reduce_direction_changes,
            inner_first: o.inner_first,
            direction: o.direction_order,
        }
    }
}

/// Smallest turn (degrees) that counts as a corner for "choose corners".
const CORNER_DEG: f64 = 30.0;

impl Chunk {
    /// How this chunk may be entered from `from` under `o`, and the jump it costs. `bound` prunes closed loops as in
    /// [`Chunk::best_entry`].
    fn entry(&self, from: Point, o: &OrderOpts, bound: f64) -> Option<(Entry, f64)> {
        match self {
            Chunk::Open(p) => {
                let (first, last) = (*p.first()?, *p.last()?);
                let (f, r) = (dist(from, first), dist(from, last));
                Some(if o.best_direction && r < f { (Entry::Reverse, r) } else { (Entry::Forward, f) })
            }
            Chunk::Closed(p) => {
                if !o.best_start {
                    return Some((Entry::At(0), dist(from, *p.first()?)));
                }
                if bbox_distance(p, from) >= bound {
                    return None;
                }
                let corners: Vec<usize> = if o.corners { corner_vertices(p) } else { Vec::new() };
                let candidates: Box<dyn Iterator<Item = usize>> =
                    if corners.is_empty() { Box::new(0..p.len()) } else { Box::new(corners.into_iter()) };
                candidates.map(|i| (Entry::At(i), dist(from, p[i]))).min_by(|a, b| a.1.total_cmp(&b.1))
            }
            Chunk::Hatch { lines, bidirectional } => {
                let first = lines.first()?[0];
                let last = lines.last()?;
                let rev_start = if *bidirectional { last[1] } else { last[0] };
                let (f, r) = (dist(from, first), dist(from, rev_start));
                Some(if o.best_direction && r < f { (Entry::Reverse, r) } else { (Entry::Forward, f) })
            }
        }
    }

    /// Where the chunk starts and the direction of its first segment, when entered as `entry`.
    fn start_heading(&self, entry: Entry) -> Option<(Point, Point)> {
        let pair = |a: Point, b: Point| Some((a, [b[0] - a[0], b[1] - a[1]]));
        match (self, entry) {
            (Chunk::Open(p), Entry::Reverse) => pair(p[p.len() - 1], p[p.len().checked_sub(2)?]),
            (Chunk::Open(p), _) => pair(p[0], *p.get(1)?),
            (Chunk::Closed(p), e) => {
                let i = if let Entry::At(i) = e { i } else { 0 };
                pair(p[i], p[(i + 1) % p.len()])
            }
            (Chunk::Hatch { lines, bidirectional }, Entry::Reverse) => {
                let l = lines.last()?;
                if *bidirectional { pair(l[1], l[0]) } else { pair(l[0], l[1]) }
            }
            (Chunk::Hatch { lines, .. }, _) => pair(lines[0][0], lines[0][1]),
        }
    }

    /// Bounding box `[x0, y0, x1, y1]` of the chunk's points.
    fn bbox(&self) -> [f64; 4] {
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let mut add = |q: &Point| {
            b[0] = b[0].min(q[0]);
            b[1] = b[1].min(q[1]);
            b[2] = b[2].max(q[0]);
            b[3] = b[3].max(q[1]);
        };
        match self {
            Chunk::Open(p) | Chunk::Closed(p) => p.iter().for_each(&mut add),
            Chunk::Hatch { lines, .. } => lines.iter().flatten().for_each(&mut add),
        }
        b
    }
}

/// Vertices of a loop (without its repeated last point) where the outline turns by at least [`CORNER_DEG`].
fn corner_vertices(p: &[Point]) -> Vec<usize> {
    let n = p.len();
    if n < 3 {
        return Vec::new();
    }
    (0..n)
        .filter(|&i| {
            let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
            let (lu, lv) = (u[0].hypot(u[1]), v[0].hypot(v[1]));
            if lu == 0.0 || lv == 0.0 {
                return false;
            }
            let cos = (u[0] * v[0] + u[1] * v[1]) / (lu * lv);
            cos.clamp(-1.0, 1.0).acos().to_degrees() >= CORNER_DEG
        })
        .collect()
}

/// Even-odd point in polygon.
fn inside(p: Point, ring: &[Point]) -> bool {
    let mut c = false;
    let n = ring.len();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// How many other closed chunks enclose each chunk (0 for open lines and hatches).
fn depths<T>(chunks: &[(Chunk, T)]) -> Vec<usize> {
    let boxes: Vec<[f64; 4]> = chunks.iter().map(|(c, _)| c.bbox()).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(i, (c, _))| {
            let Chunk::Closed(p) = c else { return 0 };
            let Some(&probe) = p.first() else { return 0 };
            chunks
                .iter()
                .enumerate()
                .filter(|(j, (other, _))| {
                    let (a, b) = (boxes[i], boxes[*j]);
                    *j != i
                        && matches!(other, Chunk::Closed(_))
                        && b[0] <= a[0]
                        && b[1] <= a[1]
                        && b[2] >= a[2]
                        && b[3] >= a[3]
                        && (a != b || *j < i)
                        && matches!(other, Chunk::Closed(q) if inside(probe, q))
                })
                .count()
        })
        .collect()
}

/// Order tagged chunks under `o` (see the module docs), starting from `start`. Returns each polyline with the tag of
/// the chunk it came from, and where the galvo ends up.
pub fn order_with<T: Copy>(chunks: Vec<(Chunk, T)>, start: Point, o: &OrderOpts) -> (Vec<(Vec<Point>, T)>, Point) {
    let mut chunks: Vec<(Chunk, T)> = chunks.into_iter().filter(|(c, _)| c.best_entry([0.0, 0.0], f64::INFINITY).is_some()).collect();
    let mut out = Vec::new();
    let mut pos = start;
    let mut heading: Option<Point> = None;
    let emit = |out: &mut Vec<(Vec<Point>, T)>, pos: &mut Point, heading: &mut Option<Point>, chunk: Chunk, tag: T, entry: Entry| {
        let polys = chunk.into_polylines(entry);
        if let Some(last) = polys.last()
            && let (Some(&e), Some(&b)) = (last.last(), last.len().checked_sub(2).and_then(|i| last.get(i)))
        {
            *heading = Some([e[0] - b[0], e[1] - b[1]]);
            *pos = e;
        } else if let Some(&e) = polys.last().and_then(|p| p.last()) {
            *pos = e;
        }
        out.extend(polys.into_iter().map(|p| (p, tag)));
    };
    if !o.enabled {
        for (c, tag) in chunks {
            let entry = match c {
                Chunk::Closed(_) => Entry::At(0),
                _ => Entry::Forward,
            };
            emit(&mut out, &mut pos, &mut heading, c, tag, entry);
        }
        return (out, pos);
    }
    // Inner shapes first: deepest level first, each level ordered on its own.
    let levels: Vec<Vec<(Chunk, T)>> = if o.inner_first {
        let d = depths(&chunks);
        let max = d.iter().copied().max().unwrap_or(0);
        let mut levels: Vec<Vec<(Chunk, T)>> = (0..=max).map(|_| Vec::new()).collect();
        for (c, depth) in chunks.drain(..).zip(d) {
            levels[max - depth].push(c);
        }
        levels
    } else {
        vec![chunks]
    };
    for mut level in levels {
        if let Some(dir) = o.direction {
            let key = |c: &Chunk| {
                let b = c.bbox();
                match dir {
                    CutDirection::TopToBottom => -b[3],
                    CutDirection::BottomToTop => b[1],
                    CutDirection::LeftToRight => b[0],
                    CutDirection::RightToLeft => -b[2],
                }
            };
            level.sort_by(|a, b| key(&a.0).total_cmp(&key(&b.0)));
            for (c, tag) in level {
                let entry = c.entry(pos, o, f64::INFINITY).map_or(Entry::Forward, |e| e.0);
                emit(&mut out, &mut pos, &mut heading, c, tag, entry);
            }
            continue;
        }
        if !o.reduce_travel {
            for (c, tag) in level {
                let entry = c.entry(pos, o, f64::INFINITY).map_or(Entry::Forward, |e| e.0);
                emit(&mut out, &mut pos, &mut heading, c, tag, entry);
            }
            continue;
        }
        while !level.is_empty() {
            let mut best: Option<(usize, Entry, f64)> = None;
            for (i, (c, _)) in level.iter().enumerate() {
                let bound = best.map_or(f64::INFINITY, |b| b.2);
                let Some((entry, d)) = c.entry(pos, o, bound) else { continue };
                let cost = match (o.reduce_direction_changes, heading, c.start_heading(entry)) {
                    (true, Some(h), Some((_, s))) => {
                        let (lh, ls) = (h[0].hypot(h[1]), s[0].hypot(s[1]));
                        if lh > 0.0 && ls > 0.0 {
                            let turn = (1.0 - (h[0] * s[0] + h[1] * s[1]) / (lh * ls)) / 2.0;
                            d + turn * (d / 2.0 + 0.5)
                        } else {
                            d
                        }
                    }
                    _ => d,
                };
                if cost < bound {
                    best = Some((i, entry, cost));
                }
            }
            let Some((i, entry, _)) = best else { break };
            let (c, tag) = level.swap_remove(i);
            emit(&mut out, &mut pos, &mut heading, c, tag, entry);
        }
    }
    (out, pos)
}

/// Order `chunks` greedily: from `start`, always enter the chunk with the shortest jump next. Returns the polylines
/// in marking order and where the galvo ends up.
#[cfg(test)]
pub fn order(chunks: Vec<Chunk>, start: Point) -> (Vec<Vec<Point>>, Point) {
    let mut remaining: Vec<Chunk> = chunks.into_iter().filter(|c| c.best_entry([0.0, 0.0], f64::INFINITY).is_some()).collect();
    let mut out = Vec::new();
    let mut pos = start;
    while !remaining.is_empty() {
        let mut best: Option<(usize, Entry, f64)> = None;
        for (i, c) in remaining.iter().enumerate() {
            let bound = best.map_or(f64::INFINITY, |b| b.2);
            if let Some((entry, d)) = c.best_entry(pos, bound)
                && d < bound
            {
                best = Some((i, entry, d));
            }
        }
        // Every remaining chunk has an entry and the first one is searched with an infinite bound.
        let Some((i, entry, _)) = best else { break };
        let polylines = remaining.swap_remove(i).into_polylines(entry);
        if let Some(&end) = polylines.last().and_then(|p| p.last()) {
            pos = end;
        }
        out.extend(polylines);
    }
    (out, pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_polylines_are_reversed_when_closer() {
        let chunks = vec![Chunk::Open(vec![[10.0, 0.0], [1.0, 0.0]])];
        let (out, end) = order(chunks, [0.0, 0.0]);
        assert_eq!(out, vec![vec![[1.0, 0.0], [10.0, 0.0]]]);
        assert_eq!(end, [10.0, 0.0]);
    }

    #[test]
    fn closed_loops_start_at_the_nearest_vertex() {
        let sq = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let (out, end) = order(vec![Chunk::Closed(sq)], [2.0, 2.0]);
        assert_eq!(out[0], vec![[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]]);
        assert_eq!(end, [1.0, 1.0]);
    }

    #[test]
    fn nearest_chunk_goes_first() {
        let far = Chunk::Open(vec![[50.0, 0.0], [51.0, 0.0]]);
        let near = Chunk::Open(vec![[1.0, 0.0], [2.0, 0.0]]);
        let (out, _) = order(vec![far, near], [0.0, 0.0]);
        assert_eq!(out[0][0], [1.0, 0.0]);
        assert_eq!(out[1][0], [50.0, 0.0]);
    }

    #[test]
    fn one_way_hatch_keeps_line_direction_when_reversed() {
        let lines = vec![[[0.0, 0.0], [1.0, 0.0]], [[0.0, 1.0], [1.0, 1.0]]];
        let (out, _) = order(vec![Chunk::Hatch { lines, bidirectional: false }], [0.0, 2.0]);
        assert_eq!(out, vec![vec![[0.0, 1.0], [1.0, 1.0]], vec![[0.0, 0.0], [1.0, 0.0]]]);
    }

    #[test]
    fn zigzag_hatch_reverses_lines_too() {
        let lines = vec![[[0.0, 0.0], [1.0, 0.0]], [[1.0, 1.0], [0.0, 1.0]]];
        let (out, _) = order(vec![Chunk::Hatch { lines, bidirectional: true }], [0.0, 2.0]);
        assert_eq!(out, vec![vec![[0.0, 1.0], [1.0, 1.0]], vec![[1.0, 0.0], [0.0, 0.0]]]);
    }

    fn square(at: Point, side: f64) -> Vec<Point> {
        let [x, y] = at;
        vec![[x, y], [x + side, y], [x + side, y + side], [x, y + side]]
    }

    fn tags(out: &[(Vec<Point>, usize)]) -> Vec<usize> {
        out.iter().map(|(_, t)| *t).collect()
    }

    #[test]
    fn default_options_match_the_greedy_order() {
        let chunks = vec![
            Chunk::Open(vec![[50.0, 0.0], [51.0, 0.0]]),
            Chunk::Closed(square([5.0, 5.0], 2.0)),
            Chunk::Open(vec![[10.0, 0.0], [1.0, 0.0]]),
        ];
        let (plain, end) = order(chunks.clone(), [0.0, 0.0]);
        let tagged: Vec<(Chunk, usize)> = chunks.into_iter().zip(0..).collect();
        let (with, end2) = order_with(tagged, [0.0, 0.0], &OrderOpts::default());
        assert_eq!(with.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), plain);
        assert_eq!(end, end2);
    }

    #[test]
    fn disabled_keeps_drawn_order_and_direction() {
        let chunks = vec![(Chunk::Open(vec![[50.0, 0.0], [51.0, 0.0]]), 0), (Chunk::Open(vec![[10.0, 0.0], [1.0, 0.0]]), 1), (Chunk::Closed(square([3.0, 3.0], 1.0)), 2)];
        let o = OrderOpts { enabled: false, ..OrderOpts::default() };
        let (out, _) = order_with(chunks, [0.0, 0.0], &o);
        assert_eq!(tags(&out), [0, 1, 2]);
        assert_eq!(out[1].0, vec![[10.0, 0.0], [1.0, 0.0]], "not reversed");
        assert_eq!(out[2].0[0], [3.0, 3.0], "a loop starts at its first point");
    }

    #[test]
    fn inner_shapes_go_first() {
        // The outer square is nearer the start, but encloses the inner one.
        let outer = Chunk::Closed(square([-1.0, -1.0], 20.0));
        let inner = Chunk::Closed(square([5.0, 5.0], 2.0));
        let far = Chunk::Closed(square([40.0, 40.0], 2.0));
        let o = OrderOpts { inner_first: true, ..OrderOpts::default() };
        let (out, _) = order_with(vec![(outer.clone(), 0), (inner.clone(), 1), (far, 2)], [-2.0, -2.0], &o);
        assert_eq!(tags(&out)[0], 1, "inner first");
        let (out, _) = order_with(vec![(outer, 0), (inner, 1)], [-2.0, -2.0], &OrderOpts::default());
        assert_eq!(tags(&out)[0], 0, "without the option the nearest goes first");
    }

    #[test]
    fn direction_order_sorts_by_position() {
        let chunks: Vec<(Chunk, usize)> = (0..4).map(|i| (Chunk::Open(vec![[0.0, f64::from(i) * 10.0], [5.0, f64::from(i) * 10.0]]), i as usize)).collect();
        let o = OrderOpts { direction: Some(CutDirection::TopToBottom), ..OrderOpts::default() };
        let (out, _) = order_with(chunks.clone(), [0.0, 0.0], &o);
        assert_eq!(tags(&out), [3, 2, 1, 0]);
        let o = OrderOpts { direction: Some(CutDirection::BottomToTop), ..OrderOpts::default() };
        assert_eq!(tags(&order_with(chunks, [0.0, 100.0], &o).0), [0, 1, 2, 3]);
    }

    #[test]
    fn best_start_corners_and_direction_can_be_switched_off() {
        // A loop whose first point is far; a corner start is wanted on a rounded-ish shape.
        let sq = square([10.0, 10.0], 4.0);
        let no_start = OrderOpts { best_start: false, ..OrderOpts::default() };
        let (out, _) = order_with(vec![(Chunk::Closed(sq.clone()), 0)], [20.0, 20.0], &no_start);
        assert_eq!(out[0].0[0], [10.0, 10.0]);
        // A loop with a smooth vertex nearest the start and corners elsewhere.
        let mut shape = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [10.0, 10.0], [0.0, 10.0]];
        let corners = OrderOpts { corners: true, ..OrderOpts::default() };
        let (out, _) = order_with(vec![(Chunk::Closed(shape.clone()), 0)], [12.0, 5.0], &corners);
        assert_ne!(out[0].0[0], [10.0, 5.0], "the straight vertex is skipped");
        shape.truncate(2);
        let no_dir = OrderOpts { best_direction: false, ..OrderOpts::default() };
        let (out, _) = order_with(vec![(Chunk::Open(vec![[10.0, 0.0], [1.0, 0.0]]), 0)], [0.0, 0.0], &no_dir);
        assert_eq!(out[0].0[0], [10.0, 0.0]);
    }

    #[test]
    fn reduce_direction_changes_prefers_going_on() {
        // From the end of a line heading +X at (10, 0): a line ahead at distance 2.0 and one behind at 1.9.
        let first = (Chunk::Open(vec![[0.0, 0.0], [10.0, 0.0]]), 0);
        let ahead = (Chunk::Open(vec![[12.0, 0.0], [20.0, 0.0]]), 1);
        let behind = (Chunk::Open(vec![[10.0, 1.9], [2.0, 1.9]]), 2);
        let o = OrderOpts { reduce_direction_changes: true, best_direction: false, ..OrderOpts::default() };
        let (out, _) = order_with(vec![first.clone(), ahead.clone(), behind.clone()], [0.0, 0.0], &o);
        assert_eq!(tags(&out), [0, 1, 2]);
        let o = OrderOpts { best_direction: false, ..OrderOpts::default() };
        assert_eq!(tags(&order_with(vec![first, ahead, behind], [0.0, 0.0], &o).0), [0, 2, 1]);
    }
}
