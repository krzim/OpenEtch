//! [`MachineProfile`]: one laser's fixed facts and hard limits, stored as TOML.
//!
//! Everything here is in mm, mm/s, kHz, ns, µs and percent. The profile is the only place hard limits live
//! (field bounds, max power, pulse-width table, frequency range): job settings and command arguments are checked
//! against it and can never raise it. Every loader validates, so a `MachineProfile` obtained from
//! [`MachineProfile::from_toml`] or [`MachineProfile::from_markcfg`] is always encodable by `lmc`.

use std::path::{Path, PathBuf};

use lmc::units;
use serde::{Deserialize, Serialize};

/// Why a profile was refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProfileError {
    #[error("profile TOML: {0}")]
    Toml(String),
    #[error("markcfg: {0}")]
    Markcfg(String),
    /// A value is outside what the hardware or `lmc` can represent.
    #[error("profile field `{field}`: {reason}")]
    Invalid { field: &'static str, reason: String },
}

/// Default motion and laser timing. Delays in µs (negative laser-on delay is allowed), speeds in mm/s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    /// Laser-on delay, µs (may be negative).
    pub laser_on_delay_us: f64,
    /// Laser-off delay, µs.
    pub laser_off_delay_us: f64,
    /// Polygon (corner) delay, µs. Sent in 10 µs units (`lmc::units::polygon_delay_units`; unverified encoding).
    pub polygon_delay_us: f64,
    /// End-of-mark delay, µs.
    pub end_delay_us: f64,
    /// Jump (travel) speed, mm/s.
    pub jump_speed_mm_s: f64,
    /// Jump delay used for jumps up to `jump_distance_limit_mm`, µs.
    pub jump_delay_min_us: f64,
    /// Jump delay used for longer jumps, µs.
    pub jump_delay_max_us: f64,
    /// Jumps longer than this use `jump_delay_max_us`, mm.
    pub jump_distance_limit_mm: f64,
}

/// Mark speeds a layer may request, mm/s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkSpeedLimits {
    pub min_mm_s: f64,
    pub max_mm_s: f64,
}

/// Default [`MachineProfile::max_jump_speed_mm_s`]: 4000 mm/s, LightBurn's B6 library entry (`JumpSpeed=4000`).
/// Provisional: not measured on the machine.
pub const DEFAULT_MAX_JUMP_SPEED_MM_S: f64 = 4000.0;

/// Default [`MachineProfile::max_delay_us`]: 2000 µs. Provisional: an order of magnitude above every delay the B6
/// profile and the references use (largest: |laser on| 300 µs, jump 200 µs), so a mistyped delay is refused.
pub const DEFAULT_MAX_DELAY_US: f64 = 2000.0;

fn default_max_jump_speed() -> f64 {
    DEFAULT_MAX_JUMP_SPEED_MM_S
}

fn default_max_delay() -> f64 {
    DEFAULT_MAX_DELAY_US
}

/// Default [`MachineProfile::open_mo_delay_us`]: 8000 µs, LightBurn's value for the B6 (`Laser_OpenMODelay`).
/// Provisional.
pub const DEFAULT_OPEN_MO_DELAY_US: f64 = 8000.0;

/// Largest [`MachineProfile::open_mo_delay_us`] accepted: 1 s.
const MAX_OPEN_MO_DELAY_US: f64 = 1_000_000.0;

fn default_open_mo_delay() -> f64 {
    DEFAULT_OPEN_MO_DELAY_US
}

fn default_true() -> bool {
    true
}

