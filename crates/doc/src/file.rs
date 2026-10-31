//! The native `.openetch` file: UTF-8 JSON `{"format": "openetch", "version": 1, "document": <Document>}`.
//!
//! Reading refuses other formats and newer versions with a message a person can act on, and repairs only one thing:
//! `next_id` is raised past every numbered id in the file, so a hand-edited file can never make new ids collide.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::Document;

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

/// Serialize `doc` as a `.openetch` file (compact JSON).
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
    if let Some(f) = doc.field_mm
        && !(f.is_finite() && f > 0.0)
    {
        return Err(FileError::InvalidDocument(format!("field_mm must be a positive number (got {f})")));
    }
    let mut seen = BTreeSet::new();
    for id in doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id)) {
        if !seen.insert(id.as_str()) {
            return Err(FileError::InvalidDocument(format!("id '{id}' is used more than once")));
        }
    }
    let max = seen.iter().filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok())).max();
    if let Some(m) = max {
        doc.next_id = doc.next_id.max(m.saturating_add(1));
    }
    Ok(())
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
        let id = d.alloc_id("obj");
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
}
