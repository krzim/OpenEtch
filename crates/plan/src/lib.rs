//! Document to LMC job: flatten, hatch, order, encode; and simulation. Pure (no I/O), wasm-ok.
//!
//! - [`plan_job`]: [`doc::Document`] -> [`JobPlan`] (passes of ordered polylines in field-frame mm), refusing
//!   anything outside the field or the [`Target`]'s limits.
//! - [`encode`]: [`JobPlan`] -> `lmc` list packets (layout documented in the `encode` module).
//! - [`simulate`]: packets -> [`Preview`] (what the board would run); [`preview_plan`]: plan -> [`Preview`]
//!   without encoding, for the live UI.
//!
//! Everything public is in mm, mm/s, kHz, %, µs and degrees in the field frame (ARCHITECTURE.md). Galvo words,
//! axis swap and negate appear only inside [`Target::to_galvo`] / [`Target::from_galvo`].
#![forbid(unsafe_code)]

mod encode;
mod geom;
mod hatch;
mod job;
mod order;
mod preview;
mod target;

pub use encode::{encode, jump_delay_us};
pub use geom::{FLATTEN_TOLERANCE_MM, Point};
pub use hatch::MAX_SCANLINES;
pub use job::{JobPlan, PassPlan, PassSettings, Stats, plan_job};
pub use preview::{Preview, PreviewSegment, SegmentKind, preview_plan, simulate};
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
    /// A setting the board cannot represent (e.g. a speed or delay out of word range).
    #[error("layer {layer}: {source}")]
    Encode { layer: LayerId, source: LmcError },
    #[error("invalid machine target: {0}")]
    Target(String),
    #[error("the job has nothing to mark")]
    Empty,
    #[error(transparent)]
    Lmc(#[from] LmcError),
}

pub type Result<T, E = PlanError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests;
