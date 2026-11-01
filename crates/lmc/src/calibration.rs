//! [`Calibration`]: galvo field corrections applied in field mm right before the conversion to galvo words, so the
//! burn (`plan`) and the red-light frame (`machine`) get exactly the same correction.
//!
//! These are LightBurn's galvo Device Settings: field offset X/Y, field angle, and per axis scale, bulge, skew and
//! trapezoid. LightBurn documents what each one is for (bulge: distortion near the centre of the field; skew:
//! distortion near the edges; trapezoid: corners tapering, keystoning) but not its formula, so the model below is
//! OpenEtch's own. Values imported from LightBurn use the same neutral points (offsets and angle 0, the rest 1) and
//! act in the same direction, but their exact effect is **unverified against LightBurn**; calibrate with
//! [`solve_lens_calibration`] instead of copying numbers.
//!
//! # Model
//!
//! With `h = field_mm / 2`, `u = x / h` and `v = y / h` (the point's position as a fraction of the half field):
//!
//! ```text
//! x1 = scale_x · x · (1 + (trapezoid_x − 1)·v + (bulge_x − 1)·(1 − v²)) + (skew_x − 1)·y
//! y1 = scale_y · y · (1 + (trapezoid_y − 1)·u + (bulge_y − 1)·(1 − u²)) + (skew_y − 1)·x
//! ```
//!
//! then a rotation by `angle_deg` (counter-clockwise, about the field centre), then `+ (offset_x_mm, offset_y_mm)`.
//! So `scale` stretches an axis, `trapezoid` makes the X width grow linearly towards +Y (and the Y height towards
//! +X), `bulge` widens the middle of the field relative to its top and bottom edges (a barrel or pincushion shape),
//! and `skew` shears one axis by the other.

use serde::{Deserialize, Serialize};

/// Field corrections. See the module docs for the model; [`Calibration::default`] is neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Calibration {
    /// Field offset X (LightBurn `FieldOffsetX`), mm.
    pub offset_x_mm: f64,
    /// Field offset Y (LightBurn `FieldOffsetY`), mm.
    pub offset_y_mm: f64,
    /// Field angle (LightBurn `FieldAngle`), degrees counter-clockwise.
    pub angle_deg: f64,
    /// X scale, 1 = neutral.
    pub scale_x: f64,
    /// Y scale, 1 = neutral.
    pub scale_y: f64,
    /// X bulge, 1 = neutral.
    pub bulge_x: f64,
    /// Y bulge, 1 = neutral.
    pub bulge_y: f64,
    /// X skew (shear of X by Y), 1 = neutral.
    pub skew_x: f64,
    /// Y skew (shear of Y by X), 1 = neutral.
    pub skew_y: f64,
    /// X trapezoid, 1 = neutral.
    pub trapezoid_x: f64,
    /// Y trapezoid, 1 = neutral.
    pub trapezoid_y: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration {
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            angle_deg: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            bulge_x: 1.0,
            bulge_y: 1.0,
            skew_x: 1.0,
            skew_y: 1.0,
            trapezoid_x: 1.0,
            trapezoid_y: 1.0,
        }
    }
}

/// Largest accepted |offset| as a fraction of the half field.
const MAX_OFFSET_FRACTION: f64 = 0.25;

impl Calibration {
    /// True when every value is neutral (the transform is the identity).
    pub fn is_neutral(&self) -> bool {
        *self == Calibration::default()
    }

