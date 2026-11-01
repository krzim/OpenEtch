//! Machine profile, the [`Link`] transport trait, [`SimLink`], `UsbLink` (nusb, feature `usb`) and the
//! [`Controller`] (init, .cor upload, light framing, run, abort).
//!
//! - [`MachineProfile`]: one laser's fixed facts and hard limits (field, axis swap/negate, laser source, port bits,
//!   default timing). TOML on disk; importable from an EzCad `markcfg7` ([`MachineProfile::from_markcfg`]) and from
//!   a LightBurn device ([`lightburn`]: the device list of LightBurn's `prefs.ini` or a `.lbdev` file,
//!   [`profile_from_lightburn`]).
//! - [`Device`] ([`device`]): an OpenEtch device, LightBurn-style: a workspace (field or bed, origin) and, for a JCZ
//!   galvo, a profile; other kinds are design-only.
//! - [`Link`]: sends 12-byte single commands and 3072-byte list packets. [`SimLink`] records traffic and fakes a
//!   board for tests and the engine's simulator mode; `UsbLink` is the real board and is only compiled with the
//!   `usb` feature.
//! - [`Controller`]: the only thing that talks to a [`Link`]. Framing goes through `lmc`'s [`lmc::LightList`] so it
//!   cannot emit; [`Controller::run`] needs an [`Armed`] token that only the engine's UI approval creates;
//!   [`Controller::abort`] is never gated.
//!
//! [`field_to_galvo`] wraps `lmc::units::field_to_galvo`, the one field mm -> galvo word conversion (field bounds,
//! negate in the field frame, then swap) that `plan::Target` encodes jobs with too.
#![forbid(unsafe_code)]

mod armed;
mod check;
mod controller;
pub mod device;
mod geometry;
mod init;
pub mod lightburn;
mod link;
mod markcfg;
mod profile;
mod sim;
#[cfg(feature = "usb")]
mod usb;

pub use armed::Armed;
pub use check::{JobCheckError, check_job};
pub use controller::{
    AXIS_TIMEOUT, AbortHandle, BoardInfo, Controller, ControllerError, ControllerState, FrameStyle, PollConfig, RunControl,
    RunPart, RunProgress, RunReport,
};
pub use geometry::{FieldError, field_to_galvo};
pub use init::InitSettings;
pub use link::{Link, LinkError, Reply};
pub use lightburn::{
    LightBurnDevice, LightBurnError, LightBurnPrefs, LightBurnSettings, ProfileDiff, find_device, parse_devices,
    parse_lbdev, profile_from_lightburn, to_lbdev,
};
pub use device::{Bases, Device, DeviceKind, DeviceSource, Origin, ProfileBase, edit_profile, merge_patch};
pub use markcfg::MarkcfgImport;
pub use profile::{
    AxisRole, DEFAULT_MAX_DELAY_US, DEFAULT_MAX_JUMP_SPEED_MM_S, DEFAULT_OPEN_MO_DELAY_US, ExtensionAxis, FiberKind, InputPort, IoPorts,
    LaserKind, MachineProfile, MarkSpeedLimits, ProfileError, RedDot, Timing,
};
pub use sim::{SimBoard, SimLink, Traffic};
#[cfg(feature = "usb")]
pub use usb::{CLONE_PID, DEFAULT_TIMEOUT, DetectedBoard, LMC_PID, LMC_VID, UsbLink, UsbOptions, detect};
