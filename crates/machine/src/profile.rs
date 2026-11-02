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

/// `fixtures/generic-jcz-galvo.toml`, the base of [`MachineProfile::generic_jcz_galvo`].
pub const GENERIC_JCZ_GALVO_TOML: &str = include_str!("../../../fixtures/generic-jcz-galvo.toml");

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

fn default_motion_time_scale() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// LightBurn's Laser Type for a galvo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaserKind {
    #[default]
    Fiber,
    Co2,
    Uv,
}

/// LightBurn's Fiber Type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiberKind {
    /// A MOPA source: pulse width (Q-pulse) settings apply.
    #[default]
    Mopa,
    /// A Q-switched source: no pulse width.
    QSwitched,
}

/// Red-dot alignment, framing only (LightBurn Device Settings > Red Dot): the pointer diode sits off the beam axis,
/// so a framed point `p` is sent to `(p.x * scale_x + offset_x_mm, p.y * scale_y + offset_y_mm)`, then through
/// `correction` (the same model as the field calibration, `lmc::Calibration`: angle, skew, trapezoid, bulge), before
/// the lens correction; the burn never uses these. `correction` exists because the red light (about 650 nm) goes
/// through an f-theta lens corrected for the burn's 1064 nm, so the frame can come out slightly trapezoidal or
/// skewed while the burn is square. The rest position is where the dot is parked when framing stops.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RedDot {
    pub offset_x_mm: f64,
    pub offset_y_mm: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rest_x_mm: f64,
    pub rest_y_mm: f64,
    /// Frame-only distortion correction, neutral by default.
    #[serde(skip_serializing_if = "lmc::Calibration::is_neutral")]
    pub correction: lmc::Calibration,
}

impl Default for RedDot {
    fn default() -> Self {
        RedDot {
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rest_x_mm: 0.0,
            rest_y_mm: 0.0,
            correction: lmc::Calibration::default(),
        }
    }
}

impl RedDot {
    pub fn is_neutral(&self) -> bool {
        *self == RedDot::default()
    }

    /// Where a framed field point is sent (`field_mm` scales the `correction`, as for the field calibration).
    pub fn apply(&self, x: f64, y: f64, field_mm: f64) -> (f64, f64) {
        self.correction.apply(x * self.scale_x + self.offset_x_mm, y * self.scale_y + self.offset_y_mm, field_mm)
    }
}

/// What the board's extension axis drives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxisRole {
    /// A motorised Z lift: positions in mm.
    #[default]
    Z,
    /// A rotary (chuck or roller): positions in steps (steps per rotation is a rotary setting of the job).
    Rotary,
    /// A linear table for Repeat Marking (LightBurn's external axis, Linear): `steps_per_mm` motor steps per mm,
    /// positions from step 0.
    LinearTable,
    /// A rotating table for Repeat Marking (LightBurn's external axis, Rotating): `steps_per_mm` holds motor steps
    /// per degree of table rotation (LightBurn's Steps Per / Unit Count / 360), positions from step 0.
    RotaryTable,
}

/// The board's extension axis (LMC `MoveAxisTo(position, invert)`, meerk40t's parameters; the position is one
/// 16-bit word of motor steps, unverified on hardware).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionAxis {
    pub role: AxisRole,
    /// Motor steps per mm of Z travel (role `z`).
    #[serde(default)]
    pub steps_per_mm: f64,
    /// Lowest Z position, mm (step 0).
    #[serde(default)]
    pub min_mm: f64,
    /// Highest Z position, mm.
    #[serde(default)]
    pub max_mm: f64,
    /// Reverse the motor direction (the `invert` parameter of `MoveAxisTo`).
    #[serde(default)]
    pub invert: bool,
}

impl ExtensionAxis {
    /// The step word for Z position `z_mm`: refused outside `min_mm..=max_mm` or for a rotary axis.
    pub fn z_steps(&self, z_mm: f64) -> Result<u16, String> {
        if self.role != AxisRole::Z {
            return Err("the extension axis is a rotary, not a Z lift".into());
        }
        if !(z_mm.is_finite() && (self.min_mm..=self.max_mm).contains(&z_mm)) {
            return Err(format!("Z {z_mm} mm is outside the axis travel {}..={} mm", self.min_mm, self.max_mm));
        }
        let steps = ((z_mm - self.min_mm) * self.steps_per_mm).round();
        if !(0.0..=65535.0).contains(&steps) {
            return Err(format!("Z {z_mm} mm is {steps} steps, more than one 16-bit word"));
        }
        // Whole and within 0..=65535 (checked above).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(steps as u16)
    }