/// One laser's fixed facts and hard limits. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineProfile {
    /// Display name.
    pub name: String,
    /// Laser source description (e.g. "JPT M7 MOPA 60W"); informational.
    #[serde(default)]
    pub laser_source: String,
    /// Side of the square galvo field, mm. Galvo words `0x0000..=0xFFFF` span it.
    pub field_mm: f64,
    /// Exchange the X and Y galvos (applied after negation, see [`crate::field_to_galvo`]).
    pub swap_xy: bool,
    /// Mirror field X (in the field frame, before any swap).
    pub negate_x: bool,
    /// Mirror field Y (in the field frame, before any swap).
    pub negate_y: bool,
    /// EzCad lens correction file, relative to the profile file ([`MachineProfile::cor_path`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cor_file: Option<PathBuf>,
    /// MOPA pulse widths the source accepts, ns, ascending.
    pub pulse_widths_ns: Vec<u16>,
    /// Lowest Q-switch frequency, kHz.
    pub frequency_khz_min: f64,
    /// Highest Q-switch frequency, kHz.
    pub frequency_khz_max: f64,
    /// Hard power cap, percent (0..=100). No job or command can exceed it.
    pub max_power_pct: f64,
    /// Output port bit of the red aiming pointer. `lmc::LightList` fixes it at 8, so only 8 is accepted.
    pub red_light_port_bit: u8,
    /// Output port bit of laser enable. `lmc` fixes it at 0, so only 0 is accepted.
    pub laser_enable_port_bit: u8,
    /// Jump speed used for red-light framing, mm/s.
    pub red_light_speed_mm_s: f64,
    /// Hard cap on any jump (travel) speed a job or frame may send, mm/s. Missing in older profile files: defaults to
    /// [`DEFAULT_MAX_JUMP_SPEED_MM_S`].
    #[serde(default = "default_max_jump_speed")]
    pub max_jump_speed_mm_s: f64,
    /// Hard cap on the magnitude of the laser-on, laser-off, polygon and jump delays a job may send, µs. Missing in
    /// older profile files: defaults to [`DEFAULT_MAX_DELAY_US`].
    #[serde(default = "default_max_delay")]
    pub max_delay_us: f64,
    /// The source takes a MOPA pulse width (LightBurn `Laser_QPulseWidthEnabled`). Informational for now: `plan`
    /// sends each layer's pulse width either way. Missing in older profile files: true.
    #[serde(default = "default_true")]
    pub pulse_width_enabled: bool,
    /// How long the source needs after the master oscillator opens, µs (LightBurn `Laser_OpenMODelay`, EzCad's "open
    /// MO delay"). Informational for now: the run prelude does not wait. Missing in older profile files:
    /// [`DEFAULT_OPEN_MO_DELAY_US`].
    #[serde(default = "default_open_mo_delay")]
    pub open_mo_delay_us: f64,
    /// Input bit of the start-mark signal (a foot pedal or button; LightBurn `StartMarkPortBit`, markcfg
    /// `STARTMARKPORT`), 0..=15; `None` when there is none. Informational: `--confirm-input` is what makes the engine
    /// wait for an input. 14 on the B6 (unverified that anything is wired to it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_mark_input_bit: Option<u8>,
    pub timing: Timing,
    pub mark_speed: MarkSpeedLimits,
    /// Where values came from and what is unverified. Informational.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl MachineProfile {
    /// A generic 150 mm-lens LMC fiber profile: galvoplotter @ fc35136 defaults for timing, the JPT M7 pulse table,
    /// 1..=4000 kHz, no correction file. The base [`MachineProfile::from_markcfg`] imports onto.
    pub fn generic_lmc_fiber() -> Self {
        MachineProfile {
            name: "Generic LMC fiber".into(),
            laser_source: String::new(),
            field_mm: 150.0,
            swap_xy: false,
            negate_x: false,
            negate_y: false,
            cor_file: None,
            pulse_widths_ns: units::JPT_M7_PULSE_WIDTHS_NS.to_vec(),
            frequency_khz_min: 1.0,
            frequency_khz_max: 4000.0,
            max_power_pct: 100.0,
            red_light_port_bit: 8,
            laser_enable_port_bit: 0,
            red_light_speed_mm_s: 2000.0,
            max_jump_speed_mm_s: DEFAULT_MAX_JUMP_SPEED_MM_S,
            max_delay_us: DEFAULT_MAX_DELAY_US,
            pulse_width_enabled: true,
            open_mo_delay_us: DEFAULT_OPEN_MO_DELAY_US,
            start_mark_input_bit: None,
            timing: Timing {
                laser_on_delay_us: 100.0,
                laser_off_delay_us: 100.0,
                polygon_delay_us: 100.0,
                end_delay_us: 300.0,
                jump_speed_mm_s: 2000.0,
                jump_delay_min_us: 8.0,
                jump_delay_max_us: 200.0,
                jump_distance_limit_mm: 10.0,
            },
            mark_speed: MarkSpeedLimits { min_mm_s: 1.0, max_mm_s: 10000.0 },
            notes: vec!["timing defaults from meerk40t/galvoplotter @ fc35136".into()],
        }
    }

    /// Parse and validate a TOML profile.
    pub fn from_toml(text: &str) -> Result<Self, ProfileError> {
        let profile: MachineProfile = toml::from_str(text).map_err(|e| ProfileError::Toml(e.to_string()))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Serialize to TOML (comments are not preserved).
    pub fn to_toml(&self) -> Result<String, ProfileError> {
        toml::to_string_pretty(self).map_err(|e| ProfileError::Toml(e.to_string()))
    }

    /// Import an EzCad `markcfg7` (`[LMC_CFG]`) onto [`MachineProfile::generic_lmc_fiber`]. See
    /// [`crate::MarkcfgImport`] for the keys used and their (partly unverified) meaning.
    pub fn from_markcfg(text: &str) -> Result<Self, ProfileError> {
        crate::markcfg::import(text, Self::generic_lmc_fiber()).map(|i| i.profile)
    }

    /// [`MachineProfile::from_markcfg`] for the raw file bytes (EzCad writes GBK/latin-1, decoded here as latin-1;
    /// only ASCII keys and values are used).
    pub fn from_markcfg_bytes(bytes: &[u8]) -> Result<Self, ProfileError> {
        Self::from_markcfg(&crate::markcfg::latin1(bytes))
    }

    /// The correction file resolved against the directory of `profile_file` (absolute paths are kept).
    pub fn cor_path(&self, profile_file: &Path) -> Option<PathBuf> {
        let cor = self.cor_file.as_ref()?;
        if cor.is_absolute() {
            return Some(cor.clone());
        }
        Some(profile_file.parent().unwrap_or(Path::new("")).join(cor))
    }

    /// Half the field, mm: points must be strictly inside `±half_field_mm()`.
    pub fn half_field_mm(&self) -> f64 {
        self.field_mm / 2.0
    }

    /// The field size, swap and negate settings as `lmc`'s shared field transform (the one `plan` uses too).
    pub fn field_transform(&self) -> lmc::FieldTransform {
        lmc::FieldTransform { field_mm: self.field_mm, swap_xy: self.swap_xy, negate_x: self.negate_x, negate_y: self.negate_y }
    }

    /// Mask of the laser-enable port bit.
    pub fn laser_enable_mask(&self) -> u16 {
        1u16 << (self.laser_enable_port_bit & 15)
    }

    /// Check every value against what the hardware and `lmc` can encode.
    pub fn validate(&self) -> Result<(), ProfileError> {
        let invalid = |field: &'static str, reason: String| Err(ProfileError::Invalid { field, reason });
        if !(self.field_mm.is_finite() && self.field_mm > 0.0) {
            return invalid("field_mm", format!("{} must be a positive number of mm", self.field_mm));
        }
        if self.pulse_widths_ns.is_empty() {
            return invalid("pulse_widths_ns", "the table is empty".into());
        }
        if self.pulse_widths_ns.contains(&0) || !self.pulse_widths_ns.is_sorted_by(|a, b| a < b) {
            return invalid("pulse_widths_ns", "must be positive and strictly ascending".into());
        }
        positive("frequency_khz_min", self.frequency_khz_min)?;
        positive("frequency_khz_max", self.frequency_khz_max)?;
        if self.frequency_khz_min > self.frequency_khz_max {
            return invalid("frequency_khz_min", "is above frequency_khz_max".into());
        }
        for (field, khz) in [("frequency_khz_min", self.frequency_khz_min), ("frequency_khz_max", self.frequency_khz_max)] {
            units::frequency_to_period(khz).map_err(|e| ProfileError::Invalid { field, reason: e.to_string() })?;
        }
        if !(0.0..=100.0).contains(&self.max_power_pct) {
            return invalid("max_power_pct", format!("{} is outside 0..=100 %", self.max_power_pct));
        }
        if u16::from(self.red_light_port_bit) != lmc::RED_POINTER_BIT.trailing_zeros() as u16 {
            return invalid("red_light_port_bit", format!("{} is unsupported: lmc's LightList drives bit 8", self.red_light_port_bit));
        }
        if u16::from(self.laser_enable_port_bit) != lmc::LASER_ENABLE_BIT.trailing_zeros() as u16 {
            return invalid("laser_enable_port_bit", format!("{} is unsupported: lmc uses bit 0", self.laser_enable_port_bit));
        }
        self.speed("max_jump_speed_mm_s", self.max_jump_speed_mm_s)?;
        for (field, mm_s) in [("red_light_speed_mm_s", self.red_light_speed_mm_s), ("timing.jump_speed_mm_s", self.timing.jump_speed_mm_s)] {
            self.speed(field, mm_s)?;
            if mm_s > self.max_jump_speed_mm_s {
                return invalid(field, format!("{mm_s} mm/s is above max_jump_speed_mm_s ({})", self.max_jump_speed_mm_s));
            }
        }
        positive("max_delay_us", self.max_delay_us)?;
        if !(self.open_mo_delay_us.is_finite() && (0.0..=MAX_OPEN_MO_DELAY_US).contains(&self.open_mo_delay_us)) {
            return invalid("open_mo_delay_us", format!("{} must be 0..={MAX_OPEN_MO_DELAY_US} µs", self.open_mo_delay_us));
        }
        if let Some(bit) = self.start_mark_input_bit.filter(|b| *b > 15) {
            return invalid("start_mark_input_bit", format!("{bit} is not an input bit 0-15"));
        }
        // The minimum is a UI bound: it may encode to 0 (1 mm/s on a 153.75 mm field); jobs refuse a 0 word.
        positive("mark_speed.min_mm_s", self.mark_speed.min_mm_s)?;
        self.speed("mark_speed.max_mm_s", self.mark_speed.max_mm_s)?;
        if self.mark_speed.min_mm_s > self.mark_speed.max_mm_s {
            return invalid("mark_speed.min_mm_s", "is above mark_speed.max_mm_s".into());
        }
        let t = &self.timing;
        for (field, us) in [
            ("timing.laser_on_delay_us", t.laser_on_delay_us),
            ("timing.laser_off_delay_us", t.laser_off_delay_us),
            ("timing.end_delay_us", t.end_delay_us),
            ("timing.jump_delay_min_us", t.jump_delay_min_us),
            ("timing.jump_delay_max_us", t.jump_delay_max_us),
        ] {
            units::delay_word(us).map_err(|e| ProfileError::Invalid { field, reason: e.to_string() })?;
        }
        units::polygon_delay_units(t.polygon_delay_us).map_err(|e| ProfileError::Invalid { field: "timing.polygon_delay_us", reason: e.to_string() })?;
        for (field, us) in [
            ("timing.laser_on_delay_us", t.laser_on_delay_us),
            ("timing.laser_off_delay_us", t.laser_off_delay_us),
            ("timing.polygon_delay_us", t.polygon_delay_us),
            ("timing.jump_delay_max_us", t.jump_delay_max_us),
        ] {
            if us.abs() > self.max_delay_us {
                return invalid(field, format!("|{us}| µs is above max_delay_us ({})", self.max_delay_us));
            }
        }
        if t.jump_delay_min_us < 0.0 || t.jump_delay_max_us < t.jump_delay_min_us {
            return invalid("timing.jump_delay_min_us", "need 0 <= jump_delay_min_us <= jump_delay_max_us".into());
        }
        if !(t.jump_distance_limit_mm.is_finite() && t.jump_distance_limit_mm >= 0.0) {
            return invalid("timing.jump_distance_limit_mm", format!("{} must be >= 0 mm", t.jump_distance_limit_mm));
        }
        Ok(())
    }

    /// A speed must be positive and give a non-zero, representable galvo speed word for this field.
    fn speed(&self, field: &'static str, mm_s: f64) -> Result<u16, ProfileError> {
        positive(field, mm_s)?;
        match units::speed_to_units(mm_s, self.field_mm) {
            Ok(0) => Err(ProfileError::Invalid { field, reason: format!("{mm_s} mm/s rounds to 0 galvo units/ms") }),
            Ok(u) => Ok(u),
            Err(e) => Err(ProfileError::Invalid { field, reason: e.to_string() }),
        }
    }
}

