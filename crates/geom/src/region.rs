//! Filled regions and polygon booleans (`i_overlay`, MIT OR Apache-2.0). A [`Region`] is a set of shapes, each an
//! outer contour (counter-clockwise) followed by its holes (clockwise), as `i_overlay` returns them. Curves are
//! flattened at [`TOLERANCE`](crate::TOLERANCE) on the way in, so results are polygons.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::clip::FloatClip;
use i_overlay::float::simplify::SimplifyShape;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::string::clip::ClipRule;
use kurbo::{BezPath, Point, Rect};

use crate::flatten::{Polyline, flatten, polygon_area};
use crate::{Result, TOLERANCE, guard};

pub type Contour = Vec<[f64; 2]>;

/// How the closed subpaths of one path make its filled area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    /// Inside where an odd number of subpaths surround the point (how `plan` hatches one object, and how LightBurn
    /// fills): a subpath inside another is a hole whatever its direction.
    EvenOdd,
    /// Inside where the winding number is not zero: subpaths drawn the same way add up (a circle drawn inside a
    /// square the same way round is swallowed), opposite ones cut holes (letter counters). Weld uses this.
    NonZero,
}

/// A boolean between two regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    /// The first region minus the second.
    Difference,
    Intersect,
    /// Inside exactly one of the two.
    Xor,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Region {
    /// Shapes: `[outer, hole, hole, ...]`, outer counter-clockwise, holes clockwise. Never empty contours.
    pub shapes: Vec<Vec<Contour>>,
}

fn to_arr(p: Point) -> [f64; 2] {
    [p.x, p.y]
}

fn to_pt(a: [f64; 2]) -> Point {
    Point::new(a[0], a[1])
}

/// The closed rings of `path` (flattened), as contours of at least 3 points.
pub fn closed_contours(path: &BezPath) -> Result<Vec<Contour>> {
    Ok(flatten(path, TOLERANCE)?
        .into_iter()
        .filter(|l| l.closed && l.points.len() >= 3)
        .map(|l| l.points.into_iter().map(to_arr).collect())
        .collect())
}

/// Whether `path` has a subpath that is not closed (see [`flatten`] for the rule).
pub fn has_open_subpaths(path: &BezPath) -> Result<bool> {
    Ok(flatten(path, TOLERANCE)?.iter().any(|l| !l.closed))
}

impl Region {
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// The filled area of one path under `fill`. Open subpaths enclose nothing and are ignored.
    pub fn from_path(path: &BezPath, fill: Fill) -> Result<Region> {
        let contours = closed_contours(path)?;
        Region::from_contours(contours, fill)
    }

    fn from_contours(contours: Vec<Contour>, fill: Fill) -> Result<Region> {
        if contours.is_empty() {
            return Ok(Region::default());
        }
        check_range(contours.iter().flatten())?;
        let rule = match fill {
            Fill::EvenOdd => FillRule::EvenOdd,
            Fill::NonZero => FillRule::NonZero,
        };
        let shapes = guard("simplify", || contours.simplify_shape(rule))?;
        Ok(Region { shapes: clean(shapes) })
    }

    /// The union of several paths, each filled on its own under `fill` first (so one object's holes stay holes
    /// unless another object covers them).
    pub fn from_paths(paths: &[BezPath], fill: Fill) -> Result<Region> {
        let regions = paths.iter().map(|p| Region::from_path(p, fill)).collect::<Result<Vec<_>>>()?;
        Region::union_all(regions)
    }

    /// The union of regions.
    pub fn union_all(regions: impl IntoIterator<Item = Region>) -> Result<Region> {
        // Normalised shapes (outer CCW, holes CW) add their winding numbers, so a non-zero simplify is the union.
        let contours: Vec<Contour> = regions.into_iter().flat_map(|r| r.shapes.into_iter().flatten()).collect();
        Region::from_contours(contours, Fill::NonZero)
    }

    /// `self op other`.
    pub fn boolean(&self, other: &Region, op: BoolOp) -> Result<Region> {
        match (self.is_empty(), other.is_empty(), op) {
            (true, _, BoolOp::Union | BoolOp::Xor) => return Ok(other.clone()),
            (_, true, BoolOp::Union | BoolOp::Xor | BoolOp::Difference) => return Ok(self.clone()),
            (true, _, _) | (_, true, BoolOp::Intersect) => return Ok(Region::default()),
            _ => {}
        }
        check_range(self.shapes.iter().chain(&other.shapes).flatten().flatten())?;
        let rule = match op {
            BoolOp::Union => OverlayRule::Union,
            BoolOp::Difference => OverlayRule::Difference,
            BoolOp::Intersect => OverlayRule::Intersect,
            BoolOp::Xor => OverlayRule::Xor,
        };
        let shapes = guard("boolean", || self.shapes.overlay(&other.shapes, rule, FillRule::NonZero))?;
        Ok(Region { shapes: clean(shapes) })
    }

    /// Area in mm² (holes subtracted).
    pub fn area(&self) -> f64 {
        self.shapes.iter().flatten().map(|c| polygon_area(&c.iter().copied().map(to_pt).collect::<Vec<_>>())).sum()
    }

