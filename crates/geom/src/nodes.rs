//! Node editing on a path (LightBurn's Edit Nodes): list nodes and segments, move nodes and handles, insert, delete,
//! break, join, change node and segment types.
//!
//! Numbering: nodes are numbered across all subpaths in order. A subpath of `n` segments has `n` nodes when closed
//! and `n + 1` when open; node `k` of a subpath is the start of its segment `k` (the last node of an open subpath is
//! its end). Segments are numbered the same way across subpaths. Quadratic segments are raised to cubics on load,
//! so every segment is a line or a cubic. The model holds plain geometry: a node's type is read from its handles.

use kurbo::{BezPath, CubicBez, Line, ParamCurve, PathSeg, Point, Vec2};

use crate::subpath::{SubPath, join, split, translated, with_end, with_start};
use crate::{EPS, Result, invalid};

/// A node's type, read from the geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// The path changes direction here (or the node is an end of an open subpath).
    Corner,
    /// The path runs straight through: both sides leave in the same direction.
    Smooth,
    /// Smooth, with both handles the same length.
    Symmetric,
}

/// A segment's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    Line,
    Curve,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub index: usize,
    pub subpath: usize,
    pub point: Point,
    pub kind: NodeKind,
    /// The handle on the incoming curve (its second control point), `None` when that side is a line or absent.
    pub handle_in: Option<Point>,
    /// The handle on the outgoing curve (its first control point).
    pub handle_out: Option<Point>,
    pub seg_in: Option<usize>,
    pub seg_out: Option<usize>,
    /// First or last node of an open subpath.
    pub endpoint: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SegInfo {
    pub index: usize,
    pub subpath: usize,
    pub kind: SegKind,
    pub from: usize,
    pub to: usize,
}

/// Which handle of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    In,
    Out,
}

/// An editable path.
#[derive(Debug, Clone, PartialEq)]
pub struct NodePath {
    pub subs: Vec<SubPath>,
}

impl NodePath {
    pub fn from_path(path: &BezPath) -> NodePath {
        NodePath { subs: split(path).into_iter().map(SubPath::cubics_only).collect() }
    }

    pub fn to_path(&self) -> BezPath {
        join(&self.subs)
    }

    pub fn node_count(&self) -> usize {
        self.subs.iter().map(SubPath::node_count).sum()
    }

    pub fn segment_count(&self) -> usize {
        self.subs.iter().map(|s| s.segs.len()).sum()
    }

    /// `(subpath, local node)` of global node `i`.
    pub fn locate(&self, i: usize) -> Result<(usize, usize)> {
        let mut base = 0;
        for (s, sub) in self.subs.iter().enumerate() {
            let n = sub.node_count();
            if i < base + n {
                return Ok((s, i - base));
            }
            base += n;
        }
        invalid(format!("no node {i}: the path has {} nodes (0..={})", self.node_count(), self.node_count().saturating_sub(1)))
    }

    /// `(subpath, local segment)` of global segment `i`.
    pub fn locate_segment(&self, i: usize) -> Result<(usize, usize)> {
        let mut base = 0;
        for (s, sub) in self.subs.iter().enumerate() {
            let n = sub.segs.len();
            if i < base + n {
                return Ok((s, i - base));
            }
            base += n;
        }
        invalid(format!("no segment {i}: the path has {} segments", self.segment_count()))
    }

    fn node_base(&self, sub: usize) -> usize {
        self.subs[..sub].iter().map(SubPath::node_count).sum()
    }

    /// Local incoming and outgoing segment of local node `k`.
    fn sides(sub: &SubPath, k: usize) -> (Option<usize>, Option<usize>) {
        let n = sub.segs.len();
        if sub.closed {
            (Some((k + n - 1) % n), Some(k % n))
        } else {
            ((k > 0).then(|| k - 1), (k < n).then_some(k))
        }
    }

