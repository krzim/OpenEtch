//! The machine facts a job is planned and encoded for, and the field mm <-> galvo word transform.
//!
//! `plan` does not depend on `machine`: the engine fills a [`Target`] from the machine profile. The transform itself
//! is `lmc::units::field_to_galvo`, the same function `machine` frames with.

use doc::GalvoTiming;
use lmc::FieldTransform;
use lmc::units::{field_to_galvo, galvo_to_field};
use serde::{Deserialize, Serialize};

use crate::{PlanError, Result};

/// Machine facts for planning and encoding. Filled by the engine from the machine profile; command arguments can
/// never change these (they are the hard limits of ARCHITECTURE.md's safety model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Width of the galvo field (and its height when `field_height_mm` is `None`). Points must lie strictly inside
    /// `±field_mm / 2` in X (and `±height / 2` in Y).
    pub field_mm: f64,
    /// Height of a non-square field, mm (the profile's `field_height_mm`); `None` for a square field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_height_mm: Option<f64>,
    /// The device's output curve tolerance in mm (the profile's `curve_tolerance_mm`): how far flattened polylines
    /// may stray from curves. A document's own `curve_tolerance_mm` wins; `None` means
    /// [`doc::DEFAULT_CURVE_TOLERANCE_MM`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve_tolerance_mm: Option<f64>,
    /// Exchange the X and Y galvo axes. Applied after negation, see [`Target::to_galvo`].
    pub swap_xy: bool,
    /// Mirror field X (in the field frame, before any swap).
    pub negate_x: bool,
    /// Mirror field Y (in the field frame, before any swap).
    pub negate_y: bool,
    /// Hard power limit in percent. A layer above it is refused, never clamped.
    pub max_power_pct: f64,
    /// Pulse widths (ns) the MOPA source accepts, e.g. [`lmc::JPT_M7_PULSE_WIDTHS_NS`].
    pub pulse_widths_ns: Vec<u16>,
    /// Timing used by layers whose `settings.timing` is `None`. Its `jump_speed_mm_s` is the default jump speed.
    pub timing: GalvoTiming,
    /// Wait after opening the master oscillator before the first motion (the profile's `open_mo_delay_us`;
    /// galvoplotter's `delay_open_mo` is 8 ms).
    pub open_mo_delay_us: f64,
    /// Lens corrections applied with the field transform (the same as the frame's). Neutral by default.
    #[serde(default, skip_serializing_if = "lmc::Calibration::is_neutral")]
    pub calibration: lmc::Calibration,
    /// The source takes a pulse width (Q-pulse enabled, a MOPA): when false, passes send none (with a warning).
    #[serde(default = "yes")]
    pub pulse_width_enabled: bool,
    /// The board drives a Z axis: Z offsets and steps per pass make parts. Without one they are ignored (warning).
    #[serde(default)]
    pub z_axis: bool,
    /// The board drives a rotary: rotary jobs are split into strips. Without one they are refused (except previews).
    #[serde(default)]
    pub rotary_axis: bool,
    /// Output port bits held high while the job's lists run (the profile's busy / ready lights): every in-list
    /// `WritePort` carries them. Never the laser-enable bit. 0 by default.
    #[serde(default)]
    pub port_outputs: u16,
}

fn yes() -> bool {
    true
}

impl Target {
    /// Half the field side: the largest |coordinate| (exclusive) that can be encoded.
    pub fn half_field_mm(&self) -> f64 {
        self.field_mm / 2.0
    }

    /// The field height, mm: `field_height_mm`, else the square `field_mm`.
    pub fn field_height(&self) -> f64 {
        self.field_height_mm.unwrap_or(self.field_mm)
    }

    /// The curve tolerance planning flattens `doc` with: the document's own setting, else this device's, else
    /// [`doc::DEFAULT_CURVE_TOLERANCE_MM`] (each kept within [`doc::CURVE_TOLERANCE_RANGE_MM`]).
    pub fn curve_tolerance(&self, doc: &doc::Document) -> f64 {
        let r = doc::CURVE_TOLERANCE_RANGE_MM;
        doc.curve_tolerance_mm
            .or(self.curve_tolerance_mm)
            .filter(|t| t.is_finite())
            .map_or(doc::DEFAULT_CURVE_TOLERANCE_MM, |t| t.clamp(*r.start(), *r.end()))
    }