    /// The step word for a table position (`units`: mm for a linear table, degrees for a rotating one), refused
    /// for another role or outside one 16-bit word.
    pub fn table_steps(&self, units: f64) -> Result<u16, String> {
        if !matches!(self.role, AxisRole::LinearTable | AxisRole::RotaryTable) {
            return Err("the extension axis is not a Repeat Marking table (role linear_table or rotary_table)".into());
        }
        let steps = (units * self.steps_per_mm).round();
        if !(steps.is_finite() && (0.0..=65535.0).contains(&steps)) {
            return Err(format!("table position {units} is {steps} steps, outside one 16-bit word (0..=65535)"));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(steps as u16)
    }

    fn problem(&self) -> Option<String> {
        if matches!(self.role, AxisRole::LinearTable | AxisRole::RotaryTable)
            && !(self.steps_per_mm.is_finite() && self.steps_per_mm > 0.0)
        {
            return Some(format!("a table's steps_per_mm {} must be positive", self.steps_per_mm));
        }
        if self.role == AxisRole::Z {
            if !(self.steps_per_mm.is_finite() && self.steps_per_mm > 0.0) {
                return Some(format!("steps_per_mm {} must be positive", self.steps_per_mm));
            }
            if !(self.min_mm.is_finite() && self.max_mm.is_finite() && self.min_mm < self.max_mm) {
                return Some(format!("travel {}..{} mm is empty", self.min_mm, self.max_mm));
            }
            if (self.max_mm - self.min_mm) * self.steps_per_mm > 65535.0 {
                return Some("the travel is more than 65535 steps (one 16-bit word)".into());
            }
        }
        None
    }
}

/// An input of the board's port with its trigger level (LightBurn's IO Ports High/Low button).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPort {
    /// Input bit 0..=15.
    pub bit: u8,
    /// The input is active when low (LightBurn "Low"); default active high.
    #[serde(default, skip_serializing_if = "is_false")]
    pub active_low: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl InputPort {
    /// Whether this input is active in the port word `inputs` (`ReadPort` reply word 1).
    pub fn active(&self, inputs: u16) -> bool {
        ((inputs >> (self.bit & 15)) & 1 == 1) != self.active_low
    }
}

/// LightBurn's galvo IO Ports beyond the start/stop marking inputs and the focus light (all unverified on hardware:
/// nothing is known to be wired to them on the B6). Outputs are active high.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct IoPorts {
    /// Door Protect: an enclosure interlock. While it is active no run starts and a running job aborts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub door_protect: Option<InputPort>,
    /// Start Framing: pressing it starts framing (the last frame request, else the job's bounds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_framing: Option<InputPort>,
    /// Busy Light output bit: on while a job runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_light: Option<u8>,
    /// Ready Light output bit (the LEDs on the galvo head): on while the board is initialised.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ready_light: Option<u8>,
    /// Done Marking output bit: pulsed when a job finishes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_marking: Option<u8>,
    /// Length of the Done Marking pulse, ms (LightBurn gives none; default 500, at most 5000).
    pub done_pulse_ms: u32,
    /// Red Dot Always On: the red pointer is lit whenever the board is idle (never while a job runs).
    #[serde(skip_serializing_if = "is_false")]
    pub red_dot_always_on: bool,
}

impl Default for IoPorts {
    fn default() -> Self {
        IoPorts {
            door_protect: None,
            start_framing: None,
            busy_light: None,
            ready_light: None,
            done_marking: None,
            done_pulse_ms: 500,
            red_dot_always_on: false,
        }
    }
}

impl IoPorts {
    pub fn is_default(&self) -> bool {
        *self == IoPorts::default()
    }

    fn mask(bit: Option<u8>) -> u16 {
        bit.map_or(0, |b| 1u16 << (b & 15))
    }

