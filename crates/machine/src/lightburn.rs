//! LightBurn device settings: the device list from LightBurn's `prefs.ini` ([`parse_devices`]), device lookup
//! ([`find_device`]) and a [`MachineProfile`] made from a device ([`profile_from_lightburn`]). Pure: callers read the
//! file ([`default_prefs_paths`] says where LightBurn keeps it).
//!
//! **Privacy.** `prefs.ini` is one JSON object holding all of LightBurn's settings, including unrelated ones and
//! secrets (e.g. `RestPendantSecret`, license-adjacent data). [`parse_devices`] reads only `DeviceList` and
//! `DefaultDevice`; inside each device only the keys listed on [`LightBurnDevice`] and [`LightBurnSettings`] are
//! deserialised (serde skips every other key without storing it), the correction file is cut down to its file name
//! (the full path names the user's folders), and error messages give only a line and column, never file content.
//! Nothing here writes to `prefs.ini`.
//!
//! **Units** (as LightBurn shows them for a JCZ galvo): mm, mm/s, µs for every delay and `Laser_OpenMODelay`, kHz for
//! `Laser_MinFreq`/`Laser_MaxFreq`. LightBurn stores floats as `f32`, so `153.8` arrives as `153.8000030517578`; such
//! values are read back as the `f32` LightBurn meant (153.8).
//!
//! **Mapping onto a profile** ([`profile_from_lightburn`]), every change reported as a [`ProfileDiff`]:
//!
//! | LightBurn | Profile field | Note |
//! |---|---|---|
//! | `Width`, `Height` | `field_mm`, `field_height_mm` | A difference of at most 0.05 mm from the base keeps the base: LightBurn shows the field to 0.1 mm, so 153.8 is EzCad's 153.75. A height more than 0.05 mm from the width is a non-square field (`field_height_mm`); otherwise the field is square. |
//! | `Galvo_1_is_X` | `swap_xy = !Galvo_1_is_X` | Verified on the owner's B6 (2026-11-01), see below. |
//! | `Galvo_1_Sign`, `Galvo_2_Sign`, `MirrorX`, `MirrorY` | `negate_x`, `negate_y` | Sign -1 of the galvo carrying the axis, XOR the mirror. **UNVERIFIED.** |
//! | `CorrectionFile` | `cor_file` (file name only) | Missing or empty keeps the base's file. |
//! | `Default_LaserOn_TC`, `_LaserOff_TC`, `_Polygon_TC`, `_End_TC` | `timing.*_delay_us` | Clamped to `max_delay_us`. |
//! | `Default_JumpSpeed` | `timing.jump_speed_mm_s` | Clamped to `max_jump_speed_mm_s`. |
//! | `Default_MinJumpDelay`, `_MaxJumpDelay`, `_MaxJumpDistance` | `timing.jump_delay_min_us`, `_max_us`, `jump_distance_limit_mm` | |
//! | `MaxSpeed` | `mark_speed.max_mm_s` | Lowers the hard limit, never raises it. |
//! | `Laser_MinFreq`, `Laser_MaxFreq` | `frequency_khz_min`, `_max` | Narrows the range, never widens it. |
//! | `Laser_QPulseWidthEnabled` | `pulse_width_enabled` | |
//! | `Laser_OpenMODelay` | `open_mo_delay_us` | |
//! | `StartMarkPortBit` | `start_mark_input_bit` (-1 = none) | |
//! | `StartMarkBitHigh` | `start_mark_active_low` (= not the flag) | false: the pedal is pressed while the input reads low. Written back from the profile. The owner's B6 has `true`, yet its foot switch reads low while pressed (measured 2026-11-01; LightBurn's `StartMarkPulseMode = true` may make the level moot there), so an import onto the B6 profile reports this difference. |
//! | `FrameSpeed` | `red_light_speed_mm_s` | Clamped to `max_jump_speed_mm_s`. |
//! | `RedLightPortBit` | `red_light_port_bit` | -1 is LightBurn's default; any other bit than 8 is reported and not applied (`lmc` drives bit 8). |
//! | `FieldOffsetX/Y`, `FieldAngle` | `calibration.offset_*_mm`, `calibration.angle_deg` | |
//! | `Galvo_n_Scale`, `_Bulge`, `_Skew`, `_Trapezoid` | `calibration.{scale,bulge,skew,trapezoid}_{x,y}` | Galvo 1 is field X unless swapped (as for the signs). OpenEtch's correction model (`lmc::calibration`) is its own: same neutral values and direction, exact effect **unverified against LightBurn**. Values outside its accepted range are reported and not applied. |
//!
//! Hard limits (`max_power_pct`, `max_jump_speed_mm_s`, `max_delay_us`, the pulse-width table) are never raised by an
//! import, and every change is checked with [`MachineProfile::validate`] one field at a time: a value it refuses is
//! not applied and is reported, so the result always validates (given a valid base).
//!
//! **Axis swap: LightBurn and EzCad on the B6.** The owner's B6 runs correctly from both programs with LightBurn's
//! `Galvo_1_is_X = false` and EzCad's `markcfg7` `GALVOX = 1`, and OpenEtch needs `swap_xy = true` on it: with
//! `swap_xy = false` a red-light frame of an "F" came out mirrored across the diagonal (2026-11-01). So this import
//! maps `swap_xy = !Galvo_1_is_X` and [`MachineProfile::from_markcfg`] maps `GALVOX = 1` to `swap_xy = true`.
//! Unswapped, OpenEtch's field X is LightBurn's galvo 1, so `negate_x` follows `Galvo_1_Sign` (and `Galvo_2_Sign`
//! when swapped). The signs are 1 on the B6, so their assignment is still **UNVERIFIED on hardware**.
//!
//! Not modelled (reported when not at their defaults): `CorScaleOverride`, `Laser_PWMEnabled`,
//! `StartMarkPulseMode`, `RedLightJumpDelay`. Written from scratch from the owner's own prefs files.
//!
//! **Export** ([`to_lbdev`]): a device back as a LightBurn `.lbdev` (`{"DeviceList": [...]}`) with the same keys,
//! from the profile's current values (so device settings edited in OpenEtch go back to LightBurn).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::profile::{MachineProfile, ProfileError};

/// A `prefs.ini` that could not be read. Messages never contain file content.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LightBurnError {
    #[error("LightBurn prefs: {0}")]
    Prefs(String),
}

/// The devices of one LightBurn `prefs.ini` (or the copy the engine stores after an upload).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightBurnPrefs {
    /// Index into `devices` of LightBurn's default device (`DefaultDevice`), when it names one.
    #[serde(default)]
    pub default_index: Option<usize>,
    #[serde(default)]
    pub devices: Vec<LightBurnDevice>,
    /// Devices skipped (no name). Key names only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl LightBurnPrefs {
    /// LightBurn's default device.
    pub fn default_device(&self) -> Option<&LightBurnDevice> {
        self.default_index.and_then(|i| self.devices.get(i))
    }
}

/// One LightBurn device (`DeviceList[i]`), reduced to what OpenEtch uses.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightBurnDevice {
    /// `Name`, e.g. "JCZFiber".
    pub name: String,
    /// `DisplayName`, e.g. "JCZFiber (Unknown)"; LightBurn 2.x writes it as the `DeviceName` of `.lbrn2` files.
    #[serde(default)]
    pub display_name: String,
    /// `Type`, e.g. "LibUSB".
    #[serde(default)]
    pub kind: String,
    /// `Width`: the galvo field, mm.
    #[serde(default)]
    pub width_mm: Option<f64>,
    /// `Height`, mm.
    #[serde(default)]
    pub height_mm: Option<f64>,
    /// `MirrorX`.
    #[serde(default)]
    pub mirror_x: Option<bool>,
    /// `MirrorY`.
    #[serde(default)]
    pub mirror_y: Option<bool>,
    /// File name of `Settings.CorrectionFile` (the folders are dropped).
    #[serde(default)]
    pub correction_file: Option<String>,
    /// `HomeOnStartup`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_on_startup: Option<bool>,
    /// File name of `LastDevLibraryPath`, the material library linked to the device (the folders are dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_file: Option<String>,
    #[serde(default)]
    pub settings: LightBurnSettings,
    /// Known keys whose value had the wrong type (and were ignored). Key names only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl LightBurnDevice {
    /// The galvo field size LightBurn lays files out for: the device width.
    pub fn field_mm(&self) -> Option<f64> {
        self.width_mm
    }

    /// Whether `query` names this device (`Name` or `DisplayName`, ignoring case and surrounding space).
    pub fn is_named(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        !q.is_empty() && (self.name.trim().to_lowercase() == q || self.display_name.trim().to_lowercase() == q)
    }
}

