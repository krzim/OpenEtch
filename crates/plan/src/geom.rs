//! Path flattening and small polyline helpers. Points are field-frame mm as `[x, y]`.

use kurbo::{BezPath, PathEl};

/// A field-frame point in mm.
pub type Point = [f64; 2];

/// Flattening tolerance: the largest distance between a curve and its polyline.
pub const FLATTEN_TOLERANCE_MM: f64 = 0.01;

/// Consecutive points closer than this are merged (far below one galvo unit on any lens).
const SAME_POINT_MM: f64 = 1e-9;

/// One flattened subpath. A closed subpath does not repeat its first point at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Subpath {
    /// The points to mark: a closed subpath gets its first point appended so the loop is closed.
    #[cfg(test)]
    pub fn to_polyline(&self) -> Vec<Point> {
        let mut out = self.points.clone();
        if self.closed
            && let Some(&first) = self.points.first()
        {
            out.push(first);
        }
        out
    }
}

/// Flatten `path` at [`FLATTEN_TOLERANCE_MM`] into subpaths.
///
/// A subpath is closed when it ends with `ClosePath` or its last point equals its first. Repeated points are merged
/// and single-point subpaths dropped (they have nothing to mark).
pub fn flatten(path: &BezPath) -> Vec<Subpath> {
    let mut out = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut closed_start: Option<Point> = None;
    kurbo::flatten(path, FLATTEN_TOLERANCE_MM, |el| match el {
        PathEl::MoveTo(p) => {
            finish(&mut current, false, &mut out);
            closed_start = None;
            current.push([p.x, p.y]);
        }
        PathEl::LineTo(p) => {
            // A LineTo straight after ClosePath starts a new subpath at the closed one's start (SVG rules).
            if current.is_empty()
                && let Some(s) = closed_start.take()
            {
                current.push(s);
            }
            push_point(&mut current, [p.x, p.y]);
        }
        PathEl::ClosePath => {
            closed_start = current.first().copied();
            finish(&mut current, true, &mut out);
        }
        // `kurbo::flatten` only emits MoveTo, LineTo and ClosePath.
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    finish(&mut current, false, &mut out);
    out
}

/// End the subpath being built in `pts` (if it has something to mark) and push it to `out`.
fn finish(pts: &mut Vec<Point>, closed: bool, out: &mut Vec<Subpath>) {
    let mut points = std::mem::take(pts);
    let mut closed = closed;
    if points.len() > 2 && points.first().zip(points.last()).is_some_and(|(a, b)| same(*a, *b)) {
        points.pop();
        closed = true;
    }
    if points.len() >= 2 {
        out.push(Subpath { closed: closed && points.len() >= 3, points });
    }
}

fn push_point(pts: &mut Vec<Point>, p: Point) {
    if pts.last().is_none_or(|&q| !same(p, q)) {
        pts.push(p);
    }
}

fn same(a: Point, b: Point) -> bool {
    dist(a, b) <= SAME_POINT_MM
}

/// Euclidean distance in mm.
pub fn dist(a: Point, b: Point) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// Length of a polyline in mm.
pub fn polyline_length(p: &[Point]) -> f64 {
    p.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// Rotate `p` about the origin by the angle whose `(sin, cos)` is given.
pub fn rotate(p: Point, (sin, cos): (f64, f64)) -> Point {
    [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos]
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{Rect, Shape};

    #[test]
    fn rectangle_is_one_closed_subpath() {
        let path = Rect::new(0.0, 0.0, 10.0, 5.0).to_path(0.01);
        let subs = flatten(&path);
        assert_eq!(subs.len(), 1);
        assert!(subs[0].closed);
        assert_eq!(subs[0].points.len(), 4);
        assert!((polyline_length(&subs[0].to_polyline()) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn open_path_stays_open() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((5.0, 0.0));
        path.line_to((5.0, 5.0));
        let subs = flatten(&path);
        assert_eq!(subs.len(), 1);
        assert!(!subs[0].closed);
        assert_eq!(subs[0].to_polyline().len(), 3);
    }

    #[test]
    fn circle_flattens_within_tolerance() {
        let path = kurbo::Circle::new((0.0, 0.0), 5.0).to_path(0.001);
        let subs = flatten(&path);
        assert_eq!(subs.len(), 1);
        assert!(subs[0].closed);
        let len = polyline_length(&subs[0].to_polyline());
        assert!((len - 10.0 * std::f64::consts::PI).abs() < 0.05, "{len}");
    }

    #[test]
    fn degenerate_subpaths_are_dropped() {
        let mut path = BezPath::new();
        path.move_to((1.0, 1.0));
        path.move_to((2.0, 2.0));
        path.line_to((2.0, 2.0));
        assert!(flatten(&path).is_empty());
    }
}
