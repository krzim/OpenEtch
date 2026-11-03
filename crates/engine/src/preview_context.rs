//! `GET /api/preview/context`: what a browser needs to plan the display preview itself (crates/webcore, in a Web
//! Worker) exactly as `GET /api/preview` does: the active device's planning [`Target`], its learned estimate
//! correction (module `marktime`), the job counter (the next run's index, for auto-rotating fills) and why a
//! design-only device cannot be planned for. A few kB, against tens of MB for the preview of an engraved image.
//!
//! Display only: a browser-computed preview never reaches the laser. Runs are planned here from the engine's own
//! document (`AppState::request_run_with`) and armed only by module `approve`. The web app refetches this after a
//! device change (`device_changed`) and a finished run (`run_finished`), the only events that change it.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use plan::Target;
use serde::{Deserialize, Serialize};

use crate::AppState;

/// The body of `GET /api/preview/context` (webcore's `preview::Context`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewContext {
    /// The active device's planning target (field, limits, timing, lens correction).
    pub target: Target,
    /// Multiply every estimated time by this (1 until the device has enough finished runs).
    pub correction: f64,
    /// The next run's index on the active device.
    pub run_index: u64,
    /// Why the active device cannot be planned for, or `None`.
    pub design_only: Option<String>,
    /// The machine configuration's generation: it changes with the device.
    pub generation: u64,
}

impl AppState {
    /// The context [`AppState::preview`] plans with, as of now.
    pub fn preview_context(&self) -> PreviewContext {
        let device = self.active_device();
        let config = self.machine.config();
        PreviewContext {
            target: config.target.clone(),
            correction: self.job.times.correction(&device.id).factor,
            run_index: self.job.count(&device.id),
            design_only: device.not_controllable_reason(),
            generation: config.generation,
        }
    }
}

/// `GET /api/preview/context`.
pub async fn handler(State(st): State<Arc<AppState>>) -> Json<PreviewContext> {
    Json(st.preview_context())
}
