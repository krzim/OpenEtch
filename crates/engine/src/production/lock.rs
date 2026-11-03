//! The production cockpit's design lock: while a cockpit is open (it renews a lease every few seconds,
//! `POST /api/production/lock`), document edits are refused for everyone (the app on other devices, scripts, MCP
//! agents) with 409 and the reason, so the job the foot switch burns stays the one on the jig. Allowed while locked:
//! queries, the variable-text counter (`var.*`: the cockpit marks slots empty, the engine moves the counter on after a
//! run) and everything that is not a document edit: Stop, Pause and Resume, framing, Start, approvals. The lease ends
//! [`LEASE`] after the last renewal (a closed tab or a lost connection unlocks by itself), when the cockpit closes,
//! or when someone unlocks it in the app (UI only; agents cannot).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::now_unix;
use crate::libs::lock;
use crate::{AppState, EngineError};

/// How long a lock lasts without renewal.
pub const LEASE: Duration = Duration::from_secs(45);

#[derive(Debug, Clone)]
struct Lease {
    holder: String,
    label: String,
    until: Instant,
    since_unix_s: u64,
}

/// The lock (one field of `Production`).
#[derive(Debug, Default)]
pub struct EditLock {
    lease: Mutex<Option<Lease>>,
}

impl EditLock {
    fn current(&self) -> Option<Lease> {
        let mut l = lock(&self.lease);
        if l.as_ref().is_some_and(|x| x.until <= Instant::now()) {
            *l = None;
        }
        l.clone()
    }

    /// Whether edits are refused now.
    pub fn locked(&self) -> bool {
        self.current().is_some()
    }

    /// Take or renew the lock for `holder` (a cockpit's id; `label` names the device for the others). Another holder's
    /// live lock is taken over: the person at the other cockpit sees it in theirs.
    pub fn renew(&self, holder: &str, label: &str) {
        let mut l = lock(&self.lease);
        let since = l.as_ref().filter(|x| x.holder == holder && x.until > Instant::now()).map_or_else(now_unix, |x| x.since_unix_s);
        *l = Some(Lease { holder: holder.to_owned(), label: label.to_owned(), until: Instant::now() + LEASE, since_unix_s: since });
    }

    /// Release the lock: `holder`'s own, or any with `force` (Unlock in the app).
    pub fn release(&self, holder: Option<&str>, force: bool) -> bool {
        let mut l = lock(&self.lease);
        let mine = l.as_ref().is_some_and(|x| force || holder.is_some_and(|h| h == x.holder));
        if mine {
            *l = None;
        }
        mine
    }

    pub fn status_json(&self) -> Value {
        match self.current() {
            Some(l) => json!({
                "locked": true,
                "holder": l.holder,
                "label": l.label,
                "since_unix_s": l.since_unix_s,
                "lease_s": l.until.saturating_duration_since(Instant::now()).as_secs(),
            }),
            None => json!({ "locked": false, "holder": null, "label": null, "since_unix_s": null, "lease_s": 0 }),
        }
    }

    fn refusal(&self) -> Option<EngineError> {
        self.current().map(|l| {
            EngineError::Conflict(format!(
                "the design is locked by the production cockpit{}: close the cockpit (or unlock it in the app) to edit",
                if l.label.is_empty() { String::new() } else { format!(" on {}", l.label) }
            ))
        })
    }
}

/// Refuse the edit command `id` while the cockpit holds the lock (called for every `Edit` command of a batch).
pub(crate) fn check_edit(st: &AppState, id: &str) -> Result<(), EngineError> {
    if id.starts_with("var.") {
        return Ok(());
    }
    st.production.lock.refusal().map_or(Ok(()), Err)
}

/// Refuse undo, redo, timeline moves and switching the active document while the cockpit holds the lock.
pub(crate) fn check_timeline(st: &AppState) -> Result<(), EngineError> {
    st.production.lock.refusal().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_renew_release() {
        let l = EditLock::default();
        assert!(!l.locked());
        l.renew("ipad-1", "iPad");
        assert!(l.locked());
        let since = l.status_json()["since_unix_s"].clone();
        l.renew("ipad-1", "iPad");
        assert_eq!(l.status_json()["since_unix_s"], since, "a renewal keeps the start");
        assert!(!l.release(Some("other"), false), "only the holder releases without force");
        assert!(l.locked());
        assert!(l.release(None, true));
        assert!(!l.locked());
        assert!(l.refusal().is_none());
        // An expired lease unlocks by itself.
        *l.lease.lock().unwrap() =
            Some(Lease { holder: "x".into(), label: String::new(), until: Instant::now(), since_unix_s: 1 });
        assert!(!l.locked());
    }
}
