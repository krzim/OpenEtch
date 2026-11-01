//! OpenEtch devices, LightBurn-style: one entry per laser the user works with, made from a LightBurn device (the
//! device list of `prefs.ini` or a `.lbdev` file, both read through [`crate::lightburn`]'s typed, privacy-safe
//! parser) or built in. Pure: the engine keeps the registry on disk.
//!
//! A [`Device`] has a workspace (`workspace_mm`, `origin`) that drives the canvas and LightBurn import/export, and,
//! for a JCZ (BJJCZ LMC, EzCad2-family) galvo, a [`MachineProfile`]: the only kind OpenEtch can drive today
//! ([`Device::controllable`]). Every other kind is **design-only**: its workspace is used, running is refused.
//!
//! **Family** ([`classify`]; LightBurn's `Name` is an open set, so it is never matched against a closed list): a galvo
//! when `Type` is `LibUSB`, `Name` contains "Fiber" or "EZCad", or any `Galvo_*` setting is present; a JCZ galvo when
//! the galvo's `Name` contains "JCZ" (LightBurn's EzCad2 driver is "JCZFiber"; "BSLFiber" and "EZCad3" are other
//! boards); "unknown" for LightBurn's "No Machine" placeholder; every other device is a gantry (GCode or DSP).
//!
//! **Origin.** A galvo's is the field centre. A gantry's is the corner LightBurn's Device Settings "Origin" names,
//! stored as `MirrorX` / `MirrorY`: front-left = neither, rear-left = `MirrorY` (an xTool `.lbdev`; LightBurn staff
//! say xTool machines use top-left), rear-right = both, front-right = `MirrorX` (the last two inferred from the
//! pattern, unconfirmed).
//!
//! **Profile** ([`from_lightburn`]): [`crate::profile_from_lightburn`] over the B6 profile when the device is the
//! same lens as the B6 (field within 0.05 mm and the same correction file, or none), else over the generic JCZ galvo
//! base ([`MachineProfile::generic_jcz_galvo`], conservative limits). [`Device::diffs`] recomputes the differences
//! from that base.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::lightburn::{LightBurnDevice, ProfileDiff, profile_from_lightburn};
use crate::profile::MachineProfile;

/// Largest galvo field accepted, mm.
pub const MAX_FIELD_MM: f64 = 2000.0;
/// Largest bed side accepted, mm.
pub const MAX_BED_MM: f64 = 5000.0;
/// How close (mm) a LightBurn width may be to a profile's field and still be the same lens (LightBurn shows 0.1 mm).
const SAME_FIELD_MM: f64 = 0.05 + 1e-9;
/// Longest id.
const MAX_ID_LEN: usize = 60;

/// What kind of machine a device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A BJJCZ LMC (EzCad2-family) galvo, LightBurn's "JCZFiber": OpenEtch can drive it.
    JczGalvo,
    /// A GCode or DSP machine with a bed (design-only).
    Gantry,
    /// Another galvo board (BSL, EzCad3; design-only).
    GalvoOther,
    /// LightBurn's "No Machine" placeholder (design-only).
    Unknown,
}

impl DeviceKind {
    pub fn name(self) -> &'static str {
        match self {
            DeviceKind::JczGalvo => "jcz_galvo",
            DeviceKind::Gantry => "gantry",
            DeviceKind::GalvoOther => "galvo_other",
            DeviceKind::Unknown => "unknown",
        }
    }

    /// Galvos lay files out on a centred square field.
    pub fn is_galvo(self) -> bool {
        matches!(self, DeviceKind::JczGalvo | DeviceKind::GalvoOther)
    }
}

