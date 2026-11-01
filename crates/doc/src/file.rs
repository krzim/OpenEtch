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

/// The structural checks [`from_openetch_json`] makes (unique ids, names, sizes, layers, groups without cycles,
/// numbers, pages), for a document put together some other way (an Art Library item, see `commands::ArtFragment`).
pub fn check_document(doc: &Document) -> Result<(), FileError> {
    check(doc)
}

/// Structural checks a deserialized document can still fail, plus the `next_id` repair.
fn validate(doc: &mut Document) -> Result<(), FileError> {
    check(doc)?;
    if let Some(m) = numbered_ids(doc).max() {
        // Below MAX_ID (checked), so this cannot overflow.
        doc.next_id = doc.next_id.max(m + 1);
    }
    // A hand-edited file may hold groups without members; they mean nothing.
    doc.prune_groups();
    Ok(())
}

/// The numeric suffixes of the ids, e.g. 12 for "obj-12".
fn numbered_ids(doc: &Document) -> impl Iterator<Item = u64> + '_ {
    let pages = doc.pages.iter().flat_map(|p| p.objects.iter().map(|o| &o.id).chain(p.groups.iter().map(|g| &g.id)));
    let ids = doc
        .layers
        .iter()
        .map(|l| &l.id)
        .chain(doc.objects.iter().map(|o| &o.id))
        .chain(doc.groups.iter().map(|g| &g.id))
        .chain(doc.guides.iter().map(|g| &g.id))
        .chain(pages);
    ids.filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok()))
}

/// Whether `mime` looks like `image/<name>` (e.g. `image/png`, `image/svg+xml`).
pub fn is_image_mime(mime: &str) -> bool {
    mime.len() <= 64
        && mime.strip_prefix("image/").is_some_and(|t| {
            !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
        })
}

