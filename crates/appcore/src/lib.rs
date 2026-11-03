//! The engine's document glue, shared with the in-browser core (crates/webcore) so both answer the same requests
//! with the same JSON (crates/webcore/tests/parity.rs checks it). Synchronous and wasm-clean: no tokio, no axum, no
//! `std::fs`. The engine keeps its locks, files, machine and broadcast channel and calls in here; webcore keeps
//! everything in memory and hands the store writes and events to its worker shell.
//!
//! - [`docset`]: the open documents and their activation; [`project`]: project names and `GET /api/project`.
//! - [`batch`]: a batch with the Query/Edit risk gate, the field and layout fill, the project state and
//!   `field_source`; [`layout`]: the layouts of new documents and LightBurn imports.
//! - [`upload`]: which command an uploaded or imported file runs, image sniffing, downloads.
//! - [`preview`]: planning targets, the preview context, the preview plan and its JSON; [`render`]: the preview SVG
//!   (and PNG with the `png` feature).
//! - [`docsync`]: document change sets; [`event`]: the events; [`http`]: request parsing both routers share.
//! - [`stores`]: typed in-memory stores (preferences, presets, libraries, devices, ...) whose mutations return the
//!   [`stores::StoreWrite`]s to persist.

#![forbid(unsafe_code)]

pub mod batch;
pub mod docset;
pub mod docsync;
pub mod error;
pub mod event;
pub mod http;
pub mod layout;
pub mod preview;
pub mod project;
pub mod render;
pub mod upload;

pub use error::{AppError, AppResult};
pub use event::Event;