    /// Every value with its name, in declaration order.
    pub fn fields(&self) -> [(&'static str, f64); 11] {
        [
            ("offset_x_mm", self.offset_x_mm),
            ("offset_y_mm", self.offset_y_mm),
            ("angle_deg", self.angle_deg),
            ("scale_x", self.scale_x),
            ("scale_y", self.scale_y),
            ("bulge_x", self.bulge_x),
            ("bulge_y", self.bulge_y),
            ("skew_x", self.skew_x),
            ("skew_y", self.skew_y),
            ("trapezoid_x", self.trapezoid_x),
            ("trapezoid_y", self.trapezoid_y),
        ]
    }

    /// Why these values cannot describe a lens correction for a field of `field_mm`, if they cannot: a value that is
    /// not finite, an offset beyond a quarter of the half field, an angle outside ±45°, a scale outside 0.5..=2, or a
    /// bulge, skew or trapezoid outside 0.8..=1.2 (a correction, not a redesign).
    pub fn problem(&self, field_mm: f64) -> Option<String> {
        if let Some((name, v)) = self.fields().into_iter().find(|(_, v)| !v.is_finite()) {
            return Some(format!("{name} = {v} is not a number"));
        }
        let max_offset = field_mm / 2.0 * MAX_OFFSET_FRACTION;
        for (name, v) in [("offset_x_mm", self.offset_x_mm), ("offset_y_mm", self.offset_y_mm)] {
            if v.abs() > max_offset {
                return Some(format!("{name} = {v} mm is beyond ±{max_offset} mm"));
            }
        }
        if self.angle_deg.abs() > 45.0 {
            return Some(format!("angle_deg = {} is outside ±45°", self.angle_deg));
        }
        for (name, v) in [("scale_x", self.scale_x), ("scale_y", self.scale_y)] {
            if !(0.5..=2.0).contains(&v) {
                return Some(format!("{name} = {v} is outside 0.5..=2"));
            }
        }
        for (name, v) in [
            ("bulge_x", self.bulge_x),
            ("bulge_y", self.bulge_y),
            ("skew_x", self.skew_x),
            ("skew_y", self.skew_y),
            ("trapezoid_x", self.trapezoid_x),
            ("trapezoid_y", self.trapezoid_y),
        ] {
            if !(0.8..=1.2).contains(&v) {
                return Some(format!("{name} = {v} is outside 0.8..=1.2"));
            }
        }
        None
    }

    /// Apply the correction to a field-frame point (mm) of a field `field_mm` wide (see the module docs).
    pub fn apply(&self, x: f64, y: f64, field_mm: f64) -> (f64, f64) {
        if self.is_neutral() {
            return (x, y);
        }
        let h = field_mm / 2.0;
        let (u, v) = (x / h, y / h);
        let x1 = self.scale_x * x * (1.0 + (self.trapezoid_x - 1.0) * v + (self.bulge_x - 1.0) * (1.0 - v * v))
            + (self.skew_x - 1.0) * y;
        let y1 = self.scale_y * y * (1.0 + (self.trapezoid_y - 1.0) * u + (self.bulge_y - 1.0) * (1.0 - u * u))
            + (self.skew_y - 1.0) * x;
        let (s, c) = self.angle_deg.to_radians().sin_cos();
        (x1 * c - y1 * s + self.offset_x_mm, x1 * s + y1 * c + self.offset_y_mm)
    }

    /// The inverse of [`Calibration::apply`] (Newton's method; within 1e-9 mm for corrections that pass
    /// [`Calibration::problem`]).
    pub fn invert(&self, x: f64, y: f64, field_mm: f64) -> (f64, f64) {
        if self.is_neutral() {
            return (x, y);
        }
        // Start from the inverse of the linear part (rotation, offset, scale).
        let (s, c) = self.angle_deg.to_radians().sin_cos();
        let (dx, dy) = (x - self.offset_x_mm, y - self.offset_y_mm);
        let mut p = ((dx * c + dy * s) / self.scale_x, (-dx * s + dy * c) / self.scale_y);
        let e = 1e-6 * field_mm.max(1.0);
        for _ in 0..30 {
            let f = self.apply(p.0, p.1, field_mm);
            let (rx, ry) = (x - f.0, y - f.1);
            if rx.abs() < 1e-11 && ry.abs() < 1e-11 {
                break;
            }
            let fx = self.apply(p.0 + e, p.1, field_mm);
            let fy = self.apply(p.0, p.1 + e, field_mm);
            let (a, b) = ((fx.0 - f.0) / e, (fy.0 - f.0) / e);
            let (cc, d) = ((fx.1 - f.1) / e, (fy.1 - f.1) / e);
            let det = a * d - b * cc;
            if det.abs() < 1e-12 || !det.is_finite() {
                break;
            }
            p = (p.0 + (d * rx - b * ry) / det, p.1 + (-cc * rx + a * ry) / det);
        }
        p
    }
}

/// What a person measured on a marked test square (see [`lens_test_pattern`]), all in mm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LensMeasurements {
    /// Width of the square's top edge.
    pub top_mm: f64,
    /// Width of its bottom edge.
    pub bottom_mm: f64,
    /// Width of the horizontal line through its centre.
    pub middle_x_mm: f64,
    /// Height of its left edge.
    pub left_mm: f64,
    /// Height of its right edge.
    pub right_mm: f64,
    /// Height of the vertical line through its centre.
    pub middle_y_mm: f64,
    /// Diagonal from the bottom-left to the top-right corner.
    pub diagonal_up_mm: f64,
    /// Diagonal from the top-left to the bottom-right corner.
    pub diagonal_down_mm: f64,
}

