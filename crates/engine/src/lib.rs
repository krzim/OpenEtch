//! The OpenEtch engine: one process that owns the live document and the laser, and serves them to the web UI
//! (HTTP + WebSocket) and to AI agents (MCP, Streamable HTTP at `/mcp`). See ARCHITECTURE.md.
//!
//! - [`AppState`]: the [`commands::Session`], the [`hw::Machine`] (controller behind its own mutex, driven from
//!   blocking tasks), pending runs and the event channel. HTTP handlers and MCP tools both call its methods, so the
//!   two surfaces behave the same.
//! - [`api::router`]: the axum router (`/api/*`, `/mcp`, static files with SPA fallback) behind
//!   [`security::guard`] (Host/Origin checks against DNS rebinding, bearer token off loopback).
//! - Safety: framing goes through `machine::Controller::frame` (light only). Nothing fires unless a person approves
//!   a pending run with `POST /api/runs/{id}/approve` and the operator PIN ([`pin`]), plus the physical input when
//!   configured; `approve` is the only module that creates a `machine::Armed`, and the endpoint refuses requests that
//!   look like they came from an MCP client. Stop is never gated and never waits behind a run (stop epoch, see
//!   `approve`). A local process with the user's OS privileges can defeat these software checks; the hardware
//!   E-stop/key switch is the real guarantee (the physical input only protects against a guessed or stolen PIN
//!   used through the API).
#![forbid(unsafe_code)]

pub mod api;
mod approve;
pub mod cli;
pub mod error;
pub mod events;
pub mod frame;
pub mod hw;
pub mod mcp;
pub mod pin;
pub mod projects;
pub mod render;
pub mod runs;
pub mod security;
pub mod state;

pub use error::EngineError;
pub use events::Event;
pub use state::AppState;

#[cfg(test)]
mod tests;