/// Where a device's coordinates start: the field centre (galvo) or a bed corner (front = toward the operator =
/// bottom). The names are those of `commands::WorkspaceOrigin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Center,
    BottomLeft,
    TopLeft,
    BottomRight,
    TopRight,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::Center => "center",
            Origin::BottomLeft => "bottom_left",
            Origin::TopLeft => "top_left",
            Origin::BottomRight => "bottom_right",
            Origin::TopRight => "top_right",
        }
    }

    /// As LightBurn's Device Settings call it.
    pub fn label(self) -> &'static str {
        match self {
            Origin::Center => "Centre (galvo)",
            Origin::BottomLeft => "Front left",
            Origin::TopLeft => "Rear left",
            Origin::BottomRight => "Front right",
            Origin::TopRight => "Rear right",
        }
    }

    /// The corner LightBurn's `MirrorX` / `MirrorY` stand for.
    pub fn from_mirror(mirror_x: bool, mirror_y: bool) -> Self {
        match (mirror_x, mirror_y) {
            (false, false) => Origin::BottomLeft,
            (false, true) => Origin::TopLeft,
            (true, false) => Origin::BottomRight,
            (true, true) => Origin::TopRight,
        }
    }
}

/// Where a device came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceSource {
    /// Built into OpenEtch (the machine profile the engine started with).
    Builtin,
    /// LightBurn's `prefs.ini` (on this computer or uploaded).
    LightburnPrefs,
    /// A LightBurn `.lbdev` file.
    Lbdev,
    /// Made by hand.
    Manual,
}

impl DeviceSource {
    pub fn name(self) -> &'static str {
        match self {
            DeviceSource::Builtin => "builtin",
            DeviceSource::LightburnPrefs => "lightburn-prefs",
            DeviceSource::Lbdev => "lbdev",
            DeviceSource::Manual => "manual",
        }
    }
}

/// The profile a JCZ galvo device was built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileBase {
    /// The built-in B6 profile (the same lens).
    Builtin,
    /// [`MachineProfile::generic_jcz_galvo`].
    GenericJczGalvo,
}

/// The two base profiles devices are built on.
#[derive(Debug, Clone, PartialEq)]
pub struct Bases {
    /// The built-in machine's profile (the B6 fixture).
    pub builtin: MachineProfile,
    /// The generic JCZ galvo base.
    pub generic: MachineProfile,
}

impl Bases {
    pub fn get(&self, base: ProfileBase) -> &MachineProfile {
        match base {
            ProfileBase::Builtin => &self.builtin,
            ProfileBase::GenericJczGalvo => &self.generic,
        }
    }
}

/// One device. See the module docs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Stable id (a slug of the display name).
    pub id: String,
    /// LightBurn's `Name`: the controller ("JCZFiber", "GRBL", ...).
    pub name: String,
    /// LightBurn's `DisplayName` (what `.lbrn2` files name as `DeviceName`), else `name`.
    pub display_name: String,
    pub source: DeviceSource,
    pub kind: DeviceKind,
    /// LightBurn's `Type`: the connection ("LibUSB", "Serial", "TCP", ...).
    #[serde(default)]
    pub lightburn_type: String,
    /// `[width, height]` mm: the field of a galvo (square), the bed of a gantry.
    pub workspace_mm: [f64; 2],
    pub origin: Origin,
    /// OpenEtch can drive it (a JCZ galvo with a profile).
    pub controllable: bool,
    /// What the profile was built on (JCZ galvos from LightBurn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<ProfileBase>,
    /// The machine profile (JCZ galvos only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<MachineProfile>,
    /// The typed LightBurn settings it was made from (never anything else of the file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightburn: Option<LightBurnDevice>,
}

/// A device file (`<id>.toml`): the [`Device`] with a format tag.
#[derive(Serialize, Deserialize)]
struct DeviceFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    device: Device,
}

const FILE_FORMAT: &str = "openetch-device";

/// The family of a LightBurn device (see the module docs).
pub fn classify(dev: &LightBurnDevice) -> DeviceKind {
    let name = dev.name.to_lowercase();
    if name.contains("no machine") {
        return DeviceKind::Unknown;
    }
    let galvo = dev.kind.trim().eq_ignore_ascii_case("libusb")
        || name.contains("fiber")
        || name.contains("ezcad")
        || dev.settings.has_galvo_keys();
    match (galvo, name.contains("jcz")) {
        (true, true) => DeviceKind::JczGalvo,
        (true, false) => DeviceKind::GalvoOther,
        (false, _) => DeviceKind::Gantry,
    }
}