    /// Output bits held while a job's lists run (busy and ready lights); they go with every `WritePort`.
    pub fn run_outputs(&self) -> u16 {
        Self::mask(self.busy_light) | Self::mask(self.ready_light)
    }

    /// Output word while the board is idle: the ready light and, with Red Dot Always On, the red pointer.
    pub fn idle_outputs(&self) -> u16 {
        Self::mask(self.ready_light) | if self.red_dot_always_on { lmc::RED_POINTER_BIT } else { 0 }
    }

    /// The Done Marking pulse bit, if any.
    pub fn done_output(&self) -> u16 {
        Self::mask(self.done_marking)
    }

    fn problem(&self, laser_bit: u8, red_bit: u8, focus: Option<u8>) -> Option<String> {
        for (name, port) in [("door_protect", self.door_protect), ("start_framing", self.start_framing)] {
            if let Some(p) = port.filter(|p| p.bit > 15) {
                return Some(format!("{name}: {} is not an input bit 0-15", p.bit));
            }
        }
        let outs = [("busy_light", self.busy_light), ("ready_light", self.ready_light), ("done_marking", self.done_marking)];
        for (i, (name, bit)) in outs.iter().enumerate() {
            let Some(b) = *bit else { continue };
            if b == 0 || b > 15 || b == laser_bit || b == red_bit || Some(b) == focus {
                return Some(format!(
                    "{name}: {b} must be an output bit 1-15 other than the laser-enable, red-pointer and focus-light bits"
                ));
            }
            if outs[..i].iter().any(|(_, o)| *o == Some(b)) {
                return Some(format!("{name}: bit {b} is used by another output"));
            }
        }
        if self.done_pulse_ms > 5000 {
            return Some(format!("done_pulse_ms {} must be at most 5000", self.done_pulse_ms));
        }
        None
    }
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
    /// Width of the galvo field (field X), mm, and its height unless `field_height_mm` is set. Galvo words
    /// `0x0000..=0xFFFF` span it.
    pub field_mm: f64,
    /// Height of a non-square field (field Y), mm (LightBurn's separate field Width and Height); `None` (the
    /// default, left out of JSON) for a square field of `field_mm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_height_mm: Option<f64>,
    /// Output curve tolerance for this device, mm (LightBurn's Curve Tolerance): how far flattened polylines may
    /// stray from curves, 0.001..=1. A document's own setting wins; `None` (the default) means 0.01 mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve_tolerance_mm: Option<f64>,
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
    /// The source takes a MOPA pulse width (LightBurn `Laser_QPulseWidthEnabled`). When false (or with a Q-switched
    /// `fiber_kind`) jobs send no pulse width (the engine's planning target drops it, with a plan warning). Missing in
    /// older profile files: true.
    #[serde(default = "default_true")]
    pub pulse_width_enabled: bool,
    /// How long the source needs after the master oscillator opens, µs (LightBurn `Laser_OpenMODelay`, EzCad's "open
    /// MO delay"). Every job list waits this long
    /// (`DelayTime`) right after the MO opens, before the first motion. Missing in older profile files:
    /// [`DEFAULT_OPEN_MO_DELAY_US`].
    #[serde(default = "default_open_mo_delay")]
    pub open_mo_delay_us: f64,
    /// How many times longer the board takes to move than distance / commanded speed, for the time estimate only
    /// (`plan::Target::motion_time_scale`; what is sent never changes). 0.5..=2; missing: 1 (as commanded). The
    /// B6 fixture's 1.30 comes from two runs measured with the board's `GetMarkTime` (see the fixture).
    #[serde(default = "default_motion_time_scale", skip_serializing_if = "is_one")]
    pub motion_time_scale: f64,
    /// Input bit of the start-mark signal (a foot pedal or button; LightBurn `StartMarkPortBit`, markcfg
    /// `STARTMARKPORT`), 0..=15; `None` when there is none. A run asked to wait for the start input waits for this bit
    /// before each iteration (LightBurn's Start Marking input); `--confirm-input` is the separate approval
    /// confirmation. 14 on the B6 (unverified that anything is wired to it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_mark_input_bit: Option<u8>,
    /// The start-mark input is active when low (LightBurn `StartMarkBitHigh` = false, its IO Ports "Low"): pressed
    /// while the bit reads 0. Default active high.
    #[serde(default, skip_serializing_if = "is_false")]
    pub start_mark_active_low: bool,
    /// Input bit of a stop-marking signal (a button wired to the board), 0..=15; `None` when there is none. While a
    /// run streams, the controller reads the inputs at every status poll and aborts the run when this bit is set
    /// (unverified on hardware: nothing is known to be wired to it on the B6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_mark_input_bit: Option<u8>,
    /// Output bit of a second pointer used as a focus light (LightBurn's focus light port), 1..=15, never the
    /// laser-enable or red-pointer bit. Framing can switch it on instead of, or with, the red pointer. `None`: there
    /// is none (the B6 has none as far as known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_light_port_bit: Option<u8>,
    /// Blanking: switch the pointer off while travelling between shapes when framing (LightBurn's "Enable
    /// Blanking"). Missing in older profile files: true.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub blanking: bool,
    /// The kind of source (LightBurn's Laser Type). Informational: only fiber sources are driven.
    #[serde(default, skip_serializing_if = "is_default")]
    pub laser_kind: LaserKind,
    /// The kind of fiber source (LightBurn's Fiber Type). A Q-switched source takes no pulse width: with
    /// `q_switched` or `pulse_width_enabled = false` jobs never send one.
    #[serde(default, skip_serializing_if = "is_default")]
    pub fiber_kind: FiberKind,
    /// Lens corrections (LightBurn's field offset X/Y, field angle and per-axis scale, bulge, skew and trapezoid),
    /// applied by `lmc::units::field_to_galvo` to the burn and the frame alike. Neutral by default.
    #[serde(default, skip_serializing_if = "lmc::Calibration::is_neutral")]
    pub calibration: lmc::Calibration,
    /// Red-dot alignment (LightBurn's Red Dot offset, scale and rest position), applied to framing only.
    #[serde(default, skip_serializing_if = "RedDot::is_neutral")]
    pub red_dot: RedDot,
    /// The board's extension axis (LMC `MoveAxisTo`), when a motor is wired to it: a Z lift or a rotary. `None`
    /// on the B6 (its lift is not driven by the board).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<ExtensionAxis>,
    /// LightBurn's IO Ports: door interlock, framing switch, busy / ready / done outputs, Red Dot Always On.
    #[serde(default, skip_serializing_if = "IoPorts::is_default")]
    pub io: IoPorts,
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
            field_height_mm: None,
            curve_tolerance_mm: None,
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
            motion_time_scale: 1.0,
            start_mark_input_bit: None,
            start_mark_active_low: false,
            stop_mark_input_bit: None,
            focus_light_port_bit: None,
            blanking: true,
            laser_kind: LaserKind::Fiber,
            fiber_kind: FiberKind::Mopa,
            calibration: lmc::Calibration::default(),
            red_dot: RedDot::default(),
            axis: None,
            io: IoPorts::default(),
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