    fn kind_of(sub: &SubPath, k: usize) -> NodeKind {
        let (si, so) = Self::sides(sub, k);
        let (Some(si), Some(so)) = (si, so) else { return NodeKind::Corner };
        let p = sub.node(k);
        let (din, hin) = match sub.segs[si] {
            PathSeg::Cubic(c) if c.p2.distance(p) > EPS => (p - c.p2, Some(c.p2)),
            s => (p - s.start(), None),
        };
        let (dout, hout) = match sub.segs[so] {
            PathSeg::Cubic(c) if c.p1.distance(p) > EPS => (c.p1 - p, Some(c.p1)),
            s => (s.end() - p, None),
        };
        // A retracted handle makes a corner.
        let retracted = |seg: PathSeg, h: Option<Point>| matches!(seg, PathSeg::Cubic(_)) && h.is_none();
        if retracted(sub.segs[si], hin) || retracted(sub.segs[so], hout) || din.hypot() <= EPS || dout.hypot() <= EPS {
            return NodeKind::Corner;
        }
        let angle = din.cross(dout).atan2(din.dot(dout)).abs();
        if angle > 0.5f64.to_radians() {
            return NodeKind::Corner;
        }
        match (hin, hout) {
            (Some(a), Some(b)) if (a.distance(p) - b.distance(p)).abs() <= 0.01 * a.distance(p).max(b.distance(p)) => {
                NodeKind::Symmetric
            }
            _ => NodeKind::Smooth,
        }
    }

    pub fn nodes(&self) -> Vec<NodeInfo> {
        let mut out = Vec::with_capacity(self.node_count());
        let (mut nb, mut sb) = (0, 0);
        for (s, sub) in self.subs.iter().enumerate() {
            for k in 0..sub.node_count() {
                let (si, so) = Self::sides(sub, k);
                let handle_in = si.and_then(|i| match sub.segs[i] {
                    PathSeg::Cubic(c) => Some(c.p2),
                    _ => None,
                });
                let handle_out = so.and_then(|i| match sub.segs[i] {
                    PathSeg::Cubic(c) => Some(c.p1),
                    _ => None,
                });
                out.push(NodeInfo {
                    index: nb + k,
                    subpath: s,
                    point: sub.node(k),
                    kind: Self::kind_of(sub, k),
                    handle_in,
                    handle_out,
                    seg_in: si.map(|i| sb + i),
                    seg_out: so.map(|i| sb + i),
                    endpoint: !sub.closed && (k == 0 || k == sub.segs.len()),
                });
            }
            nb += sub.node_count();
            sb += sub.segs.len();
        }
        out
    }

    pub fn segments(&self) -> Vec<SegInfo> {
        let mut out = Vec::with_capacity(self.segment_count());
        let (mut nb, mut sb) = (0, 0);
        for (s, sub) in self.subs.iter().enumerate() {
            let n = sub.segs.len();
            for (k, seg) in sub.segs.iter().enumerate() {
                let to = if sub.closed && k + 1 == n { 0 } else { k + 1 };
                let kind = if matches!(seg, PathSeg::Line(_)) { SegKind::Line } else { SegKind::Curve };
                out.push(SegInfo { index: sb + k, subpath: s, kind, from: nb + k, to: nb + to });
            }
            nb += sub.node_count();
            sb += n;
        }
        out
    }

    /// Move node `i` to `to`; its handles move with it.
    pub fn move_node(&mut self, i: usize, to: Point) -> Result<()> {
        let (s, k) = self.locate(i)?;
        let sub = &mut self.subs[s];
        let d = to - sub.node(k);
        let (si, so) = Self::sides(sub, k);
        if let Some(si) = si {
            sub.segs[si] = match sub.segs[si] {
                PathSeg::Cubic(c) => PathSeg::Cubic(CubicBez::new(c.p0, c.p1, c.p2 + d, to)),
                seg => with_end(seg, to),
            };
        }
        if let Some(so) = so {
            sub.segs[so] = match sub.segs[so] {
                PathSeg::Cubic(c) => PathSeg::Cubic(CubicBez::new(to, c.p1 + d, c.p2, c.p3)),
                seg => with_start(seg, to),
            };
        }
        Ok(())
    }