/// A URL- and file-safe id from `text`: lowercase ASCII letters and digits joined by single dashes (at most 60
/// characters); "device" when nothing is left.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= MAX_ID_LEN {
            break;
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "device".into() } else { out.to_owned() }
}

/// Whether `id` is a usable device id (what [`slug`] makes, possibly with a `-N` suffix).
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN + 8
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// LightBurn's GUID-free identity of a device: display name, controller, connection and size (to 0.1 mm), ignoring
/// case. A device imported again with the same identity is updated rather than added.
pub fn identity(display_name: &str, name: &str, kind: &str, size: [f64; 2]) -> String {
    format!(
        "{}|{}|{}|{:.1}x{:.1}",
        display_name.trim().to_lowercase(),
        name.trim().to_lowercase(),
        kind.trim().to_lowercase(),
        size[0],
        size[1]
    )
}

/// Whether a LightBurn galvo is the same lens as `base`: its width within 0.05 mm of the field and the same
/// correction file name (ignoring case), or none.
pub fn same_lens(dev: &LightBurnDevice, base: &MachineProfile) -> bool {
    let field = dev.width_mm.is_some_and(|w| (w - base.field_mm).abs() <= SAME_FIELD_MM);
    let base_cor = base.cor_file.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase());
    let cor = match (&dev.correction_file, base_cor) {
        (None, _) => true,
        (Some(c), Some(b)) => c.to_lowercase() == b,
        (Some(_), None) => false,
    };
    field && cor
}

fn usable(v: Option<f64>, max: f64) -> Option<f64> {
    v.filter(|v| v.is_finite() && *v > 0.0 && *v <= max)
}

/// A device for a LightBurn device (see the module docs). `id` is left empty for the registry to assign. Refused
/// when the device has no usable size.
pub fn from_lightburn(dev: &LightBurnDevice, source: DeviceSource, bases: &Bases) -> Result<Device, String> {
    let kind = classify(dev);
    let display_name = if dev.display_name.trim().is_empty() { dev.name.clone() } else { dev.display_name.trim().to_owned() };
    let (workspace_mm, origin, base, profile) = if kind.is_galvo() {
        let w = usable(dev.width_mm, MAX_FIELD_MM)
            .ok_or_else(|| format!("no usable field size (Width must be 0..={MAX_FIELD_MM} mm)"))?;
        if kind == DeviceKind::JczGalvo {
            let base = if same_lens(dev, &bases.builtin) { ProfileBase::Builtin } else { ProfileBase::GenericJczGalvo };
            let (mut p, _) = profile_from_lightburn(dev, bases.get(base));
            p.name = display_name.clone();
            ([p.field_mm, p.field_mm], Origin::Center, Some(base), Some(p))
        } else {
            ([w, w], Origin::Center, None, None)
        }
    } else {
        let w = usable(dev.width_mm, MAX_BED_MM);
        let h = usable(dev.height_mm, MAX_BED_MM);
        let (Some(w), Some(h)) = (w, h) else {
            return Err(format!("no usable work area (Width and Height must be 0..={MAX_BED_MM} mm)"));
        };
        let origin = Origin::from_mirror(dev.mirror_x.unwrap_or(false), dev.mirror_y.unwrap_or(false));
        ([w, h], origin, None, None)
    };
    Ok(Device {
        id: String::new(),
        name: dev.name.clone(),
        display_name,
        source,
        kind,
        lightburn_type: dev.kind.clone(),
        workspace_mm,
        origin,
        controllable: profile.is_some(),
        base,
        profile,
        lightburn: Some(dev.clone()),
    })
}

