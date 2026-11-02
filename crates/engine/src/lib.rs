//! The OpenEtch engine: one process that owns the live document and the laser, and serves them to the web UI
//! (HTTP + WebSocket) and to AI agents (MCP, Streamable HTTP at `/mcp`). See ARCHITECTURE.md.
//!
//! - [`AppState`]: the [`commands::Session`], the [`hw::Machine`] (controller behind its own mutex, driven from
//!   blocking tasks), pending runs and the event channel. HTTP handlers and MCP tools both call its methods, so the
//!   two surfaces behave the same.
//! - [`api::router`]: the axum router (`/api/*`, `/mcp`, static files with SPA fallback) behind
//!   [`security::guard`] (Host/Origin checks against DNS rebinding, bearer token off loopback).
//! - Safety: framing goes through `machine::Controller::frame` (light only). Nothing fires unless a person approves
//!   a pending run with `POST /api/runs/{id}/approve` and the operator PIN ([`pin`]) (or, with `--approval click`,
//!   Start in the app in a browser (`POST /api/job/start` plans and approves in one action) or the foot switch
//!   cycle, module `cycle`), plus the physical input when configured; `approve` is the only module that creates a `machine::Armed`, and the endpoint refuses requests that
//!   look like they came from an MCP client. Stop is never gated and never waits behind a run (stop epoch, see
//!   `approve`). A local process with the user's OS privileges can defeat these software checks; the hardware
//!   E-stop/key switch is the real guarantee (the physical input only protects against a guessed or stolen PIN
//!   used through the API).
#![forbid(unsafe_code)]

pub mod api;
mod api_machine;
mod approve;
mod cycle;
pub mod cli;
pub mod devices;
pub mod error;
pub mod events;
pub mod frame;
pub mod hw;
pub mod job;
pub mod marktime;
pub mod lightburn;
pub mod mcp;
pub mod pin;
pub mod projects;
pub mod render;
pub mod runs;
pub mod security;
pub mod state;
// Libraries, preferences, job log, diagnostics, camera, updates (see `libs`).
mod api_libs;
pub mod camera;
pub mod markers;
mod camera_capture;
pub mod diagnostics;
pub mod joblog;
pub mod libs;
pub mod library;
pub mod prefs;
// LightBurn .lbprefs / prefs.ini import (typed whitelist of keys).
pub mod lbprefs;
pub mod lbzip;
pub mod updates;
pub mod userfiles;
// Several open documents (File > New Window) and the Art Library.
pub mod artlib;
pub mod documents;
// Project folders and locations.
pub mod folders;
// Material test presets (and LightBurn .lbmt).
pub mod test_presets;

pub use error::EngineError;
pub use events::Event;
pub use state::AppState;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod job_tests;
#[cfg(test)]
mod libs_tests;
#[cfg(test)]
mod userfiles_tests;
#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod final_tests;