/// The typed subset of a device's `Settings` (LightBurn key in each doc line). `None`: missing or of the wrong type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightBurnSettings {
    /// `CorScaleOverride`.
    pub cor_scale_override: Option<bool>,
    /// `Default_LaserOn_TC`, µs.
    pub laser_on_tc_us: Option<f64>,
    /// `Default_LaserOff_TC`, µs.
    pub laser_off_tc_us: Option<f64>,
    /// `Default_Polygon_TC`, µs.
    pub polygon_tc_us: Option<f64>,
    /// `Default_End_TC`, µs.
    pub end_tc_us: Option<f64>,
    /// `Default_JumpSpeed`, mm/s.
    pub jump_speed_mm_s: Option<f64>,
    /// `Default_MinJumpDelay`, µs.
    pub min_jump_delay_us: Option<f64>,
    /// `Default_MaxJumpDelay`, µs.
    pub max_jump_delay_us: Option<f64>,
    /// `Default_MaxJumpDistance`, mm.
    pub max_jump_distance_mm: Option<f64>,
    /// `FieldAngle`, degrees.
    pub field_angle_deg: Option<f64>,
    /// `FieldOffsetX`, mm.
    pub field_offset_x_mm: Option<f64>,
    /// `FieldOffsetY`, mm.
    pub field_offset_y_mm: Option<f64>,
    /// `FrameSpeed`, mm/s.
    pub frame_speed_mm_s: Option<f64>,
    /// `Galvo_1_is_X`.
    pub galvo_1_is_x: Option<bool>,
    /// `Galvo_1_Sign` (1 or -1).
    pub galvo_1_sign: Option<f64>,
    /// `Galvo_2_Sign` (1 or -1).
    pub galvo_2_sign: Option<f64>,
    /// `Galvo_1_Scale`, `Galvo_1_Skew`, `Galvo_1_Trapezoid`, `Galvo_1_Bulge` (1 = neutral).
    #[serde(with = "shape_serde")]
    pub galvo_1_shape: [Option<f64>; 4],
    /// `Galvo_2_Scale`, `Galvo_2_Skew`, `Galvo_2_Trapezoid`, `Galvo_2_Bulge` (1 = neutral).
    #[serde(with = "shape_serde")]
    pub galvo_2_shape: [Option<f64>; 4],
    /// `Laser_Type`.
    pub laser_type: Option<i64>,
    /// `Laser_FiberType`.
    pub laser_fiber_type: Option<i64>,
    /// `Laser_MinFreq`, kHz.
    pub min_freq_khz: Option<f64>,
    /// `Laser_MaxFreq`, kHz.
    pub max_freq_khz: Option<f64>,
    /// `Laser_QPulseWidthEnabled` (MOPA pulse width control).
    pub q_pulse_width_enabled: Option<bool>,
    /// `Laser_PWMEnabled`.
    pub pwm_enabled: Option<bool>,
    /// `Laser_OpenMODelay`, µs.
    pub open_mo_delay_us: Option<f64>,
    /// `MaxSpeed`, mm/s.
    pub max_speed_mm_s: Option<f64>,
    /// `RedLightPortBit` (-1 = LightBurn's default).
    pub red_light_port_bit: Option<i64>,
    /// `RedLightJumpDelay`, µs.
    pub red_light_jump_delay_us: Option<f64>,
    /// `StartMarkPortBit` (-1 = none).
    pub start_mark_port_bit: Option<i64>,
    /// `StartMarkBitHigh`.
    pub start_mark_bit_high: Option<bool>,
    /// `StartMarkPulseMode`.
    pub start_mark_pulse_mode: Option<bool>,
    // Every family (Device Settings shown read-only; OpenEtch does not use them).
    /// `CutOrigin` (LightBurn's job origin selector).
    pub cut_origin: Option<i64>,
    /// `EnableBoundsCheck`.
    pub bounds_check: Option<bool>,
    /// `Sim_MaxSpeedX`, mm/s (LightBurn's preview timing).
    pub preview_max_speed_x_mm_s: Option<f64>,
    /// `Sim_MaxSpeedY`, mm/s.
    pub preview_max_speed_y_mm_s: Option<f64>,
    /// `Sim_RapidSpeed`, mm/s.
    pub preview_rapid_speed_mm_s: Option<f64>,
    /// `Sim_MaxAccelX`, mm/s².
    pub preview_max_accel_x: Option<f64>,
    /// `Sim_MaxAccelY`, mm/s².
    pub preview_max_accel_y: Option<f64>,
    // GCode family.
    /// `BaudRate`.
    pub baud_rate: Option<i64>,
    /// `NetworkPort`.
    pub network_port: Option<i64>,
    /// `S_Scale` (the S value of 100 % power).
    pub s_value_max: Option<f64>,
    /// `AirAssistM7`.
    pub air_assist_m7: Option<bool>,
    /// `EnableZ`.
    pub enable_z: Option<bool>,
    /// `EnableU`.
    pub enable_u: Option<bool>,
    /// `TransferMode`.
    pub transfer_mode: Option<i64>,
    /// `GCodeFlavor` (Custom GCode, e.g. "smoothie"), at most 40 characters.
    pub gcode_flavor: Option<String>,
}

impl LightBurnSettings {
    /// Whether any galvo setting (`Galvo_*`, `CorrectionFile` aside) is present.
    pub fn has_galvo_keys(&self) -> bool {
        self.galvo_1_is_x.is_some()
            || self.galvo_1_sign.is_some()
            || self.galvo_2_sign.is_some()
            || self.galvo_1_shape.iter().chain(&self.galvo_2_shape).any(Option::is_some)
    }
}

