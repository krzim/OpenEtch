//! Warp and deform (LightBurn's Warp Selection, 4 corner handles, and Deform Selection, a 4 x 4 grid of handles):
//! a smooth map of the plane defined on the selection's bounding box, applied to paths.
//!
//! - [`Warp::Perspective`]: the projective map that takes the box's corners to the four targets (straight lines stay
//!   straight; a perspective view of a flat sign).
//! - [`Warp::Envelope`]: the bilinear map through the four targets (edges stay straight, lines inside bend).
//! - [`Warp::Grid`]: a bicubic Bézier patch on 16 handles in a 4 x 4 grid (LightBurn's Deform; handles evenly spaced
//!   over the box change nothing).
//!
//! Curves are mapped piecewise: each segment is split until its mapped control points draw it within the
//! tolerance (checked at several points), so results stay Béziers; under a perspective map lines stay lines.

use kurbo::{BezPath, CubicBez, Line, ParamCurve, PathSeg, Point, Rect};

use crate::subpath::{SubPath, join, split};
use crate::{Result, invalid};

/// Most output segments one warp may make.
pub const MAX_WARP_SEGMENTS: usize = 500_000;

/// A map of the unit square (the selection's bounding box, `u` along +X, `v` along +Y) onto the plane.
#[derive(Debug, Clone, PartialEq)]
pub enum Warp {
    /// Projective map taking the box's corners to `[bottom_left, bottom_right, top_right, top_left]`.
    Perspective([Point; 4]),
    /// Bilinear map through `[bottom_left, bottom_right, top_right, top_left]`.
    Envelope([Point; 4]),
    /// Bicubic Bézier patch: `grid[row][col]`, row 0 at the bottom (-Y), col 0 at the left.
    Grid([[Point; 4]; 4]),
}

/// A warp fitted to a box, ready to map points.
#[derive(Debug, Clone)]
pub struct Mapper {
    rect: Rect,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    /// 3x3 homography (row-major) from unit-square coordinates.
    Homography([f64; 9]),
    Bilinear([Point; 4]),
    Patch([[Point; 4]; 4]),
}

fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

impl Mapper {
    /// Fit `warp` to `rect` (which must have a positive width and height). Refuses non-finite targets and, for a
    /// perspective, corners that do not form a convex quadrilateral in the box's order (the map would fold).
    pub fn new(rect: Rect, warp: &Warp) -> Result<Mapper> {
        if !(rect.width() > 0.0 && rect.height() > 0.0 && rect.is_finite()) {
            return invalid("the selection must have a width and a height to warp");
        }
        let kind = match warp {
            Warp::Perspective(q) => {
                if !q.iter().all(|p| finite(*p)) {
                    return invalid("every corner must be finite");
                }
                let cross = |i: usize| {
                    let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
                    (b - a).cross(c - b)
                };
                let signs: Vec<f64> = (0..4).map(cross).collect();
                if !(signs.iter().all(|s| *s > 1e-12) || signs.iter().all(|s| *s < -1e-12)) {
                    return invalid(
                        "the four corners must form a convex quadrilateral (in the order bottom-left, bottom-right, top-right, top-left)",
                    );
                }
                Kind::Homography(square_to_quad(q))
            }
            Warp::Envelope(q) => {
                if !q.iter().all(|p| finite(*p)) {
                    return invalid("every corner must be finite");
                }
                Kind::Bilinear(*q)
            }
            Warp::Grid(g) => {
                if !g.iter().flatten().all(|p| finite(*p)) {
                    return invalid("every grid handle must be finite");
                }
                Kind::Patch(*g)
            }
        };
        Ok(Mapper { rect, kind })
    }

    /// The handles that leave a shape unchanged: the box's corners, or a 4 x 4 grid evenly spread over it.
    pub fn identity_grid(rect: Rect) -> [[Point; 4]; 4] {
        let mut g = [[Point::ORIGIN; 4]; 4];
        for (r, row) in g.iter_mut().enumerate() {
            for (c, p) in row.iter_mut().enumerate() {
                *p = Point::new(rect.x0 + rect.width() * c as f64 / 3.0, rect.y0 + rect.height() * r as f64 / 3.0);
            }
        }
        g
    }