/// Whether `s` uses only the standard base64 alphabet (and `=` padding, at the end). Line breaks are not allowed.
pub fn is_base64(s: &str) -> bool {
    let body = s.trim_end_matches('=');
    s.len() - body.len() <= 2 && body.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
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
    if let Some([w, h]) = doc.workspace_mm
        && !(w.is_finite() && w > 0.0 && h.is_finite() && h > 0.0)
    {
        return Err(invalid(format!("workspace_mm must be two positive numbers (got [{w}, {h}])")));
    }
    if doc.layers.is_empty() {
        return Err(invalid("the document has no layers; it needs at least one"));
    }
    let mut seen = BTreeSet::new();
    for id in doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id)).chain(doc.groups.iter().map(|g| &g.id)) {
        if !seen.insert(id.as_str()) {
            return Err(invalid(format!("id '{id}' is used more than once")));
        }
    }
    // Guides take their ids from the same counter (`guide.add`), so they are unique among every id and count for
    // the `next_id` repair; at most as many as `guide.add` keeps, within the range it accepts.
    if doc.guides.len() > crate::MAX_GUIDES {
        return Err(invalid(format!("the document has {} guides (at most {})", doc.guides.len(), crate::MAX_GUIDES)));
    }
    for g in &doc.guides {
        if g.id.is_empty() || g.id.len() > 64 || g.id.chars().any(char::is_control) {
            return Err(invalid(format!("guide id {:?} is not usable", g.id.chars().take(80).collect::<String>())));
        }
        if !seen.insert(g.id.as_str()) {
            return Err(invalid(format!("id '{}' is used more than once", g.id)));
        }
        if !(g.position_mm.is_finite() && g.position_mm.abs() <= crate::MAX_GUIDE_MM) {
            return Err(invalid(format!("guide {} is at {} (at most ±{} mm)", g.id, g.position_mm, crate::MAX_GUIDE_MM)));
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
        .chain(doc.stock.iter().map(|s| ("the stock name", &s.name)))
        .chain(doc.groups.iter().map(|g| ("a group name", &g.name)));
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
            Geometry::Polygon { .. } => {
                if let Some(p) = o.geometry.polygon().and_then(|p| p.problem()) {
                    return Err(invalid(format!("object {}: {p}", o.id)));
                }
                true
            }
            Geometry::Path { .. } => true,
            Geometry::Text(ref t) => {
                if let Some(p) = t.problem() {
                    return Err(invalid(format!("object {}: {p}", o.id)));
                }
                if t.font.family.chars().any(char::is_control) || t.font.family.len() > 200 {
                    return Err(invalid(format!("object {} has an unusable font family name", o.id)));
                }
                if t.text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')) {
                    return Err(invalid(format!("object {}: the text holds a control character", o.id)));
                }
                true
            }
            Geometry::Image { width_mm, height_mm, ref mime, ref data_base64, ref adjust } => {
                if !is_image_mime(mime) {
                    return Err(invalid(format!("object {} has an image type {mime:?} that is not image/<name>", o.id)));
                }
                if !is_base64(data_base64) {
                    return Err(invalid(format!("object {} has image data that is not base64", o.id)));
                }
                if let Some(p) = adjust.problem() {
                    return Err(invalid(format!("object {}: {p}", o.id)));
                }
                width_mm > 0.0 && height_mm > 0.0
            }
        };
        if !size_ok {
            return Err(invalid(format!("object {} has a size that is not positive", o.id)));
        }
        if let Some(g) = &o.group
            && doc.group(g).is_none()
        {
            return Err(invalid(format!("object {} is in group '{g}', which does not exist", o.id)));
        }
    }
    for g in &doc.groups {
        if let Some(p) = &g.parent
            && doc.group(p).is_none()
        {
            return Err(invalid(format!("group {} is inside group '{p}', which does not exist", g.id)));
        }
        // Walking up from any group must reach the top within as many steps as there are groups.
        let mut at = g.parent.as_deref();
        for _ in 0..=doc.groups.len() {
            at = at.and_then(|p| doc.group(p)).and_then(|p| p.parent.as_deref());
        }
        if at.is_some() {
            return Err(invalid(format!("group {} is (indirectly) inside itself", g.id)));
        }
    }
    let extras = std::iter::once(("the document", &doc.extras))
        .chain(doc.layers.iter().map(|l| ("a layer", &l.extras)))
        .chain(doc.objects.iter().map(|o| ("an object", &o.extras)))
        .chain(doc.groups.iter().map(|g| ("a group", &g.extras)));
    for (what, map) in extras {
        if let Some(k) = map.keys().find(|k| !is_extras_key(k)) {
            let k: String = k.chars().take(80).collect();
            return Err(invalid(format!("{what} has an invalid extras key {k:?}")));
        }
    }
    if let Some(p) = doc.numeric_problem() {
        return Err(invalid(p));
    }
    check_pages(doc, seen)
}

