//! Helpers shared by the built-in commands: id lookups with close-match suggestions, input validation, result JSON.

use doc::{Document, Layer, Object, ObjectId};
use kurbo::{Point, Rect};
use serde_json::{Value, json};

use crate::{CommandError, Result};

/// Run `f` on a copy of `doc` and commit it only on success, so a failing command never leaves a half-applied edit.
pub fn transact<T>(doc: &mut Document, f: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
    let mut work = doc.clone();
    let out = f(&mut work)?;
    *doc = work;
    Ok(out)
}

/// Round to 0.1 µm for output so callers see `10` rather than `9.999999999`. Also folds `-0` into `0`.
pub fn r4(x: f64) -> f64 {
    let r = (x * 1e4).round() / 1e4;
    if r == 0.0 { 0.0 } else { r }
}

pub fn pt_json(p: Point) -> Value {
    json!([r4(p.x), r4(p.y)])
}

/// `{min, max, size, center}` of a field-frame rectangle, in mm.
pub fn rect_json(r: Rect) -> Value {
    json!({
        "min": [r4(r.x0), r4(r.y0)],
        "max": [r4(r.x1), r4(r.y1)],
        "size": [r4(r.width()), r4(r.height())],
        "center": pt_json(r.center()),
    })
}

/// Bounds of `ids` as JSON, or `null` when none of them exist (or the list is empty).
pub fn bounds_json(doc: &Document, ids: &[ObjectId]) -> Value {
    doc.bounds_of(ids).map(rect_json).unwrap_or(Value::Null)
}

/// The standard result of an edit: created ids, affected ids, and the field-frame bounds of all of them.
pub fn edit_result(doc: &Document, created: &[ObjectId], affected: &[ObjectId]) -> Value {
    let all: Vec<ObjectId> = created.iter().chain(affected).cloned().collect();
    json!({ "created": created, "affected": affected, "bounds": bounds_json(doc, &all) })
}

pub fn invalid(id: &str, message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput { id: id.to_string(), message: message.into() }
}

/// Reject NaN/infinity, which serde happily accepts from some clients.
pub fn finite(cmd: &str, field: &str, v: f64) -> Result<f64> {
    if v.is_finite() { Ok(v) } else { Err(invalid(cmd, format!("{field} must be a finite number"))) }
}

pub fn positive(cmd: &str, field: &str, v: f64) -> Result<f64> {
    if finite(cmd, field, v)? > 0.0 { Ok(v) } else { Err(invalid(cmd, format!("{field} must be > 0 (got {v})"))) }
}

pub fn non_negative(cmd: &str, field: &str, v: f64) -> Result<f64> {
    if finite(cmd, field, v)? >= 0.0 { Ok(v) } else { Err(invalid(cmd, format!("{field} must be >= 0 (got {v})"))) }
}

pub fn finite2(cmd: &str, field: &str, v: [f64; 2]) -> Result<[f64; 2]> {
    finite(cmd, field, v[0])?;
    finite(cmd, field, v[1])?;
    Ok(v)
}

/// Trimmed, non-empty display name.
pub fn name(cmd: &str, field: &str, s: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() {
        return Err(invalid(cmd, format!("{field} must not be empty")));
    }
    if t.chars().count() > 200 {
        return Err(invalid(cmd, format!("{field} is longer than 200 characters")));
    }
    Ok(t.to_string())
}

/// A display colour. Any CSS colour string is accepted; only obviously broken values are refused.
pub fn color(cmd: &str, s: &str) -> Result<String> {
    let t = s.trim();
    if t.is_empty() || t.len() > 64 || t.contains(['<', '>', ';', '"', '{', '}']) {
        return Err(invalid(cmd, format!("'{s}' is not a usable colour; use e.g. \"#ff0000\"")));
    }
    Ok(t.to_string())
}

/// Look up an object by exact id, or fail with close matches.
pub fn object<'a>(doc: &'a Document, id: &str) -> Result<&'a Object> {
    doc.object(id).ok_or_else(|| not_found("object", id, doc.objects.iter().map(|o| (o.id.as_str(), o.name.as_str()))))
}

