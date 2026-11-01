//! Check for updates (LightBurn's Help > Check for Updates), opt-in: off until the preference
//! `check_for_updates` is turned on, and then only when asked (`POST /api/updates/check`). It reads the newest
//! release of the GitHub repository from the public releases API and compares its tag with this build's version.
//! Nothing about the user or the machine is sent (a plain GET with a User-Agent).
//!
//! The request goes through the system's `curl` (Windows 10+, macOS and Raspberry Pi OS ship it), so the engine
//! carries no TLS stack of its own; without curl the check reports that it could not run.

use std::process::Command;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// The repository whose releases are checked.
pub const REPOSITORY: &str = "krzim/OpenEtch";
const TIMEOUT: Duration = Duration::from_secs(15);

/// The outcome of one check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: Option<String>,
    pub newer: bool,
    pub url: Option<String>,
    pub published_at: Option<String>,
    pub checked_unix_s: u64,
    pub error: Option<String>,
}

/// `v1.2.3` / `1.2.3-beta.1` → `[1, 2, 3]` (pre-release suffixes ignored); `None` when it has no number.
pub fn parse_version(tag: &str) -> Option<Vec<u64>> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    let core = t.split(['-', '+']).next()?;
    let parts: Vec<u64> = core.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    (!parts.is_empty()).then_some(parts)
}

/// Whether `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

/// Read GitHub's `releases/latest` answer.
pub fn from_release_json(body: &str, current: &str, now: u64) -> UpdateCheck {
    let mut out = UpdateCheck {
        current: current.to_string(),
        latest: None,
        newer: false,
        url: None,
        published_at: None,
        checked_unix_s: now,
        error: None,
    };
    match serde_json::from_str::<Value>(body) {
        Ok(v) => match v.get("tag_name").and_then(Value::as_str) {
            Some(tag) => {
                out.newer = is_newer(tag, current);
                out.latest = Some(tag.to_string());
                // Only a GitHub page of this repository is offered as a link (the UI opens it).
                let prefix = format!("https://github.com/{REPOSITORY}/");
                out.url = v.get("html_url").and_then(Value::as_str).filter(|u| u.starts_with(&prefix)).map(str::to_string);
                out.published_at = v.get("published_at").and_then(Value::as_str).map(str::to_string);
            }
            None => out.error = Some(v.get("message").and_then(Value::as_str).unwrap_or("no release found").to_string()),
        },
        Err(e) => out.error = Some(format!("unreadable answer: {e}")),
    }
    out
}

/// Ask GitHub (blocking; run it on a blocking thread).
pub fn check_now() -> UpdateCheck {
    let current = env!("CARGO_PKG_VERSION");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", &TIMEOUT.as_secs().to_string(), "-H", "Accept: application/vnd.github+json"])
        .args(["-A", &format!("OpenEtch/{current}"), &url])
        .output();
    match out {
        Ok(o) if o.status.success() => from_release_json(&String::from_utf8_lossy(&o.stdout), current, now),
        Ok(o) => UpdateCheck {
            current: current.into(),
            latest: None,
            newer: false,
            url: None,
            published_at: None,
            checked_unix_s: now,
            error: Some(format!("the release check failed ({}): {}", o.status, String::from_utf8_lossy(&o.stderr).trim())),
        },
        Err(e) => UpdateCheck {
            current: current.into(),
            latest: None,
            newer: false,
            url: None,
            published_at: None,
            checked_unix_s: now,
            error: Some(format!("cannot run curl for the release check: {e}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version("0.3.0-beta.1"), Some(vec![0, 3, 0]));
        assert_eq!(parse_version("nightly"), None);
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("junk", "0.1.0"));
    }

    #[test]
    fn reads_github_answers() {
        let page = "https://github.com/krzim/OpenEtch/releases/tag/v9.0.0";
        let r = from_release_json(&format!(r#"{{"tag_name": "v9.0.0", "html_url": "{page}", "published_at": "2026-10-01"}}"#), "0.1.0", 5);
        assert!(r.newer && r.error.is_none());
        assert_eq!(r.url.as_deref(), Some(page));
        let odd = from_release_json(r#"{"tag_name": "v9.0.0", "html_url": "javascript:alert(1)"}"#, "0.1.0", 5);
        assert_eq!(odd.url, None, "only this repository's GitHub page is linked");
        let none = from_release_json(r#"{"message": "Not Found"}"#, "0.1.0", 5);
        assert_eq!((none.newer, none.error.as_deref()), (false, Some("Not Found")));
        assert!(from_release_json("<html>", "0.1.0", 5).error.is_some());
    }
}