/// Pages (LightBurn 2.2): the shapes of every other page become the document's own when that page is shown, so they
/// meet the same rules; ids stay unique across all pages; the active page exists and its own entry is empty (its
/// shapes are the document's).
fn check_pages<'a>(doc: &'a Document, mut seen: BTreeSet<&'a str>) -> Result<(), FileError> {
    if doc.pages.is_empty() {
        return Ok(());
    }
    let mut page_ids = BTreeSet::new();
    for p in &doc.pages {
        if !page_ids.insert(p.id.as_str()) || seen.contains(p.id.as_str()) {
            return Err(invalid(format!("page id '{}' is used more than once", p.id)));
        }
        if p.name.chars().any(char::is_control) {
            return Err(invalid(format!("a page name contains a control character: {:?}", p.name)));
        }
    }
    let active = doc.active_page.as_deref().and_then(|a| doc.pages.iter().find(|p| p.id == a));
    let Some(active) = active else {
        return Err(invalid("active_page does not name one of the pages"));
    };
    if !(active.objects.is_empty() && active.groups.is_empty()) {
        return Err(invalid(format!("the active page \"{}\" holds shapes of its own; they belong in the document", active.name)));
    }
    for p in &doc.pages {
        for id in p.objects.iter().map(|o| &o.id).chain(p.groups.iter().map(|g| &g.id)) {
            if !seen.insert(id.as_str()) {
                return Err(invalid(format!("id '{id}' is used more than once (page \"{}\")", p.name)));
            }
        }
        let view = Document {
            layers: doc.layers.clone(),
            objects: p.objects.clone(),
            groups: p.groups.clone(),
            next_id: doc.next_id,
            ..Document::default()
        };
        check(&view).map_err(|e| invalid(format!("page \"{}\": {e}", p.name)))?;
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
        let id = d.alloc_id("obj").unwrap();
        d.objects.push(Object {
            id,
            name: "ring".into(),
            layer: "layer-1".into(),
            geometry: Geometry::Ellipse { rx_mm: 10.0, ry_mm: 8.0 },
            transform: Affine::translate((1.0, 2.0)),
            group: None,
            extras: Default::default(),
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
        let mut d = sample();
        d.workspace_mm = Some([400.0, 0.0]);
        assert!(from_openetch_json(&to_openetch_json(&d)).is_err());
        d.workspace_mm = Some([400.0, 300.0]);
        assert_eq!(from_openetch_json(&to_openetch_json(&d)).unwrap().workspace_mm, Some([400.0, 300.0]));
    }

    /// A hand-edited file cannot make `guide.add` hand out an id a guide already has (the id counter is raised past
    /// guide ids too), nor hold duplicate, unusable or out-of-range guides.
    #[test]
    fn guide_ids_count_for_the_counter_and_are_checked() {
        use crate::{Guide, GuideAxis};
        let guide = |id: &str, p: f64| Guide { id: id.into(), axis: GuideAxis::X, position_mm: p };
        let mut d = sample();
        d.next_id = 1;
        d.guides = vec![guide("guide-50", 1.0)];
        let mut back = from_openetch_json(&to_openetch_json(&d)).unwrap();
        assert_eq!(back.next_id, 51, "raised past guide-50");
        assert_ne!(back.alloc_id("guide").unwrap(), "guide-50");
        let mut bad = d.clone();
        bad.guides.push(guide("guide-50", 2.0));
        refused(&bad, "more than once");
        let mut bad = d.clone();
        bad.guides[0].id = "obj-2".into();
        refused(&bad, "more than once");
        let mut bad = d.clone();
        bad.guides[0].id = "g\u{7}".into();
        refused(&bad, "not usable");
        let mut bad = d.clone();
        bad.guides[0].position_mm = 1e300;
        refused(&bad, "guide-50");
        let mut bad = d.clone();
        bad.guides = (0..=crate::MAX_GUIDES).map(|k| guide(&format!("guide-{}", k + 100), 0.0)).collect();
        refused(&bad, "guides");
    }

    fn refused(d: &Document, what: &str) {
        let e = from_openetch_json(&to_openetch_json(d)).unwrap_err();
        assert!(matches!(e, FileError::InvalidDocument(_)) && e.to_string().contains(what), "{what}: {e}");
    }

    /// A hand-edited or hostile file cannot hide broken shapes on a page that is not shown: showing it would make
    /// them the document's own (a group inside itself hung the group walk; a missing layer broke every command).
    #[test]
    fn pages_meet_the_same_rules() {
        let page = |id: &str, objects: Vec<Object>, groups: Vec<crate::Group>| crate::Page {
            id: id.into(),
            name: id.into(),
            objects,
            groups,
            locked: Default::default(),
            object_options: Default::default(),
            image_masks: Default::default(),
        };
        let other = |id: &str, layer: &str| Object {
            id: id.into(),
            name: "o".into(),
            layer: layer.into(),
            geometry: Geometry::Rect { width_mm: 1.0, height_mm: 1.0, corner_radius_mm: 0.0 },
            transform: Affine::IDENTITY,
            group: None,
            extras: Default::default(),
        };
        let mut d = sample();
        d.pages = vec![page("page-7", vec![], vec![]), page("page-8", vec![other("obj-9", "layer-1")], vec![])];
        d.active_page = Some("page-7".into());
        let back = from_openetch_json(&to_openetch_json(&d)).unwrap();
        assert!(back.next_id > 9, "ids on pages count");
        let mut bad = d.clone();
        bad.pages[1].objects[0].layer = "layer-404".into();
        refused(&bad, "layer-404");
        let mut bad = d.clone();
        bad.pages[1].objects[0].id = d.objects[0].id.clone();
        refused(&bad, "more than once");
        let mut bad = d.clone();
        let mut g = crate::Group::new("group-20".into(), "loop", Some("group-21".into()));
        let mut h = crate::Group::new("group-21".into(), "loop", Some("group-20".into()));
        g.extras.clear();
        h.extras.clear();
        bad.pages[1].objects[0].group = Some("group-20".into());
        bad.pages[1].groups = vec![g, h];
        refused(&bad, "inside itself");
        let mut bad = d.clone();
        bad.pages[1].objects[0].geometry = Geometry::Rect { width_mm: -1.0, height_mm: 1.0, corner_radius_mm: 0.0 };
        refused(&bad, "not positive");
        let mut bad = d.clone();
        bad.active_page = Some("page-404".into());
        refused(&bad, "active_page");
        let mut bad = d.clone();
        bad.pages[0].objects.push(other("obj-12", "layer-1"));
        refused(&bad, "holds shapes");
        let mut bad = d;
        bad.pages[1].id = "page-7".into();
        refused(&bad, "page id");
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

    #[test]
    fn groups_images_and_extras_round_trip_and_are_checked() {
        let mut d = sample();
        let gid = d.alloc_id("group").unwrap();
        d.groups.push(crate::Group::new(gid.clone(), "Group 1", None));
        d.groups[0].extras.insert("lbrn2.XForm".into(), "1 0 0 1 0 0".into());
        d.objects[0].group = Some(gid.clone());
        d.objects[0].extras.insert("lbrn2.Font".into(), "Arial,-1,100".into());
        d.extras.insert("lbrn2.Notes".into(), "<Notes/>".into());
        let id = d.alloc_id("obj").unwrap();
        let image =
            Geometry::Image { width_mm: 4.0, height_mm: 3.0, mime: "image/png".into(), data_base64: "iVBORw0KGgo=".into(), adjust: Default::default() };
        d.objects.push(Object::new(id, "photo", "layer-1".into(), image, Affine::IDENTITY));
        assert_eq!(from_openetch_json(&to_openetch_json(&d)).unwrap(), d);

        let mut bad = d.clone();
        bad.objects[0].group = Some("group-404".into());
        refused(&bad, "does not exist");
        let mut bad = d.clone();
        bad.groups[0].parent = Some(gid.clone());
        refused(&bad, "inside itself");
        let mut bad = d.clone();
        bad.groups.push(crate::Group::new("obj-2".into(), "clash", None));
        refused(&bad, "more than once");
        let mut bad = d.clone();
        bad.objects[0].extras.insert("bad key".into(), String::new());
        refused(&bad, "extras key");
        let mut bad = d.clone();
        bad.extras.insert("x".into(), String::new());
        refused(&bad, "extras key");
        let mut bad = d.clone();
        bad.groups[0].name = "a\u{7}".into();
        refused(&bad, "control character");
        for (mime, data) in [("text/html", "AAAA"), ("image/png", "<svg>"), ("image/png", "AA\nAA")] {
            let mut bad = d.clone();
            bad.objects[1].geometry =
                Geometry::Image { width_mm: 4.0, height_mm: 3.0, mime: mime.into(), data_base64: data.into(), adjust: Default::default() };
            refused(&bad, "image");
        }
        let mut bad = d.clone();
        bad.objects[1].geometry =
            Geometry::Image { width_mm: 0.0, height_mm: 3.0, mime: "image/png".into(), data_base64: "".into(), adjust: Default::default() };
        refused(&bad, "not positive");

        // An empty group is dropped on load, and group ids count for next_id.
        let mut d2 = d.clone();
        d2.groups.push(crate::Group::new("group-40".into(), "empty", None));
        let back = from_openetch_json(&to_openetch_json(&d2)).unwrap();
        assert_eq!(back.groups.len(), 1);
        assert!(back.next_id > 40);
    }

}