    /// Map one point.
    pub fn map(&self, p: Point) -> Point {
        let u = (p.x - self.rect.x0) / self.rect.width();
        let v = (p.y - self.rect.y0) / self.rect.height();
        match &self.kind {
            Kind::Homography(h) => {
                let w = h[6] * u + h[7] * v + h[8];
                Point::new((h[0] * u + h[1] * v + h[2]) / w, (h[3] * u + h[4] * v + h[5]) / w)
            }
            Kind::Bilinear(q) => {
                let a = q[0].to_vec2() * ((1.0 - u) * (1.0 - v));
                let b = q[1].to_vec2() * (u * (1.0 - v));
                let c = q[2].to_vec2() * (u * v);
                let d = q[3].to_vec2() * ((1.0 - u) * v);
                (a + b + c + d).to_point()
            }
            Kind::Patch(g) => {
                let (bu, bv) = (bern(u), bern(v));
                let mut s = kurbo::Vec2::ZERO;
                for (r, row) in g.iter().enumerate() {
                    for (c, p) in row.iter().enumerate() {
                        s += p.to_vec2() * (bv[r] * bu[c]);
                    }
                }
                s.to_point()
            }
        }
    }

    fn keeps_lines(&self) -> bool {
        matches!(self.kind, Kind::Homography(_))
    }

    /// Map a path within `tolerance` mm (segments split as needed). Fails past [`MAX_WARP_SEGMENTS`] or when a point
    /// maps to infinity (a perspective seen edge-on).
    pub fn map_path(&self, path: &BezPath, tolerance: f64) -> Result<BezPath> {
        let tol = if tolerance.is_finite() && tolerance > 0.0 { tolerance } else { 0.01 };
        let mut out = Vec::new();
        let mut count = 0usize;
        for sub in split(path) {
            let mut segs = Vec::new();
            for seg in &sub.segs {
                self.map_seg(*seg, tol, 0, &mut segs, &mut count)?;
            }
            if segs.is_empty() {
                continue;
            }
            for i in 1..segs.len() {
                let p = segs[i - 1].end();
                segs[i] = crate::subpath::with_start(segs[i], p);
            }
            if sub.closed {
                let n = segs.len() - 1;
                segs[n] = crate::subpath::with_end(segs[n], segs[0].start());
            }
            out.push(SubPath { segs, closed: sub.closed });
        }
        Ok(join(&out))
    }

    fn map_seg(&self, seg: PathSeg, tol: f64, depth: usize, out: &mut Vec<PathSeg>, count: &mut usize) -> Result<()> {
        *count += 1;
        if *count > MAX_WARP_SEGMENTS {
            return invalid(format!("the warped shape would need more than {MAX_WARP_SEGMENTS} segments"));
        }
        let mapped = match seg {
            PathSeg::Line(l) if self.keeps_lines() => PathSeg::Line(Line::new(self.map(l.p0), self.map(l.p1))),
            PathSeg::Line(l) => {
                let (a, b) = (l.p0, l.p1);
                let c = CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b);
                PathSeg::Cubic(CubicBez::new(self.map(c.p0), self.map(c.p1), self.map(c.p2), self.map(c.p3)))
            }
            PathSeg::Quad(q) => {
                let c = q.raise();
                PathSeg::Cubic(CubicBez::new(self.map(c.p0), self.map(c.p1), self.map(c.p2), self.map(c.p3)))
            }
            PathSeg::Cubic(c) => {
                PathSeg::Cubic(CubicBez::new(self.map(c.p0), self.map(c.p1), self.map(c.p2), self.map(c.p3)))
            }
        };
        let pts = match mapped {
            PathSeg::Line(l) => vec![l.p0, l.p1],
            PathSeg::Quad(q) => vec![q.p0, q.p1, q.p2],
            PathSeg::Cubic(c) => vec![c.p0, c.p1, c.p2, c.p3],
        };
        if !pts.iter().all(|p| finite(*p) && p.x.abs() < 1e9 && p.y.abs() < 1e9) {
            return invalid("the warp sends part of the shape to infinity");
        }
        let good =
            [0.125, 0.25, 0.5, 0.75, 0.875].iter().all(|&t| mapped.eval(t).distance(self.map(seg.eval(t))) <= tol);
        if good || depth >= 12 {
            // A mapped line that is still straight stays a line.
            if let PathSeg::Cubic(c) = mapped
                && matches!(seg, PathSeg::Line(_))
                && [c.p1, c.p2].iter().all(|p| dist_to_line(*p, c.p0, c.p3) <= tol * 1e-3)
            {
                out.push(PathSeg::Line(Line::new(c.p0, c.p3)));
            } else {
                out.push(mapped);
            }
            return Ok(());
        }
        let (a, b) = (seg.subsegment(0.0..0.5), seg.subsegment(0.5..1.0));
        self.map_seg(a, tol, depth + 1, out, count)?;
        self.map_seg(b, tol, depth + 1, out, count)
    }
}

fn dist_to_line(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let l = ab.hypot();
    if l <= 0.0 { p.distance(a) } else { ((p - a).cross(ab) / l).abs() }
}

fn bern(t: f64) -> [f64; 4] {
    let m = 1.0 - t;
    [m * m * m, 3.0 * t * m * m, 3.0 * t * t * m, t * t * t]
}

