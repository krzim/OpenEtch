//! The in-browser core: WebAssembly run in a dedicated Web Worker (web/src/wasm/previewWorker.ts), so the web app
//! plans the **display** preview and its stats itself instead of downloading the engine's (tens of MB of JSON for an
//! engraved image), and answers Fill Region hovers without a round trip.
//!
//! Display only. Nothing here reaches the laser: every run is still planned by the engine from its own document
//! (ARCHITECTURE.md, safety model), and only the engine's `approve` module arms a run. The browser's preview is a
//! picture of what the engine would plan; the engine parity tests (crates/engine/src/webcore_parity_tests.rs) check
//! that it is the same picture.
//!
//! The wasm interface ([`Core`]) takes JSON strings in (the engine's `GET /api/document` and
//! `GET /api/preview/context`) and gives the preview back as flat typed arrays ([`preview::Packed`]) that the worker
//! transfers to the page without copying.
//!
//! The design-only build (docs/design-only/plan.md) also runs the engine's document API here ([`backend`]): the
//! worker hosts one [`backend::Backend`] and answers every `/api/...` request of the web app with
//! [`handle`] (or the [`Backend`] class). Its preview is planned from its own document ([`Backend::preview`]), so
//! the design build needs no second worker.

#![forbid(unsafe_code)]

#[cfg(feature = "design")]
pub mod backend;
pub mod fill;
pub mod preview;

use doc::Document;
use serde_json::Value;
use wasm_bindgen::prelude::*;

use preview::{Context, Packed};

/// The worker's state: the document of the last revision it was given, the engine's planning context and the
/// prepared Fill Region boundaries.
#[wasm_bindgen]
#[derive(Debug, Default)]
pub struct Core {
    doc: Option<Document>,
    ctx: Option<Context>,
    fill: fill::Cache,
}

/// A packed preview handed to JS: each array is taken once (moved out, then copied into a JS typed array by
/// wasm-bindgen), so the worker can transfer it.
#[wasm_bindgen]
#[derive(Debug, Default)]
pub struct PackedPreview {
    pub(crate) packed: Packed,
}

#[wasm_bindgen]
impl PackedPreview {
    /// The preview without its segments, as JSON (or `{"error": ...}`).
    pub fn meta(&self) -> String {
        self.packed.meta.to_string()
    }
    pub fn take_xy(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.packed.xy)
    }
    pub fn take_t(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.packed.t)
    }
    pub fn take_info(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.packed.info)
    }
    pub fn take_pass(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.packed.pass)
    }
    pub fn take_power(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.packed.power)
    }
    /// The image rasters' dots (`meta.rasters` indexes them).
    pub fn take_levels(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.packed.levels)
    }
    pub fn take_row_segments(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.packed.row_segments)
    }
    pub fn take_row_span(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.packed.row_span)
    }
}

#[wasm_bindgen]
impl Core {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Core {
        Core::default()
    }

    /// Replace the document (the engine's `GET /api/document` JSON); returns its revision.
    pub fn set_document(&mut self, json: &str) -> Result<f64, JsError> {
        let d: Document = serde_json::from_str(json).map_err(|e| JsError::new(&format!("document: {e}")))?;
        #[allow(clippy::cast_precision_loss)]
        let revision = d.revision as f64;
        self.doc = Some(d);
        self.fill.clear();
        Ok(revision)
    }

    /// Replace the planning context (the engine's `GET /api/preview/context` JSON).
    pub fn set_context(&mut self, json: &str) -> Result<(), JsError> {
        self.ctx = Some(serde_json::from_str(json).map_err(|e| JsError::new(&format!("preview context: {e}")))?);
        Ok(())
    }

    /// The preview of the current document, packed (its meta is `{"error": ...}` when it cannot be planned).
    pub fn preview(&self) -> Result<PackedPreview, JsError> {
        let (doc, ctx) = self.inputs()?;
        Ok(PackedPreview { packed: Self::packed(doc, ctx) })
    }

    /// The preview's summary only (stats, passes, layers, warnings), as JSON, or `{"error": ...}`.
    pub fn stats(&self) -> Result<String, JsError> {
        let (doc, ctx) = self.inputs()?;
        Ok(match preview::plan_preview(doc, ctx) {
            Ok(p) => preview::summary(doc, p),
            Err(e) => preview::error_meta(&e),
        }
        .to_string())
    }

    /// `path.fill_region_preview` for these params (JSON), as the engine's result JSON; an error is
    /// `{"error": ...}` (`"unsupported here"` for options only the engine takes).
    pub fn fill_region_preview(&mut self, params: &str) -> Result<String, JsError> {
        let doc = self.doc.as_ref().ok_or_else(|| JsError::new("no document yet"))?;
        let params: Value = serde_json::from_str(params).map_err(|e| JsError::new(&format!("params: {e}")))?;
        Ok(match fill::preview(doc, &mut self.fill, &params) {
            Ok(v) => v,
            Err(e) => preview::error_meta(&e),
        }
        .to_string())
    }
}

impl Core {
    fn inputs(&self) -> Result<(&Document, &Context), JsError> {
        let doc = self.doc.as_ref().ok_or_else(|| JsError::new("no document yet"))?;
        let ctx = self.ctx.as_ref().ok_or_else(|| JsError::new("no preview context yet"))?;
        Ok((doc, ctx))
    }

    /// The packed preview of `doc` (natively too, for tests).
    pub fn packed(doc: &Document, ctx: &Context) -> Packed {
        match preview::plan_preview(doc, ctx) {
            Ok(p) => preview::pack(doc, p),
            Err(e) => Packed { meta: preview::error_meta(&e), ..Packed::default() },
        }
    }
}

#[cfg(feature = "design")]
mod design;
#[cfg(feature = "design")]
pub use design::{Backend, handle, init};