    /// Move one handle of node `i` to `to`. On a smooth node the other handle turns to stay opposite (keeping its
    /// length); on a symmetric node it also takes the same length.
    pub fn move_handle(&mut self, i: usize, which: Handle, to: Point) -> Result<()> {
        let (s, k) = self.locate(i)?;
        let kind = Self::kind_of(&self.subs[s], k);
        let sub = &mut self.subs[s];
        let p = sub.node(k);
        let (si, so) = Self::sides(sub, k);
        let (mine, other) = match which {
            Handle::In => (si, so),
            Handle::Out => (so, si),
        };
        let Some(mine) = mine else { return invalid(format!("node {i} has no {} side", side_name(which))) };
        let PathSeg::Cubic(c) = sub.segs[mine] else {
            return invalid(format!(
                "the {} segment of node {i} is a line, which has no handle; make it a curve first (node.segment_type)",
                side_name(which)
            ));
        };
        sub.segs[mine] = PathSeg::Cubic(match which {
            Handle::In => CubicBez::new(c.p0, c.p1, to, c.p3),
            Handle::Out => CubicBez::new(c.p0, to, c.p2, c.p3),
        });
        if matches!(kind, NodeKind::Smooth | NodeKind::Symmetric)
            && let Some(other) = other
            && other != mine
            && let PathSeg::Cubic(o) = sub.segs[other]
        {
            let dir = p - to;
            if dir.hypot() > EPS {
                let dir = dir / dir.hypot();
                let old = match which {
                    Handle::In => o.p1.distance(p),
                    Handle::Out => o.p2.distance(p),
                };
                let len = if kind == NodeKind::Symmetric { to.distance(p) } else { old };
                let h = p + dir * len;
                sub.segs[other] = PathSeg::Cubic(match which {
                    Handle::In => CubicBez::new(o.p0, h, o.p2, o.p3),
                    Handle::Out => CubicBez::new(o.p0, o.p1, h, o.p3),
                });
            }
        }
        Ok(())
    }

    /// Split segment `seg` at `t` (0 < t < 1), keeping its shape. Returns the new node's index.
    pub fn insert(&mut self, seg: usize, t: f64) -> Result<usize> {
        if !(t.is_finite() && t > 0.0 && t < 1.0) {
            return invalid("t must be strictly between 0 and 1");
        }
        let (s, k) = self.locate_segment(seg)?;
        let base = self.node_base(s);
        let sub = &mut self.subs[s];
        let a = sub.segs[k].subsegment(0.0..t);
        let b = with_start(sub.segs[k].subsegment(t..1.0), a.end());
        sub.segs[k] = a;
        sub.segs.insert(k + 1, b);
        Ok(base + k + 1)
    }

    /// Delete nodes, joining their neighbours with one segment (a line when both sides were lines, else a curve
    /// keeping the outer handles). Deleting an end of an open subpath drops its end segment. Subpaths left with too
    /// few nodes go. Fails when nothing would remain.
    pub fn delete(&mut self, indices: &[usize]) -> Result<()> {
        let mut idx: Vec<usize> = indices.to_vec();
        idx.sort_unstable();
        idx.dedup();
        for &i in &idx {
            self.locate(i)?;
        }
        // Resolve every index first, so removing a subpath cannot shift the meaning of the others.
        let targets = idx.iter().map(|&i| self.locate(i)).collect::<Result<Vec<_>>>()?;
        let mut work = self.clone();
        let mut removed: Vec<usize> = Vec::new();
        for &(s, k) in targets.iter().rev() {
            if removed.contains(&s) {
                continue;
            }
            let sub = &mut work.subs[s];
            let n = sub.segs.len();
            let merge = |a: PathSeg, b: PathSeg| -> PathSeg {
                match (a, b) {
                    (PathSeg::Line(a), PathSeg::Line(b)) => PathSeg::Line(Line::new(a.p0, b.p1)),
                    (a, b) => {
                        let h1 = if let PathSeg::Cubic(c) = a { c.p1 } else { a.start() };
                        let h2 = if let PathSeg::Cubic(c) = b { c.p2 } else { b.end() };
                        PathSeg::Cubic(CubicBez::new(a.start(), h1, h2, b.end()))
                    }
                }
            };
            let remove = if sub.closed {
                if n <= 2 {
                    true
                } else {
                    let (si, so) = ((k + n - 1) % n, k);
                    let m = merge(sub.segs[si], sub.segs[so]);
                    if so == 0 {
                        sub.segs.pop();
                        sub.segs[0] = m;
                    } else {
                        sub.segs[si] = m;
                        sub.segs.remove(so);
                    }
                    false
                }
            } else if k == 0 {
                sub.segs.remove(0);
                sub.segs.is_empty()
            } else if k == n {
                sub.segs.pop();
                sub.segs.is_empty()
            } else {
                let m = merge(sub.segs[k - 1], sub.segs[k]);
                sub.segs[k - 1] = m;
                sub.segs.remove(k);
                false
            };
            if remove {
                removed.push(s);
            }
        }
        removed.sort_unstable();
        for s in removed.into_iter().rev() {
            work.subs.remove(s);
        }
        if work.subs.is_empty() {
            return invalid("deleting these nodes would delete the whole shape; delete the object instead");
        }
        *self = work;
        Ok(())
    }