    pub fn bounds(&self) -> Option<Rect> {
        let mut it = self.shapes.iter().flatten().flatten().map(|&a| to_pt(a));
        let first = it.next()?;
        Some(it.fold(Rect::from_points(first, first), |r, p| r.union_pt(p)))
    }

    /// Every contour as one path (outer and holes; draw with even-odd or non-zero, both give the same area).
    pub fn to_path(&self) -> BezPath {
        let mut p = BezPath::new();
        for c in self.shapes.iter().flatten() {
            push_ring(&mut p, c);
        }
        p
    }

    /// One path per island: an outer contour with its own holes.
    pub fn islands(&self) -> Vec<BezPath> {
        self.shapes
            .iter()
            .map(|s| {
                let mut p = BezPath::new();
                for c in s {
                    push_ring(&mut p, c);
                }
                p
            })
            .collect()
    }

    /// Number of points over all contours.
    pub fn point_count(&self) -> usize {
        self.shapes.iter().flatten().map(Vec::len).sum()
    }
}

fn push_ring(p: &mut BezPath, c: &Contour) {
    let Some((&first, rest)) = c.split_first() else { return };
    p.move_to(to_pt(first));
    for &q in rest {
        p.line_to(to_pt(q));
    }
    p.close_path();
}

/// Refuse coordinates beyond [`MAX_COORD`](crate::MAX_COORD) before they reach `i_overlay` (which panics on them;
/// on wasm a panic aborts).
pub(crate) fn check_range<'a>(points: impl IntoIterator<Item = &'a [f64; 2]>) -> Result<()> {
    let max = crate::MAX_COORD;
    if points.into_iter().any(|p| !(p[0].abs() <= max && p[1].abs() <= max)) {
        return crate::invalid(format!(
            "a shape lies more than {max} mm from the origin (or has a coordinate that is not a number); move it closer first"
        ));
    }
    Ok(())
}

/// Drop contours too small to be rings and shapes whose outer contour went.
fn clean(shapes: Vec<Vec<Contour>>) -> Vec<Vec<Contour>> {
    shapes
        .into_iter()
        .filter_map(|s| {
            let mut it = s.into_iter();
            let outer = it.next().filter(|c| c.len() >= 3)?;
            Some(std::iter::once(outer).chain(it.filter(|c| c.len() >= 3)).collect())
        })
        .collect()
}

/// The parts of open polylines inside (`inside = true`) or outside `region`. Lines along the boundary count as
/// inside. Pieces come back as point lists of at least 2 points.
pub fn clip_lines(lines: &[Vec<Point>], region: &Region, inside: bool) -> Result<Vec<Vec<Point>>> {
    let paths: Vec<Contour> =
        lines.iter().filter(|l| l.len() >= 2).map(|l| l.iter().copied().map(to_arr).collect()).collect();
    if paths.is_empty() {
        return Ok(vec![]);
    }
    check_range(paths.iter().flatten())?;
    if region.is_empty() {
        return Ok(if inside { vec![] } else { paths.into_iter().map(|c| c.into_iter().map(to_pt).collect()).collect() });
    }
    let rule = ClipRule { invert: !inside, boundary_included: inside };
    let out = guard("clip", || paths.clip_by(&region.shapes, FillRule::NonZero, rule))?;
    Ok(out.into_iter().filter(|c| c.len() >= 2).map(|c| c.into_iter().map(to_pt).collect()).collect())
}