    /// True when a field-frame point can be encoded: x strictly inside `±field_mm / 2` and y strictly inside
    /// `±height / 2`.
    ///
    /// Swap and negate keep the field symmetric, so this is checked in field mm. The bound is strict because
    /// `+field/2` is not encodable and either sign can become `+` after a negate. Same bound as
    /// `lmc::units::field_to_galvo`.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let (hx, hy) = (self.half_field_mm(), self.field_height() / 2.0);
        p[0].is_finite() && p[1].is_finite() && p[0].abs() < hx && p[1].abs() < hy
    }

    /// The field size, swap, negate and lens correction as `lmc`'s shared transform.
    pub fn transform(&self) -> FieldTransform {
        FieldTransform {
            field_mm: self.field_mm,
            field_height_mm: self.field_height_mm,
            swap_xy: self.swap_xy,
            negate_x: self.negate_x,
            negate_y: self.negate_y,
            calibration: self.calibration,
        }
    }

    /// Field-frame mm -> galvo words with `lmc::units::field_to_galvo`: negate in the field frame, then swap, then
    /// `mm_to_galvo`. Refused (never clamped) outside the field.
    pub fn to_galvo(&self, p: [f64; 2]) -> Result<(u16, u16)> {
        Ok(field_to_galvo(p[0], p[1], &self.transform())?)
    }

    /// Galvo words -> field-frame mm: the inverse of [`Target::to_galvo`], up to half a galvo unit.
    pub fn from_galvo(&self, w: (u16, u16)) -> [f64; 2] {
        let (x, y) = galvo_to_field(w.0, w.1, &self.transform());
        [x, y]
    }

    /// One galvo unit in mm.
    pub fn galvo_unit_mm(&self) -> f64 {
        self.field_mm / 65535.0
    }

    /// Refuse a target whose numbers cannot describe a machine.
    pub(crate) fn validate(&self) -> Result<()> {
        if !(self.field_mm.is_finite() && self.field_mm > 0.0) {
            return Err(PlanError::Target(format!("field_mm must be positive (got {})", self.field_mm)));
        }
        if let Some(h) = self.field_height_mm
            && !(h.is_finite() && h > 0.0)
        {
            return Err(PlanError::Target(format!("field_height_mm must be positive (got {h})")));
        }
        if let Some(t) = self.curve_tolerance_mm
            && !(t.is_finite() && doc::CURVE_TOLERANCE_RANGE_MM.contains(&t))
        {
            return Err(PlanError::Target(format!("curve_tolerance_mm must be within 0.001..=1 (got {t})")));
        }
        if !(self.max_power_pct.is_finite() && (0.0..=100.0).contains(&self.max_power_pct)) {
            return Err(PlanError::Target(format!("max_power_pct must be within 0..=100 (got {})", self.max_power_pct)));
        }
        if !(self.open_mo_delay_us.is_finite() && self.open_mo_delay_us >= 0.0) {
            return Err(PlanError::Target(format!("open_mo_delay_us must be >= 0 (got {})", self.open_mo_delay_us)));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lmc::units::mm_to_galvo;

    /// A 110 mm B6-like target with a JPT M7 table and LightBurn-like default timing.
    pub(crate) fn target() -> Target {
        Target {
            field_mm: 110.0,
            field_height_mm: None,
            curve_tolerance_mm: None,
            swap_xy: false,
            negate_x: false,
            negate_y: false,
            max_power_pct: 80.0,
            pulse_widths_ns: lmc::JPT_M7_PULSE_WIDTHS_NS.to_vec(),
            timing: GalvoTiming {
                laser_on_delay_us: 100.0,
                laser_off_delay_us: 100.0,
                polygon_delay_us: 10.0,
                end_delay_us: 0.0,
                jump_speed_mm_s: 4000.0,
                min_jump_delay_us: 100.0,
                max_jump_delay_us: 400.0,
                jump_distance_limit_mm: 10.0,
            },
            open_mo_delay_us: 8000.0,
            calibration: lmc::Calibration::default(),
            pulse_width_enabled: true,
            z_axis: false,
            rotary_axis: false,
            port_outputs: 0,
        }
    }

    #[test]
    fn identity_transform_round_trips() {
        let t = target();
        assert_eq!(t.to_galvo([0.0, 0.0]).unwrap(), (0x8000, 0x8000));
        let back = t.from_galvo(t.to_galvo([12.5, -7.25]).unwrap());
        assert!((back[0] - 12.5).abs() <= t.galvo_unit_mm() && (back[1] + 7.25).abs() <= t.galvo_unit_mm());
    }

    #[test]
    fn negate_in_field_frame_then_swap() {
        let mut t = target();
        t.swap_xy = true;
        t.negate_x = true;
        let (gx, gy) = t.to_galvo([10.0, 20.0]).unwrap();
        // Negate field X gives (-10, 20); swap gives galvo axes (20, -10).
        assert_eq!(gx, mm_to_galvo(20.0, 110.0).unwrap());
        assert_eq!(gy, mm_to_galvo(-10.0, 110.0).unwrap());
        let back = t.from_galvo((gx, gy));
        assert!((back[0] - 10.0).abs() < 0.01 && (back[1] - 20.0).abs() < 0.01);
    }

    /// All 8 swap/negate combinations: `Target` gives exactly `lmc::units::field_to_galvo`'s words (the function
    /// `machine` frames with) and the contract's order (negate in the field frame, then swap), and refuses the
    /// same points.
    #[test]
    fn all_transforms_match_lmc_and_the_contract() {
        let w = |v: f64| mm_to_galvo(v, 110.0).unwrap();
        for bits in 0..8u8 {
            let t = Target { swap_xy: bits & 1 != 0, negate_x: bits & 2 != 0, negate_y: bits & 4 != 0, ..target() };
            let lmc = lmc::units::field_to_galvo(10.0, 5.0, &t.transform()).unwrap();
            assert_eq!(t.to_galvo([10.0, 5.0]).unwrap(), lmc, "{bits:03b}");
            let (x, y) = (if t.negate_x { -10.0 } else { 10.0 }, if t.negate_y { -5.0 } else { 5.0 });
            assert_eq!(lmc, if t.swap_xy { (w(y), w(x)) } else { (w(x), w(y)) }, "{bits:03b}");
            let back = t.from_galvo(lmc);
            assert!((back[0] - 10.0).abs() < t.galvo_unit_mm() && (back[1] - 5.0).abs() < t.galvo_unit_mm(), "{bits:03b}");
            for p in [[55.0, 0.0], [0.0, -55.0], [54.9, -54.9]] {
                assert_eq!(t.to_galvo(p).is_ok(), t.contains(p), "{bits:03b} {p:?}");
            }
        }
    }

    #[test]
    fn contains_is_strict_and_symmetric() {
        let t = target();
        assert!(t.contains([54.9, -54.9]));
        assert!(!t.contains([55.0, 0.0]));
        assert!(!t.contains([0.0, -55.0]));
        assert!(!t.contains([f64::NAN, 0.0]));
    }

    #[test]
    fn non_square_field_and_curve_tolerance() {
        let t = Target { field_height_mm: Some(60.0), ..target() };
        assert!(t.contains([54.9, 29.9]));
        assert!(!t.contains([0.0, 30.0]));
        assert_eq!(t.to_galvo([0.0, 15.0]).unwrap().1, mm_to_galvo(15.0, 60.0).unwrap());
        assert!(t.to_galvo([0.0, 31.0]).is_err());
        let mut d = doc::Document::default();
        assert_eq!(target().curve_tolerance(&d), doc::DEFAULT_CURVE_TOLERANCE_MM);
        let dev = Target { curve_tolerance_mm: Some(0.05), ..target() };
        assert_eq!(dev.curve_tolerance(&d), 0.05);
        d.curve_tolerance_mm = Some(0.002);
        assert_eq!(dev.curve_tolerance(&d), 0.002, "the document wins");
        assert!(Target { field_height_mm: Some(0.0), ..target() }.validate().is_err());
        assert!(Target { curve_tolerance_mm: Some(2.0), ..target() }.validate().is_err());
    }

    #[test]
    fn bad_targets_are_refused() {
        let mut t = target();
        t.field_mm = 0.0;
        assert!(t.validate().is_err());
        let mut t = target();
        t.max_power_pct = 120.0;
        assert!(t.validate().is_err());
    }
}
