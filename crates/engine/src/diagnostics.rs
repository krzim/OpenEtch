//! Diagnostics (LightBurn's Enable Debug Log and Help > Report a Bug): a tracing layer that keeps the engine's most
//! recent log lines in memory (info and above; debug from OpenEtch's own crates while the debug log is on), and a
//! bug report bundle of those lines plus versions, the machine and device status, the job log tail and the
//! document's shape (counts, never its content). Nothing secret goes in: the bearer token and PIN are never logged
//! or reported, and `token=` / `Bearer` values in log lines are masked anyway.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Lines kept.
pub const RING_LINES: usize = 2000;
/// The crates whose debug events the debug log keeps.
const OWN_CRATES: &[&str] = &["engine", "openetch", "machine", "plan", "commands", "lbrn", "lmc", "doc", "geom", "text", "corfile"];

static DEBUG: AtomicBool = AtomicBool::new(false);
static RING: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn ring() -> &'static Mutex<VecDeque<String>> {
    RING.get_or_init(|| Mutex::new(VecDeque::with_capacity(RING_LINES)))
}

/// Turn the debug log on or off.
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::SeqCst);
}

pub fn debug_enabled() -> bool {
    DEBUG.load(Ordering::SeqCst)
}

/// Whether the ring keeps events with this metadata.
pub fn keeps(meta: &Metadata<'_>) -> bool {
    let level = *meta.level();
    if level <= Level::INFO {
        return true;
    }
    level == Level::DEBUG && debug_enabled() && OWN_CRATES.iter().any(|c| meta.target() == *c || meta.target().starts_with(&format!("{c}::")))
}

/// Add a line (also used by tests).
pub fn push_line(line: String) {
    let mut r = ring().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if r.len() >= RING_LINES {
        r.pop_front();
    }
    r.push_back(mask_secrets(&line));
}

/// The kept lines, oldest first.
pub fn lines() -> Vec<String> {
    ring().lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect()
}

/// Keys whose values are masked (ASCII, lower case; matched case-insensitively at a word start, so `access_token`
/// and `X-Pin` are masked too).
const SECRET_KEYS: &[&str] = &["token", "pin", "password", "passwd", "secret", "api_key", "apikey", "cookie"];

/// Mask secret values in a log line: `token=v`, `token: v`, `token="v"` (tracing's quoted fields), `"pin": "v"`
/// (JSON), `Bearer v`, and the same for every [`SECRET_KEYS`] key.
pub fn mask_secrets(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = String::with_capacity(line.len());
    let (mut copied, mut i) = (0usize, 0usize);
    let word_start = |i: usize| i == 0 || !b[i - 1].is_ascii_alphanumeric();
    let skip_spaces = |mut j: usize| {
        while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
            j += 1;
        }
        j
    };
    while i < b.len() {
        // Where the value starts, if a secret key (or `Bearer`) starts at `i`.
        let mut value_at = None;
        if word_start(i) {
            if b[i..].starts_with(b"bearer ") {
                value_at = Some(skip_spaces(i + 7));
            } else if let Some(k) = SECRET_KEYS.iter().find(|k| b[i..].starts_with(k.as_bytes())) {
                let mut j = i + k.len();
                if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                    j += 1;
                }
                j = skip_spaces(j);
                if j < b.len() && (b[j] == b'=' || b[j] == b':') {
                    value_at = Some(skip_spaces(j + 1));
                }
            }
        }
        let Some(start) = value_at.filter(|&s| s < b.len()) else {
            i += 1;
            continue;
        };
        let (value_start, end) = match b[start] {
            q @ (b'"' | b'\'') => {
                let close = b[start + 1..].iter().position(|&c| c == q).map_or(b.len(), |p| start + 1 + p);
                (start + 1, close)
            }
            _ => {
                let end = b[start..]
                    .iter()
                    .position(|&c| c.is_ascii_whitespace() || matches!(c, b'&' | b'"' | b'\'' | b',' | b')' | b';' | b'}' | b']'))
                    .map_or(b.len(), |p| start + p);
                (start, end)
            }
        };
        if end > value_start {
            out.push_str(&line[copied..value_start]);
            out.push_str("***");
            copied = end;
        }
        i = end.max(i + 1);
    }
    out.push_str(&line[copied..]);
    out
}

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }
}

/// The tracing layer that fills the ring. Add it with its filter: `RingLayer.with_filter(ring_filter())`.
pub struct RingLayer;

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        let mut f = Fields(format!("{secs:.3} {} {}:", meta.level(), meta.target()));
        event.record(&mut f);
        push_line(f.0);
    }
}

