//! Job control around runs (LightBurn's Laser window): the job counter (Count / Reset Count, kept per device in
//! `<config-dir>/job-counters.json`), Stop At, "end after the current run" for Repeat and Run Continuously, and the
//! progress of the job that holds the machine (`GET /api/job`, `job_progress` events).
//!
//! Safety is unchanged: an approved run still needs the operator PIN; `repeat` and `continuous` make one approval
//! cover the configured iterations, each started by `approve` with its own arming token; Stop ends them all and is
//! never gated. This module never arms anything.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::hw::Machine;

/// The counter file in the config directory.
pub const COUNTERS_FILE: &str = "job-counters.json";
/// How often `job_progress` events are sent while a job runs.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CounterFile {
    format: String,
    version: u32,
    counts: BTreeMap<String, u64>,
}

/// Job counts per device id, persisted (when a path is set) after every change.
#[derive(Debug, Default)]
struct Counters {
    path: Option<PathBuf>,
    counts: BTreeMap<String, u64>,
}

impl Counters {
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let file = CounterFile { format: "openetch-job-counters".into(), version: 1, counts: self.counts.clone() };
        let Ok(text) = serde_json::to_string_pretty(&file) else { return };
        let Some(dir) = path.parent() else { return };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(COUNTERS_FILE);
        if let Err(e) = crate::devices::write_atomic(dir, name, &text) {
            tracing::warn!(error = %e, "cannot save the job counters");
        }
    }
}

/// The job that holds the machine (approved, waiting for its start input, or running).
#[derive(Debug, Clone)]
struct Active {
    run_id: String,
    device: String,
    /// 1-based.
    iteration: u32,
    /// `None`: continuous.
    iterations: Option<u32>,
    completed: u32,
    waiting_for_input: bool,
    estimated_time_s: f64,
    /// When the current iteration started streaming.
    started: Option<Instant>,
    /// Time spent paused in the current iteration.
    paused: Duration,
    last_tick: Option<Instant>,
}

/// Counter, Stop At and the active job. See the module docs.
#[derive(Debug, Default)]
pub struct JobTracker {
    counters: Mutex<Counters>,
    stop_at: Mutex<Option<u64>>,
    active: Mutex<Option<Active>>,
    end_after_current: AtomicBool,
}

