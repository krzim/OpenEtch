//! [`RuidaProfile`]: one Ruida DSP laser's network address, scrambling key, hard limits and bed.
//!
//! Like [`crate::MachineProfile`] for the galvo and [`crate::grbl::GrblProfile`] for GRBL, this is where the gantry's
//! hard limits live: the power limit, the fastest speed and the work area. Programs are checked against it before
//! anything is sent ([`super::check_program`]) and command arguments never raise it.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::device::{MAX_BED_MM, Origin};
use crate::profile::ProfileError;

/// Fastest cutting speed a profile may allow, mm/s.
pub const MAX_SPEED_MM_S: f64 = 5000.0;

fn default_magic() -> u8 {
    ruida::DEFAULT_MAGIC
}

fn default_power() -> f64 {
    100.0
}

fn default_max_speed() -> f64 {
    500.0
}

fn default_origin() -> Origin {
    Origin::TopRight
}

/// A Ruida controller (RDC644x / 634x) over Ethernet (UDP). See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuidaProfile {
    /// The controller's IP address (it listens on UDP port 50200; OpenEtch listens on 40200). Empty until set.
    #[serde(default)]
    pub address: String,
    /// The byte-scrambling key: `0x88` on most controllers, `0x11` on the RDC634XG.
    #[serde(default = "default_magic")]
    pub magic: u8,
    /// Hard power limit in percent: layers above it are refused, never clamped.
    #[serde(default = "default_power")]
    pub max_power_pct: f64,
    /// Fastest cutting speed a job may ask for, mm/s (layers above it are refused).
    #[serde(default = "default_max_speed")]
    pub max_speed_mm_s: f64,
    /// The bed, `[width, height]` mm.
    pub work_area_mm: [f64; 2],
    /// The corner at machine (0, 0): coordinates are mm from it, positive into the bed. MeerK40t's `.rd` writer
    /// implies the rear right corner (X mirrored, Y from the rear), the default here; unverified on hardware.
    #[serde(default = "default_origin")]
    pub origin: Origin,
}

impl Default for RuidaProfile {
    fn default() -> Self {
        RuidaProfile {
            address: String::new(),
            magic: default_magic(),
            max_power_pct: default_power(),
            max_speed_mm_s: default_max_speed(),
            work_area_mm: [600.0, 400.0],
            origin: default_origin(),
        }
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> ProfileError {
    ProfileError::Invalid { field, reason: reason.into() }
}

impl RuidaProfile {
    /// Refuse values no Ruida laser can have.
    pub fn validate(&self) -> Result<(), ProfileError> {
        if !self.address.is_empty() && self.address.trim().parse::<IpAddr>().is_err() {
            return Err(invalid("address", format!("{:?} is not an IP address (e.g. 192.168.1.100)", self.address)));
        }
        if !(self.max_power_pct.is_finite() && self.max_power_pct > 0.0 && self.max_power_pct <= 100.0) {
            return Err(invalid("max_power_pct", format!("must be within 0..=100 (got {})", self.max_power_pct)));
        }
        if !(self.max_speed_mm_s.is_finite() && self.max_speed_mm_s > 0.0 && self.max_speed_mm_s <= MAX_SPEED_MM_S) {
            return Err(invalid("max_speed_mm_s", format!("must be within 0..={MAX_SPEED_MM_S} (got {})", self.max_speed_mm_s)));
        }
        if !self.work_area_mm.iter().all(|v| v.is_finite() && *v > 0.0 && *v <= MAX_BED_MM) {
            return Err(invalid("work_area_mm", format!("each side must be within 0..={MAX_BED_MM} mm")));
        }
        if self.origin == Origin::Center {
            return Err(invalid("origin", "a gantry's origin is a bed corner"));
        }
        Ok(())
    }

    /// The controller's address, parsed.
    pub fn ip(&self) -> Option<IpAddr> {
        self.address.trim().parse().ok()
    }

    /// `(mirror_x, mirror_y)`: which axes point from the right or rear edge (LightBurn's `MirrorX` / `MirrorY`).
    pub fn mirror(&self) -> (bool, bool) {
        mirror_of(self.origin)
    }

    /// Field frame (origin at the bed centre, +Y away from the operator) to machine coordinates (mm from the origin
    /// corner, positive into the bed), as [`crate::grbl::GrblProfile::to_machine`]. Refused (never clamped) outside the
    /// bed.
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

    /// Machine coordinates back to the field frame (the inverse of [`RuidaProfile::to_machine`]).
    pub fn to_field(&self, mx: f64, my: f64) -> (f64, f64) {
        let [w, h] = self.work_area_mm;
        let (fx, fy) = self.mirror();
        let sx = if fx { -1.0 } else { 1.0 };
        let sy = if fy { -1.0 } else { 1.0 };
        (sx * (mx - w / 2.0), sy * (my - h / 2.0))
    }

    pub fn to_toml(&self) -> Result<String, ProfileError> {
        toml::to_string_pretty(self).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    pub fn from_toml(text: &str) -> Result<Self, ProfileError> {
        let p: RuidaProfile = toml::from_str(text).map_err(|e| ProfileError::Toml(e.to_string()))?;
        p.validate()?;
        Ok(p)
    }
}

/// `(mirror_x, mirror_y)` of a bed origin corner.
pub fn mirror_of(origin: Origin) -> (bool, bool) {
    match origin {
        Origin::BottomLeft | Origin::Center => (false, false),
        Origin::TopLeft => (false, true),
        Origin::BottomRight => (true, false),
        Origin::TopRight => (true, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_round_trip() {
        let p = RuidaProfile { address: "192.168.1.100".into(), ..RuidaProfile::default() };
        p.validate().unwrap();
        assert_eq!(RuidaProfile::from_toml(&p.to_toml().unwrap()).unwrap(), p);
        assert_eq!(p.ip(), Some("192.168.1.100".parse().unwrap()));
        RuidaProfile::default().validate().unwrap();
        for bad in [
            RuidaProfile { address: "laser.local".into(), ..p.clone() },
            RuidaProfile { max_power_pct: 0.0, ..p.clone() },
            RuidaProfile { max_power_pct: 100.5, ..p.clone() },
            RuidaProfile { max_speed_mm_s: f64::NAN, ..p.clone() },
            RuidaProfile { max_speed_mm_s: 6000.0, ..p.clone() },
            RuidaProfile { work_area_mm: [0.0, 10.0], ..p.clone() },
            RuidaProfile { origin: Origin::Center, ..p.clone() },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn corners_map_both_ways() {
        let p = RuidaProfile { work_area_mm: [600.0, 400.0], ..RuidaProfile::default() };
        // Rear right origin: the rear right corner of the field frame is machine (0, 0).
        assert_eq!(p.to_machine(300.0, 200.0).unwrap(), (0.0, 0.0));
        assert_eq!(p.to_machine(-300.0, -200.0).unwrap(), (600.0, 400.0));
        for o in [Origin::BottomLeft, Origin::TopLeft, Origin::TopRight, Origin::BottomRight] {
            let q = RuidaProfile { origin: o, ..p.clone() };
            let (mx, my) = q.to_machine(12.5, -40.0).unwrap();
            let (x, y) = q.to_field(mx, my);
            assert!((x - 12.5).abs() < 1e-9 && (y + 40.0).abs() < 1e-9, "{o:?}");
        }
        assert!(p.to_machine(300.5, 0.0).is_err());
        assert!(p.to_machine(0.0, f64::INFINITY).is_err());
    }
}