/// The galvo shape arrays as numbers, a missing value as NaN (TOML has no null; JSON writes NaN as null), read back
/// from numbers, NaN or null.
mod shape_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[Option<f64>; 4], s: S) -> Result<S::Ok, S::Error> {
        let numbers: [f64; 4] = v.map(|x| x.unwrap_or(f64::NAN));
        serde::Serialize::serialize(&numbers, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[Option<f64>; 4], D::Error> {
        let v: [Option<f64>; 4] = Deserialize::deserialize(d)?;
        Ok(v.map(|x| x.filter(|f| !f.is_nan())))
    }
}

// --- prefs.ini reading -------------------------------------------------------------------------------------------

/// The two top-level keys read; serde skips every other key of the file without keeping it.
#[derive(Deserialize)]
struct RawPrefs {
    #[serde(rename = "DeviceList", default)]
    device_list: Option<Vec<RawDevice>>,
    #[serde(rename = "DefaultDevice", default)]
    default_device: Option<Value>,
}

/// The keys of a device that are read (each holds only its own value).
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawDevice {
    #[serde(rename = "Name")]
    name: Option<Value>,
    #[serde(rename = "DisplayName")]
    display_name: Option<Value>,
    #[serde(rename = "Type")]
    kind: Option<Value>,
    #[serde(rename = "Width")]
    width: Option<Value>,
    #[serde(rename = "Height")]
    height: Option<Value>,
    #[serde(rename = "MirrorX")]
    mirror_x: Option<Value>,
    #[serde(rename = "MirrorY")]
    mirror_y: Option<Value>,
    #[serde(rename = "HomeOnStartup")]
    home_on_startup: Option<Value>,
    #[serde(rename = "LastDevLibraryPath")]
    library_path: Option<Value>,
    #[serde(rename = "Settings")]
    settings: Option<RawSettings>,
}

/// Declares [`RawSettings`] (one `Option<Value>` per LightBurn key read from `Settings`).
macro_rules! raw_settings {
    ($($field:ident = $key:literal),* $(,)?) => {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct RawSettings {
            $(#[serde(rename = $key)] $field: Option<Value>,)*
        }
    };
}

raw_settings! {
    correction_file = "CorrectionFile",
    cor_scale_override = "CorScaleOverride",
    laser_on = "Default_LaserOn_TC",
    laser_off = "Default_LaserOff_TC",
    polygon = "Default_Polygon_TC",
    end = "Default_End_TC",
    jump_speed = "Default_JumpSpeed",
    min_jump_delay = "Default_MinJumpDelay",
    max_jump_delay = "Default_MaxJumpDelay",
    max_jump_distance = "Default_MaxJumpDistance",
    field_angle = "FieldAngle",
    field_offset_x = "FieldOffsetX",
    field_offset_y = "FieldOffsetY",
    frame_speed = "FrameSpeed",
    galvo_1_is_x = "Galvo_1_is_X",
    galvo_1_sign = "Galvo_1_Sign",
    galvo_2_sign = "Galvo_2_Sign",
    galvo_1_scale = "Galvo_1_Scale",
    galvo_1_skew = "Galvo_1_Skew",
    galvo_1_trapezoid = "Galvo_1_Trapezoid",
    galvo_1_bulge = "Galvo_1_Bulge",
    galvo_2_scale = "Galvo_2_Scale",
    galvo_2_skew = "Galvo_2_Skew",
    galvo_2_trapezoid = "Galvo_2_Trapezoid",
    galvo_2_bulge = "Galvo_2_Bulge",
    laser_type = "Laser_Type",
    laser_fiber_type = "Laser_FiberType",
    min_freq = "Laser_MinFreq",
    max_freq = "Laser_MaxFreq",
    q_pulse_width_enabled = "Laser_QPulseWidthEnabled",
    pwm_enabled = "Laser_PWMEnabled",
    open_mo_delay = "Laser_OpenMODelay",
    max_speed = "MaxSpeed",
    red_light_port_bit = "RedLightPortBit",
    red_light_jump_delay = "RedLightJumpDelay",
    start_mark_port_bit = "StartMarkPortBit",
    start_mark_bit_high = "StartMarkBitHigh",
    start_mark_pulse_mode = "StartMarkPulseMode",
    cut_origin = "CutOrigin",
    bounds_check = "EnableBoundsCheck",
    sim_max_speed_x = "Sim_MaxSpeedX",
    sim_max_speed_y = "Sim_MaxSpeedY",
    sim_rapid_speed = "Sim_RapidSpeed",
    sim_max_accel_x = "Sim_MaxAccelX",
    sim_max_accel_y = "Sim_MaxAccelY",
    baud_rate = "BaudRate",
    network_port = "NetworkPort",
    s_scale = "S_Scale",
    air_assist_m7 = "AirAssistM7",
    enable_z = "EnableZ",
    enable_u = "EnableU",
    transfer_mode = "TransferMode",
    gcode_flavor = "GCodeFlavor",
}

/// A LightBurn float as the `f32` LightBurn stored (`153.8000030517578` -> `153.8`); other values unchanged.
fn clean(v: f64) -> f64 {
    let f = v as f32;
    if f.is_finite() && f64::from(f) == v { f.to_string().parse().unwrap_or(v) } else { v }
}

/// Typed reads that record wrongly typed keys (by name) instead of failing.
struct Reader<'a> {
    warnings: &'a mut Vec<String>,
}

impl Reader<'_> {
    fn wrong(&mut self, key: &str, want: &str) {
        self.warnings.push(format!("{key} is not {want}; ignored"));
    }

    fn num(&mut self, key: &str, v: &Option<Value>) -> Option<f64> {
        match v {
            None | Some(Value::Null) => None,
            Some(v) => match v.as_f64().filter(|f| f.is_finite()) {
                Some(f) => Some(clean(f)),
                None => {
                    self.wrong(key, "a number");
                    None
                }
            },
        }
    }

    fn int(&mut self, key: &str, v: &Option<Value>) -> Option<i64> {
        match v {
            None | Some(Value::Null) => None,
            Some(v) => match v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e15).map(|f| f as i64)) {
                Some(i) => Some(i),
                None => {
                    self.wrong(key, "a whole number");
                    None
                }
            },
        }
    }

    fn flag(&mut self, key: &str, v: &Option<Value>) -> Option<bool> {
        match v {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(v) if v.as_i64() == Some(0) => Some(false),
            Some(v) if v.as_i64() == Some(1) => Some(true),
            Some(_) => {
                self.wrong(key, "true or false");
                None
            }
        }
    }

    fn text(&mut self, key: &str, v: &Option<Value>) -> Option<String> {
        match v {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                self.wrong(key, "text");
                None
            }
        }
    }
}

/// The file name of a Windows or POSIX path (`C:/a/b/jcz150.cor` -> `jcz150.cor`), `None` when empty.
fn file_name(path: &str) -> Option<String> {
    let name = path.rsplit(['\\', '/']).next().unwrap_or("").trim();
    (!name.is_empty()).then(|| name.to_owned())
}

