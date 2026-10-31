//! The native `.openetch` file: UTF-8 JSON `{"format": "openetch", "version": 1, "document": <Document>}`.
//!
//! Reading refuses other formats and newer versions with a message a person can act on, and repairs only one thing:
//! `next_id` is raised past every numbered id in the file, so a hand-edited file can never make new ids collide.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::{Document, Geometry, MAX_ID, is_extras_key};

/// The `format` tag of a `.openetch` file.
pub const FORMAT: &str = "openetch";
/// The newest file format version this build reads and the one it writes.
pub const VERSION: u64 = 1;
/// File name extension, without the dot.
pub const EXTENSION: &str = "openetch";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FileError {
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("not an OpenEtch file: {0}")]
    NotOpenEtch(String),
    #[error(
        "the file uses OpenEtch file format version {found}, but this OpenEtch reads versions up to {supported}; \
         update OpenEtch to open it"
    )]
    NewerVersion { found: u64, supported: u64 },
    #[error("invalid OpenEtch document: {0}")]
    InvalidDocument(String),
}

#[derive(Serialize)]
struct FileOut<'a> {
    format: &'static str,
    version: u64,
    document: &'a Document,
}

/// Serialize `doc` as a `.openetch` file (compact JSON), refusing a document that [`from_openetch_json`] would not
/// read back: JSON has no NaN or infinity (serde writes them as `null`, which then fails to load), so writing such a
/// document would lose the project.
pub fn try_to_openetch_json(doc: &Document) -> Result<String, FileError> {
    check(doc).map_err(|e| match e {
        FileError::InvalidDocument(m) => FileError::InvalidDocument(format!("cannot save: {m}")),
        other => other,
    })?;
    Ok(to_openetch_json(doc))
}

/// Serialize `doc` as a `.openetch` file (compact JSON) without checking it. Only for documents that are known to
/// be sound, such as a `commands::Session`'s (commands refuse to commit non-finite geometry); prefer
/// [`try_to_openetch_json`], which refuses a document that would not load again.
pub fn to_openetch_json(doc: &Document) -> String {
    serde_json::to_string(&FileOut { format: FORMAT, version: VERSION, document: doc })
        .expect("a Document always serializes to JSON")
}

/// Parse a `.openetch` file.
pub fn from_openetch_json(text: &str) -> Result<Document, FileError> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('<') {
        return Err(FileError::NotOpenEtch(
            "this is XML (a LightBurn .lbrn2 or an SVG?), not an OpenEtch project; import it instead".into(),
        ));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| FileError::Json(e.to_string()))?;
    let obj = v.as_object().ok_or_else(|| FileError::NotOpenEtch("the JSON is not an object".into()))?;
    match obj.get("format") {
        Some(Value::String(f)) if f == FORMAT => {}
        Some(other) => return Err(FileError::NotOpenEtch(format!("\"format\" is {other}, expected \"{FORMAT}\""))),
        None => return Err(FileError::NotOpenEtch("there is no \"format\": \"openetch\" field".into())),
    }
    let version = match obj.get("version").and_then(Value::as_u64) {
        Some(v) if v >= 1 => v,
        _ => return Err(FileError::InvalidDocument("\"version\" must be a positive integer".into())),
    };
    if version > VERSION {
        return Err(FileError::NewerVersion { found: version, supported: VERSION });
    }
    let doc_value = obj.get("document").ok_or_else(|| FileError::InvalidDocument("no \"document\" field".into()))?;
    let mut doc: Document =
        serde_json::from_value(doc_value.clone()).map_err(|e| FileError::InvalidDocument(e.to_string()))?;
    validate(&mut doc)?;
    Ok(doc)
}

/// Structural checks a deserialized document can still fail, plus the `next_id` repair.
fn validate(doc: &mut Document) -> Result<(), FileError> {
    check(doc)?;
    if let Some(m) = numbered_ids(doc).max() {
        // Below MAX_ID (checked), so this cannot overflow.
        doc.next_id = doc.next_id.max(m + 1);
    }
    Ok(())
}

/// The numeric suffixes of the ids, e.g. 12 for "obj-12".
fn numbered_ids(doc: &Document) -> impl Iterator<Item = u64> + '_ {
    let ids = doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id));
    ids.filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()))
}

fn invalid(m: impl Into<String>) -> FileError {
    FileError::InvalidDocument(m.into())
}