/// The per-layer filter of [`RingLayer`] (dynamic: follows [`set_debug`]).
pub fn ring_filter<S: Subscriber>() -> tracing_subscriber::filter::DynFilterFn<S> {
    tracing_subscriber::filter::dynamic_filter_fn(|meta, _cx| keeps(meta))
}

/// Versions and platform for reports.
pub fn versions() -> Value {
    json!({
        "openetch": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "usb_support": cfg!(feature = "usb"),
        "camera_support": cfg!(feature = "camera"),
    })
}

/// The bug report bundle (`GET /api/diagnostics/report`).
pub fn report(st: &crate::AppState) -> Value {
    let d = st.document();
    let doc = json!({
        "revision": d.revision,
        "field_mm": d.field_mm,
        "layers": d.layers.len(),
        "objects": d.objects.len(),
        "groups": d.groups.len(),
        "images": d.objects.iter().filter(|o| matches!(o.geometry, doc::Geometry::Image { .. })).count(),
        "texts": d.objects.iter().filter(|o| matches!(o.geometry, doc::Geometry::Text(_))).count(),
    });
    let mut machine = st.machine_status();
    if let Some(m) = machine.as_object_mut() {
        // LightBurn's prefs.ini values (the matched device and its differences) are the user's preferences, not
        // needed to report a bug: only whether a LightBurn device matched.
        if let Some(lb) = m.get_mut("lightburn") {
            *lb = json!(!lb.is_null());
        }
        // Runs carry document summaries (layer names); keep only their states and errors.
        for key in ["pending_runs", "runs"] {
            if let Some(Value::Array(runs)) = m.get_mut(key) {
                for r in runs.iter_mut() {
                    *r = json!({ "id": r["id"], "state": r["state"], "origin": r["origin"], "error": r["error"], "stats": r["stats"] });
                }
            }
        }
    }
    json!({
        "format": "openetch-bug-report",
        "generated_unix_s": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "versions": versions(),
        "debug_log": debug_enabled(),
        "machine": machine,
        // Outcomes only: no project names or summaries (layer names).
        "job_log": st
            .libs
            .read_job_log(50)
            .unwrap_or_default()
            .into_iter()
            .map(|e| json!({
                "time_unix_s": e.time_unix_s, "state": e.state, "origin": e.origin, "device": e.device,
                "duration_s": e.duration_s, "completed": e.completed, "error": e.error.map(|x| mask_secrets(&x)),
            }))
            .collect::<Vec<_>>(),
        "document": doc,
        "logs": lines(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_secrets() {
        assert_eq!(mask_secrets("GET /api/events?token=abc123&x=1"), "GET /api/events?token=***&x=1");
        assert_eq!(mask_secrets("Authorization: Bearer xyz rest"), "Authorization: Bearer *** rest");
        assert_eq!(mask_secrets("pin=123456 and TOKEN=q"), "pin=*** and TOKEN=***");
        assert_eq!(mask_secrets("nothing here"), "nothing here");
        // tracing writes non-message fields quoted (`token="..."`): the old masker stopped at the quote.
        assert_eq!(mask_secrets(r#"INFO engine: started token="abc123" port=7878"#), r#"INFO engine: started token="***" port=7878"#);
        assert_eq!(mask_secrets(r#"body {"pin": "123456", "x": 1}"#), r#"body {"pin": "***", "x": 1}"#);
        assert_eq!(mask_secrets("access_token=s3cr3t;x"), "access_token=***;x");
        assert_eq!(mask_secrets("password: hunter2 ok"), "password: *** ok");
        assert_eq!(mask_secrets("X-Pin:4321"), "X-Pin:***");
        // Words that merely contain a key, and keys without a value, are left alone.
        assert_eq!(mask_secrets("spinning=3 tokens pin_set=true pin"), "spinning=3 tokens pin_set=true pin");
        assert_eq!(mask_secrets("token=\"unterminated"), "token=\"***");
        assert_eq!(mask_secrets("µm token=ü pin=1"), "µm token=*** pin=***", "non-ASCII text is kept intact");
    }

    #[test]
    fn ring_keeps_the_newest_lines() {
        for i in 0..(RING_LINES + 5) {
            push_line(format!("line {i} token=t"));
        }
        let l = lines();
        assert_eq!(l.len(), RING_LINES);
        assert!(l.last().unwrap().ends_with("token=***"));
    }
}