/// Look up a layer by exact id, or fail with close matches.
pub fn layer<'a>(doc: &'a Document, id: &str) -> Result<&'a Layer> {
    doc.layer(id).ok_or_else(|| not_found("layer", id, doc.layers.iter().map(|l| (l.id.as_str(), l.name.as_str()))))
}

/// Resolve an optional layer reference; `None` means the first layer.
pub fn layer_or_default(doc: &Document, id: Option<&str>) -> Result<String> {
    match id {
        Some(id) => Ok(layer(doc, id)?.id.clone()),
        None => doc
            .layers
            .first()
            .map(|l| l.id.clone())
            .ok_or_else(|| CommandError::Failed("the document has no layers".into())),
    }
}

/// Check every id exists and return them de-duplicated in the given order. An empty list is an error.
pub fn object_ids(cmd: &str, doc: &Document, ids: &[ObjectId]) -> Result<Vec<ObjectId>> {
    if ids.is_empty() {
        return Err(invalid(cmd, "ids must list at least one object id"));
    }
    let mut out: Vec<ObjectId> = Vec::with_capacity(ids.len());
    for id in ids {
        object(doc, id)?;
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

/// A `NotFound` error naming up to five close matches (by id or by name). Candidates are exact ids.
fn not_found<'a>(kind: &str, query: &str, items: impl Iterator<Item = (&'a str, &'a str)>) -> CommandError {
    let q = query.to_lowercase();
    let mut scored: Vec<(usize, &str, &str)> = items
        .filter_map(|(id, name)| {
            let (lid, lname) = (id.to_lowercase(), name.to_lowercase());
            let d = levenshtein(&q, &lid).min(levenshtein(&q, &lname));
            let close =
                d <= (q.chars().count() / 3).max(2) || lname.contains(&q) || (!q.is_empty() && q.contains(&lid));
            close.then_some((d, id, name))
        })
        .collect();
    scored.sort_by_key(|&(d, id, _)| (d, id.to_string()));
    scored.truncate(5);
    let mut message = format!("no {kind} with id '{query}'");
    if scored.is_empty() {
        message.push_str("; use doc.inspect to list ids");
    } else {
        let list: Vec<String> = scored.iter().map(|(_, id, name)| format!("{id} (\"{name}\")")).collect();
        message.push_str(&format!("; close matches: {}. Re-issue with an exact id", list.join(", ")));
    }
    CommandError::NotFound { message, candidates: scored.into_iter().map(|(_, id, _)| id.to_string()).collect() }
}

/// Plain edit distance over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Geometry, Object};
    use kurbo::Affine;

    fn doc_with(names: &[&str]) -> Document {
        let mut d = Document::default();
        for n in names {
            let id = d.alloc_id("obj");
            let layer = d.layers[0].id.clone();
            d.objects.push(Object {
                id,
                name: n.to_string(),
                layer,
                geometry: Geometry::Ellipse { rx_mm: 1.0, ry_mm: 1.0 },
                transform: Affine::IDENTITY,
            });
        }
        d
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("obj-1", "obj-1"), 0);
        assert_eq!(levenshtein("obj-1", "obj-12"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn not_found_suggests_by_id_and_name() {
        let d = doc_with(&["Logo", "Border"]);
        match object(&d, "obj-9") {
            Err(CommandError::NotFound { candidates, .. }) => assert!(candidates.contains(&"obj-2".to_string())),
            other => panic!("expected NotFound, got {other:?}"),
        }
        match object(&d, "logo") {
            Err(CommandError::NotFound { candidates, message }) => {
                assert_eq!(candidates[0], "obj-2");
                assert!(message.contains("Logo"));
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn object_ids_dedupes_and_rejects_empty() {
        let d = doc_with(&["a"]);
        assert_eq!(object_ids("t", &d, &["obj-2".into(), "obj-2".into()]).unwrap(), vec!["obj-2".to_string()]);
        assert!(object_ids("t", &d, &[]).is_err());
    }

    #[test]
    fn rounding_folds_negative_zero() {
        assert_eq!(r4(-0.00001).to_string(), "0");
        assert_eq!(r4(9.999999999), 10.0);
    }
}