    /// Break the path at node `i`: a closed subpath opens there (the node becomes both ends); an open one splits in
    /// two. Returns the indices of the two end nodes made.
    pub fn break_at(&mut self, i: usize) -> Result<(usize, usize)> {
        let (s, k) = self.locate(i)?;
        let base = self.node_base(s);
        let sub = &mut self.subs[s];
        if sub.closed {
            sub.segs.rotate_left(k);
            sub.closed = false;
            let n = sub.segs.len();
            return Ok((base, base + n));
        }
        if k == 0 || k == sub.segs.len() {
            return invalid(format!("node {i} is already an end of an open path; there is nothing to break"));
        }
        let tail = sub.segs.split_off(k);
        self.subs.insert(s + 1, SubPath { segs: tail, closed: false });
        Ok((base + k, base + k + 1))
    }

    /// Join two end nodes of open subpaths into one node at their midpoint: the ends of one subpath close it, the
    /// ends of two subpaths connect them. Returns the joined node's index.
    pub fn join_nodes(&mut self, a: usize, b: usize) -> Result<usize> {
        if a == b {
            return invalid("join needs two different nodes");
        }
        let (sa, ka) = self.locate(a)?;
        let (sb, kb) = self.locate(b)?;
        for (s, k, i) in [(sa, ka, a), (sb, kb, b)] {
            let sub = &self.subs[s];
            if sub.closed || (k != 0 && k != sub.segs.len()) {
                return invalid(format!("node {i} is not an end of an open path; only end nodes can be joined"));
            }
        }
        let m = self.subs[sa].node(ka).midpoint(self.subs[sb].node(kb));
        if sa == sb {
            let sub = &mut self.subs[sa];
            if sub.segs.len() == 1 && matches!(sub.segs[0], PathSeg::Line(_)) {
                return invalid("joining the two ends of a single straight line would fold it onto itself");
            }
            let n = sub.segs.len() - 1;
            sub.segs[0] = with_start(sub.segs[0], m);
            sub.segs[n] = with_end(sub.segs[n], m);
            sub.closed = true;
            return Ok(self.node_base(sa));
        }
        // Orient: first ends at a, second starts at b.
        let mut first = self.subs[sa].clone();
        if ka == 0 {
            first = first.reversed();
        }
        let mut second = self.subs[sb].clone();
        if kb != 0 {
            second = second.reversed();
        }
        let n = first.segs.len() - 1;
        first.segs[n] = with_end(first.segs[n], m);
        second.segs[0] = with_start(second.segs[0], m);
        let joint = first.segs.len();
        first.segs.extend(second.segs);
        let (keep, drop) = (sa.min(sb), sa.max(sb));
        self.subs[keep] = first;
        self.subs.remove(drop);
        Ok(self.node_base(keep) + joint)
    }

