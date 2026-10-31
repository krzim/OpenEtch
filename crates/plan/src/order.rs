//! Travel ordering: a greedy nearest-neighbour over chunks to cut jump distance.
//!
//! A chunk is something marked without interruption from our side: an open polyline (may run backwards), a closed
//! loop (may start at any vertex, direction kept), or one hatch set of one object (its scanline order is kept; the
//! whole set may run from its last line back to its first). Hatch sets are chunks rather than single lines so the
//! search stays small and the zig-zag the hatcher made is kept.

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

/// Order `chunks` greedily: from `start`, always enter the chunk with the shortest jump next. Returns the polylines
/// in marking order and where the galvo ends up.
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
}
