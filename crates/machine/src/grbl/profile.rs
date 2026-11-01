//! [`GrblProfile`]: one GRBL laser's connection, power range, speed limit and bed.
//!
//! Like [`crate::MachineProfile`] for the galvo, this is where a gantry's hard limits live: the power limit, the
//! largest feed and the work area. Programs are checked against it before anything is sent
//! ([`super::check_program`]) and command arguments never raise it.

use serde::{Deserialize, Serialize};

use crate::device::{MAX_BED_MM, Origin};
use crate::profile::ProfileError;

/// Largest `$30` accepted.
pub const MAX_S_VALUE: f64 = 100_000.0;
/// Largest feed accepted, mm/min.
pub const MAX_FEED_MM_MIN: f64 = 100_000.0;

fn default_baud() -> u32 {
    115_200
}

fn default_s_max() -> f64 {
    1000.0
}

fn default_max_feed() -> f64 {
    6000.0
}

fn default_frame_feed() -> f64 {
    3000.0
}

fn default_power() -> f64 {
    100.0
}

fn yes() -> bool {
    true
}

/// A GRBL 1.1 laser over a serial port. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrblProfile {
    /// Serial port: `COM5` on Windows, `/dev/ttyUSB0` or `/dev/ttyACM0` on Linux. Empty until set.
    #[serde(default)]
    pub port: String,
    /// Baud rate (GRBL 1.1's default is 115200).
    #[serde(default = "default_baud")]
    pub baud: u32,
    /// The board's `$30` (maximum spindle/laser S value): 100 % power is this S. Runs are refused when the board
    /// reports another `$30`, so a job never fires at a power it was not planned for.
    #[serde(default = "default_s_max")]
    pub s_max: f64,
    /// The board's `$31` (minimum S value). Checked like `s_max`.
    #[serde(default)]
    pub s_min: f64,
    /// Hard power limit in percent of `s_max`: layers above it are refused, never clamped.
    #[serde(default = "default_power")]
    pub max_power_pct: f64,
    /// Fastest feed a program may ask for, mm/min (layers above it are refused).
    #[serde(default = "default_max_feed")]
    pub max_feed_mm_min: f64,
    /// Feed of framing and jogging, mm/min (at most `max_feed_mm_min`).
    #[serde(default = "default_frame_feed")]
    pub frame_feed_mm_min: f64,
    /// The bed, `[width, height]` mm.
    pub work_area_mm: [f64; 2],
    /// Homing switches are fitted (`$22=1`): Home is offered, and the board starts locked until homed or unlocked.
    #[serde(default)]
    pub homing: bool,
    /// The corner at machine (0, 0): coordinates are mm from it, positive into the bed (LightBurn's Origin).
    pub origin: Origin,
    /// Dynamic power, `M4` (the default: power follows speed, so corners do not burn deeper); `false` uses
    /// constant power, `M3`.
    #[serde(default = "yes")]
    pub dynamic_power: bool,
}

impl Default for GrblProfile {
    fn default() -> Self {
        GrblProfile {
            port: String::new(),
            baud: default_baud(),
            s_max: default_s_max(),
            s_min: 0.0,
            max_power_pct: default_power(),
            max_feed_mm_min: default_max_feed(),
            frame_feed_mm_min: default_frame_feed(),
            work_area_mm: [400.0, 400.0],
            homing: false,
            origin: Origin::BottomLeft,
            dynamic_power: true,
        }
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ProfileError {
    ProfileError::Invalid { field, reason: reason.into() }
}

impl GrblProfile {
    /// Refuse values no GRBL laser can have.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.port.len() > 128 || self.port.chars().any(char::is_control) {
            return Err(invalid("port", "at most 128 printable characters"));
        }
        if !(300..=4_000_000).contains(&self.baud) {
            return Err(invalid("baud", format!("{} is not a usable baud rate", self.baud)));
        }
        if !(self.s_max.is_finite() && self.s_max > 0.0 && self.s_max <= MAX_S_VALUE) {
            return Err(invalid("s_max", format!("must be within 0..={MAX_S_VALUE} (got {})", self.s_max)));
        }
        if !(self.s_min.is_finite() && self.s_min >= 0.0 && self.s_min < self.s_max) {
            return Err(invalid("s_min", format!("must be at least 0 and below s_max (got {})", self.s_min)));
        }
        if !(self.max_power_pct.is_finite() && self.max_power_pct > 0.0 && self.max_power_pct <= 100.0) {
            return Err(invalid("max_power_pct", format!("must be within 0..=100 (got {})", self.max_power_pct)));
        }
        if !(self.max_feed_mm_min.is_finite() && self.max_feed_mm_min > 0.0 && self.max_feed_mm_min <= MAX_FEED_MM_MIN) {
            return Err(invalid("max_feed_mm_min", format!("must be within 0..={MAX_FEED_MM_MIN} (got {})", self.max_feed_mm_min)));
        }
        if !(self.frame_feed_mm_min.is_finite() && self.frame_feed_mm_min > 0.0 && self.frame_feed_mm_min <= self.max_feed_mm_min) {
            return Err(invalid("frame_feed_mm_min", "must be positive and at most max_feed_mm_min"));
        }
        if !self.work_area_mm.iter().all(|v| v.is_finite() && *v > 0.0 && *v <= MAX_BED_MM) {
            return Err(invalid("work_area_mm", format!("each side must be within 0..={MAX_BED_MM} mm")));
        }
        if self.origin == Origin::Center {
            return Err(invalid("origin", "a gantry's origin is a bed corner"));
        }
        Ok(())
    }