    /// Set node types. `Corner` pulls both handles onto the node (a sharp point); `Smooth` lines the two sides up
    /// (straight sides next to the node become curves) keeping each handle's length; `Symmetric` also makes the
    /// handles the same length. End nodes of open subpaths only take `Corner`; they are skipped otherwise. Returns the
    /// nodes changed.
    pub fn set_kind(&mut self, indices: &[usize], kind: NodeKind) -> Result<Vec<usize>> {
        for &i in indices {
            self.locate(i)?;
        }
        let mut changed = Vec::new();
        for &i in indices {
            let (s, k) = self.locate(i)?;
            let sub = &mut self.subs[s];
            let p = sub.node(k);
            let (si, so) = Self::sides(sub, k);
            if kind == NodeKind::Corner {
                // Only handles that are not already on the node count as a change (a node between two lines, or
                // the end of an open line, is a corner already).
                let mut moved = false;
                if let Some(si) = si
                    && let PathSeg::Cubic(c) = sub.segs[si]
                    && c.p2 != p
                {
                    sub.segs[si] = PathSeg::Cubic(CubicBez::new(c.p0, c.p1, p, c.p3));
                    moved = true;
                }
                if let Some(so) = so
                    && let PathSeg::Cubic(c) = sub.segs[so]
                    && c.p1 != p
                {
                    sub.segs[so] = PathSeg::Cubic(CubicBez::new(c.p0, p, c.p2, c.p3));
                    moved = true;
                }
                if moved {
                    changed.push(i);
                }
                continue;
            }
            let (Some(si), Some(so)) = (si, so) else { continue };
            if si == so {
                continue;
            }
            let as_cubic = |seg: PathSeg| match seg {
                PathSeg::Cubic(c) => c,
                s => {
                    let (a, b) = (s.start(), s.end());
                    CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
                }
            };
            let (ci, co) = (as_cubic(sub.segs[si]), as_cubic(sub.segs[so]));
            let chord = |c: &CubicBez| c.p0.distance(c.p3) / 3.0;
            let (mut lin, mut lout) = (ci.p2.distance(p), co.p1.distance(p));
            if lin <= EPS {
                lin = chord(&ci);
            }
            if lout <= EPS {
                lout = chord(&co);
            }
            let unit = |v: Vec2| if v.hypot() > EPS { v / v.hypot() } else { Vec2::ZERO };
            let din = unit(if ci.p2.distance(p) > EPS { p - ci.p2 } else { p - ci.p0 });
            let dout = unit(if co.p1.distance(p) > EPS { co.p1 - p } else { co.p3 - p });
            let mut t = unit(din + dout);
            if t == Vec2::ZERO {
                // A hairpin: point across it.
                t = Vec2::new(-din.y, din.x);
            }
            if kind == NodeKind::Symmetric {
                let l = (lin + lout) / 2.0;
                lin = l;
                lout = l;
            }
            sub.segs[si] = PathSeg::Cubic(CubicBez::new(ci.p0, ci.p1, p - t * lin, ci.p3));
            sub.segs[so] = PathSeg::Cubic(CubicBez::new(co.p0, p + t * lout, co.p2, co.p3));
            changed.push(i);
        }
        Ok(changed)
    }

    /// Make segment `seg` a straight line or a curve (a straight curve with handles at a third and two thirds, to
    /// drag from there).
    pub fn set_segment(&mut self, seg: usize, kind: SegKind) -> Result<()> {
        let (s, k) = self.locate_segment(seg)?;
        let cur = self.subs[s].segs[k];
        let (a, b) = (cur.start(), cur.end());
        self.subs[s].segs[k] = match kind {
            SegKind::Line => PathSeg::Line(Line::new(a, b)),
            SegKind::Curve => match cur {
                PathSeg::Cubic(_) => cur,
                _ => PathSeg::Cubic(CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)),
            },
        };
        Ok(())
    }

    /// The whole path moved by `d` (for tests and callers).
    pub fn translate(&mut self, d: Vec2) {
        for s in &mut self.subs {
            for seg in &mut s.segs {
                *seg = translated(*seg, d);
            }
        }
    }
}