impl Device {
    /// The built-in device for the engine's own profile (the B6 fixture): "<profile name> (EzCad)".
    pub fn builtin(profile: MachineProfile) -> Self {
        let name = format!("{} (EzCad)", profile.name);
        Device {
            id: slug(&name),
            name: name.clone(),
            display_name: name,
            source: DeviceSource::Builtin,
            kind: DeviceKind::JczGalvo,
            lightburn_type: "LibUSB".into(),
            workspace_mm: [profile.field_mm, profile.field_mm],
            origin: Origin::Center,
            controllable: true,
            base: None,
            profile: Some(profile),
            lightburn: None,
        }
    }

    /// [`identity`] of this device.
    pub fn identity(&self) -> String {
        let size = match &self.lightburn {
            // The size LightBurn gave, so a re-import matches although the profile kept the base's field.
            Some(d) => [d.width_mm.unwrap_or(self.workspace_mm[0]), d.height_mm.unwrap_or(self.workspace_mm[1])],
            None => self.workspace_mm,
        };
        let name = self.lightburn.as_ref().map_or(self.name.as_str(), |d| d.name.as_str());
        identity(&self.display_name, name, &self.lightburn_type, size)
    }

    /// Whether `query` names this device: its id, display name or name, ignoring case and surrounding space.
    pub fn is_named(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        !q.is_empty() && [&self.id, &self.display_name, &self.name].iter().any(|n| n.trim().to_lowercase() == q)
    }

    /// The square field size of a galvo; the smaller bed side of a gantry.
    pub fn field_mm(&self) -> f64 {
        self.workspace_mm[0].min(self.workspace_mm[1])
    }

    /// Why OpenEtch cannot drive this device, when it cannot.
    pub fn not_controllable_reason(&self) -> Option<String> {
        if self.controllable {
            return None;
        }
        let what = match self.kind {
            DeviceKind::JczGalvo => "it has no machine profile".to_owned(),
            DeviceKind::Gantry => format!("OpenEtch can't drive {} controllers yet (only JCZ/LMC galvos)", self.name),
            DeviceKind::GalvoOther => format!("OpenEtch can't drive {} galvos yet (only JCZ/LMC boards)", self.name),
            DeviceKind::Unknown => "it is a placeholder without a controller".to_owned(),
        };
        Some(format!("\"{}\" is a design-only device: {what}; its workspace is used for designing", self.display_name))
    }

    /// The differences between the profile and the base it was built on (empty for the built-in device).
    pub fn diffs(&self, bases: &Bases) -> Vec<ProfileDiff> {
        match (&self.lightburn, self.base) {
            (Some(d), Some(b)) if self.kind == DeviceKind::JczGalvo => profile_from_lightburn(d, bases.get(b)).1,
            _ => Vec::new(),
        }
    }

    /// The device as its `<id>.toml` file.
    pub fn to_toml(&self) -> Result<String, String> {
        let file = DeviceFile { format: FILE_FORMAT.into(), version: 1, device: self.clone() };
        toml::to_string_pretty(&file).map_err(|e| e.to_string())
    }

