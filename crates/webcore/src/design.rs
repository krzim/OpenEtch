//! The wasm exports of the design-only build's backend (feature `design`): [`init`], [`handle`] and the
//! [`Backend`] class. See [`crate::backend`].

use wasm_bindgen::prelude::*;

use crate::backend;
use crate::preview;
use crate::{Core, PackedPreview};

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
    /// Fill Region's prepared boundaries (as [`Core`]'s), for hovers on the active document.
    fill: crate::fill::Cache,
}

#[wasm_bindgen]
impl Backend {
    /// `init`: `{stores: [{store, key, value}], epoch?}` as JSON (`""` for a first start).
    #[wasm_bindgen(constructor)]
    pub fn new(init: &str) -> Result<Backend, JsError> {
        Ok(Backend { inner: backend::Backend::new(parse_init(init)?), fill: crate::fill::Cache::default() })
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

    /// Fill Region's hover answer on the active document (as [`Core::fill_region_preview`]), as JSON.
    pub fn fill_region_preview(&mut self, params: &str) -> Result<String, JsError> {
        let params: serde_json::Value = serde_json::from_str(params).map_err(|e| JsError::new(&format!("params: {e}")))?;
        Ok(match crate::fill::preview(self.inner.document(), &mut self.fill, &params) {
            Ok(v) => v,
            Err(e) => preview::error_meta(&e),
        }
        .to_string())
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
