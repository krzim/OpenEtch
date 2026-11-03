//! Answers and request pieces of the browser backend: [`Reply`] (what `handle` returns), [`Answer`] (one route's
//! HTTP answer) and the query string parser.

use std::collections::HashMap;

use appcore::AppError;
use appcore::stores::StoreWrite;
use serde_json::Value;

/// What one request gives back: the HTTP answer, the events it caused (each the JSON text of one
/// `appcore::Event`) and the store records to persist.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub events: Vec<String>,
    pub writes: Vec<StoreWrite>,
}

impl Reply {
    /// The body as JSON (tests).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// A header's value (names compare without case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// One route's HTTP answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub type Routed = Result<Answer, AppError>;

impl Answer {
    /// `200` with a JSON body.
    pub fn json(v: &Value) -> Answer {
        Answer::with_status(200, v)
    }

    /// `status` with a JSON body.
    pub fn with_status(status: u16, v: &Value) -> Answer {
        Answer { status, headers: vec![("content-type".into(), "application/json".into())], body: serde_json::to_vec(v).unwrap_or_default() }
    }

    /// `200` with `body` of `content_type`.
    pub fn bytes(content_type: &str, body: Vec<u8>) -> Answer {
        Answer { status: 200, headers: vec![("content-type".into(), content_type.into())], body }
    }

    /// Add a header.
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Answer {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// A refusal: the error's status and `{ok: false, error}`.
    pub fn error(e: &AppError) -> Answer {
        Answer::with_status(e.status(), &e.body())
    }

    /// A file to download (as the engine's `file_response`): its type, `Content-Disposition`, no caching and the
    /// warning count.
    pub fn file(kind: &str, stem: &str, ext: &str, mime: &str, body: Vec<u8>, warnings: usize) -> Answer {
        Answer::bytes(mime, body)
            .header("content-disposition", appcore::http::disposition(kind, stem, ext))
            .header("cache-control", "no-store")
            .header("x-openetch-warnings", warnings.to_string())
    }
}

/// A command failure as `200 {ok: false, error}` (the contract's shape for `/api/execute`, `/api/batch`, undo, redo,
/// the timeline and uploads); any other refusal keeps its status.
pub fn command_answer(r: Result<Value, AppError>) -> Routed {
    match r {
        Ok(v) => Ok(Answer::json(&v)),
        Err(e @ AppError::Command(_)) => Ok(Answer::json(&e.body())),
        Err(e) => Err(e),
    }
}

/// Decode `application/x-www-form-urlencoded` text (`+` is a space, `%XX` a byte; bad escapes are kept as they are).
pub fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = |b: u8| (b as char).to_digit(16);
                match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A query string (without `?`) as a map; a repeated name keeps its last value.
pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.trim_start_matches('?')
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (url_decode(k), url_decode(v)),
            None => (url_decode(p), String::new()),
        })
        .collect()
}

/// Path segments after `/api/`, each percent-decoded (`%2F` stays inside its segment).
pub fn segments(path: &str) -> Vec<String> {
    path.split('/').filter(|s| !s.is_empty()).map(|s| url_decode(&s.replace('+', "%2B"))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_and_paths_decode() {
        let q = parse_query("format=svg&name=Logo%20%26%20mark&layer=layer+2&x");
        assert_eq!(q["format"], "svg");
        assert_eq!(q["name"], "Logo & mark");
        assert_eq!(q["layer"], "layer 2");
        assert_eq!(q["x"], "");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(url_decode("%zz"), "%zz");
        assert_eq!(segments("/api/runs/run%2F1/approve"), ["api", "runs", "run/1", "approve"]);
        assert_eq!(segments("/api/library/a+b"), ["api", "library", "a+b"]);
    }
}
