//! The hour meter and maintenance reminders (short list #12), per device, in `<config-dir>/hour-meter.json`.
//!
//! Every iteration that ran to the end adds its marking time: the board's own count (`GetMarkTime`,
//! `machine::RunReport::mark_time_s`) when it gave one, else the run's estimate (the simulator, GRBL and Ruida
//! gantries, a board that did not answer), kept apart so the meter says how much of it is estimated. The figures are
//! the controller's list time, not the laser source's own hours (the source keeps none the board can read).
//!
//! A reminder (`{name, every_hours?, every_runs?}`, e.g. "Clean the lens" every 40 h) is due once the device has
//! marked that long (or that many runs) since it was last done; it shows as a status chip in the app and is sent once
//! as the `maintenance_due` notification. Done resets it. Editing reminders and marking them done are UI-only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::notify::{Kind, Operation};
use super::now_unix;
use crate::libs::lock;
use crate::{AppState, EngineError};

pub const METER_FILE: &str = "hour-meter.json";
const FORMAT: &str = "openetch-hour-meter";
/// Most reminders per device.
pub const MAX_REMINDERS: usize = 20;

/// One maintenance reminder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub name: String,
    /// Due after this many marking hours since it was last done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_hours: Option<f64>,
    /// Due after this many runs (iterations) since it was last done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_runs: Option<u64>,
    /// The meter (s) and runs when it was last done (or added).
    #[serde(default)]
    pub done_at_s: f64,
    #[serde(default)]
    pub done_at_runs: u64,
    #[serde(default)]
    pub done_unix_s: u64,
    /// The `maintenance_due` notification went out since it was last done.
    #[serde(default)]
    pub notified: bool,
}

/// One device's meter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceMeter {
    /// Marking time the board measured, s.
    pub measured_s: f64,
    /// Marking time estimated (no board count), s.
    pub estimated_s: f64,
    /// Iterations that ran to the end.
    pub runs: u64,
    /// When this meter started counting.
    pub since_unix_s: u64,
    pub reminders: Vec<Reminder>,
}

impl DeviceMeter {
    pub fn total_s(&self) -> f64 {
        self.measured_s + self.estimated_s
    }

    /// Whether `r` is due now.
    pub fn due(&self, r: &Reminder) -> bool {
        let hours = (self.total_s() - r.done_at_s) / 3600.0;
        r.every_hours.is_some_and(|h| hours >= h) || r.every_runs.is_some_and(|n| self.runs.saturating_sub(r.done_at_runs) >= n)
    }