fn positive(field: &'static str, v: f64) -> Result<(), ProfileError> {
    if v.is_finite() && v > 0.0 { Ok(()) } else { Err(ProfileError::Invalid { field, reason: format!("{v} must be positive") }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
    const MARKCFG: &[u8] = include_bytes!("../../../fixtures/markcfg7.ini");

    #[test]
    fn b6_fixture_loads() {
        let p = MachineProfile::from_toml(B6).expect("fixture is valid");
        assert_eq!(p.field_mm, 153.75);
        assert!(!p.swap_xy && !p.negate_x && !p.negate_y);
        assert_eq!(p.cor_file.as_deref(), Some(Path::new("jcz150.cor")));
        assert_eq!(p.pulse_widths_ns, units::JPT_M7_PULSE_WIDTHS_NS.to_vec());
        assert_eq!((p.frequency_khz_min, p.frequency_khz_max), (1.0, 4000.0));
        assert_eq!((p.red_light_port_bit, p.laser_enable_port_bit), (8, 0));
        assert_eq!(p.red_light_speed_mm_s, 3000.0);
        assert_eq!((p.timing.laser_on_delay_us, p.timing.laser_off_delay_us, p.timing.polygon_delay_us), (-300.0, 100.0, 100.0));
        assert_eq!((p.mark_speed.min_mm_s, p.mark_speed.max_mm_s), (1.0, 10000.0));
        assert_eq!((p.max_jump_speed_mm_s, p.max_delay_us), (4000.0, 2000.0));
        assert_eq!((p.pulse_width_enabled, p.open_mo_delay_us, p.start_mark_input_bit), (true, 8000.0, Some(14)));
    }

    #[test]
    fn missing_limits_default() {
        let added = ["max_jump_speed_mm_s", "max_delay_us", "pulse_width_enabled", "open_mo_delay_us", "start_mark_input_bit"];
        let text: String = B6.lines().filter(|l| !added.iter().any(|k| l.starts_with(k))).collect::<Vec<_>>().join("
");
        let p = MachineProfile::from_toml(&text).unwrap();
        assert_eq!((p.max_jump_speed_mm_s, p.max_delay_us), (DEFAULT_MAX_JUMP_SPEED_MM_S, DEFAULT_MAX_DELAY_US));
        assert_eq!((p.pulse_width_enabled, p.open_mo_delay_us, p.start_mark_input_bit), (true, DEFAULT_OPEN_MO_DELAY_US, None));
    }

    #[test]
    fn b6_fixture_agrees_with_markcfg() {
        let fixture = MachineProfile::from_toml(B6).unwrap();
        let imported = MachineProfile::from_markcfg_bytes(MARKCFG).unwrap();
        assert_eq!(imported.field_mm, fixture.field_mm);
        assert_eq!((imported.swap_xy, imported.negate_x, imported.negate_y), (fixture.swap_xy, fixture.negate_x, fixture.negate_y));
        assert_eq!(imported.cor_file, fixture.cor_file);
        assert_eq!(imported.red_light_speed_mm_s, fixture.red_light_speed_mm_s);
        assert_eq!(imported.mark_speed, fixture.mark_speed);
        assert_eq!((imported.frequency_khz_min, imported.frequency_khz_max), (fixture.frequency_khz_min, fixture.frequency_khz_max));
        assert_eq!(imported.start_mark_input_bit, fixture.start_mark_input_bit);
    }

    #[test]
    fn toml_round_trip() {
        let p = MachineProfile::from_toml(B6).unwrap();
        let again = MachineProfile::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(again, p);
        let g = MachineProfile::generic_lmc_fiber();
        assert_eq!(MachineProfile::from_toml(&g.to_toml().unwrap()).unwrap(), g);
    }

    #[test]
    fn invalid_values_refused() {
        let base = MachineProfile::generic_lmc_fiber();
        let refused = |f: fn(&mut MachineProfile)| {
            let mut p = base.clone();
            f(&mut p);
            assert!(p.validate().is_err(), "{p:?}");
            assert!(MachineProfile::from_toml(&p.to_toml().unwrap()).is_err());
        };
        refused(|p| p.field_mm = 0.0);
        refused(|p| p.field_mm = -5.0);
        refused(|p| p.max_power_pct = 101.0);
        refused(|p| p.max_power_pct = -1.0);
        refused(|p| p.red_light_port_bit = 3);
        refused(|p| p.laser_enable_port_bit = 8);
        refused(|p| p.pulse_widths_ns = vec![]);
        refused(|p| p.pulse_widths_ns = vec![4, 2]);
        refused(|p| p.frequency_khz_min = 5000.0);
        refused(|p| p.frequency_khz_max = 50000.0);
        refused(|p| p.timing.laser_on_delay_us = 70000.0);
        refused(|p| p.timing.jump_delay_max_us = 1.0);
        refused(|p| p.red_light_speed_mm_s = 0.0);
        refused(|p| p.mark_speed.min_mm_s = 20000.0);
        refused(|p| p.timing.jump_speed_mm_s = 4500.0);
        refused(|p| p.red_light_speed_mm_s = 4500.0);
        refused(|p| p.max_jump_speed_mm_s = 0.0);
        refused(|p| p.max_delay_us = -1.0);
        refused(|p| p.timing.laser_on_delay_us = -2500.0);
        refused(|p| p.timing.polygon_delay_us = 2500.0);
        refused(|p| p.timing.polygon_delay_us = 700_000.0);
        refused(|p| p.open_mo_delay_us = -1.0);
        refused(|p| p.open_mo_delay_us = f64::NAN);
        refused(|p| p.open_mo_delay_us = 2_000_000.0);
        refused(|p| p.start_mark_input_bit = Some(16));
    }

    #[test]
    fn unknown_keys_refused() {
        let text = format!("{B6}\nmax_powr_pct = 50\n");
        assert!(matches!(MachineProfile::from_toml(&text), Err(ProfileError::Toml(_))));
    }

    #[test]
    fn cor_path_is_relative_to_profile() {
        let p = MachineProfile::from_toml(B6).unwrap();
        let path = p.cor_path(Path::new("fixtures/b6-mopa-60w.toml")).unwrap();
        assert_eq!(path, Path::new("fixtures").join("jcz150.cor"));
        assert_eq!(MachineProfile::generic_lmc_fiber().cor_path(Path::new("x.toml")), None);
    }
}