/// A polyline (open, or closed with its first point repeated) as plain points, for [`clip_lines`].
pub fn polyline_points(l: &Polyline) -> Vec<Point> {
    l.drawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> BezPath {
        Rect::new(x0, y0, x1, y1).to_path(0.01)
    }

    fn area(r: &Region) -> f64 {
        r.area()
    }

    #[test]
    fn booleans_of_two_squares() {
        let a = Region::from_path(&rect(0.0, 0.0, 10.0, 10.0), Fill::EvenOdd).unwrap();
        let b = Region::from_path(&rect(5.0, 0.0, 15.0, 10.0), Fill::EvenOdd).unwrap();
        assert!((area(&a.boolean(&b, BoolOp::Union).unwrap()) - 150.0).abs() < 1e-6);
        assert!((area(&a.boolean(&b, BoolOp::Intersect).unwrap()) - 50.0).abs() < 1e-6);
        assert!((area(&a.boolean(&b, BoolOp::Difference).unwrap()) - 50.0).abs() < 1e-6);
        let x = a.boolean(&b, BoolOp::Xor).unwrap();
        assert!((area(&x) - 100.0).abs() < 1e-6);
        assert_eq!(x.shapes.len(), 2, "two islands");
        let far = Region::from_path(&rect(50.0, 50.0, 60.0, 60.0), Fill::EvenOdd).unwrap();
        assert!(a.boolean(&far, BoolOp::Intersect).unwrap().is_empty());
    }

    #[test]
    fn holes_and_fill_rules() {
        // A square with a same-direction square inside: even-odd makes a hole, non-zero swallows it.
        let mut p = rect(0.0, 0.0, 10.0, 10.0);
        p.extend(rect(2.0, 2.0, 8.0, 8.0));
        let eo = Region::from_path(&p, Fill::EvenOdd).unwrap();
        assert!((eo.area() - 64.0).abs() < 1e-6);
        assert_eq!(eo.shapes.len(), 1);
        assert_eq!(eo.shapes[0].len(), 2, "outer + hole");
        assert!(polygon_area(&eo.shapes[0][0].iter().map(|&a| to_pt(a)).collect::<Vec<_>>()) > 0.0, "outer is CCW");
        let nz = Region::from_path(&p, Fill::NonZero).unwrap();
        assert!((nz.area() - 100.0).abs() < 1e-6);
        // Union keeps a hole that no other object covers.
        let other = Region::from_path(&rect(20.0, 0.0, 30.0, 10.0), Fill::EvenOdd).unwrap();
        let u = eo.boolean(&other, BoolOp::Union).unwrap();
        assert!((u.area() - 164.0).abs() < 1e-6);
        assert_eq!(u.islands().len(), 2);
    }

    #[test]
    fn self_intersecting_bowtie() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 10.0));
        p.line_to((10.0, 0.0));
        p.line_to((0.0, 10.0));
        p.close_path();
        let r = Region::from_path(&p, Fill::EvenOdd).unwrap();
        assert!((r.area() - 50.0).abs() < 1e-6, "{}", r.area());
        assert_eq!(r.shapes.len(), 2);
    }

    #[test]
    fn open_paths_enclose_nothing_and_empty_inputs_are_fine() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((10.0, 0.0));
        assert!(Region::from_path(&p, Fill::EvenOdd).unwrap().is_empty());
        assert!(has_open_subpaths(&p).unwrap());
        let e = Region::default();
        let a = Region::from_path(&rect(0.0, 0.0, 1.0, 1.0), Fill::EvenOdd).unwrap();
        assert_eq!(e.boolean(&a, BoolOp::Union).unwrap(), a);
        assert!(e.boolean(&a, BoolOp::Intersect).unwrap().is_empty());
        assert_eq!(a.boolean(&e, BoolOp::Difference).unwrap(), a);
        assert!(Region::union_all(vec![]).unwrap().is_empty());
    }

    #[test]
    fn clipping_lines_inside_and_outside() {
        let r = Region::from_path(&rect(0.0, 0.0, 10.0, 10.0), Fill::EvenOdd).unwrap();
        let line = vec![Point::new(-5.0, 5.0), Point::new(15.0, 5.0)];
        let inside = clip_lines(std::slice::from_ref(&line), &r, true).unwrap();
        let outside = clip_lines(&[line], &r, false).unwrap();
        let len = |v: &Vec<Vec<Point>>| -> f64 { v.iter().map(|l| l.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>()).sum() };
        assert!((len(&inside) - 10.0).abs() < 1e-6, "{inside:?}");
        assert!((len(&outside) - 10.0).abs() < 1e-6, "{outside:?}");
        assert_eq!(outside.len(), 2);
    }

    #[test]
    fn absurd_coordinates_are_refused_not_a_panic() {
        // Finite but far beyond what i_overlay accepts (it panics past ~1e150; on wasm that aborts).
        let far = rect(1e300, 0.0, 1e300 + 1e290, 1e290);
        assert!(matches!(Region::from_path(&far, Fill::EvenOdd), Err(crate::GeomError::Invalid(_))));
        let a = Region::from_path(&rect(0.0, 0.0, 10.0, 10.0), Fill::EvenOdd).unwrap();
        let built = Region { shapes: vec![vec![vec![[0.0, 0.0], [1e308, 0.0], [0.0, 1e308]]]] };
        assert!(a.boolean(&built, BoolOp::Union).is_err());
        let line = vec![Point::new(-1e200, 5.0), Point::new(1e200, 5.0)];
        assert!(clip_lines(&[line], &a, true).is_err());
        assert!(crate::offset::offset_region(&built, 1.0, crate::offset::Join::Round).is_err());
        assert!(crate::offset::offset_polyline(&[Point::new(0.0, 0.0), Point::new(2e6, 0.0)], 1.0, crate::offset::Join::Round).is_err());
        // Up to the limit works.
        let big = Region::from_path(&rect(-1e6, -1e6, 1e6, 1e6), Fill::EvenOdd).unwrap();
        assert!((big.area() - 4e12).abs() < 1e3);
    }

    #[test]
    fn circle_difference_stays_within_tolerance() {
        let big = kurbo::Circle::new((0.0, 0.0), 20.0).to_path(0.01);
        let small = kurbo::Circle::new((0.0, 0.0), 10.0).to_path(0.01);
        let a = Region::from_path(&big, Fill::EvenOdd).unwrap();
        let b = Region::from_path(&small, Fill::EvenOdd).unwrap();
        let ring = a.boolean(&b, BoolOp::Difference).unwrap();
        let expect = std::f64::consts::PI * (400.0 - 100.0);
        assert!((ring.area() - expect).abs() < 2.0, "{}", ring.area());
        assert_eq!(ring.shapes[0].len(), 2);
    }
}