    /// The generic JCZ galvo base (`fixtures/generic-jcz-galvo.toml`): conservative hard limits for an LMC galvo
    /// that is not a known machine, which a LightBurn device is imported onto ([`crate::device`]). See the file's
    /// comments for each value.
    pub fn generic_jcz_galvo() -> Self {
        Self::from_toml(GENERIC_JCZ_GALVO_TOML).expect("fixtures/generic-jcz-galvo.toml is a valid profile")
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

    /// The field size, swap, negate and lens correction as `lmc`'s shared field transform (the one `plan` uses
    /// too).
    pub fn field_transform(&self) -> lmc::FieldTransform {
        lmc::FieldTransform {
            field_mm: self.field_mm,
            field_height_mm: self.field_height_mm,
            swap_xy: self.swap_xy,
            negate_x: self.negate_x,
            negate_y: self.negate_y,
            calibration: self.calibration,
        }
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
        if let Some(h) = self.field_height_mm
            && !(h.is_finite() && h > 0.0)
        {
            return invalid("field_height_mm", format!("{h} must be a positive number of mm"));
        }
        if let Some(t) = self.curve_tolerance_mm
            && !(t.is_finite() && (0.001..=1.0).contains(&t))
        {
            return invalid("curve_tolerance_mm", format!("{t} must be within 0.001..=1 mm"));
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
        let (lo, hi) = (*units::MOTION_TIME_SCALE_RANGE.start(), *units::MOTION_TIME_SCALE_RANGE.end());
        if !(self.motion_time_scale.is_finite() && (lo..=hi).contains(&self.motion_time_scale)) {
            return invalid("motion_time_scale", format!("{} must be {lo}..={hi}", self.motion_time_scale));
        }
        if let Some(bit) = self.start_mark_input_bit.filter(|b| *b > 15) {
            return invalid("start_mark_input_bit", format!("{bit} is not an input bit 0-15"));
        }
        if let Some(bit) = self.stop_mark_input_bit.filter(|b| *b > 15) {
            return invalid("stop_mark_input_bit", format!("{bit} is not an input bit 0-15"));
        }
        if let Some(bit) = self.focus_light_port_bit
            && (bit == 0 || bit > 15 || bit == self.laser_enable_port_bit || bit == self.red_light_port_bit)
        {
            return invalid(
                "focus_light_port_bit",
                format!("{bit} must be an output bit 1-15 other than the laser-enable and red-pointer bits"),
            );
        }
        if let Some(p) = self.calibration.problem(self.field_mm) {
            return invalid("calibration", p);
        }
        if let Some(p) = self.red_dot.correction.problem(self.field_mm) {
            return invalid("red_dot.correction", p);
        }
        let d = &self.red_dot;
        let eighth = self.field_mm / 8.0;
        if [d.offset_x_mm, d.offset_y_mm, d.scale_x, d.scale_y, d.rest_x_mm, d.rest_y_mm].iter().any(|v| !v.is_finite())
            || d.offset_x_mm.abs() > eighth
            || d.offset_y_mm.abs() > eighth
            || !(0.5..=2.0).contains(&d.scale_x)
            || !(0.5..=2.0).contains(&d.scale_y)
            || d.rest_x_mm.abs() >= self.half_field_mm()
            || d.rest_y_mm.abs() >= self.field_height_mm.unwrap_or(self.field_mm) / 2.0
        {
            return invalid(
                "red_dot",
                format!("offsets within ±{eighth} mm, scales 0.5..=2 and a rest position inside the field are needed ({d:?})"),
            );
        }
        if let Some(p) = self.axis.as_ref().and_then(ExtensionAxis::problem) {
            return invalid("axis", p);
        }
        if let Some(p) = self.io.problem(self.laser_enable_port_bit, self.red_light_port_bit, self.focus_light_port_bit) {
            return invalid("io", p);
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
        // Swapped since 2026-11-01 (the owner's red frame came out transposed without it).
        assert!(p.swap_xy && !p.negate_x && !p.negate_y);
        assert_eq!(p.motion_time_scale, 1.30);
        assert_eq!(p.cor_file.as_deref(), Some(Path::new("jcz150.cor")));
        assert_eq!(p.pulse_widths_ns, units::JPT_M7_PULSE_WIDTHS_NS.to_vec());
        assert_eq!((p.frequency_khz_min, p.frequency_khz_max), (1.0, 4000.0));
        assert_eq!((p.red_light_port_bit, p.laser_enable_port_bit), (8, 0));
        assert_eq!(p.red_light_speed_mm_s, 3000.0);
        assert_eq!((p.timing.laser_on_delay_us, p.timing.laser_off_delay_us, p.timing.polygon_delay_us), (-300.0, 100.0, 100.0));
        assert_eq!((p.mark_speed.min_mm_s, p.mark_speed.max_mm_s), (1.0, 10000.0));
        assert_eq!((p.max_jump_speed_mm_s, p.max_delay_us), (15000.0, 2000.0));
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
        assert_eq!((imported.negate_x, imported.negate_y), (fixture.negate_x, fixture.negate_y));
        assert_eq!(imported.swap_xy, fixture.swap_xy);
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
        refused(|p| p.stop_mark_input_bit = Some(16));
        refused(|p| p.focus_light_port_bit = Some(0));
        refused(|p| p.focus_light_port_bit = Some(8));
        refused(|p| p.calibration.scale_x = 3.0);
        refused(|p| p.red_dot.scale_y = 0.1);
        refused(|p| p.red_dot.rest_x_mm = 500.0);
        refused(|p| {
            p.axis = Some(ExtensionAxis { role: AxisRole::Z, steps_per_mm: 0.0, min_mm: 0.0, max_mm: 10.0, invert: false })
        });
        refused(|p| {
            p.axis = Some(ExtensionAxis { role: AxisRole::Z, steps_per_mm: 10000.0, min_mm: 0.0, max_mm: 10.0, invert: false })
        });
    }

    #[test]
    fn generic_jcz_galvo_is_conservative() {
        let g = MachineProfile::generic_jcz_galvo();
        let b6 = MachineProfile::from_toml(B6).unwrap();
        assert_eq!(MachineProfile::from_toml(&g.to_toml().unwrap()).unwrap(), g);
        assert!(g.cor_file.is_none() && !g.swap_xy && !g.negate_x && !g.negate_y && !g.pulse_width_enabled);
        assert!(g.max_jump_speed_mm_s <= b6.max_jump_speed_mm_s && g.max_delay_us <= b6.max_delay_us);
        assert!(g.mark_speed.max_mm_s <= b6.mark_speed.max_mm_s && g.frequency_khz_min >= b6.frequency_khz_min);
        assert_eq!(g.start_mark_input_bit, None);
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

    #[test]
    fn machine_features_round_trip_and_default() {
        let b6 = MachineProfile::from_toml(B6).unwrap();
        assert!(b6.blanking && b6.calibration.is_neutral() && b6.red_dot.is_neutral() && b6.axis.is_none());
        assert_eq!(
            (b6.laser_kind, b6.fiber_kind, b6.focus_light_port_bit, b6.stop_mark_input_bit),
            (LaserKind::Fiber, FiberKind::Mopa, None, None)
        );
        assert!(!b6.to_toml().unwrap().contains("calibration"), "neutral values are left out");
        let mut p = b6.clone();
        p.calibration.angle_deg = 0.5;
        p.calibration.scale_x = 1.01;
        p.red_dot = RedDot { offset_x_mm: 0.4, rest_y_mm: -60.0, ..RedDot::default() };
        p.focus_light_port_bit = Some(9);
        p.stop_mark_input_bit = Some(13);
        p.blanking = false;
        p.fiber_kind = FiberKind::QSwitched;
        p.axis = Some(ExtensionAxis { role: AxisRole::Z, steps_per_mm: 400.0, min_mm: 0.0, max_mm: 100.0, invert: true });
        p.validate().unwrap();
        let again = MachineProfile::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(again, p);
        assert_eq!(p.field_transform().calibration, p.calibration);
        let axis = p.axis.as_ref().unwrap();
        assert_eq!(axis.z_steps(2.5), Ok(1000));
        assert!(axis.z_steps(120.0).is_err() && axis.z_steps(-1.0).is_err());
        assert_eq!(p.red_dot.apply(10.0, 10.0, p.field_mm), (10.4, 10.0));
    }

    #[test]
    fn red_dot_correction_bends_the_frame_only() {
        let mut p = MachineProfile::from_toml(B6).unwrap();
        // A trapezoid on the red light only: wider towards +Y, the burn's calibration untouched.
        p.red_dot.correction.trapezoid_x = 1.02;
        p.validate().unwrap();
        let (back, _) = p.red_dot.apply(50.0, 60.0, p.field_mm);
        let (front, _) = p.red_dot.apply(50.0, -60.0, p.field_mm);
        assert!(back > 50.0 && front < 50.0, "{back} {front}");
        assert!(p.calibration.is_neutral() || p.calibration == MachineProfile::from_toml(B6).unwrap().calibration);
        let again = MachineProfile::from_toml(&p.to_toml().unwrap()).unwrap();
        assert_eq!(again.red_dot, p.red_dot);
        p.red_dot.correction.trapezoid_x = 1.5;
        assert!(p.validate().is_err(), "a red-dot correction out of range is refused like the field calibration");
        // Neutral by default: an existing profile's frame is unchanged.
        assert_eq!(RedDot::default().apply(12.5, -3.0, 153.75), (12.5, -3.0));
    }
}