fn side_name(h: Handle) -> &'static str {
    match h {
        Handle::In => "incoming",
        Handle::Out => "outgoing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn square() -> NodePath {
        NodePath::from_path(&kurbo::Rect::new(0.0, 0.0, 10.0, 10.0).to_path(0.01))
    }

    #[test]
    fn list_nodes_and_segments() {
        let p = square();
        assert_eq!(p.node_count(), 4);
        let nodes = p.nodes();
        assert!(nodes.iter().all(|n| n.kind == NodeKind::Corner && n.handle_in.is_none()));
        let segs = p.segments();
        assert_eq!(segs[3].to, 0, "the closing segment returns to node 0");
        let c = NodePath::from_path(&kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01));
        assert!(c.nodes().iter().all(|n| matches!(n.kind, NodeKind::Smooth | NodeKind::Symmetric)), "{:?}", c.nodes());
        assert!(c.segments().iter().all(|s| s.kind == SegKind::Curve));
    }

    #[test]
    fn move_insert_delete() {
        let mut p = square();
        p.move_node(2, Point::new(12.0, 12.0)).unwrap();
        assert_eq!(p.nodes()[2].point, Point::new(12.0, 12.0));
        assert_eq!(p.subs[0].segs[1].end(), Point::new(12.0, 12.0));
        let n = p.insert(0, 0.5).unwrap();
        assert_eq!(n, 1);
        assert_eq!(p.nodes()[1].point, Point::new(5.0, 0.0));
        assert_eq!(p.node_count(), 5);
        p.delete(&[1]).unwrap();
        assert_eq!(p.node_count(), 4);
        assert!(p.insert(0, 1.0).is_err() && p.insert(9, 0.5).is_err());
        assert!(p.delete(&[0, 1, 2, 3]).is_err(), "would delete everything");
        assert!(p.move_node(17, Point::ORIGIN).is_err());
        // Deleting node 0 of a closed path keeps it closed.
        p.delete(&[0]).unwrap();
        assert_eq!(p.node_count(), 3);
        assert!(p.subs[0].closed);
    }

    #[test]
    fn handles_follow_node_types() {
        let mut c = NodePath::from_path(&kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.01));
        let n = c.nodes()[0].clone();
        let p = n.point;
        let h = n.handle_out.unwrap();
        let moved = p + (h - p) * 2.0;
        c.move_handle(0, Handle::Out, moved).unwrap();
        let after = c.nodes()[0].clone();
        assert_eq!(after.handle_out, Some(moved));
        let hin = after.handle_in.unwrap();
        assert!((hin - p).cross(moved - p).abs() < 1e-9, "still opposite");
        assert_eq!(after.kind, NodeKind::Symmetric, "a symmetric node stays symmetric");
        let mut sq = square();
        assert!(sq.move_handle(0, Handle::Out, Point::new(1.0, 1.0)).is_err(), "lines have no handles");
        sq.set_segment(0, SegKind::Curve).unwrap();
        sq.move_handle(0, Handle::Out, Point::new(1.0, -3.0)).unwrap();
        assert_eq!(sq.segments()[0].kind, SegKind::Curve);
        sq.set_segment(0, SegKind::Line).unwrap();
        assert_eq!(sq.segments()[0].kind, SegKind::Line);
        // Smooth a square corner, then make it a corner again.
        let mut sq = square();
        assert_eq!(sq.set_kind(&[1], NodeKind::Smooth).unwrap(), [1]);
        assert_eq!(sq.nodes()[1].kind, NodeKind::Symmetric, "equal sides give equal handles");
        assert_eq!(sq.set_kind(&[1], NodeKind::Corner).unwrap(), [1]);
        assert_eq!(sq.nodes()[1].kind, NodeKind::Corner);
        // Already a corner (handles on the node, or lines on both sides): nothing changes, nothing is reported.
        assert!(sq.set_kind(&[1], NodeKind::Corner).unwrap().is_empty());
        assert!(square().set_kind(&[0, 2], NodeKind::Corner).unwrap().is_empty());
    }

    #[test]
    fn break_and_join() {
        let mut p = square();
        let (a, b) = p.break_at(2).unwrap();
        assert!(!p.subs[0].closed);
        assert_eq!(p.node_count(), 5);
        assert_eq!(p.nodes()[a].point, p.nodes()[b].point);
        let j = p.join_nodes(a, b).unwrap();
        assert!(p.subs[0].closed);
        assert_eq!(p.node_count(), 4);
        assert_eq!(j, 0);
        // Split an open path in two and join it back.
        let mut l = NodePath::from_path(&{
            let mut b = BezPath::new();
            b.move_to((0.0, 0.0));
            b.line_to((1.0, 0.0));
            b.line_to((2.0, 0.0));
            b
        });
        assert!(l.break_at(0).is_err());
        l.break_at(1).unwrap();
        assert_eq!(l.subs.len(), 2);
        l.join_nodes(1, 2).unwrap();
        assert_eq!(l.subs.len(), 1);
        assert_eq!(l.node_count(), 3);
        assert!(l.join_nodes(1, 1).is_err());
        assert!(square().join_nodes(0, 1).is_err(), "closed paths have no ends");
    }
}