/// The lens test pattern for a square of side `size_mm` centred in the field: the square, the two centre lines and
/// the two diagonals, as polylines in field mm (what [`LensMeasurements`] measures).
pub fn lens_test_pattern(size_mm: f64) -> Vec<Vec<(f64, f64)>> {
    let a = size_mm / 2.0;
    vec![
        vec![(-a, -a), (a, -a), (a, a), (-a, a), (-a, -a)],
        vec![(-a, 0.0), (a, 0.0)],
        vec![(0.0, -a), (0.0, a)],
        vec![(-a, -a), (a, a)],
        vec![(-a, a), (a, -a)],
    ]
}

/// Why a lens calibration cannot be solved.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CalibrationError {
    #[error("{0}")]
    Invalid(String),
}

/// New corrections from measurements of the test square (side `size_mm`) marked with `current`, for a field
/// `field_mm` wide: the scale, bulge and trapezoid of each axis are solved so the three measured widths (top, middle,
/// bottom) and heights (left, middle, right) come out at `size_mm`, and the X skew from the two diagonals. Offset and
/// angle are kept. Assumes the lens error is small and of the model's shape (see the module docs), so a second
/// test mark after applying the result should be close to exact; repeat to refine.
pub fn solve_lens_calibration(
    current: &Calibration,
    field_mm: f64,
    size_mm: f64,
    m: &LensMeasurements,
) -> Result<Calibration, CalibrationError> {
    let bad = |s: String| Err(CalibrationError::Invalid(s));
    if !(field_mm.is_finite() && field_mm > 0.0) {
        return bad(format!("field {field_mm} mm is not usable"));
    }
    if !(size_mm.is_finite() && size_mm > 0.0 && size_mm < field_mm) {
        return bad(format!("the test square of {size_mm} mm must be positive and smaller than the {field_mm} mm field"));
    }
    let values = [
        ("top_mm", m.top_mm),
        ("bottom_mm", m.bottom_mm),
        ("middle_x_mm", m.middle_x_mm),
        ("left_mm", m.left_mm),
        ("right_mm", m.right_mm),
        ("middle_y_mm", m.middle_y_mm),
        ("diagonal_up_mm", m.diagonal_up_mm),
        ("diagonal_down_mm", m.diagonal_down_mm),
    ];
    for (name, v) in values {
        if !(v.is_finite() && v > size_mm * 0.5 && v < size_mm * 2.0 * std::f64::consts::SQRT_2) {
            return bad(format!("{name} = {v} mm is not a plausible measurement of a {size_mm} mm square"));
        }
    }
    let va = size_mm / field_mm; // a / h with a = size / 2, h = field / 2
    // Width multipliers the current correction applies at the top, bottom and middle of the square (X), and at the
    // right, left and middle (Y).
    let mult = |scale: f64, trap: f64, bulge: f64, at: f64| scale * (1.0 + (trap - 1.0) * at + (bulge - 1.0) * (1.0 - at * at));
    let solve = |scale: f64, trap: f64, bulge: f64, plus: f64, minus: f64, middle: f64| {
        // The lens multiplies each width by measured / commanded; the new multipliers cancel that.
        let target = |current_mult: f64, measured: f64| current_mult * size_mm / measured;
        let mt = target(mult(scale, trap, bulge, va), plus);
        let mb = target(mult(scale, trap, bulge, -va), minus);
        let mm = target(mult(scale, trap, bulge, 0.0), middle);
        let avg = (mt + mb) / 2.0;
        let half_diff = (mt - mb) / 2.0;
        let s_b = (mm - avg) / (va * va);
        let s = mm - s_b;
        (s, 1.0 + half_diff / (s * va), 1.0 + s_b / s)
    };
    let (scale_x, trapezoid_x, bulge_x) =
        solve(current.scale_x, current.trapezoid_x, current.bulge_x, m.top_mm, m.bottom_mm, m.middle_x_mm);
    let (scale_y, trapezoid_y, bulge_y) =
        solve(current.scale_y, current.trapezoid_y, current.bulge_y, m.right_mm, m.left_mm, m.middle_y_mm);
    // A shear k (x += k·y) of a square of side S makes the diagonals differ: up² − down² = 4·k·S².
    let k = (m.diagonal_up_mm.powi(2) - m.diagonal_down_mm.powi(2)) / (4.0 * size_mm * size_mm);
    let skew_x = current.skew_x - k;
    let out = Calibration { scale_x, scale_y, bulge_x, bulge_y, skew_x, trapezoid_x, trapezoid_y, ..*current };
    match out.problem(field_mm) {
        Some(p) => bad(format!("the measurements need a correction outside the accepted range ({p}); check them")),
        None => Ok(out),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const F: f64 = 150.0;

    fn sample() -> Calibration {
        Calibration {
            offset_x_mm: 0.4,
            offset_y_mm: -0.3,
            angle_deg: 0.8,
            scale_x: 1.01,
            scale_y: 0.99,
            bulge_x: 1.004,
            bulge_y: 0.997,
            skew_x: 1.002,
            skew_y: 0.999,
            trapezoid_x: 1.003,
            trapezoid_y: 0.996,
        }
    }

    #[test]
    fn neutral_is_the_identity() {
        let c = Calibration::default();
        assert!(c.is_neutral() && c.problem(F).is_none());
        assert_eq!(c.apply(12.5, -40.0, F), (12.5, -40.0));
        assert_eq!(c.invert(12.5, -40.0, F), (12.5, -40.0));
    }

    #[test]
    fn each_term_does_what_it_says() {
        let only = |edit: fn(&mut Calibration)| {
            let mut c = Calibration::default();
            edit(&mut c);
            c
        };
        let p = only(|c| c.offset_x_mm = 1.0).apply(10.0, 10.0, F);
        assert!((p.0 - 11.0).abs() < 1e-12 && (p.1 - 10.0).abs() < 1e-12);
        let p = only(|c| c.angle_deg = 90.0).apply(10.0, 0.0, F);
        assert!(p.0.abs() < 1e-9 && (p.1 - 10.0).abs() < 1e-9);
        assert_eq!(only(|c| c.scale_x = 2.0).apply(10.0, 5.0, F), (20.0, 5.0));
        // Trapezoid: X grows towards +Y.
        let t = only(|c| c.trapezoid_x = 1.1);
        assert!(t.apply(10.0, 75.0, F).0 > 10.5 && t.apply(10.0, -75.0, F).0 < 9.5);
        // Bulge: the middle widens, the edges stay.
        let b = only(|c| c.bulge_x = 1.1);
        assert!((b.apply(10.0, 0.0, F).0 - 11.0).abs() < 1e-12 && (b.apply(10.0, 75.0, F).0 - 10.0).abs() < 1e-12);
        // Skew: X shifts with Y.
        assert!((only(|c| c.skew_x = 1.01).apply(0.0, 50.0, F).0 - 0.5).abs() < 1e-12);
    }

    #[test]
    fn invert_undoes_apply() {
        let c = sample();
        for &(x, y) in &[(0.0, 0.0), (70.0, -70.0), (-33.3, 12.5), (74.0, 74.0)] {
            let (a, b) = c.apply(x, y, F);
            let (bx, by) = c.invert(a, b, F);
            assert!((bx - x).abs() < 1e-9 && (by - y).abs() < 1e-9, "({x}, {y}) -> ({bx}, {by})");
        }
    }

    #[test]
    fn problems_are_named() {
        assert!(sample().problem(F).is_none());
        let c = Calibration { scale_x: f64::NAN, ..Calibration::default() };
        assert!(c.problem(F).unwrap().contains("scale_x"));
        let c = Calibration { offset_y_mm: 30.0, ..Calibration::default() };
        assert!(c.problem(F).unwrap().contains("offset_y_mm"));
        let c = Calibration { bulge_y: 1.5, ..Calibration::default() };
        assert!(c.problem(F).unwrap().contains("bulge_y"));
        let c = Calibration { angle_deg: 90.0, ..Calibration::default() };
        assert!(c.problem(F).is_some());
    }

    /// A lens with a known error of the model's shape: marking with the solved correction cancels it.
    #[test]
    fn the_wizard_solves_a_simulated_lens() {
        let lens = Calibration {
            scale_x: 1.02,
            scale_y: 0.985,
            bulge_x: 1.006,
            bulge_y: 0.995,
            trapezoid_x: 0.993,
            trapezoid_y: 1.004,
            skew_x: 1.003,
            ..Calibration::default()
        };
        let size = 100.0;
        // What the operator measures: the marked square is lens(correction(design)).
        let measure = |corr: &Calibration| {
            let mark = |x: f64, y: f64| {
                let (a, b) = corr.apply(x, y, F);
                lens.apply(a, b, F)
            };
            let d = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).hypot(p.1 - q.1);
            let a = size / 2.0;
            LensMeasurements {
                top_mm: mark(a, a).0 - mark(-a, a).0,
                bottom_mm: mark(a, -a).0 - mark(-a, -a).0,
                middle_x_mm: mark(a, 0.0).0 - mark(-a, 0.0).0,
                left_mm: mark(-a, a).1 - mark(-a, -a).1,
                right_mm: mark(a, a).1 - mark(a, -a).1,
                middle_y_mm: mark(0.0, a).1 - mark(0.0, -a).1,
                diagonal_up_mm: d(mark(-a, -a), mark(a, a)),
                diagonal_down_mm: d(mark(-a, a), mark(a, -a)),
            }
        };
        let mut corr = Calibration::default();
        let first = measure(&corr);
        assert!((first.middle_x_mm - size).abs() > 1.0, "the lens is visibly off: {first:?}");
        for _ in 0..3 {
            corr = solve_lens_calibration(&corr, F, size, &measure(&corr)).unwrap();
        }
        let m = measure(&corr);
        for v in [m.top_mm, m.bottom_mm, m.middle_x_mm, m.left_mm, m.right_mm, m.middle_y_mm] {
            assert!((v - size).abs() < 0.05, "{m:?}");
        }
        assert!((m.diagonal_up_mm - m.diagonal_down_mm).abs() < 0.1, "{m:?}");
    }

    #[test]
    fn implausible_measurements_are_refused() {
        let m = LensMeasurements {
            top_mm: 100.0,
            bottom_mm: 100.0,
            middle_x_mm: 100.0,
            left_mm: 100.0,
            right_mm: 100.0,
            middle_y_mm: 100.0,
            diagonal_up_mm: 141.4,
            diagonal_down_mm: 141.4,
        };
        let c = Calibration::default();
        let ok = solve_lens_calibration(&c, F, 100.0, &m).unwrap();
        assert!((ok.scale_x - 1.0).abs() < 1e-12 && (ok.skew_x - 1.0).abs() < 1e-3);
        assert!(solve_lens_calibration(&c, F, 100.0, &LensMeasurements { top_mm: 10.0, ..m }).is_err());
        assert!(solve_lens_calibration(&c, F, 200.0, &m).is_err());
        assert!(solve_lens_calibration(&c, F, 100.0, &LensMeasurements { left_mm: f64::NAN, ..m }).is_err());
    }

    #[test]
    fn the_pattern_is_a_square_with_centre_lines_and_diagonals() {
        let p = lens_test_pattern(50.0);
        assert_eq!(p.len(), 5);
        assert_eq!(p[0][0], (-25.0, -25.0));
        assert_eq!(p[0].last(), Some(&(-25.0, -25.0)));
    }
}