fn device(raw: RawDevice) -> Option<LightBurnDevice> {
    let mut warnings = Vec::new();
    let mut r = Reader { warnings: &mut warnings };
    let name = r.text("Name", &raw.name).map(|n| n.trim().to_owned()).filter(|n| !n.is_empty())?;
    let display_name = r.text("DisplayName", &raw.display_name).unwrap_or_default();
    let kind = r.text("Type", &raw.kind).unwrap_or_default();
    let width_mm = r.num("Width", &raw.width);
    let height_mm = r.num("Height", &raw.height);
    let mirror_x = r.flag("MirrorX", &raw.mirror_x);
    let mirror_y = r.flag("MirrorY", &raw.mirror_y);
    let home_on_startup = r.flag("HomeOnStartup", &raw.home_on_startup);
    let library_file = r.text("LastDevLibraryPath", &raw.library_path).and_then(|p| file_name(&p));
    let s = raw.settings.unwrap_or_default();
    let correction_file = r.text("Settings.CorrectionFile", &s.correction_file).and_then(|p| file_name(&p));
    let settings = LightBurnSettings {
        cor_scale_override: r.flag("Settings.CorScaleOverride", &s.cor_scale_override),
        laser_on_tc_us: r.num("Settings.Default_LaserOn_TC", &s.laser_on),
        laser_off_tc_us: r.num("Settings.Default_LaserOff_TC", &s.laser_off),
        polygon_tc_us: r.num("Settings.Default_Polygon_TC", &s.polygon),
        end_tc_us: r.num("Settings.Default_End_TC", &s.end),
        jump_speed_mm_s: r.num("Settings.Default_JumpSpeed", &s.jump_speed),
        min_jump_delay_us: r.num("Settings.Default_MinJumpDelay", &s.min_jump_delay),
        max_jump_delay_us: r.num("Settings.Default_MaxJumpDelay", &s.max_jump_delay),
        max_jump_distance_mm: r.num("Settings.Default_MaxJumpDistance", &s.max_jump_distance),
        field_angle_deg: r.num("Settings.FieldAngle", &s.field_angle),
        field_offset_x_mm: r.num("Settings.FieldOffsetX", &s.field_offset_x),
        field_offset_y_mm: r.num("Settings.FieldOffsetY", &s.field_offset_y),
        frame_speed_mm_s: r.num("Settings.FrameSpeed", &s.frame_speed),
        galvo_1_is_x: r.flag("Settings.Galvo_1_is_X", &s.galvo_1_is_x),
        galvo_1_sign: r.num("Settings.Galvo_1_Sign", &s.galvo_1_sign),
        galvo_2_sign: r.num("Settings.Galvo_2_Sign", &s.galvo_2_sign),
        galvo_1_shape: [
            r.num("Settings.Galvo_1_Scale", &s.galvo_1_scale),
            r.num("Settings.Galvo_1_Skew", &s.galvo_1_skew),
            r.num("Settings.Galvo_1_Trapezoid", &s.galvo_1_trapezoid),
            r.num("Settings.Galvo_1_Bulge", &s.galvo_1_bulge),
        ],
        galvo_2_shape: [
            r.num("Settings.Galvo_2_Scale", &s.galvo_2_scale),
            r.num("Settings.Galvo_2_Skew", &s.galvo_2_skew),
            r.num("Settings.Galvo_2_Trapezoid", &s.galvo_2_trapezoid),
            r.num("Settings.Galvo_2_Bulge", &s.galvo_2_bulge),
        ],
        laser_type: r.int("Settings.Laser_Type", &s.laser_type),
        laser_fiber_type: r.int("Settings.Laser_FiberType", &s.laser_fiber_type),
        min_freq_khz: r.num("Settings.Laser_MinFreq", &s.min_freq),
        max_freq_khz: r.num("Settings.Laser_MaxFreq", &s.max_freq),
        q_pulse_width_enabled: r.flag("Settings.Laser_QPulseWidthEnabled", &s.q_pulse_width_enabled),
        pwm_enabled: r.flag("Settings.Laser_PWMEnabled", &s.pwm_enabled),
        open_mo_delay_us: r.num("Settings.Laser_OpenMODelay", &s.open_mo_delay),
        max_speed_mm_s: r.num("Settings.MaxSpeed", &s.max_speed),
        red_light_port_bit: r.int("Settings.RedLightPortBit", &s.red_light_port_bit),
        red_light_jump_delay_us: r.num("Settings.RedLightJumpDelay", &s.red_light_jump_delay),
        start_mark_port_bit: r.int("Settings.StartMarkPortBit", &s.start_mark_port_bit),
        start_mark_bit_high: r.flag("Settings.StartMarkBitHigh", &s.start_mark_bit_high),
        start_mark_pulse_mode: r.flag("Settings.StartMarkPulseMode", &s.start_mark_pulse_mode),
        cut_origin: r.int("Settings.CutOrigin", &s.cut_origin),
        bounds_check: r.flag("Settings.EnableBoundsCheck", &s.bounds_check),
        preview_max_speed_x_mm_s: r.num("Settings.Sim_MaxSpeedX", &s.sim_max_speed_x),
        preview_max_speed_y_mm_s: r.num("Settings.Sim_MaxSpeedY", &s.sim_max_speed_y),
        preview_rapid_speed_mm_s: r.num("Settings.Sim_RapidSpeed", &s.sim_rapid_speed),
        preview_max_accel_x: r.num("Settings.Sim_MaxAccelX", &s.sim_max_accel_x),
        preview_max_accel_y: r.num("Settings.Sim_MaxAccelY", &s.sim_max_accel_y),
        baud_rate: r.int("Settings.BaudRate", &s.baud_rate),
        network_port: r.int("Settings.NetworkPort", &s.network_port),
        s_value_max: r.num("Settings.S_Scale", &s.s_scale),
        air_assist_m7: r.flag("Settings.AirAssistM7", &s.air_assist_m7),
        enable_z: r.flag("Settings.EnableZ", &s.enable_z),
        enable_u: r.flag("Settings.EnableU", &s.enable_u),
        transfer_mode: r.int("Settings.TransferMode", &s.transfer_mode),
        gcode_flavor: r.text("Settings.GCodeFlavor", &s.gcode_flavor).map(|f| f.trim().chars().take(40).collect()),
    };
    Some(LightBurnDevice {
        name,
        display_name,
        kind,
        width_mm,
        height_mm,
        mirror_x,
        mirror_y,
        correction_file,
        home_on_startup,
        library_file,
        settings,
        warnings,
    })
}

/// A JSON error as a message without file content (serde's data errors quote values).
fn json_error(e: &serde_json::Error) -> LightBurnError {
    use serde_json::error::Category;
    let what = match e.classify() {
        Category::Syntax | Category::Eof | Category::Io => "is not valid JSON",
        Category::Data => "has a DeviceList or DefaultDevice of an unexpected shape",
    };
    LightBurnError::Prefs(format!("the file {what} (line {}, column {})", e.line(), e.column()))
}

/// Read the device list of a LightBurn `prefs.ini` (JSON). Only `DeviceList` and `DefaultDevice` are read, see the
/// module docs. A file without `DeviceList` has no devices; devices without a `Name` are skipped (with a warning).
pub fn parse_devices(json: &str) -> Result<LightBurnPrefs, LightBurnError> {
    let raw: RawPrefs = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| json_error(&e))?;
    let mut prefs = LightBurnPrefs::default();
    let default = raw.default_device.as_ref().and_then(Value::as_u64);
    for (i, d) in raw.device_list.unwrap_or_default().into_iter().enumerate() {
        match device(d) {
            Some(dev) => {
                if default == Some(i as u64) {
                    prefs.default_index = Some(prefs.devices.len());
                }
                prefs.devices.push(dev);
            }
            None => prefs.warnings.push(format!("DeviceList[{i}] has no Name; skipped")),
        }
    }
    Ok(prefs)
}

/// Read a LightBurn `.lbdev` device file (LightBurn 1.7's Devices > Export): plain JSON of the same shape as
/// `prefs.ini`'s device list, `{"DeviceList": [...]}`, without `DefaultDevice`. Read with the same typed whitelist as
/// [`parse_devices`] (macros, G-code, addresses, paths and every other key are skipped). A file without `DeviceList`
/// is refused.
pub fn parse_lbdev(json: &str) -> Result<LightBurnPrefs, LightBurnError> {
    let raw: RawPrefs = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| json_error(&e))?;
    if raw.device_list.is_none() {
        return Err(LightBurnError::Prefs("the file has no DeviceList (expected a LightBurn .lbdev device file)".into()));
    }
    let mut prefs = parse_devices(json)?;
    prefs.default_index = None;
    Ok(prefs)
}

/// The device whose `Name` or `DisplayName` is `name_or_display_name`, ignoring case (the first when several match).
pub fn find_device<'a>(prefs: &'a LightBurnPrefs, name_or_display_name: &str) -> Option<&'a LightBurnDevice> {
    prefs.devices.iter().find(|d| d.is_named(name_or_display_name))
}

/// Where LightBurn keeps `prefs.ini` on `os` (`std::env::consts::OS`), most likely first; `env` reads environment
/// variables. Windows: `%LOCALAPPDATA%\LightBurn\prefs.ini`. macOS: `~/Library/Preferences/LightBurn/prefs.ini`, then
/// `~/Library/Application Support/LightBurn/prefs.ini` (unverified). Linux and others: `~/.local/share/LightBurn`
/// (`$XDG_DATA_HOME`; LightBurn staff on the forum, and Qt's app-data location), then `~/.config/LightBurn`
/// (`$XDG_CONFIG_HOME`): both **unverified**, LightBurn's documentation does not say (File > Open Prefs Folder in
/// LightBurn shows the real one).
pub fn default_prefs_paths(os: &str, env: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let var = |k: &str| env(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let file = |dir: PathBuf| dir.join("LightBurn").join("prefs.ini");
    match os {
        "windows" => var("LOCALAPPDATA").map(file).into_iter().collect(),
        "macos" => match var("HOME") {
            Some(home) => {
                let lib = home.join("Library");
                vec![file(lib.join("Preferences")), file(lib.join("Application Support"))]
            }
            None => Vec::new(),
        },
        _ => {
            let home = var("HOME");
            let data = var("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local").join("share")));
            let config = var("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| h.join(".config")));
            data.into_iter().chain(config).map(file).collect()
        }
    }
}

// --- profile import ----------------------------------------------------------------------------------------------

/// One difference between a base profile and what a LightBurn device says, or a LightBurn setting that was not
/// applied. `base_value` is the base profile's value, `lightburn_value` LightBurn's (in profile units); `note` says
/// what was done when it is not simply "taken from LightBurn" (kept, clamped, not modelled, unverified).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfileDiff {
    pub field: String,
    pub base_value: Value,
    pub lightburn_value: Value,
    #[serde(default)]
    pub note: String,
}

/// Builds the imported profile one checked change at a time.
struct Import {
    p: MachineProfile,
    diffs: Vec<ProfileDiff>,
}

/// JSON for a diff value (non-finite numbers become null).
fn j<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

impl Import {
    fn diff(&mut self, field: &str, base: Value, lightburn: Value, note: impl Into<String>) {
        self.diffs.push(ProfileDiff { field: field.into(), base_value: base, lightburn_value: lightburn, note: note.into() });
    }