    fn reminder_json(&self, r: &Reminder) -> Value {
        let hours = (self.total_s() - r.done_at_s) / 3600.0;
        let runs = self.runs.saturating_sub(r.done_at_runs);
        json!({
            "id": r.id,
            "name": r.name,
            "every_hours": r.every_hours,
            "every_runs": r.every_runs,
            "hours_since": hours,
            "runs_since": runs,
            "left_hours": r.every_hours.map(|h| h - hours),
            "left_runs": r.every_runs.map(|n| i128::from(n) - i128::from(runs)),
            "done_unix_s": r.done_unix_s,
            "due": self.due(r),
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    devices: BTreeMap<String, DeviceMeter>,
}

/// The meters (one field of `Production`).
#[derive(Debug, Default)]
pub struct Meter {
    path: Mutex<Option<PathBuf>>,
    devices: Mutex<BTreeMap<String, DeviceMeter>>,
}

impl Meter {
    pub fn open(&self, dir: &Path) -> Option<String> {
        let path = dir.join(METER_FILE);
        let (devices, note) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(f) if f.format == FORMAT => (f.devices, None),
                _ => (BTreeMap::new(), Some(format!("{}: not usable; the hour meter starts from 0", path.display()))),
            },
            Err(_) => (BTreeMap::new(), None),
        };
        *lock(&self.devices) = devices;
        *lock(&self.path) = Some(path);
        note
    }

    fn save(&self, devices: &BTreeMap<String, DeviceMeter>) {
        let Some(path) = lock(&self.path).clone() else { return };
        let file = File { format: FORMAT.into(), version: 1, devices: devices.clone() };
        let Ok(text) = serde_json::to_string_pretty(&file) else { return };
        if let Err(e) = crate::prefs::write_atomic(&path, text.as_bytes()) {
            tracing::warn!(error = %e, "cannot save the hour meter");
        }
    }

    pub fn device(&self, id: &str) -> DeviceMeter {
        lock(&self.devices).get(id).cloned().unwrap_or_default()
    }

    /// Add one iteration; returns the reminders that became due with it (not notified before).
    fn add(&self, device: &str, measured_s: Option<f64>, estimated_s: f64) -> Vec<String> {
        let mut all = lock(&self.devices);
        let d = all.entry(device.to_owned()).or_insert_with(|| DeviceMeter { since_unix_s: now_unix(), ..DeviceMeter::default() });
        match measured_s.filter(|s| s.is_finite() && *s >= 0.0) {
            Some(s) => d.measured_s += s,
            None if estimated_s.is_finite() && estimated_s > 0.0 => d.estimated_s += estimated_s,
            None => {}
        }
        d.runs += 1;
        let mut newly = Vec::new();
        let due: Vec<bool> = d.reminders.iter().map(|r| d.due(r)).collect();
        for (r, due) in d.reminders.iter_mut().zip(due) {
            if due && !r.notified {
                r.notified = true;
                newly.push(r.name.clone());
            }
        }
        let snapshot = all.clone();
        drop(all);
        self.save(&snapshot);
        newly
    }

    /// Replace device `device`'s reminders (UI only). A reminder keeps its "last done" when its id is kept; a new one
    /// counts from now.
    pub fn set_reminders(&self, device: &str, input: Vec<ReminderInput>) -> Result<(), EngineError> {
        if input.len() > MAX_REMINDERS {
            return Err(EngineError::BadRequest(format!("at most {MAX_REMINDERS} reminders")));
        }
        let mut all = lock(&self.devices);
        let d = all.entry(device.to_owned()).or_insert_with(|| DeviceMeter { since_unix_s: now_unix(), ..DeviceMeter::default() });
        let mut out: Vec<Reminder> = Vec::with_capacity(input.len());
        for (i, r) in input.into_iter().enumerate() {
            let name = r.name.trim().to_owned();
            let n = name.chars().count();
            if n == 0 || n > 80 || name.chars().any(char::is_control) {
                return Err(EngineError::BadRequest(format!("reminder name \"{name}\" must be 1-80 printable characters")));
            }
            let hours_ok = r.every_hours.is_none_or(|h| h.is_finite() && h > 0.0 && h <= 100_000.0);
            let runs_ok = r.every_runs.is_none_or(|n| (1..=10_000_000).contains(&n));
            if !hours_ok || !runs_ok || (r.every_hours.is_none() && r.every_runs.is_none()) {
                return Err(EngineError::BadRequest(format!(
                    "reminder \"{name}\" needs every_hours (> 0, at most 100000) and/or every_runs (1..=10000000)"
                )));
            }
            let kept = r.id.as_ref().and_then(|id| d.reminders.iter().find(|o| &o.id == id)).cloned();
            let id = r.id.filter(|id| !id.is_empty() && out.iter().all(|o| &o.id != id)).unwrap_or_else(|| {
                let mut k = i + 1;
                while out.iter().any(|o| o.id == format!("rem-{k}")) || d.reminders.iter().any(|o| o.id == format!("rem-{k}")) {
                    k += 1;
                }
                format!("rem-{k}")
            });
            let (done_at_s, done_at_runs, done_unix_s, notified) = match &kept {
                Some(k) => (k.done_at_s, k.done_at_runs, k.done_unix_s, k.notified),
                None => (d.total_s(), d.runs, now_unix(), false),
            };
            out.push(Reminder { id, name, every_hours: r.every_hours, every_runs: r.every_runs, done_at_s, done_at_runs, done_unix_s, notified });
        }
        d.reminders = out;
        let snapshot = all.clone();
        drop(all);
        self.save(&snapshot);
        Ok(())
    }

    /// Mark reminder `id` of `device` done now (UI only).
    pub fn done(&self, device: &str, id: &str) -> Result<(), EngineError> {
        let mut all = lock(&self.devices);
        let d = all.get_mut(device).ok_or_else(|| EngineError::NotFound(format!("no reminder '{id}'")))?;
        let (total, runs) = (d.total_s(), d.runs);
        let r = d.reminders.iter_mut().find(|r| r.id == id).ok_or_else(|| EngineError::NotFound(format!("no reminder '{id}'")))?;
        r.done_at_s = total;
        r.done_at_runs = runs;
        r.done_unix_s = now_unix();
        r.notified = false;
        let snapshot = all.clone();
        drop(all);
        self.save(&snapshot);
        Ok(())
    }

    /// Forget removed devices' meters (as the other per-device settings are).
    pub fn forget(&self, gone: impl Fn(&str) -> bool) {
        let mut all = lock(&self.devices);
        let before = all.len();
        all.retain(|d, _| !gone(d));
        if all.len() != before {
            let snapshot = all.clone();
            drop(all);
            self.save(&snapshot);
        }
    }
}

/// A reminder as clients send it (`PUT /api/production/maintenance`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub every_hours: Option<f64>,
    #[serde(default)]
    pub every_runs: Option<u64>,
}

