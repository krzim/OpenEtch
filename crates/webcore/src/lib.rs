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
    packed: Packed,
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

// ---------------------------------------------------------------------------------------------------------------
// The design-only build's backend
// ---------------------------------------------------------------------------------------------------------------

/// A [`backend::Reply`] as the plain JS object the worker shell posts back: `{status: number, headers: [[name,
/// value]], body: Uint8Array, events: [JSON string], writes: [{store, key, value: string | null}]}`.
fn reply_object(r: backend::Reply) -> js_sys::Object {
    let set = |o: &js_sys::Object, k: &str, v: &JsValue| {
        let _ = js_sys::Reflect::set(o, &JsValue::from_str(k), v);
    };
    let o = js_sys::Object::new();
    set(&o, "status", &JsValue::from(r.status));
    let headers = js_sys::Array::new();
    for (k, v) in &r.headers {
        headers.push(&js_sys::Array::of2(&JsValue::from_str(k), &JsValue::from_str(v)));
    }
    set(&o, "headers", &headers);
    set(&o, "body", &js_sys::Uint8Array::from(r.body.as_slice()));
    let events = js_sys::Array::new();
    for e in &r.events {
        events.push(&JsValue::from_str(e));
    }
    set(&o, "events", &events);
    let writes = js_sys::Array::new();
    for w in &r.writes {
        let wo = js_sys::Object::new();
        set(&wo, "store", &JsValue::from_str(&w.store));
        set(&wo, "key", &JsValue::from_str(&w.key));
        set(&wo, "value", &w.value.as_deref().map_or(JsValue::NULL, JsValue::from_str));
        writes.push(&wo);
    }
    set(&o, "writes", &writes);
    o
}

fn parse_init(init: &str) -> Result<backend::Init, JsError> {
    if init.trim().is_empty() {
        return Ok(backend::Init::default());
    }
    serde_json::from_str(init).map_err(|e| JsError::new(&format!("init: {e}")))
}

/// The design build's backend as a JS class (the worker keeps one).
#[wasm_bindgen]
pub struct Backend {
    inner: backend::Backend,
}

#[wasm_bindgen]
impl Backend {
    /// `init`: `{stores: [{store, key, value}], epoch?}` as JSON (`""` for a first start).
    #[wasm_bindgen(constructor)]
    pub fn new(init: &str) -> Result<Backend, JsError> {
        Ok(Backend { inner: backend::Backend::new(parse_init(init)?) })
    }

    /// Answer one request (see [`handle`]).
    pub fn handle(&mut self, method: &str, path: &str, query: &str, body: Vec<u8>, since: Option<String>) -> js_sys::Object {
        reply_object(self.inner.handle(method, path, query, body, since.as_deref()))
    }

    /// Problems with stored records found at start, as a JSON array of text.
    pub fn notes(&self) -> String {
        serde_json::to_string(&self.inner.notes).unwrap_or_else(|_| "[]".into())
    }

    /// The display preview of the active document, packed (as [`Core::preview`], without a copy of the document).
    pub fn preview(&self) -> PackedPreview {
        let ctx = preview::Context::from(self.inner.preview_context());
        PackedPreview { packed: Core::packed(self.inner.document(), &ctx) }
    }

    /// The preview's summary only, as JSON, or `{"error": ...}` (as [`Core::stats`]).
    pub fn stats(&self) -> String {
        let ctx = preview::Context::from(self.inner.preview_context());
        match preview::plan_preview(self.inner.document(), &ctx) {
            Ok(p) => preview::summary(self.inner.document(), p),
            Err(e) => preview::error_meta(&e),
        }
        .to_string()
    }

    /// The active document's revision (what [`Backend::preview`] is of).
    pub fn revision(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let r = self.inner.document().revision as f64;
        r
    }
}

thread_local! {
    /// The backend of [`init`] and [`handle`].
    static BACKEND: std::cell::RefCell<Option<backend::Backend>> = const { std::cell::RefCell::new(None) };
}

/// Start (or restart) the worker's backend from the stored records: `init` is `{stores: [{store, key, value}],
/// epoch?}` as JSON. Returns the problems found with the records (a JSON array of text).
#[wasm_bindgen]
pub fn init(init: &str) -> Result<String, JsError> {
    let b = backend::Backend::new(parse_init(init)?);
    let notes = serde_json::to_string(&b.notes).unwrap_or_else(|_| "[]".into());
    BACKEND.with(|cell| *cell.borrow_mut() = Some(b));
    Ok(notes)
}

/// Answer one `/api/...` (or shell `/core/...`) request: `method`, `path`, `query` (without `?`), the body bytes and
/// the optional `X-OpenEtch-Since` value. Returns `{status, headers: [[k, v]], body: Uint8Array, events: [JSON
/// string], writes: [{store, key, value | null}]}`. Starts an empty backend if [`init`] was not called.
#[wasm_bindgen]
pub fn handle(method: &str, path: &str, query: &str, body: Vec<u8>, since: Option<String>) -> js_sys::Object {
    let reply = BACKEND.with(|cell| {
        let mut cell = cell.borrow_mut();
        let b = cell.get_or_insert_with(|| backend::Backend::new(backend::Init::default()));
        b.handle(method, path, query, body, since.as_deref())
    });
    reply_object(reply)
}
