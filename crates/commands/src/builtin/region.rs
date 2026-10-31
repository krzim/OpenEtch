//! Stock-relative geometry: signed distance to the stock outline in the stock's own frame, and containment tests used
//! by `art.place` and the `doc.inspect` warnings.
//!
//! The stock's own frame has its origin at the stock centre and its axes along the stock's (rotated) edges. A point
//! `p` in that frame is inside the stock when `sdf(p) <= 0`, and at least `m` mm inside when `sdf(p) <= -m`. Both stock
//! shapes are convex, so a box is inside when its four corners are.

use doc::{Stock, StockShape};
use kurbo::{BezPath, PathEl, Point, Rect, Shape};

/// Tolerance for "touching counts as inside", in mm.
pub const EPS: f64 = 1e-6;

/// Signed distance from `p` (stock frame, mm) to the stock outline: negative inside.
pub fn sdf(stock: &Stock, p: Point) -> f64 {
    match stock.shape {
        StockShape::Circle { diameter_mm } => p.to_vec2().hypot() - diameter_mm / 2.0,
        StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
            let r = corner_radius_mm.clamp(0.0, width_mm.min(height_mm) / 2.0);
            let qx = p.x.abs() - (width_mm / 2.0 - r);
            let qy = p.y.abs() - (height_mm / 2.0 - r);
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
        }
    }
}

/// Half-extents of the stock's bounding box in its own frame.
pub fn half_size(stock: &Stock) -> (f64, f64) {
    match stock.shape {
        StockShape::Circle { diameter_mm } => (diameter_mm / 2.0, diameter_mm / 2.0),
        StockShape::Rect { width_mm, height_mm, .. } => (width_mm / 2.0, height_mm / 2.0),
    }
}

/// True when `rect` (stock frame) lies at least `margin` mm inside the stock.
pub fn box_inside(stock: &Stock, rect: Rect, margin: f64) -> bool {
    [(rect.x0, rect.y0), (rect.x1, rect.y0), (rect.x0, rect.y1), (rect.x1, rect.y1)]
        .into_iter()
        .all(|(x, y)| sdf(stock, Point::new(x, y)) <= -margin + EPS)
}

/// True when the stock outline has curved parts, so anchoring has to search instead of snapping to edges.
pub fn is_curved(stock: &Stock) -> bool {
    match stock.shape {
        StockShape::Circle { .. } => true,
        StockShape::Rect { corner_radius_mm, .. } => corner_radius_mm > 0.0,
    }
}

/// Vertices of `path` flattened to `tolerance` mm.
pub fn flat_points(path: &BezPath, tolerance: f64) -> Vec<Point> {
    let mut pts = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => pts.push(p),
        _ => {}
    });
    pts
}

/// Like [`flat_points`], but long straight runs are subdivided so no two consecutive points are more than `step` mm
/// apart. Used where a straight segment crossing the stock must not be mistaken for one outside it.
pub fn dense_points(path: &BezPath, tolerance: f64, step: f64) -> Vec<Point> {
    let mut pts: Vec<Point> = Vec::new();
    kurbo::flatten(path.iter(), tolerance, |el| match el {
        PathEl::MoveTo(p) => pts.push(p),
        PathEl::LineTo(p) => {
            if let Some(&last) = pts.last() {
                let n = ((p - last).hypot() / step).ceil().min(10_000.0) as usize;
                pts.extend((1..n).map(|i| last.lerp(p, i as f64 / n as f64)));
            }
            pts.push(p);
        }
        _ => {}
    });
    pts
}

/// The largest signed distance of any point of `path` (stock frame) to the stock outline. `<= 0` means the path is
/// entirely inside; the clearance to the edge is the negation.
pub fn max_sdf(stock: &Stock, path: &BezPath) -> Option<f64> {
    flat_points(path, 0.005).into_iter().map(|p| sdf(stock, p)).reduce(f64::max)
}

/// How a path sits relative to the stock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    Inside,
    Partly,
    Outside,
}

/// Classify a field-frame path against the stock outline.
pub fn containment(stock: &Stock, field_path: &BezPath) -> Containment {
    let local = stock.transform().inverse() * field_path.clone();
    let pts = dense_points(&local, 0.01, 0.5);
    let inside = pts.iter().filter(|&&p| sdf(stock, p) <= EPS).count();
    if inside == pts.len() {
        Containment::Inside
    } else if inside > 0 || local.bounding_box().contains(Point::ORIGIN) {
        // A path that encloses the whole stock has no vertex inside it but still overlaps it.
        Containment::Partly
    } else {
        Containment::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_stock(w: f64, h: f64, r: f64) -> Stock {
        Stock {
            name: "s".into(),
            shape: StockShape::Rect { width_mm: w, height_mm: h, corner_radius_mm: r },
            thickness_mm: 1.0,
            material: None,
            center: [0.0, 0.0],
            rotation_deg: 0.0,
        }
    }

    #[test]
    fn rect_sdf() {
        let s = rect_stock(20.0, 10.0, 0.0);
        assert!((sdf(&s, Point::ORIGIN) + 5.0).abs() < 1e-12);
        assert!((sdf(&s, Point::new(12.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!((sdf(&s, Point::new(13.0, 9.0)) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn rounded_corner_excludes_sharp_corner() {
        let s = rect_stock(20.0, 20.0, 5.0);
        assert!(sdf(&s, Point::new(9.9, 9.9)) > 0.0);
        assert!(!box_inside(&s, Rect::new(-10.0, -10.0, 10.0, 10.0), 0.0));
        assert!(box_inside(&s, Rect::new(-6.0, -6.0, 6.0, 6.0), 0.0));
    }

    #[test]
    fn circle_box() {
        let s = Stock { shape: StockShape::Circle { diameter_mm: 20.0 }, ..rect_stock(1.0, 1.0, 0.0) };
        let half = 10.0 / 2f64.sqrt();
        assert!(box_inside(&s, Rect::new(-half, -half, half, half), 0.0));
        assert!(!box_inside(&s, Rect::new(-half, -half, half, half), 0.1));
    }

    #[test]
    fn containment_classes() {
        let s = rect_stock(10.0, 10.0, 0.0);
        let path = |r: Rect| r.to_path(0.1);
        assert_eq!(containment(&s, &path(Rect::new(-1.0, -1.0, 1.0, 1.0))), Containment::Inside);
        assert_eq!(containment(&s, &path(Rect::new(4.0, -1.0, 6.0, 1.0))), Containment::Partly);
        assert_eq!(containment(&s, &path(Rect::new(20.0, 20.0, 21.0, 21.0))), Containment::Outside);
        assert_eq!(containment(&s, &path(Rect::new(-50.0, -50.0, 50.0, 50.0))), Containment::Partly);
        // A long line crossing the stock with both ends outside.
        let mut line = BezPath::new();
        line.move_to((-30.0, 3.0));
        line.line_to((30.0, 3.0));
        assert_eq!(containment(&s, &line), Containment::Partly);
    }
}