    /// Apply `f` when the result still validates; record a diff when the profile changed or there is a note. A
    /// refused change is not applied and is reported with the reason.
    fn set(&mut self, field: &str, base: Value, lightburn: Value, note: impl Into<String>, f: impl FnOnce(&mut MachineProfile)) {
        let note = note.into();
        let mut next = self.p.clone();
        f(&mut next);
        if next == self.p {
            if !note.is_empty() {
                self.diff(field, base, lightburn, note);
            }
            return;
        }
        match next.validate() {
            Ok(()) => {
                self.p = next;
                self.diff(field, base, lightburn, note);
            }
            Err(e) => {
                let reason = match e {
                    ProfileError::Invalid { reason, .. } => reason,
                    other => other.to_string(),
                };
                let note = if note.is_empty() { String::new() } else { format!("{note}; ") };
                self.diff(field, base, lightburn, format!("{note}not applied: {reason}"));
            }
        }
    }

    /// A delay of at most `max_delay_us` in magnitude: the value to apply and a note when clamped.
    fn delay(&self, us: f64) -> (f64, String) {
        let cap = self.p.max_delay_us;
        if us.abs() > cap {
            (cap.copysign(us), format!("clamped to max_delay_us ({cap} µs)"))
        } else {
            (us, String::new())
        }
    }

    /// A travel speed of at most `max_jump_speed_mm_s`: the value to apply and a note when clamped.
    fn travel(&self, mm_s: f64) -> (f64, String) {
        let cap = self.p.max_jump_speed_mm_s;
        if mm_s > cap {
            (cap, format!("clamped to max_jump_speed_mm_s ({cap} mm/s)"))
        } else {
            (mm_s, String::new())
        }
    }
}

/// How close (mm) a LightBurn field width may be to the base's and still be the same lens.
const SAME_FIELD_MM: f64 = 0.05 + 1e-9;

