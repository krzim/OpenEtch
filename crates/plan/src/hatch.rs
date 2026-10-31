//! Scanline hatching of closed rings with the even-odd fill rule.
//!
//! The rings are rotated by `-angle` so hatch lines are horizontal, cut by scanlines `interval` apart starting half
//! an interval inside the rings' lowest point (so a 10 mm square at 0.1 mm gets exactly 100 lines and none lies on
//! the outline), then the segments are rotated back.

use crate::geom::{Point, rotate};

/// More hatch lines than this in one hatch set is refused as a likely unit mistake.
pub const MAX_SCANLINES: usize = 2_000_000;

/// One straight hatch segment, marked from `[0]` to `[1]`.
pub type HatchLine = [Point; 2];

/// Hatch segments shorter than this are dropped (nothing to mark).
const MIN_SEGMENT_MM: f64 = 1e-6;

/// Why [`hatch`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HatchError {
    /// More than [`MAX_SCANLINES`] scanlines would be needed (likely a unit mistake).
    TooManyScanlines,
    /// More than the caller's `max_lines` hatch lines; hatching stopped as soon as the budget was passed.
    OverBudget,
}

/// Hatch the even-odd interior of `rings` (closed, without a repeated last point) with lines along `angle_deg`.
///
/// Lines come out in scanline order. With `bidirectional` every other scanline runs backwards (a zig-zag);
/// otherwise every segment runs the same way (+X of the hatch direction). Refused when more than
/// [`MAX_SCANLINES`] scanlines would be needed, or as soon as more than `max_lines` lines have been made (the job's
/// remaining segment budget), so a huge hatch never allocates past the budget.
pub fn hatch(rings: &[Vec<Point>], angle_deg: f64, interval_mm: f64, bidirectional: bool, max_lines: usize) -> Result<Vec<HatchLine>, HatchError> {
    let rad = angle_deg.to_radians();
    let to_local = (-rad).sin_cos();
    let to_field = rad.sin_cos();

    // Edges in the hatch frame, normalised so a.y <= b.y; horizontal edges never cross a scanline.
    let mut edges: Vec<(Point, Point)> = Vec::new();
    for ring in rings.iter().filter(|r| r.len() >= 3) {
        let local: Vec<Point> = ring.iter().map(|&p| rotate(p, to_local)).collect();
        for (i, &a) in local.iter().enumerate() {
            let b = local[(i + 1) % local.len()];
            if a[1] != b[1] {
                edges.push(if a[1] < b[1] { (a, b) } else { (b, a) });
            }
        }
    }
    let Some(y_min) = edges.iter().map(|e| e.0[1]).reduce(f64::min) else { return Ok(vec![]) };
    let y_max = edges.iter().map(|e| e.1[1]).fold(f64::NEG_INFINITY, f64::max);

    let span = y_max - y_min - interval_mm / 2.0;
    if span < 0.0 {
        return Ok(vec![]);
    }
    let count_f = (span / interval_mm).floor() + 1.0;
    if !count_f.is_finite() || count_f > MAX_SCANLINES as f64 {
        return Err(HatchError::TooManyScanlines);
    }
    // `count_f` is finite, whole and within 1..=MAX_SCANLINES (checked above).
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = count_f as usize;

    // Active-edge scan: edges sorted by their low end enter the active set as the scanline passes them.
    edges.sort_by(|a, b| a.0[1].total_cmp(&b.0[1]));
    let mut next_edge = 0;
    let mut active: Vec<(Point, Point)> = Vec::new();
    let mut xs: Vec<f64> = Vec::new();
    let mut out = Vec::new();
    for k in 0..count {
        #[allow(clippy::cast_precision_loss)]
        let y = y_min + interval_mm * (k as f64 + 0.5);
        while next_edge < edges.len() && edges[next_edge].0[1] <= y {
            active.push(edges[next_edge]);
            next_edge += 1;
        }
        active.retain(|e| e.1[1] > y);
        // Half-open crossing rule (a.y <= y < b.y) counts every vertex exactly once.
        xs.clear();
        xs.extend(active.iter().map(|(a, b)| a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1])));
        xs.sort_by(f64::total_cmp);
        let mut segments: Vec<HatchLine> = xs
            .as_chunks::<2>()
            .0
            .iter()
            .filter(|p| p[1] - p[0] > MIN_SEGMENT_MM)
            .map(|p| [rotate([p[0], y], to_field), rotate([p[1], y], to_field)])
            .collect();
        if bidirectional && k % 2 == 1 {
            segments.reverse();
            for s in &mut segments {
                s.swap(0, 1);
            }
        }
        out.extend(segments);
        if out.len() > max_lines {
            return Err(HatchError::OverBudget);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(side: f64) -> Vec<Point> {
        let h = side / 2.0;
        vec![[-h, -h], [h, -h], [h, h], [-h, h]]
    }

    #[test]
    fn square_gets_one_line_per_interval() {
        let lines = hatch(&[square(10.0)], 0.0, 0.1, true, usize::MAX).unwrap();
        assert_eq!(lines.len(), 100);
        for l in &lines {
            assert!(((l[1][0] - l[0][0]).abs() - 10.0).abs() < 1e-9);
        }
    }

    #[test]
    fn bidirectional_alternates_and_unidirectional_does_not() {
        let zig = hatch(&[square(10.0)], 0.0, 1.0, true, usize::MAX).unwrap();
        assert!(zig[0][1][0] > zig[0][0][0] && zig[1][1][0] < zig[1][0][0]);
        let uni = hatch(&[square(10.0)], 0.0, 1.0, false, usize::MAX).unwrap();
        assert!(uni.iter().all(|l| l[1][0] > l[0][0]));
    }

    #[test]
    fn even_odd_leaves_holes_empty() {
        let lines = hatch(&[square(10.0), square(4.0)], 0.0, 0.5, false, usize::MAX).unwrap();
        // Scanlines through the hole give two segments, the rest one.
        let through_hole = lines.iter().filter(|l| l[0][1].abs() < 2.0).count();
        assert!(through_hole > 0);
        for l in &lines {
            let mid = [(l[0][0] + l[1][0]) / 2.0, l[0][1]];
            assert!(!(mid[0].abs() < 2.0 && mid[1].abs() < 2.0), "segment {l:?} crosses the hole");
        }
    }

    #[test]
    fn angled_hatch_runs_along_the_angle() {
        let lines = hatch(&[square(10.0)], 90.0, 0.5, false, usize::MAX).unwrap();
        assert_eq!(lines.len(), 20);
        for l in &lines {
            assert!((l[0][0] - l[1][0]).abs() < 1e-9, "{l:?} is not vertical");
            assert!(l[1][1] > l[0][1]);
        }
    }

    #[test]
    fn absurd_interval_is_refused() {
        assert_eq!(hatch(&[square(100.0)], 0.0, 1e-6, true, usize::MAX), Err(HatchError::TooManyScanlines));
    }

    #[test]
    fn budget_stops_hatching_early() {
        assert_eq!(hatch(&[square(10.0)], 0.0, 0.1, true, 100).map(|l| l.len()), Ok(100));
        assert_eq!(hatch(&[square(10.0)], 0.0, 0.1, true, 99), Err(HatchError::OverBudget));
    }
}
