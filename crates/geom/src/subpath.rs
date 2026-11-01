//! Curve-level subpaths: a path split into runs of connected segments (lines, quadratic and cubic Béziers), so tools
//! can edit nodes and cut segments without flattening.
//!
//! Normal form: zero-length lines and curves (every point on the start) are dropped, a `ClosePath` whose last point
//! is not the start adds an explicit closing [`Line`](kurbo::PathSeg::Line), and subpaths without segments are
//! dropped. [`join`] writes a closing line back as `ClosePath`, so `join(split(p))` draws exactly what `p` draws.

use kurbo::{BezPath, CubicBez, Line, ParamCurve, ParamCurveArclen, PathEl, PathSeg, Point, QuadBez, Vec2};

use crate::EPS;

#[derive(Debug, Clone, PartialEq)]
pub struct SubPath {
    /// Connected segments: each starts where the previous one ends. Never empty.
    pub segs: Vec<PathSeg>,
    /// Closed: the last segment ends at the first one's start.
    pub closed: bool,
}

impl SubPath {
    pub fn start(&self) -> Point {
        self.segs[0].start()
    }

    pub fn end(&self) -> Point {
        self.segs[self.segs.len() - 1].end()
    }

    /// Number of nodes: one per segment start, plus the end node of an open subpath.
    pub fn node_count(&self) -> usize {
        self.segs.len() + usize::from(!self.closed)
    }

    /// The node at `k` (`0..node_count()`).
    pub fn node(&self, k: usize) -> Point {
        if k < self.segs.len() { self.segs[k].start() } else { self.end() }
    }

    pub fn reversed(&self) -> SubPath {
        SubPath { segs: self.segs.iter().rev().map(|s| s.reverse()).collect(), closed: self.closed }
    }

    pub fn length(&self) -> f64 {
        self.segs.iter().map(|s| s.arclen(1e-4)).sum()
    }

    /// Every quadratic segment raised to an equivalent cubic (node editing works on lines and cubics only).
    pub fn cubics_only(mut self) -> SubPath {
        for s in &mut self.segs {
            if let PathSeg::Quad(q) = *s {
                *s = PathSeg::Cubic(q.raise());
            }
        }
        self
    }
}

/// A segment with its start point moved to `p` (a cubic's first handle moves along, keeping its offset).
pub fn with_start(seg: PathSeg, p: Point) -> PathSeg {
    match seg {
        PathSeg::Line(l) => PathSeg::Line(Line::new(p, l.p1)),
        PathSeg::Quad(q) => PathSeg::Quad(QuadBez::new(p, q.p1, q.p2)),
        PathSeg::Cubic(c) => {
            let d = p - c.p0;
            PathSeg::Cubic(CubicBez::new(p, c.p1 + d, c.p2, c.p3))
        }
    }
}

/// A segment with its end point moved to `p` (a cubic's last handle moves along).
pub fn with_end(seg: PathSeg, p: Point) -> PathSeg {
    match seg {
        PathSeg::Line(l) => PathSeg::Line(Line::new(l.p0, p)),
        PathSeg::Quad(q) => PathSeg::Quad(QuadBez::new(q.p0, q.p1, p)),
        PathSeg::Cubic(c) => {
            let d = p - c.p3;
            PathSeg::Cubic(CubicBez::new(c.p0, c.p1, c.p2 + d, p))
        }
    }
}

/// A segment moved by `d`.
pub fn translated(seg: PathSeg, d: Vec2) -> PathSeg {
    match seg {
        PathSeg::Line(l) => PathSeg::Line(Line::new(l.p0 + d, l.p1 + d)),
        PathSeg::Quad(q) => PathSeg::Quad(QuadBez::new(q.p0 + d, q.p1 + d, q.p2 + d)),
        PathSeg::Cubic(c) => PathSeg::Cubic(CubicBez::new(c.p0 + d, c.p1 + d, c.p2 + d, c.p3 + d)),
    }
}