/// A profile for `dev` built on `base` (see the module docs for the mapping), and every difference from `base`.
/// The result always passes [`MachineProfile::validate`] when `base` does.
pub fn profile_from_lightburn(dev: &LightBurnDevice, base: &MachineProfile) -> (MachineProfile, Vec<ProfileDiff>) {
    let mut im = Import { p: base.clone(), diffs: Vec::new() };
    let s = &dev.settings;
    for w in &dev.warnings {
        im.diff("(LightBurn prefs)", Value::Null, Value::Null, w.clone());
    }

    // Field.
    if let Some(w) = dev.width_mm {
        let note = String::new();
        // A non-square field (LightBurn's Height differs from its Width by more than its 0.1 mm display) keeps its
        // own height; a square one clears a base's height.
        let height = dev.height_mm.filter(|h| h.is_finite() && *h > 0.0 && (h - w).abs() > SAME_FIELD_MM);
        let base_h = base.field_height_mm.map_or(Value::Null, j);
        let lb_h = height.map_or(Value::Null, j);
        im.set("field_height_mm", base_h, lb_h, "", |p| p.field_height_mm = height);
        if (w - base.field_mm).abs() <= SAME_FIELD_MM && w != base.field_mm {
            let keep = format!(
                "same lens: LightBurn shows the field to 0.1 mm, kept {} (e.g. EzCad's FIELDSIZE)",
                base.field_mm
            );
            let note = if note.is_empty() { keep } else { format!("{note}; {keep}") };
            im.diff("field_mm", j(base.field_mm), j(w), note);
        } else {
            im.set("field_mm", j(base.field_mm), j(w), note, |p| p.field_mm = w);
        }
    }

    // Axes. See the module docs: swap_xy = !Galvo_1_is_X (verified on the B6, matching from_markcfg's GALVOX = 1).
    const UNVERIFIED: &str = "UNVERIFIED on hardware";
    if let Some(g1x) = s.galvo_1_is_x {
        let swap = !g1x;
        let note = "swap_xy = !Galvo_1_is_X (Galvo_1_is_X false, like EzCad GALVOX=1, is the swap)".to_owned();
        let note = if swap != base.swap_xy { note } else { String::new() };
        im.set("swap_xy", j(base.swap_xy), j(swap), note, |p| p.swap_xy = swap);
    }
    let swapped = im.p.swap_xy;
    let (x_sign, y_sign) = if swapped { (s.galvo_2_sign, s.galvo_1_sign) } else { (s.galvo_1_sign, s.galvo_2_sign) };
    for (field, sign, mirror, base_v) in [
        ("negate_x", x_sign, dev.mirror_x, base.negate_x),
        ("negate_y", y_sign, dev.mirror_y, base.negate_y),
    ] {
        if sign.is_none() && mirror.is_none() {
            continue;
        }
        let negate = sign.is_some_and(|v| v < 0.0) ^ mirror.unwrap_or(false);
        let note = if negate != base_v { format!("from the galvo sign and Mirror setting; {UNVERIFIED}") } else { String::new() };
        im.set(field, j(base_v), j(negate), note, |p| {
            if field == "negate_x" {
                p.negate_x = negate;
            } else {
                p.negate_y = negate;
            }
        });
    }

    // Correction file (name only; it is looked up next to the profile file).
    match &dev.correction_file {
        Some(name) => {
            let cor = PathBuf::from(name);
            im.set("cor_file", j(&base.cor_file), j(name), "", |p| p.cor_file = Some(cor));
        }
        None => {
            if base.cor_file.is_some() {
                im.diff("cor_file", j(&base.cor_file), Value::Null, "LightBurn names no correction file; kept the profile's");
            }
        }
    }
    if s.cor_scale_override == Some(true) {
        im.diff("cor_file", Value::Null, j(true), "CorScaleOverride is on in LightBurn; not modelled");
    }

    // Timing.
    let t = base.timing.clone();
    if let Some(v) = s.laser_on_tc_us {
        let (v2, note) = im.delay(v);
        im.set("timing.laser_on_delay_us", j(t.laser_on_delay_us), j(v), note, |p| p.timing.laser_on_delay_us = v2);
    }
    if let Some(v) = s.laser_off_tc_us {
        let (v2, note) = im.delay(v);
        im.set("timing.laser_off_delay_us", j(t.laser_off_delay_us), j(v), note, |p| p.timing.laser_off_delay_us = v2);
    }
    if let Some(v) = s.polygon_tc_us {
        let (v2, note) = im.delay(v);
        im.set("timing.polygon_delay_us", j(t.polygon_delay_us), j(v), note, |p| p.timing.polygon_delay_us = v2);
    }
    if let Some(v) = s.end_tc_us {
        im.set("timing.end_delay_us", j(t.end_delay_us), j(v), "", |p| p.timing.end_delay_us = v);
    }
    if let Some(v) = s.jump_speed_mm_s {
        let (v2, note) = im.travel(v);
        im.set("timing.jump_speed_mm_s", j(t.jump_speed_mm_s), j(v), note, |p| p.timing.jump_speed_mm_s = v2);
    }
    // Both jump delays at once: applying one at a time could pass through min > max.
    if s.min_jump_delay_us.is_some() || s.max_jump_delay_us.is_some() {
        let (max, max_note) = im.delay(s.max_jump_delay_us.unwrap_or(t.jump_delay_max_us));
        let min = s.min_jump_delay_us.unwrap_or(t.jump_delay_min_us).clamp(0.0, max.max(0.0));
        let lb_min = s.min_jump_delay_us;
        if let Some(v) = s.max_jump_delay_us {
            im.set("timing.jump_delay_max_us", j(t.jump_delay_max_us), j(v), max_note, |p| {
                p.timing.jump_delay_max_us = max;
                if lb_min.is_some() {
                    p.timing.jump_delay_min_us = min;
                }
            });
        }
        if let Some(v) = lb_min {
            let note = if min != v { format!("clamped to {min} µs") } else { String::new() };
            if s.max_jump_delay_us.is_some() {
                // Applied together with the maximum above (or refused with it).
                let applied = im.p.timing.jump_delay_min_us == min;
                if applied && (min != t.jump_delay_min_us || !note.is_empty()) {
                    im.diff("timing.jump_delay_min_us", j(t.jump_delay_min_us), j(v), note);
                } else if !applied {
                    im.diff("timing.jump_delay_min_us", j(t.jump_delay_min_us), j(v), "not applied (see jump_delay_max_us)");
                }
            } else {
                im.set("timing.jump_delay_min_us", j(t.jump_delay_min_us), j(v), note, |p| p.timing.jump_delay_min_us = min);
            }
        }
    }
    if let Some(v) = s.max_jump_distance_mm {
        let note = if v == 0.0 && v != t.jump_distance_limit_mm { "0: every jump uses jump_delay_max_us" } else { "" };
        im.set("timing.jump_distance_limit_mm", j(t.jump_distance_limit_mm), j(v), note, |p| {
            p.timing.jump_distance_limit_mm = v
        });
    }

    // Hard limits: narrowed, never widened.
    if let Some(v) = s.max_speed_mm_s {
        let cap = base.mark_speed.max_mm_s;
        if v > cap {
            im.diff("mark_speed.max_mm_s", j(cap), j(v), "LightBurn allows more; the profile's hard limit stays");
        } else {
            im.set("mark_speed.max_mm_s", j(cap), j(v), "", |p| p.mark_speed.max_mm_s = v);
        }
    }
    if let Some(v) = s.min_freq_khz {
        let lo = base.frequency_khz_min;
        if v < lo {
            im.diff("frequency_khz_min", j(lo), j(v), "LightBurn allows a lower frequency; the profile's limit stays");
        } else {
            im.set("frequency_khz_min", j(lo), j(v), "", |p| p.frequency_khz_min = v);
        }
    }
    if let Some(v) = s.max_freq_khz {
        let hi = base.frequency_khz_max;
        if v > hi {
            im.diff("frequency_khz_max", j(hi), j(v), "LightBurn allows a higher frequency; the profile's limit stays");
        } else {
            im.set("frequency_khz_max", j(hi), j(v), "", |p| p.frequency_khz_max = v);
        }
    }

    // Laser source and inputs.
    if let Some(v) = s.q_pulse_width_enabled {
        im.set("pulse_width_enabled", j(base.pulse_width_enabled), j(v), "", |p| p.pulse_width_enabled = v);
    }
    if let Some(v) = s.open_mo_delay_us {
        im.set("open_mo_delay_us", j(base.open_mo_delay_us), j(v), "", |p| p.open_mo_delay_us = v);
    }
    if let Some(v) = s.start_mark_port_bit {
        let base_v = j(base.start_mark_input_bit);
        match v {
            -1 => im.set("start_mark_input_bit", base_v, j(v), "", |p| p.start_mark_input_bit = None),
            0..=15 => {
                let bit = v as u8;
                im.set("start_mark_input_bit", base_v, j(v), "", |p| p.start_mark_input_bit = Some(bit));
            }
            _ => im.diff("start_mark_input_bit", base_v, j(v), "not an input bit 0-15; not applied"),
        }
    }
    // The start input's level (LightBurn's IO Ports High/Low): false = active low, pressed while the input reads 0.
    if let Some(high) = s.start_mark_bit_high {
        im.set(
            "start_mark_active_low",
            j(base.start_mark_active_low),
            j(!high),
            if high { "" } else { "StartMarkBitHigh is false: the start input is active low (pressed = input low)" },
            |p| p.start_mark_active_low = !high,
        );
    }

    // Red pointer.
    if let Some(v) = s.frame_speed_mm_s {
        let (v2, note) = im.travel(v);
        im.set("red_light_speed_mm_s", j(base.red_light_speed_mm_s), j(v), note, |p| p.red_light_speed_mm_s = v2);
    }
    if let Some(bit) = s.red_light_port_bit
        && bit != -1
        && bit != i64::from(base.red_light_port_bit)
    {
        im.diff("red_light_port_bit", j(base.red_light_port_bit), j(bit), "unsupported: lmc drives the red pointer on bit 8; not applied");
    }

    // Lens corrections (see the module docs). Galvo 1 carries field X unless swapped.
    let (x_shape, y_shape) = if swapped { (s.galvo_2_shape, s.galvo_1_shape) } else { (s.galvo_1_shape, s.galvo_2_shape) };
    let cal = base.calibration;
    type Field = fn(&mut lmc::Calibration) -> &mut f64;
    let corrections: [(&str, &str, Option<f64>, f64, Field); 11] = [
        ("calibration.offset_x_mm", "FieldOffsetX", s.field_offset_x_mm, cal.offset_x_mm, |c| &mut c.offset_x_mm),
        ("calibration.offset_y_mm", "FieldOffsetY", s.field_offset_y_mm, cal.offset_y_mm, |c| &mut c.offset_y_mm),
        ("calibration.angle_deg", "FieldAngle", s.field_angle_deg, cal.angle_deg, |c| &mut c.angle_deg),
        ("calibration.scale_x", "Galvo_n_Scale", x_shape[0], cal.scale_x, |c| &mut c.scale_x),
        ("calibration.skew_x", "Galvo_n_Skew", x_shape[1], cal.skew_x, |c| &mut c.skew_x),
        ("calibration.trapezoid_x", "Galvo_n_Trapezoid", x_shape[2], cal.trapezoid_x, |c| &mut c.trapezoid_x),
        ("calibration.bulge_x", "Galvo_n_Bulge", x_shape[3], cal.bulge_x, |c| &mut c.bulge_x),
        ("calibration.scale_y", "Galvo_n_Scale", y_shape[0], cal.scale_y, |c| &mut c.scale_y),
        ("calibration.skew_y", "Galvo_n_Skew", y_shape[1], cal.skew_y, |c| &mut c.skew_y),
        ("calibration.trapezoid_y", "Galvo_n_Trapezoid", y_shape[2], cal.trapezoid_y, |c| &mut c.trapezoid_y),
        ("calibration.bulge_y", "Galvo_n_Bulge", y_shape[3], cal.bulge_y, |c| &mut c.bulge_y),
    ];
    for (field, key, v, base_v, at) in corrections {
        let Some(v) = v else { continue };
        let note = if v != base_v { format!("LightBurn {key}; OpenEtch's correction model, unverified against LightBurn") } else { String::new() };
        im.set(field, j(base_v), j(v), note, |p| *at(&mut p.calibration) = v);
    }
    if s.pwm_enabled == Some(false) {
        im.diff("(LightBurn Laser_PWMEnabled)", Value::Null, j(false), "not modelled by OpenEtch; ignored");
    }

    let Import { mut p, diffs } = im;
    let mut note = format!("imported from LightBurn device \"{}\"", dev.name);
    if let (Some(t), Some(f)) = (s.laser_type, s.laser_fiber_type) {
        note.push_str(&format!(" (Laser_Type {t}, Laser_FiberType {f})"));
    }
    p.notes.push(note);
    p.notes.push("LightBurn galvo signs / Mirror mapping onto negate is unverified (the swap is verified on the B6)".into());
    debug_assert!(base.validate().is_err() || p.validate().is_ok());
    (p, diffs)
}

