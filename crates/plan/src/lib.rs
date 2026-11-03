//! Document to LMC job: flatten, hatch, order, encode; and simulation. Pure (no I/O), wasm-ok.
//!
//! - [`plan_job`]: [`doc::Document`] -> [`JobPlan`] (passes of ordered polylines in field-frame mm, images as
//!   dithered scan runs), refusing anything outside the field or the [`Target`]'s limits.
//! - [`encode`]: [`JobPlan`] -> `lmc` list packets (layout documented in the `encode` module).
//! - [`simulate`]: packets -> [`Preview`] (what the board would run); [`preview_plan`]: plan -> [`Preview`]
//!   without encoding, for the live UI; [`preview_document`] adds per-layer totals.
//! - [`fill_regions`]: the even-odd fill areas of fill layers, for display.
//! - [`gcode_lines`] / [`gcode_file`]: [`JobPlan`] -> G-code for a GRBL laser ([`GcodeSettings`], module `gcode`).
//!
//! Everything public is in mm, mm/s, kHz, %, µs and degrees in the field frame (ARCHITECTURE.md). Galvo words,
//! axis swap and negate appear only inside [`Target::to_galvo`] / [`Target::from_galvo`].
#![forbid(unsafe_code)]

mod cutopts;
mod encode;
mod gcode;
mod geom;
mod hatch;
mod image;
mod job;
// Offset Fill, kerf offset and start points (crates/geom offsets).
pub mod offsets;
mod order;
mod preview;
mod ramp;
mod regions;
mod rotary;
mod target;
pub mod zones;

pub use encode::{EncodedPart, encode, encode_parts, jump_delay_us};
pub use gcode::{GcodeSettings, gcode_file, gcode_lines, gcode_number};
pub use geom::{FLATTEN_TOLERANCE_MM, Point};
pub use hatch::MAX_SCANLINES;
pub use image::{MAX_IMAGE_DOTS, MAX_PROCESSED_DOTS, MAX_SOURCE_PIXELS, ProcessedBitmap, processed_bitmap, processed_png, tone_factors};
pub use job::{
    GantryPass, ImageRaster, JobOptions, JobPlan, MAX_LAYER_PASSES, MAX_PASSES, MAX_SEGMENTS, MotionTime, PartPlan, PassPlan, PassSettings, Stats, layer_schedule,
    plan_job, plan_job_with,
};
pub use preview::{
    LayerPreview, MAX_RASTER_DOTS, PackedRasters, PassPreview, Preview, PreviewRaster, PreviewSegment, RasterHeader, SegmentKind,
    SubLayerPreview, preview_document, preview_plan, raster_mismatch, scale_times, simulate, start_at,
};
pub use regions::{DISPLAY_TOLERANCE_MM, FillRegions, LayerFill, fill_regions};
pub use target::Target;

use doc::{LayerId, ObjectId};
use lmc::LmcError;

/// Everything planning or encoding refuses. Nothing is clamped.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlanError {
    /// Objects with at least one point on or outside `±field_mm / 2`.
    #[error("objects {} fall outside the ±{} mm field", .objects.join(", "), .field_mm / 2.0)]
    OutOfField { objects: Vec<ObjectId>, field_mm: f64 },
    /// A point of a plan handed to [`encode`] that cannot be encoded.
    #[error("layer {layer}: point ({}, {}) mm is outside the ±{} mm field", .point[0], .point[1], .field_mm / 2.0)]
    PointOutOfField { layer: LayerId, point: Point, field_mm: f64 },
    #[error("layer {layer}: power {power_pct} % is above the machine limit of {max_pct} %")]
    PowerTooHigh { layer: LayerId, power_pct: f64, max_pct: f64 },
    #[error("layer {layer}: pulse width {requested_ns} ns is not in the source's table (nearest is {nearest_ns} ns)")]
    PulseWidth { layer: LayerId, requested_ns: f64, nearest_ns: u16 },
    /// A setting that is not finite or not positive where it must be.
    #[error("layer {layer}: {what} = {value} is invalid")]
    Setting { layer: LayerId, what: &'static str, value: f64 },
    #[error("layer {layer}: a hatch needs more than {max} lines; check the interval")]
    TooManyHatchLines { layer: LayerId, max: usize },
    /// More than [`MAX_PASSES`] repetitions of one layer or sub-layer.
    #[error("layer {layer}: {passes} passes is more than the limit of {max}")]
    TooManyPasses { layer: LayerId, passes: u32, max: u32 },
    /// The job would have more than [`MAX_SEGMENTS`] mark segments (planning stopped there).
    #[error("the job has more than {max} mark segments; reduce passes, hatch density or detail")]
    TooManySegments { max: usize },
    /// An image that cannot be decoded or adjusted.
    #[error("image {object} cannot be engraved: {reason}")]
    Image { object: ObjectId, reason: String },
    /// An image that would be sampled into more than [`MAX_IMAGE_DOTS`] dots in one pass.
    #[error(
        "image {object} would need {rows} x {cols} dots at this line interval (at most {max}); increase the interval \
         or make the image smaller"
    )]
    ImageTooLarge { object: ObjectId, rows: u64, cols: u64, max: u64 },
    /// The job's mark segments went over [`MAX_SEGMENTS`] while an image was rastered.
    #[error(
        "image {object} would take the job past {max} mark segments; increase the line interval, use fewer passes, \
         or make the image smaller"
    )]
    ImageSegments { object: ObjectId, max: usize },
    /// A setting the board cannot represent (e.g. a speed or delay out of word range).
    #[error("layer {layer}: {source}")]
    Encode { layer: LayerId, source: LmcError },
    #[error("invalid machine target: {0}")]
    Target(String),
    /// Something G-code output for a GRBL laser cannot express (module `gcode`).
    #[error("G-code: {0}")]
    Gcode(String),
    #[error("the job has nothing to mark")]
    Empty,
    /// A selection that names no object or group of the document.
    #[error("{0}")]
    Selection(String),
    /// A mark inside a keep-out zone (a jig wall, a clamp; module `zones`).
    #[error(
        "the job marks inside keep-out zone \"{zone}\" (layer {layer}, at {:.2}, {:.2} mm); move the design, or the          jig, or switch the zone off",
        .point[0], .point[1]
    )]
    KeepOut { zone: String, layer: LayerId, point: Point },
    #[error(transparent)]
    Lmc(#[from] LmcError),
}

pub type Result<T, E = PlanError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod options_tests;
#[cfg(test)]
mod offsets_tests;