/// Every rule a loaded (or saved) document must meet: what the edit commands guarantee, so a loaded document behaves
/// like an edited one.
fn check(doc: &Document) -> Result<(), FileError> {
    if let Some(f) = doc.field_mm
        && !(f.is_finite() && f > 0.0)
    {
        return Err(invalid(format!("field_mm must be a positive number (got {f})")));
    }
    if doc.layers.is_empty() {
        return Err(invalid("the document has no layers; it needs at least one"));
    }
    let mut seen = BTreeSet::new();
    for id in doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id)) {
        if !seen.insert(id.as_str()) {
            return Err(invalid(format!("id '{id}' is used more than once")));
        }
    }
    if let Some(n) = numbered_ids(doc).find(|n| *n >= MAX_ID) {
        return Err(invalid(format!("id number {n} is too large (ids must stay below 2^53)")));
    }
    if doc.next_id > MAX_ID {
        return Err(invalid(format!("next_id {} is too large (ids must stay below 2^53)", doc.next_id)));
    }
    let names = std::iter::once(("the document name", &doc.name))
        .chain(doc.layers.iter().map(|l| ("a layer name", &l.name)))
        .chain(doc.objects.iter().map(|o| ("an object name", &o.name)))
        .chain(doc.stock.iter().map(|s| ("the stock name", &s.name)));
    for (what, name) in names {
        if name.chars().any(char::is_control) {
            return Err(invalid(format!("{what} contains a control character: {name:?}")));
        }
    }
    for l in &doc.layers {
        if let Some(k) = l.extras.keys().find(|k| !is_extras_key(k)) {
            let k: String = k.chars().take(80).collect();
            return Err(invalid(format!("layer {} has an invalid extras key {k:?}", l.id)));
        }
    }
    for o in &doc.objects {
        if doc.layer(&o.layer).is_none() {
            return Err(invalid(format!("object {} is on layer '{}', which does not exist", o.id, o.layer)));
        }
        let size_ok = match o.geometry {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                width_mm > 0.0 && height_mm > 0.0 && corner_radius_mm >= 0.0
            }
            Geometry::Ellipse { rx_mm, ry_mm } => rx_mm > 0.0 && ry_mm > 0.0,
            Geometry::Path { .. } => true,
        };
        if !size_ok {
            return Err(invalid(format!("object {} has a size that is not positive", o.id)));
        }
    }
    match doc.numeric_problem() {
        Some(p) => Err(invalid(p)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Geometry, Object};
    use kurbo::Affine;
    use serde_json::json;

    fn sample() -> Document {
        let mut d = Document { name: "Coaster".into(), field_mm: Some(153.75), ..Document::default() };
        d.layers[0].extras.insert("lbrn2.priority".into(), "0".into());
        let id = d.alloc_id("obj").unwrap();
        d.objects.push(Object {
            id,
            name: "ring".into(),
            layer: "layer-1".into(),
            geometry: Geometry::Ellipse { rx_mm: 10.0, ry_mm: 8.0 },
            transform: Affine::translate((1.0, 2.0)),
        });
        d
    }

    #[test]
    fn round_trips() {
        let d = sample();
        let text = to_openetch_json(&d);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["format"], json!("openetch"));
        assert_eq!(v["version"], json!(1));
        assert_eq!(from_openetch_json(&text).unwrap(), d);
        // A UTF-8 BOM (some editors add one) is accepted.
        assert_eq!(from_openetch_json(&format!("\u{feff}{text}")).unwrap(), d);
    }

    #[test]
    fn refuses_other_formats_and_newer_versions() {
        let doc = serde_json::to_value(Document::default()).unwrap();
        let e = from_openetch_json(r#"<?xml version="1.0"?><LightBurnProject/>"#).unwrap_err();
        assert!(matches!(e, FileError::NotOpenEtch(_)) && e.to_string().contains("LightBurn"), "{e}");
        assert!(matches!(from_openetch_json("{nope"), Err(FileError::Json(_))));
        let e = from_openetch_json(&json!({"format": "svg", "version": 1, "document": doc}).to_string()).unwrap_err();
        assert!(matches!(e, FileError::NotOpenEtch(_)), "{e}");
        assert!(matches!(from_openetch_json(&json!({"version": 1}).to_string()), Err(FileError::NotOpenEtch(_))));
        let newer = json!({"format": "openetch", "version": 2, "document": doc}).to_string();
        let e = from_openetch_json(&newer).unwrap_err();
        assert_eq!(e, FileError::NewerVersion { found: 2, supported: 1 });
        assert!(e.to_string().contains("update OpenEtch"), "{e}");
        for bad in [json!(0), json!(-1), json!("1"), json!(1.5)] {
            let t = json!({"format": "openetch", "version": bad, "document": doc}).to_string();
            assert!(matches!(from_openetch_json(&t), Err(FileError::InvalidDocument(_))), "{bad}");
        }
        let t = json!({"format": "openetch", "version": 1}).to_string();
        assert!(matches!(from_openetch_json(&t), Err(FileError::InvalidDocument(_))));
        let t = json!({"format": "openetch", "version": 1, "document": {"name": 3}}).to_string();
        assert!(matches!(from_openetch_json(&t), Err(FileError::InvalidDocument(_))));
    }

    #[test]
    fn checks_ids_and_repairs_next_id() {
        let mut d = sample();
        d.next_id = 1;
        let back = from_openetch_json(&to_openetch_json(&d)).unwrap();
        assert_eq!(back.next_id, 3, "raised past obj-2");
        d.objects[0].id = "layer-1".into();
        let e = from_openetch_json(&to_openetch_json(&d)).unwrap_err();
        assert!(e.to_string().contains("more than once"), "{e}");
        let mut d = sample();
        d.field_mm = Some(-1.0);
        assert!(from_openetch_json(&to_openetch_json(&d)).is_err());
    }

    fn refused(d: &Document, what: &str) {
        let e = from_openetch_json(&to_openetch_json(d)).unwrap_err();
        assert!(matches!(e, FileError::InvalidDocument(_)) && e.to_string().contains(what), "{what}: {e}");
    }

    #[test]
    fn refuses_ids_that_would_overflow_next_id() {
        let mut d = sample();
        d.objects[0].id = "obj-18446744073709551615".into();
        refused(&d, "too large");
        d.objects[0].id = format!("obj-{}", MAX_ID);
        refused(&d, "too large");
        // The largest allowed id loads, and the next allocation fails cleanly instead of wrapping.
        d.objects[0].id = format!("obj-{}", MAX_ID - 1);
        let mut back = from_openetch_json(&to_openetch_json(&d)).unwrap();
        assert_eq!(back.next_id, MAX_ID);
        assert!(back.alloc_id("obj").is_err());
        let mut d = sample();
        d.next_id = u64::MAX;
        refused(&d, "next_id");
    }

    #[test]
    fn refuses_documents_the_commands_would_never_make() {
        let mut d = sample();
        d.objects[0].layer = "layer-404".into();
        refused(&d, "does not exist");
        let mut d = sample();
        d.layers.clear();
        d.objects.clear();
        refused(&d, "no layers");
        let mut d = sample();
        d.objects[0].geometry = Geometry::Rect { width_mm: -3.0, height_mm: 1.0, corner_radius_mm: 0.0 };
        refused(&d, "not positive");
        let mut d = sample();
        d.objects[0].geometry = Geometry::Ellipse { rx_mm: 0.0, ry_mm: 1.0 };
        refused(&d, "not positive");
        let mut d = sample();
        d.objects[0].transform = Affine::new([0.0; 6]);
        refused(&d, "cannot be inverted");
        let mut d = sample();
        d.name = "a\u{0}b".into();
        refused(&d, "control character");
        let mut d = sample();
        d.layers[0].name = "C\u{1}".into();
        refused(&d, "control character");
        let mut d = sample();
        d.objects[0].name = "x\ny".into();
        refused(&d, "control character");
        for key in ["lbrn2.evil a=\"1\"/><Shape Type=\"Rect\" W=\"1\" H=\"1\"/><x", "lbrn2.", "noprefix"] {
            let mut d = sample();
            d.layers[0].extras.insert(key.into(), "1".into());
            refused(&d, "extras key");
        }
    }

    #[test]
    fn saving_refuses_what_loading_would_refuse() {
        let d = sample();
        assert_eq!(try_to_openetch_json(&d).unwrap(), to_openetch_json(&d));
        let mut d = sample();
        d.objects[0].transform = Affine::scale(1e200) * Affine::scale(1e200);
        assert!(to_openetch_json(&d).contains("null"), "the unchecked writer loses the value");
        let e = try_to_openetch_json(&d).unwrap_err();
        assert!(matches!(e, FileError::InvalidDocument(_)) && e.to_string().contains("cannot save"), "{e}");
        let mut d = sample();
        d.layers[0].settings.power_pct = f64::NAN;
        assert!(try_to_openetch_json(&d).is_err());
    }
}