/// Split a path into subpaths (see the module docs for the normal form).
pub fn split(path: &BezPath) -> Vec<SubPath> {
    let mut out = Vec::new();
    let mut segs: Vec<PathSeg> = Vec::new();
    let mut start = Point::ORIGIN;
    let mut last = Point::ORIGIN;
    let mut have_start = false;
    let finish = |segs: &mut Vec<PathSeg>, closed: bool, out: &mut Vec<SubPath>| {
        if !segs.is_empty() {
            out.push(SubPath { segs: std::mem::take(segs), closed });
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                finish(&mut segs, false, &mut out);
                start = p;
                last = p;
                have_start = true;
            }
            PathEl::LineTo(p) => {
                if !have_start {
                    start = last;
                    have_start = true;
                }
                if p.distance(last) > EPS {
                    segs.push(PathSeg::Line(Line::new(last, p)));
                }
                last = p;
            }
            PathEl::QuadTo(a, p) => {
                if !have_start {
                    start = last;
                    have_start = true;
                }
                if [a, p].iter().any(|q| q.distance(last) > EPS) {
                    segs.push(PathSeg::Quad(QuadBez::new(last, a, p)));
                }
                last = p;
            }
            PathEl::CurveTo(a, b, p) => {
                if !have_start {
                    start = last;
                    have_start = true;
                }
                if [a, b, p].iter().any(|q| q.distance(last) > EPS) {
                    segs.push(PathSeg::Cubic(CubicBez::new(last, a, b, p)));
                }
                last = p;
            }
            PathEl::ClosePath => {
                if !segs.is_empty() {
                    if last.distance(start) > EPS {
                        segs.push(PathSeg::Line(Line::new(last, start)));
                    } else {
                        // Snap the end exactly onto the start.
                        let n = segs.len() - 1;
                        segs[n] = with_end(segs[n], start);
                    }
                }
                finish(&mut segs, true, &mut out);
                // A segment straight after ClosePath starts at the closed subpath's start (SVG rules).
                last = start;
                have_start = false;
            }
        }
    }
    finish(&mut segs, false, &mut out);
    out
}

/// Subpaths back to one path.
pub fn join(subs: &[SubPath]) -> BezPath {
    let mut p = BezPath::new();
    for s in subs {
        if s.segs.is_empty() {
            continue;
        }
        p.move_to(s.start());
        let n = s.segs.len();
        for (i, seg) in s.segs.iter().enumerate() {
            let closing_line = s.closed && i == n - 1 && n > 1 && matches!(seg, PathSeg::Line(_));
            if closing_line {
                break;
            }
            match *seg {
                PathSeg::Line(l) => p.line_to(l.p1),
                PathSeg::Quad(q) => p.quad_to(q.p1, q.p2),
                PathSeg::Cubic(c) => p.curve_to(c.p1, c.p2, c.p3),
            }
        }
        if s.closed {
            p.close_path();
        }
    }
    p
}

/// The part of an open run of segments between positions `a <= b`, where a position is `segment index + t`
/// (`0..=segs.len()`). Empty when `a == b`.
pub fn slice(segs: &[PathSeg], a: f64, b: f64) -> Vec<PathSeg> {
    let n = segs.len() as f64;
    let (a, b) = (a.clamp(0.0, n), b.clamp(0.0, n));
    if b - a <= 1e-12 {
        return vec![];
    }
    let mut out = Vec::new();
    let first = (a.floor() as usize).min(segs.len() - 1);
    let last = ((b.ceil() as usize).max(1) - 1).min(segs.len() - 1);
    for (i, seg) in segs.iter().enumerate().take(last + 1).skip(first) {
        let t0 = if i == first { a - i as f64 } else { 0.0 };
        let t1 = if i == last { b - i as f64 } else { 1.0 };
        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
        if t1 - t0 <= 1e-12 {
            continue;
        }
        let piece = if t0 == 0.0 && t1 == 1.0 { *seg } else { seg.subsegment(t0..t1) };
        if matches!(piece, PathSeg::Line(l) if l.p0.distance(l.p1) <= EPS) {
            continue;
        }
        out.push(piece);
    }
    // Keep the pieces connected exactly.
    for i in 1..out.len() {
        let p = out[i - 1].end();
        out[i] = with_start(out[i], p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    #[test]
    fn round_trip_keeps_curves_and_closing() {
        let c = kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01);
        let subs = split(&c);
        assert_eq!(subs.len(), 1);
        assert!(subs[0].closed);
        assert!(subs[0].segs.iter().all(|s| matches!(s, PathSeg::Cubic(_))));
        let back = join(&subs);
        assert!((back.area() - c.area()).abs() < 1e-9);
        let r = kurbo::Rect::new(0.0, 0.0, 4.0, 2.0).to_path(0.01);
        let subs = split(&r);
        assert_eq!(subs[0].segs.len(), 4, "the closing line is explicit");
        assert_eq!(subs[0].node_count(), 4);
        assert_eq!(join(&subs).elements().len(), r.elements().len());
    }

    #[test]
    fn degenerate_input() {
        let mut p = BezPath::new();
        p.move_to((1.0, 1.0));
        p.line_to((1.0, 1.0));
        p.close_path();
        p.move_to((0.0, 0.0));
        assert!(split(&p).is_empty());
        assert!(join(&[]).elements().is_empty());
    }

    #[test]
    fn slicing_keeps_curves() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        p.curve_to((12.0, 0.0), (14.0, 2.0), (14.0, 4.0));
        let s = split(&p);
        let piece = slice(&s[0].segs, 0.5, 1.5);
        assert_eq!(piece.len(), 2);
        assert!((piece[0].start().x - 5.0).abs() < 1e-12);
        assert!(matches!(piece[1], PathSeg::Cubic(_)));
        assert!(slice(&s[0].segs, 1.0, 1.0).is_empty());
        assert_eq!(slice(&s[0].segs, 0.0, 2.0).len(), 2);
    }
}
