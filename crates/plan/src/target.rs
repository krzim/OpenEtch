//! The machine facts a job is planned and encoded for, and the field mm <-> galvo word transform.
//!
//! `plan` does not depend on `machine`: the engine fills a [`Target`] from the machine profile.

use doc::GalvoTiming;
use lmc::units::{galvo_to_mm, mm_to_galvo};
use serde::{Deserialize, Serialize};

use crate::{PlanError, Result};

/// Machine facts for planning and encoding. Filled by the engine from the machine profile; command arguments can
/// never change these (they are the hard limits of ARCHITECTURE.md's safety model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// Side of the square galvo field. Points must lie strictly inside `±field_mm / 2`.
    pub field_mm: f64,
    /// Exchange the X and Y galvo axes. Applied first, see [`Target::to_galvo`].
    pub swap_xy: bool,
    /// Mirror the galvo X axis (after any swap).
    pub negate_x: bool,
    /// Mirror the galvo Y axis (after any swap).
    pub negate_y: bool,
    /// Hard power limit in percent. A layer above it is refused, never clamped.
    pub max_power_pct: f64,
    /// Pulse widths (ns) the MOPA source accepts, e.g. [`lmc::JPT_M7_PULSE_WIDTHS_NS`].
    pub pulse_widths_ns: Vec<u16>,
    /// Timing used by layers whose `settings.timing` is `None`. Its `jump_speed_mm_s` is the default jump speed.
    pub timing: GalvoTiming,
    /// Wait after opening the master oscillator before the first motion (galvoplotter's `delay_open_mo`, 8 ms).
    pub open_mo_delay_us: f64,
}

impl Target {
    /// Half the field side: the largest |coordinate| (exclusive) that can be encoded.
    pub fn half_field_mm(&self) -> f64 {
        self.field_mm / 2.0
    }

    /// True when a field-frame point can be encoded: both coordinates strictly inside `±field_mm / 2`.
    ///
    /// Swap and negate keep the field symmetric, so this is checked in field mm. The bound is strict because
    /// `+field/2` is not encodable and either sign can become `+` after a negate.
    pub fn contains(&self, p: [f64; 2]) -> bool {
        let h = self.half_field_mm();
        p.iter().all(|v| v.is_finite() && v.abs() < h)
    }

    /// Field-frame mm -> galvo words: swap X/Y if `swap_xy`, then negate the resulting axes, then
    /// [`mm_to_galvo`]. Refused (never clamped) outside the field.
    pub fn to_galvo(&self, p: [f64; 2]) -> Result<(u16, u16)> {
        let [gx, gy] = self.field_to_axes(p);
        Ok((mm_to_galvo(gx, self.field_mm)?, mm_to_galvo(gy, self.field_mm)?))
    }

    /// Galvo words -> field-frame mm: the inverse of [`Target::to_galvo`], up to half a galvo unit.
    pub fn from_galvo(&self, w: (u16, u16)) -> [f64; 2] {
        let gx = galvo_to_mm(w.0, self.field_mm);
        let gy = galvo_to_mm(w.1, self.field_mm);
        let gx = if self.negate_x { -gx } else { gx };
        let gy = if self.negate_y { -gy } else { gy };
        if self.swap_xy { [gy, gx] } else { [gx, gy] }
    }

    /// One galvo unit in mm.
    pub fn galvo_unit_mm(&self) -> f64 {
        self.field_mm / 65535.0
    }

    /// Field-frame mm -> galvo-axis mm (swap, then negate).
    fn field_to_axes(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let [gx, gy] = if self.swap_xy { [y, x] } else { [x, y] };
        [if self.negate_x { -gx } else { gx }, if self.negate_y { -gy } else { gy }]
    }

    /// Refuse a target whose numbers cannot describe a machine.
    pub(crate) fn validate(&self) -> Result<()> {
        if !(self.field_mm.is_finite() && self.field_mm > 0.0) {
            return Err(PlanError::Target(format!("field_mm must be positive (got {})", self.field_mm)));
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

    /// A 110 mm B6-like target with a JPT M7 table and LightBurn-like default timing.
    pub(crate) fn target() -> Target {
        Target {
            field_mm: 110.0,
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
    fn swap_then_negate() {
        let mut t = target();
        t.swap_xy = true;
        t.negate_x = true;
        let (gx, gy) = t.to_galvo([10.0, 20.0]).unwrap();
        // Swap gives axes (20, 10); negate X gives (-20, 10).
        assert_eq!(gx, mm_to_galvo(-20.0, 110.0).unwrap());
        assert_eq!(gy, mm_to_galvo(10.0, 110.0).unwrap());
        let back = t.from_galvo((gx, gy));
        assert!((back[0] - 10.0).abs() < 0.01 && (back[1] - 20.0).abs() < 0.01);
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
    fn bad_targets_are_refused() {
        let mut t = target();
        t.field_mm = 0.0;
        assert!(t.validate().is_err());
        let mut t = target();
        t.max_power_pct = 120.0;
        assert!(t.validate().is_err());
    }
}