/// The homography taking (0,0), (1,0), (1,1), (0,1) to `q[0..4]` (Heckbert, "Fundamentals of Texture Mapping").
fn square_to_quad(q: &[Point; 4]) -> [f64; 9] {
    let (x0, y0, x1, y1, x2, y2, x3, y3) = (q[0].x, q[0].y, q[1].x, q[1].y, q[2].x, q[2].y, q[3].x, q[3].y);
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    if sx.abs() < 1e-15 && sy.abs() < 1e-15 {
        // Affine.
        return [x1 - x0, x3 - x0, x0, y1 - y0, y3 - y0, y0, 0.0, 0.0, 1.0];
    }
    let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
    let den = dx1 * dy2 - dx2 * dy1;
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 10.0, 10.0)
    }

    #[test]
    fn identity_handles_change_nothing() {
        let r = rect();
        let corners = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)];
        let circle = kurbo::Circle::new((5.0, 5.0), 4.0).to_path(0.01);
        for w in [Warp::Perspective(corners), Warp::Envelope(corners), Warp::Grid(Mapper::identity_grid(r))] {
            let m = Mapper::new(r, &w).unwrap();
            for p in [Point::new(1.0, 2.0), Point::new(9.0, 7.5)] {
                assert!(m.map(p).distance(p) < 1e-9, "{w:?}");
            }
            let out = m.map_path(&circle, 0.01).unwrap();
            assert!((out.area() - circle.area()).abs() < 1e-6, "{w:?}");
        }
    }

    #[test]
    fn perspective_keeps_lines_and_hits_the_corners() {
        let q = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(8.0, 6.0), Point::new(2.0, 6.0)];
        let m = Mapper::new(rect(), &Warp::Perspective(q)).unwrap();
        for (src, dst) in [
            (Point::new(0.0, 0.0), q[0]),
            (Point::new(10.0, 0.0), q[1]),
            (Point::new(10.0, 10.0), q[2]),
            (Point::new(0.0, 10.0), q[3]),
        ] {
            assert!(m.map(src).distance(dst) < 1e-9);
        }
        let square = Rect::new(2.0, 2.0, 8.0, 8.0).to_path(0.01);
        let out = m.map_path(&square, 0.01).unwrap();
        assert!(split(&out)[0].segs.iter().all(|s| matches!(s, PathSeg::Line(_))));
        // Lines further up are compressed (perspective), unlike a bilinear envelope.
        let mid_p = m.map(Point::new(5.0, 5.0));
        let e = Mapper::new(rect(), &Warp::Envelope(q)).unwrap();
        assert!((e.map(Point::new(5.0, 5.0)).y - 3.0).abs() < 1e-9);
        assert!(mid_p.y > 3.0, "{mid_p:?}");
        // A folded quadrilateral is refused.
        let bow = [q[0], q[2], q[1], q[3]];
        assert!(Mapper::new(rect(), &Warp::Perspective(bow)).is_err());
        assert!(Mapper::new(Rect::new(0.0, 0.0, 0.0, 5.0), &Warp::Envelope(q)).is_err());
    }

    #[test]
    fn envelope_bends_lines_within_tolerance() {
        let q = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(14.0, 10.0), Point::new(-4.0, 6.0)];
        let m = Mapper::new(rect(), &Warp::Envelope(q)).unwrap();
        let mut p = BezPath::new();
        p.move_to((0.0, 5.0));
        p.line_to((10.0, 5.0));
        p.move_to((5.0, 0.0));
        p.line_to((5.0, 10.0));
        let out = m.map_path(&p, 0.005).unwrap();
        for sub in split(&out) {
            for seg in &sub.segs {
                for t in [0.3, 0.6] {
                    // Every drawn point is the image of some source point: invert numerically by sampling.
                    let q = seg.eval(t);
                    let best = (0..=2000)
                        .map(|i| {
                            let s = i as f64 / 2000.0;
                            let a = m.map(Point::new(10.0 * s, 5.0)).distance(q);
                            let b = m.map(Point::new(5.0, 10.0 * s)).distance(q);
                            a.min(b)
                        })
                        .fold(f64::MAX, f64::min);
                    assert!(best < 0.02, "{q:?} {best}");
                }
            }
        }
        let mut g = Mapper::identity_grid(rect());
        g[3][3] = Point::new(20.0, 20.0);
        let d = Mapper::new(rect(), &Warp::Grid(g)).unwrap();
        assert!(d.map(Point::new(10.0, 10.0)).distance(Point::new(20.0, 20.0)) < 1e-9);
        assert!(d.map(Point::new(0.0, 0.0)).distance(Point::new(0.0, 0.0)) < 1e-9);
    }
}