    /// Read a `<id>.toml` device file; the profile must validate and the kind, workspace and origin must agree.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: DeviceFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.format != FILE_FORMAT || file.version != 1 {
            return Err("not an OpenEtch device file (version 1)".into());
        }
        let mut d = file.device;
        if !is_valid_id(&d.id) {
            return Err(format!("unusable device id \"{}\"", d.id));
        }
        if let Some(p) = &d.profile {
            p.validate().map_err(|e| e.to_string())?;
        }
        let max = if d.kind.is_galvo() { MAX_FIELD_MM } else { MAX_BED_MM };
        if !d.workspace_mm.iter().all(|v| v.is_finite() && *v > 0.0 && *v <= max) {
            return Err(format!("unusable workspace {:?} mm", d.workspace_mm));
        }
        if d.kind.is_galvo() != (d.origin == Origin::Center) {
            return Err("a galvo's origin is its centre and a gantry's a corner".into());
        }
        // Only a JCZ galvo with a profile can be driven, whatever the file says.
        d.controllable = d.kind == DeviceKind::JczGalvo && d.profile.is_some();
        Ok(d)
    }

    /// The field LightBurn files of this galvo are laid out for: LightBurn's `Width` (153.8 on the B6, where the
    /// profile's field is EzCad's 153.75), else the workspace width. None for gantries (their files use the bed).
    pub fn lightburn_field_mm(&self) -> Option<f64> {
        if !self.kind.is_galvo() {
            return None;
        }
        let width = self.lightburn.as_ref().and_then(|d| d.width_mm).filter(|w| w.is_finite() && *w > 0.0 && *w <= MAX_FIELD_MM);
        Some(width.unwrap_or(self.workspace_mm[0]))
    }

    /// `GET /api/devices` entry: `{id, name, display_name, source, kind, lightburn_type, workspace_mm,
    /// lightburn_field_mm, origin, controllable, reason}`.
    pub fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "display_name": self.display_name,
            "source": self.source.name(),
            "kind": self.kind.name(),
            "lightburn_type": self.lightburn_type,
            "workspace_mm": self.workspace_mm,
            "lightburn_field_mm": self.lightburn_field_mm(),
            "origin": self.origin.name(),
            "controllable": self.controllable,
            "reason": self.not_controllable_reason(),
        })
    }

    /// LightBurn's Device Settings, read-only, in sections (`[{title, rows: [{label, key, value}]}]`; `key` is the
    /// LightBurn key, null for derived rows). Only the typed fields; values OpenEtch does not use are shown as read.
    pub fn settings_view(&self) -> Value {
        let mut sections = Vec::new();
        let row = |label: &str, key: Option<&str>, value: Value| json!({ "label": label, "key": key, "value": value });
        let mut general = vec![
            row("Name", Some("DisplayName"), json!(self.display_name)),
            row("Controller", Some("Name"), json!(self.name)),
            row("Connection", Some("Type"), json!(self.lightburn_type)),
            row("Work area (mm)", Some("Width/Height"), json!(self.workspace_mm)),
            row("Origin", Some("MirrorX/MirrorY"), json!(self.origin.label())),
        ];
        let Some(d) = &self.lightburn else {
            if let Some(p) = &self.profile {
                general.push(row("Machine profile", None, json!(p.name)));
                general.push(row("Correction file", None, json!(p.cor_file)));
            }
            sections.push(json!({ "title": "Device", "rows": general }));
            return json!(sections);
        };
        let s = &d.settings;
        if let (Some(w), Some(h)) = (d.width_mm, d.height_mm)
            && [w, h] != self.workspace_mm
        {
            general.push(row("LightBurn Width x Height (mm)", Some("Width/Height"), json!([w, h])));
        }
        general.extend([
            row("Auto-home on startup", Some("HomeOnStartup"), json!(d.home_on_startup)),
            row("Bounds check", Some("EnableBoundsCheck"), json!(s.bounds_check)),
            row("Job origin", Some("CutOrigin"), json!(s.cut_origin)),
            row("Material library", Some("LastDevLibraryPath"), json!(d.library_file)),
            row("Preview max speed X / Y (mm/s)", Some("Sim_MaxSpeedX/Y"), json!([s.preview_max_speed_x_mm_s, s.preview_max_speed_y_mm_s])),
            row("Preview rapid speed (mm/s)", Some("Sim_RapidSpeed"), json!(s.preview_rapid_speed_mm_s)),
            row("Preview acceleration X / Y (mm/s²)", Some("Sim_MaxAccelX/Y"), json!([s.preview_max_accel_x, s.preview_max_accel_y])),
        ]);
        sections.push(json!({ "title": "Device", "rows": general }));
        if self.kind.is_galvo() {
            let shape = |v: &[Option<f64>; 4]| json!(v);
            sections.push(json!({ "title": "Galvo", "rows": [
                row("Field size (mm)", Some("Width"), json!(d.width_mm)),
                row("Field offset X / Y (mm)", Some("FieldOffsetX/Y"), json!([s.field_offset_x_mm, s.field_offset_y_mm])),
                row("Field angle (°)", Some("FieldAngle"), json!(s.field_angle_deg)),
                row("Correction file", Some("CorrectionFile"), json!(d.correction_file)),
                row("Galvo 1 is X axis", Some("Galvo_1_is_X"), json!(s.galvo_1_is_x)),
                row("Galvo 1 / 2 direction (sign)", Some("Galvo_1/2_Sign"), json!([s.galvo_1_sign, s.galvo_2_sign])),
                row("Galvo 1 scale, skew, trapezoid, bulge", Some("Galvo_1_*"), shape(&s.galvo_1_shape)),
                row("Galvo 2 scale, skew, trapezoid, bulge", Some("Galvo_2_*"), shape(&s.galvo_2_shape)),
                row("Frame speed (mm/s)", Some("FrameSpeed"), json!(s.frame_speed_mm_s)),
                row("Max speed (mm/s)", Some("MaxSpeed"), json!(s.max_speed_mm_s)),
                row("Jump speed (mm/s)", Some("Default_JumpSpeed"), json!(s.jump_speed_mm_s)),
                row("Min / max jump delay (µs)", Some("Default_Min/MaxJumpDelay"), json!([s.min_jump_delay_us, s.max_jump_delay_us])),
                row("Jump distance limit (mm)", Some("Default_MaxJumpDistance"), json!(s.max_jump_distance_mm)),
                row("Laser on / off / end / polygon TC (µs)", Some("Default_*_TC"), json!([s.laser_on_tc_us, s.laser_off_tc_us, s.end_tc_us, s.polygon_tc_us])),
                row("Laser type / fiber type", Some("Laser_Type/FiberType"), json!([s.laser_type, s.laser_fiber_type])),
                row("Frequency range (kHz)", Some("Laser_Min/MaxFreq"), json!([s.min_freq_khz, s.max_freq_khz])),
                row("Q-pulse width enabled", Some("Laser_QPulseWidthEnabled"), json!(s.q_pulse_width_enabled)),
                row("PWM enabled", Some("Laser_PWMEnabled"), json!(s.pwm_enabled)),
                row("Open MO delay (µs)", Some("Laser_OpenMODelay"), json!(s.open_mo_delay_us)),
                row("Red light port bit", Some("RedLightPortBit"), json!(s.red_light_port_bit)),
                row("Start mark port bit", Some("StartMarkPortBit"), json!(s.start_mark_port_bit)),
            ]}));
        } else if self.kind == DeviceKind::Gantry {
            sections.push(json!({ "title": "GCode / DSP", "rows": [
                row("Baud rate", Some("BaudRate"), json!(s.baud_rate)),
                row("Network port", Some("NetworkPort"), json!(s.network_port)),
                row("S-value max", Some("S_Scale"), json!(s.s_value_max)),
                row("Air assist M7", Some("AirAssistM7"), json!(s.air_assist_m7)),
                row("Z axis", Some("EnableZ"), json!(s.enable_z)),
                row("U axis", Some("EnableU"), json!(s.enable_u)),
                row("Transfer mode", Some("TransferMode"), json!(s.transfer_mode)),
                row("G-code flavor", Some("GCodeFlavor"), json!(s.gcode_flavor)),
            ]}));
        }
        json!(sections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lightburn::{parse_devices, parse_lbdev};

    const B6: &str = include_str!("../../../fixtures/b6-mopa-60w.toml");
    const PREFS: &str = include_str!("../tests/fixtures/lightburn-prefs.json");
    const LBDEV: &str = include_str!("../tests/fixtures/gantry.lbdev");

    fn bases() -> Bases {
        Bases { builtin: MachineProfile::from_toml(B6).unwrap(), generic: MachineProfile::generic_jcz_galvo() }
    }

    #[test]
    fn families_are_worked_out_not_listed() {
        let dev = |name: &str, kind: &str| LightBurnDevice { name: name.into(), kind: kind.into(), ..Default::default() };
        assert_eq!(classify(&dev("JCZFiber", "LibUSB")), DeviceKind::JczGalvo);
        assert_eq!(classify(&dev("JCZFiber", "")), DeviceKind::JczGalvo, "the name says fiber");
        assert_eq!(classify(&dev("BSLFiber", "LibUSB")), DeviceKind::GalvoOther);
        assert_eq!(classify(&dev("EZCad3", "LibUSB")), DeviceKind::GalvoOther);
        assert_eq!(classify(&dev("Mystery", "LibUSB")), DeviceKind::GalvoOther, "LibUSB is a galvo connection");
        assert_eq!(classify(&dev("GRBL", "Serial")), DeviceKind::Gantry);
        assert_eq!(classify(&dev("Custom GCode", "TCP")), DeviceKind::Gantry);
        assert_eq!(classify(&dev("Ruida", "UDP")), DeviceKind::Gantry);
        assert_eq!(classify(&dev("No Machine", "")), DeviceKind::Unknown);
        let mut galvo_keys = dev("Odd", "Serial");
        galvo_keys.settings.galvo_1_sign = Some(1.0);
        assert_eq!(classify(&galvo_keys), DeviceKind::GalvoOther);
    }

    #[test]
    fn slugs_and_identity() {
        assert_eq!(slug("JCZFiber (Unknown)"), "jczfiber-unknown");
        assert_eq!(slug("  Ünïcode / *** "), "n-code");
        assert_eq!(slug("***"), "device");
        assert!(slug(&"x".repeat(200)).len() <= 60);
        assert!(is_valid_id("jczfiber-unknown-2") && !is_valid_id("../x") && !is_valid_id("") && !is_valid_id("A"));
        assert_eq!(identity(" WAZER ", "Custom GCode", "TCP", [400.0, 300.0]), "wazer|custom gcode|tcp|400.0x300.0");
    }

    #[test]
    fn the_b6_device_builds_on_the_b6_profile_and_others_on_the_generic_base() {
        let prefs = parse_devices(PREFS).unwrap();
        let b = bases();
        let b6 = from_lightburn(&prefs.devices[0], DeviceSource::LightburnPrefs, &b).unwrap();
        assert_eq!((b6.kind, b6.base, b6.origin, b6.controllable), (DeviceKind::JczGalvo, Some(ProfileBase::Builtin), Origin::Center, true));
        assert_eq!(b6.workspace_mm, [153.75, 153.75], "same lens: the B6 field");
        let lb_field = b6.summary()["lightburn_field_mm"].as_f64().unwrap();
        assert!((lb_field - 153.8).abs() < 1e-4, "LightBurn files are laid out for LightBurn's width: {lb_field}");
        let p = b6.profile.as_ref().unwrap();
        assert_eq!((p.name.as_str(), p.cor_file.as_deref()), ("JCZFiber (Unknown)", Some(std::path::Path::new("jcz150.cor"))));
        assert_eq!(p.mark_speed.max_mm_s, 7000.0);
        assert!(b6.diffs(&b).iter().any(|d| d.field == "mark_speed.max_mm_s"));

        // The fixture's second device names no known controller: a LibUSB galvo OpenEtch cannot drive.
        let odd = from_lightburn(&prefs.devices[1], DeviceSource::LightburnPrefs, &b).unwrap();
        assert_eq!((odd.kind, odd.controllable, odd.workspace_mm), (DeviceKind::GalvoOther, false, [110.0, 110.0]));
        // As a JCZFiber with another lens it is built on the generic base.
        let jcz = LightBurnDevice { name: "JCZFiber".into(), ..prefs.devices[1].clone() };
        let bench = from_lightburn(&jcz, DeviceSource::LightburnPrefs, &b).unwrap();
        assert_eq!(bench.base, Some(ProfileBase::GenericJczGalvo));
        let p = bench.profile.as_ref().unwrap();
        assert_eq!(p.field_mm, 110.0);
        assert_eq!(p.max_jump_speed_mm_s, b.generic.max_jump_speed_mm_s, "conservative limits are kept");
        assert_eq!(p.frequency_khz_min, 20.0);
        p.validate().unwrap();
        // The device file round-trips and is checked.
        let mut stored = bench.clone();
        stored.id = "bench-110-lens".into();
        let text = stored.to_toml().unwrap();
        assert_eq!(Device::from_toml(&text).unwrap(), stored);
        assert!(!text.to_lowercase().contains("secret") && !text.contains("someone"));
        let forged = text.replace("controllable = true", "controllable = true\n").replace("kind = \"jcz_galvo\"", "kind = \"gantry\"");
        assert!(Device::from_toml(&forged).is_err(), "a gantry with a centre origin is refused");
        assert!(Device::from_toml(&text.replace("bench-110-lens", "../evil")).is_err());
    }

    #[test]
    fn a_gantry_lbdev_is_a_design_only_bed() {
        let prefs = parse_lbdev(LBDEV).unwrap();
        assert_eq!(prefs.default_index, None);
        let d = from_lightburn(&prefs.devices[0], DeviceSource::Lbdev, &bases()).unwrap();
        assert_eq!((d.kind, d.origin, d.workspace_mm, d.controllable), (DeviceKind::Gantry, Origin::TopLeft, [400.0, 300.0], false));
        assert_eq!(d.field_mm(), 300.0);
        assert_eq!((d.lightburn_field_mm(), &d.summary()["lightburn_field_mm"]), (None, &Value::Null));
        assert!(d.profile.is_none() && d.diffs(&bases()).is_empty());
        assert!(d.not_controllable_reason().unwrap().contains("can't drive Custom GCode"));
        let lb = d.lightburn.as_ref().unwrap();
        assert_eq!((lb.settings.baud_rate, lb.settings.gcode_flavor.as_deref()), (Some(115200), Some("smoothie")));
        assert_eq!(lb.library_file.as_deref(), Some("materials.clb"), "the folders are dropped");
        let text = serde_json::to_string(&d).unwrap() + &d.to_toml().unwrap() + &d.settings_view().to_string();
        for leaked in ["M1403", "Desk", "10.0.0.", "start_user", "synthetic-secret", "synthetic-guid", "macro"] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
        let mut stored = d.clone();
        stored.id = "test-bed".into();
        assert_eq!(Device::from_toml(&stored.to_toml().unwrap()).unwrap(), stored);
        // Origins from the mirror flags.
        assert_eq!(Origin::from_mirror(false, false), Origin::BottomLeft);
        assert_eq!(Origin::from_mirror(true, true), Origin::TopRight);
        assert!(parse_lbdev("{\"DefaultDevice\": 0}").is_err(), "a .lbdev holds a DeviceList");
    }

    #[test]
    fn devices_without_a_size_are_refused() {
        let dev = LightBurnDevice { name: "GRBL".into(), kind: "Serial".into(), width_mm: Some(400.0), ..Default::default() };
        assert!(from_lightburn(&dev, DeviceSource::Lbdev, &bases()).unwrap_err().contains("work area"));
        let dev = LightBurnDevice { name: "JCZFiber".into(), kind: "LibUSB".into(), ..Default::default() };
        assert!(from_lightburn(&dev, DeviceSource::Lbdev, &bases()).unwrap_err().contains("field size"));
    }

    #[test]
    fn builtin_device_and_lookup() {
        let d = Device::builtin(MachineProfile::from_toml(B6).unwrap());
        assert_eq!((d.id.as_str(), d.display_name.as_str()), ("commarker-b6-mopa-60w-ezcad", "ComMarker B6 MOPA 60W (EzCad)"));
        assert!(d.is_named("commarker b6 mopa 60w (ezcad)") && d.is_named("COMMARKER-B6-MOPA-60W-EZCAD") && !d.is_named(""));
        assert_eq!(d.summary()["kind"], "jcz_galvo");
        assert_eq!(d.summary()["reason"], Value::Null);
        assert_eq!(d.summary()["lightburn_field_mm"], 153.75, "no LightBurn settings: the workspace");
        assert!(d.diffs(&bases()).is_empty());
    }
}
