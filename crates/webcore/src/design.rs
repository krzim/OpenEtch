//! The design-only build's entry points that are not HTTP routes (feature `design`; docs/design-only/plan.md, WP3):
//! fonts the browser has, and a plain command session.
//!
//! - [`add_font`] / [`remove_font`]: TrueType/OpenType (`.ttf`, `.otf`, `.ttc`), `.shx` and `.jsf` bytes that the
//!   worker shell keeps in IndexedDB (uploads from the Font picker, Local Font Access) and adds at every start, so
//!   `font.list` lists them and text can use them. The bundled Hershey fonts are always there.
//! - [`Commands`]: one [`commands::Session`] run by command id and JSON params, the same calls the engine's
//!   `POST /api/commands` makes. The WP2 router (`handle`) supersedes it; until then it is how Node tests reach the
//!   command core in wasm (`web/scripts/wasm-design-test.mjs`).
//!
//! The clock for `{date}` / `{time}` variable text needs no call: on wasm32 crates/commands builds text with
//! `wasm-clock`, which reads JS `Date` (the time and the browser's time zone).

use commands::{Invocation, Origin, Session};
use doc::Document;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// Add a font from its bytes. `family` names a single-line `.shx` / `.jsf` font (TrueType/OpenType fonts are named
/// by their own family name). Adding a face again replaces it. Returns `{"families": [...]}` (the family names
/// added) or `{"error": ...}`.
#[wasm_bindgen]
pub fn add_font(family: &str, bytes: Vec<u8>) -> String {
    match text::registry::add_font_data(family, bytes) {
        Ok(families) => json!({ "families": families }),
        Err(e) => json!({ "error": e }),
    }
    .to_string()
}

/// Remove the fonts added under `family` (any kind, any case); returns how many faces went. Bundled fonts stay.
#[wasm_bindgen]
pub fn remove_font(family: &str) -> u32 {
    u32::try_from(text::registry::remove_font_data(family)).unwrap_or(u32::MAX)
}

/// A command session on one document.
#[wasm_bindgen]
pub struct Commands {
    session: Session,
}

impl Default for Commands {
    fn default() -> Self {
        Commands { session: Session::new(Document::default()) }
    }
}

#[wasm_bindgen]
impl Commands {
    /// A session on an empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Commands {
        Commands::default()
    }

    /// Run one command (`id`, params as JSON; empty means `{}`), as the user would. Returns `{"result", "revision"}`
    /// or `{"error": ...}`.
    pub fn run(&mut self, id: &str, params: &str) -> String {
        let params: Value = if params.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str(params) {
                Ok(v) => v,
                Err(e) => return json!({ "error": format!("params: {e}") }).to_string(),
            }
        };
        match self.session.execute(Invocation { id: id.into(), params }, Origin::Ui) {
            Ok(o) => json!({ "result": o.result, "revision": o.revision }),
            Err(e) => json!({ "error": e.to_string() }),
        }
        .to_string()
    }

    /// The document, as the engine's `GET /api/document` JSON.
    pub fn document(&self) -> String {
        serde_json::to_string(self.session.document()).unwrap_or_else(|e| json!({ "error": e.to_string() }).to_string())
    }

    /// The ids of every command this build has, as a JSON array (the design UI disables what is missing).
    pub fn ids(&self) -> String {
        json!(self.session.registry().specs().map(|c| c.id).collect::<Vec<_>>()).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_and_commands_natively() {
        let added: Value = serde_json::from_str(&add_font("x", include_bytes!("../../text/tests/fixtures/ttf/openetch-box.ttf").to_vec())).unwrap();
        assert_eq!(added, json!({ "families": ["OpenEtch Box"] }));
        let bad: Value = serde_json::from_str(&add_font("junk", b"junk".to_vec())).unwrap();
        assert!(bad["error"].as_str().unwrap().contains("not a TrueType"), "{bad}");

        let mut c = Commands::new();
        let ids: Vec<String> = serde_json::from_str(&c.ids()).unwrap();
        for id in ["art.import_dxf", "art.import_pdf", "art.import_ai", "font.list", "shape.text"] {
            assert!(ids.iter().any(|i| i == id), "{id} missing");
        }
        let list: Value = serde_json::from_str(&c.run("font.list", "")).unwrap();
        assert!(list["result"].as_array().unwrap().iter().any(|f| f["family"] == "OpenEtch Box"), "{list}");
        let r: Value = serde_json::from_str(&c.run("shape.text", r#"{"text": "OE", "font": {"family": "OpenEtch Box"}, "height_mm": 10}"#)).unwrap();
        assert!(r["result"]["created"][0].is_string(), "{r}");
        assert!(c.document().contains("OpenEtch Box"));
        let e: Value = serde_json::from_str(&c.run("nope.nope", "{}")).unwrap();
        assert!(e["error"].as_str().unwrap().contains("unknown command"), "{e}");
        assert_eq!(remove_font("OpenEtch Box"), 1);
    }
}