/// A LightBurn `.lbdev` file (`{"DeviceList": [device]}`, pretty JSON) for a device: the typed LightBurn settings it
/// was made from (`lightburn`, may be empty), overridden by the values of `profile` where OpenEtch has them (field,
/// axes, correction file name, timing, speeds, frequency range, pulse width, open-MO delay, start-mark bit, frame
/// speed, lens corrections). Nothing else is written: no paths, macros or secrets. Galvo 2 is written as field X
/// (see the module docs).
pub fn to_lbdev(display_name: &str, lightburn: Option<&LightBurnDevice>, profile: Option<&MachineProfile>) -> String {
    use serde_json::{Map, json};
    let lb = lightburn.cloned().unwrap_or_default();
    let mut dev = Map::new();
    let name = if lb.name.is_empty() { "JCZFiber".to_owned() } else { lb.name.clone() };
    dev.insert("Name".into(), json!(name));
    dev.insert("DisplayName".into(), json!(display_name));
    dev.insert("Type".into(), json!(if lb.kind.is_empty() { "LibUSB" } else { lb.kind.as_str() }));
    let width = profile.map(|p| p.field_mm).or(lb.width_mm);
    let height = profile.map(|p| p.field_height_mm.unwrap_or(p.field_mm)).or(lb.height_mm).or(width);
    // LightBurn shows the field to 0.1 mm and the B6 profile keeps EzCad's 153.75: keep LightBurn's own value
    // when it is the same lens.
    let same = |a: Option<f64>, b: Option<f64>| matches!((a, b), (Some(a), Some(b)) if (a - b).abs() <= SAME_FIELD_MM);
    let width = if same(width, lb.width_mm) { lb.width_mm } else { width };
    let height = if same(height, lb.height_mm) { lb.height_mm } else { height };
    if let Some(w) = width {
        dev.insert("Width".into(), json!(w));
    }
    if let Some(h) = height {
        dev.insert("Height".into(), json!(h));
    }
    for (key, v) in [("MirrorX", lb.mirror_x), ("MirrorY", lb.mirror_y), ("HomeOnStartup", lb.home_on_startup)] {
        if let Some(v) = v {
            dev.insert(key.into(), json!(v));
        }
    }
    let st = &lb.settings;
    let mut set = Map::new();
    let mut put = |key: &str, v: Value| {
        if !v.is_null() {
            set.insert(key.into(), v);
        }
    };
    put("CorrectionFile", json!(profile.and_then(|p| p.cor_file.as_ref()).map(|c| c.display().to_string()).or(lb.correction_file.clone())));
    match profile {
        Some(p) => {
            put("Galvo_1_is_X", json!(!p.swap_xy));
            let (g1_neg, g2_neg) = if p.swap_xy { (p.negate_y, p.negate_x) } else { (p.negate_x, p.negate_y) };
            // Signs XOR the mirror flags on import; write the sign that gives the same negate back.
            let mirror = |m: Option<bool>| m.unwrap_or(false);
            let (mx, my) = (mirror(lb.mirror_x), mirror(lb.mirror_y));
            let (g1_mirror, g2_mirror) = if p.swap_xy { (my, mx) } else { (mx, my) };
            put("Galvo_1_Sign", json!(if g1_neg ^ g1_mirror { -1 } else { 1 }));
            put("Galvo_2_Sign", json!(if g2_neg ^ g2_mirror { -1 } else { 1 }));
            let t = &p.timing;
            put("Default_LaserOn_TC", json!(t.laser_on_delay_us));
            put("Default_LaserOff_TC", json!(t.laser_off_delay_us));
            put("Default_Polygon_TC", json!(t.polygon_delay_us));
            put("Default_End_TC", json!(t.end_delay_us));
            put("Default_JumpSpeed", json!(t.jump_speed_mm_s));
            put("Default_MinJumpDelay", json!(t.jump_delay_min_us));
            put("Default_MaxJumpDelay", json!(t.jump_delay_max_us));
            put("Default_MaxJumpDistance", json!(t.jump_distance_limit_mm));
            put("MaxSpeed", json!(p.mark_speed.max_mm_s));
            put("Laser_MinFreq", json!(p.frequency_khz_min));
            put("Laser_MaxFreq", json!(p.frequency_khz_max));
            put("Laser_QPulseWidthEnabled", json!(p.pulse_width_enabled));
            put("Laser_OpenMODelay", json!(p.open_mo_delay_us));
            put("StartMarkPortBit", json!(p.start_mark_input_bit.map_or(-1, i64::from)));
            put("FrameSpeed", json!(p.red_light_speed_mm_s));
            let c = &p.calibration;
            put("FieldOffsetX", json!(c.offset_x_mm));
            put("FieldOffsetY", json!(c.offset_y_mm));
            put("FieldAngle", json!(c.angle_deg));
            let (g1, g2) = if p.swap_xy {
                ([c.scale_y, c.skew_y, c.trapezoid_y, c.bulge_y], [c.scale_x, c.skew_x, c.trapezoid_x, c.bulge_x])
            } else {
                ([c.scale_x, c.skew_x, c.trapezoid_x, c.bulge_x], [c.scale_y, c.skew_y, c.trapezoid_y, c.bulge_y])
            };
            for (n, shape) in [(1, g1), (2, g2)] {
                for (k, v) in ["Scale", "Skew", "Trapezoid", "Bulge"].iter().zip(shape) {
                    put(&format!("Galvo_{n}_{k}"), json!(v));
                }
            }
        }
        None => {
            put("Galvo_1_is_X", json!(st.galvo_1_is_x));
            put("Galvo_1_Sign", json!(st.galvo_1_sign));
            put("Galvo_2_Sign", json!(st.galvo_2_sign));
            put("FieldOffsetX", json!(st.field_offset_x_mm));
            put("FieldOffsetY", json!(st.field_offset_y_mm));
            put("FieldAngle", json!(st.field_angle_deg));
            put("FrameSpeed", json!(st.frame_speed_mm_s));
            put("MaxSpeed", json!(st.max_speed_mm_s));
        }
    }
    // Typed values OpenEtch keeps but does not model, passed through as read.
    put("Laser_Type", json!(st.laser_type));
    put("Laser_FiberType", json!(st.laser_fiber_type));
    put("Laser_PWMEnabled", json!(st.pwm_enabled));
    put("RedLightPortBit", json!(st.red_light_port_bit));
    put("RedLightJumpDelay", json!(st.red_light_jump_delay_us));
    put("StartMarkBitHigh", json!(profile.map_or(st.start_mark_bit_high, |p| Some(!p.start_mark_active_low))));
    put("StartMarkPulseMode", json!(st.start_mark_pulse_mode));
    put("CorScaleOverride", json!(st.cor_scale_override));
    put("CutOrigin", json!(st.cut_origin));
    put("EnableBoundsCheck", json!(st.bounds_check));
    dev.insert("Settings".into(), Value::Object(set));
    let file = json!({ "DeviceList": [Value::Object(dev)] });
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// The diffs as a plain-text table (`field | profile | LightBurn | note`).
pub fn diff_table(diffs: &[ProfileDiff]) -> String {
    let cell = |v: &Value| match v {
        Value::Null => "-".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let rows: Vec<[String; 4]> = diffs
        .iter()
        .map(|d| [d.field.clone(), cell(&d.base_value), cell(&d.lightburn_value), d.note.clone()])
        .collect();
    let head = ["field", "profile", "LightBurn", "note"].map(String::from);
    let mut width = head.clone().map(|h| h.chars().count());
    for r in &rows {
        for (w, c) in width.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let line = |r: &[String; 4]| {
        let mut s = String::new();
        for (i, c) in r.iter().enumerate() {
            if i == 3 {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<w$} | ", w = width[i]));
            }
        }
        s.trim_end().to_owned() + "\n"
    };
    let mut out = line(&head);
    out.push_str(&format!("{}\n", "-".repeat(width.iter().take(3).sum::<usize>() + 9 + width[3])));
    if rows.is_empty() {
        out.push_str("(no differences)\n");
    }
    for r in &rows {
        out.push_str(&line(r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_values_are_cleaned() {
        assert_eq!(clean(153.8000030517578), 153.8);
        assert_eq!(clean(0.009999999776482582), 0.01);
        assert_eq!(clean(4000.0), 4000.0);
        assert_eq!(clean(153.75), 153.75);
        assert_eq!(clean(0.1), 0.1, "not an f32 value: unchanged");
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("C:/Users/x/EZCAD/jcz150.cor").as_deref(), Some("jcz150.cor"));
        assert_eq!(file_name(r"C:\a\b.cor").as_deref(), Some("b.cor"));
        assert_eq!(file_name("/Users/x/").as_deref(), None);
        assert_eq!(file_name("").as_deref(), None);
    }

    #[test]
    fn errors_never_quote_content() {
        let e = parse_devices(r#"{"DeviceList": "RestPendantSecret-abc123"}"#).unwrap_err();
        assert!(!e.to_string().contains("abc123"), "{e}");
        let e = parse_devices(r#"{"DeviceList": [{"Name": "x", "Settings": "secret-xyz"}]}"#).unwrap_err();
        assert!(!e.to_string().contains("xyz"), "{e}");
        let e = parse_devices("{ not json secret-q }").unwrap_err();
        assert!(!e.to_string().contains("secret"), "{e}");
        assert!(parse_devices("[1, 2]").is_err());
    }

    #[test]
    fn other_keys_are_skipped_and_wrong_types_named() {
        let p = parse_devices(
            r#"{"RestPendantSecret": "s3cr3t", "DefaultDevice": 1, "DeviceList": [
                {"DisplayName": "nameless"},
                {"Name": "A", "RestPendantSecret": "s3cr3t", "Width": "wide", "Settings": {"MaxSpeed": true, "Galvo_1_is_X": 1}}
            ]}"#,
        )
        .unwrap();
        assert_eq!(p.devices.len(), 1);
        assert_eq!(p.default_index, Some(0), "DefaultDevice 1 is the second entry, the first kept");
        assert_eq!(p.warnings, ["DeviceList[0] has no Name; skipped"]);
        let d = &p.devices[0];
        assert_eq!(d.width_mm, None);
        assert_eq!(d.settings.max_speed_mm_s, None);
        assert_eq!(d.settings.galvo_1_is_x, Some(true));
        assert_eq!(d.warnings, ["Width is not a number; ignored", "Settings.MaxSpeed is not a number; ignored"]);
        let stored = serde_json::to_string(&p).unwrap();
        assert!(!stored.contains("s3cr3t") && !stored.contains("wide"), "{stored}");
        assert_eq!(serde_json::from_str::<LightBurnPrefs>(&stored).unwrap(), p);
        assert_eq!(parse_devices("{}").unwrap(), LightBurnPrefs::default());
    }

    #[test]
    fn prefs_paths() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        let lb = |p: PathBuf| p.join("LightBurn").join("prefs.ini");
        assert_eq!(
            default_prefs_paths("windows", env(&[("LOCALAPPDATA", r"C:\Users\u\AppData\Local")])),
            [lb(PathBuf::from(r"C:\Users\u\AppData\Local"))]
        );
        assert!(default_prefs_paths("windows", env(&[])).is_empty());
        let mac = default_prefs_paths("macos", env(&[("HOME", "/Users/u")]));
        assert_eq!(mac[0], lb(PathBuf::from("/Users/u").join("Library").join("Preferences")));
        let linux = default_prefs_paths("linux", env(&[("HOME", "/home/u")]));
        assert_eq!(
            linux,
            [lb(PathBuf::from("/home/u").join(".local").join("share")), lb(PathBuf::from("/home/u").join(".config"))]
        );
        let xdg = default_prefs_paths("linux", env(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/cfg"), ("XDG_DATA_HOME", "")]));
        assert_eq!(xdg[1], lb(PathBuf::from("/cfg")));
        assert_eq!(xdg[0], lb(PathBuf::from("/home/u").join(".local").join("share")), "an empty variable is unset");
    }

    #[test]
    fn diff_table_lines_up() {
        let t = diff_table(&[ProfileDiff { field: "field_mm".into(), base_value: j(150.0), lightburn_value: j(153.8), note: String::new() }]);
        assert!(t.lines().nth(2).unwrap().starts_with("field_mm | 150.0   | 153.8"), "{t}");
        assert!(diff_table(&[]).contains("(no differences)"));
    }

    #[test]
    fn lens_corrections_are_imported_onto_the_calibration() {
        let mut d = LightBurnDevice { name: "JCZFiber".into(), width_mm: Some(150.0), ..Default::default() };
        d.settings.field_angle_deg = Some(1.5);
        d.settings.field_offset_x_mm = Some(0.25);
        d.settings.galvo_1_is_x = Some(false);
        d.settings.galvo_2_shape = [Some(1.01), Some(1.002), Some(0.995), Some(1.003)];
        d.settings.galvo_1_shape = [Some(0.99), None, None, None];
        let (p, diffs) = profile_from_lightburn(&d, &MachineProfile::generic_jcz_galvo());
        let c = p.calibration;
        assert_eq!((c.angle_deg, c.offset_x_mm), (1.5, 0.25));
        assert_eq!((c.scale_x, c.skew_x, c.trapezoid_x, c.bulge_x), (1.01, 1.002, 0.995, 1.003), "galvo 2 is X");
        assert_eq!(c.scale_y, 0.99);
        assert!(diffs.iter().any(|d| d.field == "calibration.angle_deg"));
        // A value outside the model's range is reported, not applied.
        d.settings.field_angle_deg = Some(90.0);
        let (p, diffs) = profile_from_lightburn(&d, &MachineProfile::generic_jcz_galvo());
        assert_eq!(p.calibration.angle_deg, 0.0);
        assert!(diffs.iter().any(|d| d.field == "calibration.angle_deg" && d.note.contains("not applied")), "{diffs:?}");
    }

    #[test]
    fn lbdev_export_reads_back_to_the_same_profile() {
        let mut base = MachineProfile::generic_jcz_galvo();
        base.calibration.scale_x = 1.01;
        base.calibration.angle_deg = -0.5;
        base.timing.laser_on_delay_us = -150.0;
        base.start_mark_input_bit = Some(14);
        base.negate_y = true;
        let text = to_lbdev("My galvo", None, Some(&base));
        assert!(!text.contains("Secret") && !text.contains(":\\\\"), "{text}");
        let prefs = parse_lbdev(&text).unwrap();
        let d = &prefs.devices[0];
        assert_eq!((d.name.as_str(), d.display_name.as_str(), d.width_mm), ("JCZFiber", "My galvo", Some(base.field_mm)));
        let (back, _) = profile_from_lightburn(d, &MachineProfile::generic_jcz_galvo());
        assert_eq!(back.calibration, base.calibration);
        assert_eq!(back.timing, base.timing);
        assert_eq!((back.negate_x, back.negate_y, back.swap_xy), (base.negate_x, base.negate_y, base.swap_xy));
        assert_eq!(back.start_mark_input_bit, Some(14));
    }

    #[test]
    fn non_square_fields_import_and_export_their_height() {
        let d = LightBurnDevice { name: "JCZFiber".into(), width_mm: Some(200.0), height_mm: Some(120.0), ..Default::default() };
        let (p, diffs) = profile_from_lightburn(&d, &MachineProfile::generic_jcz_galvo());
        assert_eq!((p.field_mm, p.field_height_mm), (200.0, Some(120.0)));
        assert!(diffs.iter().any(|d| d.field == "field_height_mm"), "{diffs:?}");
        assert_eq!(p.field_transform().height_mm(), 120.0);
        let text = to_lbdev("Wide", None, Some(&p));
        let back = &parse_lbdev(&text).unwrap().devices[0];
        assert_eq!((back.width_mm, back.height_mm), (Some(200.0), Some(120.0)));
        // A square LightBurn field (to its 0.1 mm display) stays square.
        let sq = LightBurnDevice { name: "JCZFiber".into(), width_mm: Some(150.0), height_mm: Some(150.04), ..Default::default() };
        assert_eq!(profile_from_lightburn(&sq, &MachineProfile::generic_jcz_galvo()).0.field_height_mm, None);
    }

    #[test]
    fn an_active_low_start_input_keeps_its_level() {
        let mut d = LightBurnDevice { name: "JCZFiber".into(), width_mm: Some(150.0), ..Default::default() };
        d.settings.start_mark_port_bit = Some(14);
        d.settings.start_mark_bit_high = Some(false);
        let mut base = MachineProfile::generic_jcz_galvo();
        base.start_mark_input_bit = Some(14);
        let (p, diffs) = profile_from_lightburn(&d, &base);
        assert_eq!((p.start_mark_input_bit, p.start_mark_active_low), (Some(14), true));
        assert!(diffs.iter().any(|d| d.field == "start_mark_active_low" && d.note.contains("active low")), "{diffs:?}");
        // Written back as LightBurn has it, also when the profile changed the level.
        let lbdev = to_lbdev("x", Some(&d), Some(&p));
        assert!(lbdev.contains("\"StartMarkBitHigh\": false"), "{lbdev}");
        let high = MachineProfile { start_mark_active_low: false, ..p.clone() };
        assert!(to_lbdev("x", Some(&d), Some(&high)).contains("\"StartMarkBitHigh\": true"));
        d.settings.start_mark_bit_high = Some(true);
        let p = profile_from_lightburn(&d, &MachineProfile::generic_jcz_galvo()).0;
        assert_eq!((p.start_mark_input_bit, p.start_mark_active_low), (Some(14), false));
    }
}