/// One iteration finished (module `production::iteration_done`).
pub(crate) fn record(st: &Arc<AppState>, device: &str, measured_s: Option<f64>, estimated_s: f64) {
    for name in st.production.meter.add(device, measured_s, estimated_s) {
        st.production.notify.emit(st, Kind::MaintenanceDue, Operation::None, &format!("Maintenance due: {name}"), None);
        super::changed(st, "maintenance");
    }
}

/// The meter of `device` as the cockpit and Settings show it.
pub fn status_json(st: &AppState, device: &str) -> Value {
    let d = st.production.meter.device(device);
    let reminders: Vec<Value> = d.reminders.iter().map(|r| d.reminder_json(r)).collect();
    let due: Vec<&Value> = reminders.iter().filter(|r| r["due"] == true).collect();
    json!({
        "device": device,
        "hours": d.total_s() / 3600.0,
        "measured_hours": d.measured_s / 3600.0,
        "estimated_hours": d.estimated_s / 3600.0,
        "runs": d.runs,
        "since_unix_s": if d.since_unix_s == 0 { Value::Null } else { json!(d.since_unix_s) },
        "due": due,
        "reminders": reminders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_runs_and_reminders() {
        let dir = std::env::temp_dir().join(format!("openetch-meter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = Meter::default();
        m.open(&dir);
        m.set_reminders("b6", vec![
            ReminderInput { id: None, name: "Clean the lens".into(), every_hours: Some(1.0), every_runs: None },
            ReminderInput { id: None, name: "Check the jig".into(), every_hours: None, every_runs: Some(3) },
        ])
        .unwrap();
        assert!(m.add("b6", Some(1800.0), 10.0).is_empty());
        assert!(m.add("b6", None, 600.0).is_empty(), "estimated time counts when the board gives none");
        let newly = m.add("b6", Some(1300.0), 5.0);
        assert_eq!(newly, ["Clean the lens", "Check the jig"]);
        assert!(m.add("b6", Some(1.0), 0.0).is_empty(), "sent once");
        let d = m.device("b6");
        assert_eq!((d.measured_s, d.estimated_s, d.runs), (3101.0, 600.0, 4));
        assert!(d.reminders.iter().all(|r| d.due(r)));
        let lens = d.reminders[0].id.clone();
        m.done("b6", &lens).unwrap();
        let d = m.device("b6");
        assert!(!d.due(&d.reminders[0]) && d.due(&d.reminders[1]));
        // Kept across a restart; a reminder sent back with its id keeps its last done.
        let again = Meter::default();
        again.open(&dir);
        assert_eq!(again.device("b6").runs, 4);
        let ids: Vec<ReminderInput> = again
            .device("b6")
            .reminders
            .iter()
            .map(|r| ReminderInput { id: Some(r.id.clone()), name: r.name.clone(), every_hours: r.every_hours, every_runs: r.every_runs })
            .collect();
        again.set_reminders("b6", ids).unwrap();
        assert_eq!(again.device("b6").reminders[0].done_at_s, 3701.0);
        assert!(again.set_reminders("b6", vec![ReminderInput { id: None, name: "x".into(), every_hours: None, every_runs: None }]).is_err());
        assert!(again.done("b6", "nope").is_err());
        again.forget(|d| d == "b6");
        assert_eq!(again.device("b6").runs, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