    /// `(mirror_x, mirror_y)`: which axes point from the right or rear edge (LightBurn's `MirrorX` / `MirrorY`).
    pub fn mirror(&self) -> (bool, bool) {
        match self.origin {
            Origin::BottomLeft | Origin::Center => (false, false),
            Origin::TopLeft => (false, true),
            Origin::BottomRight => (true, false),
            Origin::TopRight => (true, true),
        }
    }

    /// Field frame (origin at the bed centre, +Y away from the operator) to machine coordinates (mm from the origin
    /// corner, positive into the bed): `S · field + [W/2, H/2]` with `S` negating the mirrored axes, the rule
    /// `lbrn` uses for gantry projects. Refused (never clamped) outside the bed.
    pub fn to_machine(&self, x: f64, y: f64) -> Result<(f64, f64), String> {
        let [w, h] = self.work_area_mm;
        if !(x.is_finite() && y.is_finite() && x.abs() <= w / 2.0 + 1e-9 && y.abs() <= h / 2.0 + 1e-9) {
            return Err(format!("({x:.3}, {y:.3}) mm is outside the {w} x {h} mm bed"));
        }
        let (mx, my) = self.mirror();
        let sx = if mx { -1.0 } else { 1.0 };
        let sy = if my { -1.0 } else { 1.0 };
        Ok(((sx * x + w / 2.0).clamp(0.0, w), (sy * y + h / 2.0).clamp(0.0, h)))
    }

    /// Machine coordinates back to the field frame (the inverse of [`GrblProfile::to_machine`]).
    pub fn to_field(&self, mx: f64, my: f64) -> (f64, f64) {
        let [w, h] = self.work_area_mm;
        let (fx, fy) = self.mirror();
        let sx = if fx { -1.0 } else { 1.0 };
        let sy = if fy { -1.0 } else { 1.0 };
        (sx * (mx - w / 2.0), sy * (my - h / 2.0))
    }

    /// The S value of `power_pct` percent: `power_pct / 100 · s_max`.
    pub fn s_value(&self, power_pct: f64) -> f64 {
        power_pct / 100.0 * self.s_max
    }

    /// The largest S a program may use: `max_power_pct` of `s_max`.
    pub fn s_limit(&self) -> f64 {
        self.s_value(self.max_power_pct)
    }

    pub fn to_toml(&self) -> Result<String, ProfileError> {
        toml::to_string_pretty(self).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    pub fn from_toml(text: &str) -> Result<Self, ProfileError> {
        let p: GrblProfile = toml::from_str(text).map_err(|e| ProfileError::Toml(e.to_string()))?;
        p.validate()?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_round_trip() {
        let p = GrblProfile::default();
        p.validate().unwrap();
        assert_eq!(GrblProfile::from_toml(&p.to_toml().unwrap()).unwrap(), p);
        for bad in [
            GrblProfile { s_max: 0.0, ..p.clone() },
            GrblProfile { s_min: 1000.0, ..p.clone() },
            GrblProfile { max_power_pct: 101.0, ..p.clone() },
            GrblProfile { frame_feed_mm_min: 7000.0, ..p.clone() },
            GrblProfile { work_area_mm: [0.0, 10.0], ..p.clone() },
            GrblProfile { origin: Origin::Center, ..p.clone() },
            GrblProfile { port: "COM1\n".into(), ..p.clone() },
            GrblProfile { baud: 0, ..p.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn corners_map_like_lightburn_gantry_files() {
        let p = GrblProfile { work_area_mm: [400.0, 300.0], ..GrblProfile::default() };
        assert_eq!(p.to_machine(0.0, 0.0).unwrap(), (200.0, 150.0));
        assert_eq!(p.to_machine(-200.0, -150.0).unwrap(), (0.0, 0.0), "front left is (0, 0)");
        let rear_left = GrblProfile { origin: Origin::TopLeft, ..p.clone() };
        assert_eq!(rear_left.to_machine(-200.0, 150.0).unwrap(), (0.0, 0.0));
        assert_eq!(rear_left.to_machine(-190.0, 140.0).unwrap(), (10.0, 10.0));
        let rear_right = GrblProfile { origin: Origin::TopRight, ..p.clone() };
        assert_eq!(rear_right.to_machine(200.0, 150.0).unwrap(), (0.0, 0.0));
        for q in [&p, &rear_left, &rear_right] {
            let (mx, my) = q.to_machine(12.5, -40.0).unwrap();
            let (x, y) = q.to_field(mx, my);
            assert!((x - 12.5).abs() < 1e-9 && (y + 40.0).abs() < 1e-9);
        }
        assert!(p.to_machine(200.5, 0.0).is_err());
        assert!(p.to_machine(f64::NAN, 0.0).is_err());
        assert_eq!((p.s_value(50.0), p.s_limit()), (500.0, 1000.0));
    }
}