impl JobTracker {
    /// Keep the counters in `path` (`<config-dir>/job-counters.json`), loading what is there. A missing or unusable
    /// file starts from zero (and is reported).
    pub fn open(&self, path: PathBuf) {
        let counts = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<CounterFile>(&text) {
                Ok(f) if f.format == "openetch-job-counters" => f.counts,
                _ => {
                    tracing::warn!(path = %path.display(), "job counter file is not usable; counting from 0");
                    BTreeMap::new()
                }
            },
            Err(_) => BTreeMap::new(),
        };
        *lock(&self.counters) = Counters { path: Some(path), counts };
    }

    /// Runs completed on `device`.
    pub fn count(&self, device: &str) -> u64 {
        lock(&self.counters).counts.get(device).copied().unwrap_or(0)
    }

    /// Count one completed run on `device`; returns the new count.
    pub fn increment(&self, device: &str) -> u64 {
        let mut c = lock(&self.counters);
        let n = c.counts.entry(device.to_owned()).or_insert(0);
        *n += 1;
        let n = *n;
        c.save();
        n
    }

    /// Reset Count.
    pub fn reset(&self, device: &str) {
        let mut c = lock(&self.counters);
        c.counts.remove(device);
        c.save();
    }

    /// Stop At: approvals and further iterations are refused once the count reaches it (`None` turns it off).
    pub fn set_stop_at(&self, v: Option<u64>) {
        *lock(&self.stop_at) = v;
    }

    pub fn stop_at(&self) -> Option<u64> {
        *lock(&self.stop_at)
    }

    /// Whether `device` has reached Stop At.
    pub fn at_stop(&self, device: &str) -> bool {
        self.stop_at().is_some_and(|s| self.count(device) >= s)
    }

    /// Ask the running job to end after its current iteration (Repeat / Run Continuously turned off).
    pub fn end_after_current(&self) -> bool {
        let active = lock(&self.active).is_some();
        if active {
            self.end_after_current.store(true, Ordering::SeqCst);
        }
        active
    }

    pub(crate) fn ending(&self) -> bool {
        self.end_after_current.load(Ordering::SeqCst)
    }

    pub(crate) fn begin(&self, run_id: &str, device: &str, iterations: Option<u32>, estimated_time_s: f64) {
        self.end_after_current.store(false, Ordering::SeqCst);
        *lock(&self.active) = Some(Active {
            run_id: run_id.to_owned(),
            device: device.to_owned(),
            iteration: 1,
            iterations,
            completed: 0,
            waiting_for_input: false,
            estimated_time_s,
            started: None,
            paused: Duration::ZERO,
            last_tick: None,
        });
    }

    pub(crate) fn waiting(&self, iteration: u32) {
        if let Some(a) = lock(&self.active).as_mut() {
            a.iteration = iteration;
            a.waiting_for_input = true;
            a.started = None;
        }
    }

    pub(crate) fn started(&self, iteration: u32) {
        if let Some(a) = lock(&self.active).as_mut() {
            a.iteration = iteration;
            a.waiting_for_input = false;
            a.started = Some(Instant::now());
            a.paused = Duration::ZERO;
            a.last_tick = None;
        }
    }

    pub(crate) fn completed(&self, completed: u32) {
        if let Some(a) = lock(&self.active).as_mut() {
            a.completed = completed;
            a.started = None;
        }
    }

    /// The job of `run_id` ended. A newer job's progress is left alone.
    pub(crate) fn end(&self, run_id: &str) {
        let mut active = lock(&self.active);
        if active.as_ref().is_some_and(|a| a.run_id == run_id) {
            *active = None;
            self.end_after_current.store(false, Ordering::SeqCst);
        }
    }

    pub(crate) fn active_run(&self) -> Option<String> {
        lock(&self.active).as_ref().map(|a| a.run_id.clone())
    }

    /// Account paused time (called by the progress ticker).
    pub(crate) fn tick(&self, paused: bool) {
        if let Some(a) = lock(&self.active).as_mut() {
            let now = Instant::now();
            if let Some(last) = a.last_tick
                && paused
                && a.started.is_some()
            {
                a.paused += now - last;
            }
            a.last_tick = Some(now);
        }
    }

    /// `GET /api/job` for `device` (the active device): `{active, run_id, state, iteration, iterations, completed,
    /// packets_sent, packets_total, part, parts, paused, elapsed_s, estimated_time_s, remaining_s, progress, count,
    /// stop_at, end_after_current}`.
    pub fn status(&self, machine: &Machine, device: &str) -> Value {
        let a = lock(&self.active).clone();
        let count = self.count(device);
        let stop_at = self.stop_at();
        let Some(a) = a else {
            return json!({
                "active": false, "run_id": null, "state": "idle", "iteration": 0, "iterations": null, "completed": 0,
                "packets_sent": 0, "packets_total": 0, "part": 0, "parts": 0, "paused": false, "elapsed_s": 0.0,
                "estimated_time_s": 0.0, "remaining_s": 0.0, "progress": 0.0, "count": count, "stop_at": stop_at,
                "end_after_current": false,
            });
        };
        let p = machine.progress();
        // A pause asked for between iterations holds the job too (module `approve`).
        let paused = p.paused || machine.pause_requested();
        let elapsed = a.started.map_or(0.0, |s| (s.elapsed().saturating_sub(a.paused)).as_secs_f64());
        let state = if paused {
            "paused"
        } else if a.waiting_for_input {
            "waiting_for_start_input"
        } else if a.started.is_none() {
            "starting"
        } else {
            "running"
        };
        let est = a.estimated_time_s;
        let by_time = if est > 0.0 { (elapsed / est).min(1.0) } else { 0.0 };
        // Packets are buffered ahead of the galvo, so the time estimate is the better guide; never report done
        // before the board is.
        let progress = if a.started.is_some() { by_time.min(0.99) } else { 0.0 };
        json!({
            "active": true,
            "run_id": a.run_id,
            "device": a.device,
            "state": state,
            "iteration": a.iteration,
            "iterations": a.iterations,
            "completed": a.completed,
            "packets_sent": if a.started.is_some() { p.packets_sent } else { 0 },
            "packets_total": p.packets_total,
            "part": p.part,
            "parts": p.parts,
            "paused": paused,
            "elapsed_s": elapsed,
            "estimated_time_s": est,
            "remaining_s": if a.started.is_some() { (est - elapsed).max(0.0) } else { est },
            "progress": progress,
            "count": count,
            "stop_at": stop_at,
            "end_after_current": self.ending(),
        })
    }
}

/// Body of `PUT /api/job/settings`: `{stop_at: n | null}`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSettings {
    #[serde(default, deserialize_with = "double_option")]
    pub stop_at: Option<Option<u64>>,
}

fn double_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_persist_per_device() {
        let dir = crate::pin::temp_dir("jobcount");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(COUNTERS_FILE);
        let t = JobTracker::default();
        t.open(path.clone());
        assert_eq!(t.count("b6"), 0);
        assert_eq!((t.increment("b6"), t.increment("b6"), t.increment("other")), (1, 2, 1));
        let again = JobTracker::default();
        again.open(path.clone());
        assert_eq!((again.count("b6"), again.count("other")), (2, 1));
        again.reset("b6");
        let third = JobTracker::default();
        third.open(path);
        assert_eq!((third.count("b6"), third.count("other")), (0, 1));
    }

    #[test]
    fn stop_at_and_end_after_current() {
        let t = JobTracker::default();
        t.set_stop_at(Some(2));
        assert!(!t.at_stop("d"));
        t.increment("d");
        t.increment("d");
        assert!(t.at_stop("d"));
        t.set_stop_at(None);
        assert!(!t.at_stop("d"));
        assert!(!t.end_after_current(), "nothing to end");
        t.begin("run-1", "d", None, 1.0);
        assert!(t.end_after_current() && t.ending());
        t.end("run-1");
        assert!(!t.ending() && t.active_run().is_none());
        // The end of an older job never clears a newer one.
        t.begin("run-2", "d", None, 1.0);
        t.end("run-1");
        assert_eq!(t.active_run().as_deref(), Some("run-2"));
    }
}
