//! Request parsing and answer pieces both routers share (moved from the engine's `api`, `state` and `userfiles`
//! modules), so a mistake gets the same message from the engine and from the browser core.

use std::collections::HashMap;

use commands::Origin;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::AppError;

/// Parse a request's `origin`: `"ui"`, `"script"`, `"agent"` / `"agent:<name>"`, or the serde form of
/// [`Origin`] (`{"kind": "agent", "name": "..."}`). Missing means [`Origin::Ui`].
pub fn parse_origin(v: Option<&Value>) -> Result<Origin, AppError> {
    match v {
        None | Some(Value::Null) => Ok(Origin::Ui),
        Some(Value::String(s)) => match s.as_str() {
            "ui" => Ok(Origin::Ui),
            "script" => Ok(Origin::Script),
            "agent" => Ok(Origin::Agent("agent".into())),
            other => match other.strip_prefix("agent:") {
                Some(name) if !name.is_empty() => Ok(Origin::Agent(name.to_owned())),
                _ => Err(AppError::BadRequest(format!("unknown origin '{other}' (expected \"ui\", \"script\" or \"agent:<name>\")"))),
            },
        },
        Some(obj) => serde_json::from_value(obj.clone()).map_err(|e| AppError::BadRequest(format!("bad origin: {e}"))),
    }
}

/// Parse a JSON body; an empty body means `T::default()`.
pub fn body_or_default<T: DeserializeOwned + Default>(body: &[u8]) -> Result<T, AppError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| AppError::BadRequest(format!("bad JSON body: {e}")))
}

/// Parse a required JSON body.
pub fn body_required<T: DeserializeOwned>(body: &[u8]) -> Result<T, AppError> {
    serde_json::from_slice(body).map_err(|e| AppError::BadRequest(format!("bad JSON body: {e}")))
}

/// Refuse a query parameter not in `allowed`.
pub fn check_query(q: &HashMap<String, String>, allowed: &[&str]) -> Result<(), AppError> {
    if let Some(k) = q.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(AppError::BadRequest(format!("unknown query parameter '{k}' (allowed: {})", allowed.join(", "))));
    }
    Ok(())
}

/// A finite number from a query parameter. `parse` accepts "NaN" and "inf", which would become JSON null (the
/// command's default) downstream, so they are refused.
pub fn finite_param(name: &str, v: &str) -> Result<f64, AppError> {
    match v.parse::<f64>() {
        Ok(f) if f.is_finite() => Ok(f),
        _ => Err(AppError::BadRequest(format!("{name} '{v}' is not a finite number"))),
    }
}

/// `Content-Disposition` for `<stem>.<ext>` (`kind` is `attachment` or `inline`): an ASCII `filename` (other
/// characters become `_`) and an RFC 5987 `filename*` with the full UTF-8 name.
pub fn disposition(kind: &str, stem: &str, ext: &str) -> String {
    let stem: String = stem.chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\')).collect();
    let stem = match stem.trim() {
        "" => "Untitled",
        s => s,
    };
    let file = format!("{stem}.{ext}");
    let ascii: String = file
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') { c } else { '_' })
        .collect();
    let mut encoded = String::new();
    for b in file.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(char::from(b));
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    format!("{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// `object` (JSON text of an object) with member `name: value` added at the end, or `None` when it is not an object.
pub fn splice_member(object: &[u8], name: &str, value: &[u8]) -> Option<Vec<u8>> {
    let start = object.iter().position(|b| !b.is_ascii_whitespace())?;
    let end = object.iter().rposition(|b| !b.is_ascii_whitespace())?;
    if object[start] != b'{' || object[end] != b'}' || end == start {
        return None;
    }
    let empty = object[start + 1..end].iter().all(u8::is_ascii_whitespace);
    let mut out = Vec::with_capacity(object.len() + value.len() + name.len() + 6);
    out.extend_from_slice(&object[..end]);
    if !empty {
        out.push(b',');
    }
    out.push(b'"');
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\":");
    out.extend_from_slice(value);
    out.push(b'}');
    Some(out)
}

/// Refused unless the request names the UI as its origin (the browser core has no MCP clients to tell apart).
pub fn ui_origin_only(origin: Option<&Value>, what: &str) -> Result<(), AppError> {
    if parse_origin(origin)? != Origin::Ui {
        return Err(AppError::Forbidden(format!("{what} only from the UI")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn origins_parse() {
        assert_eq!(parse_origin(None).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("ui"))).unwrap(), Origin::Ui);
        assert_eq!(parse_origin(Some(&json!("agent:claude"))).unwrap(), Origin::Agent("claude".into()));
        assert_eq!(parse_origin(Some(&json!({"kind": "script"}))).unwrap(), Origin::Script);
        assert!(parse_origin(Some(&json!("root"))).is_err());
    }

    #[test]
    fn dispositions_and_splices() {
        assert_eq!(disposition("attachment", "Mug ü", "svg"), "attachment; filename=\"Mug _.svg\"; filename*=UTF-8''Mug%20%C3%BC.svg");
        assert_eq!(disposition("inline", "  ", "pdf"), "inline; filename=\"Untitled.pdf\"; filename*=UTF-8''Untitled.pdf");
        assert_eq!(splice_member(b"{}", "s", b"1").unwrap(), b"{\"s\":1}");
        assert_eq!(splice_member(b"{\"a\":2} ", "s", b"1").unwrap(), b"{\"a\":2,\"s\":1}");
        assert!(splice_member(b"[1]", "s", b"1").is_none());
    }
}
