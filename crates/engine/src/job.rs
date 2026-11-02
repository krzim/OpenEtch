//! Job control around runs (LightBurn's Laser window): the job counter (Count / Reset Count, kept per device in
//! `<config-dir>/job-counters.json`), Stop At, "end after the current run" for Repeat and Run Continuously, and the
//! progress of the job that holds the machine (`GET /api/job`, `job_progress` events).
//!
//! Safety is unchanged: a run is still approved by a person (the operator PIN, or Start / the foot switch in the
//! no-PIN `click` approval mode, module `approve`); `repeat` and `continuous` make one approval
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
    /// The learned correction in `estimated_time_s` (module `marktime`), as applied when the job was planned.
    estimate_correction: f64,
    /// When the current iteration started streaming.
    started: Option<Instant>,
    /// Time spent paused in the current iteration (finished pauses).
    paused: Duration,
    /// When the pause in progress began (see [`Active::account`]).
    pause_began: Option<Instant>,
    /// The last elapsed time reported, so the clock never runs backwards.
    last_elapsed: f64,
    last_tick: Option<Instant>,
}

impl Active {
    /// Follow the pause state at `now`: a pause starts the moment it is first seen (by the ticker or a status read)
    /// and ends when it is first seen over, so the elapsed time stands still while paused and never jumps back.
    fn account(&mut self, paused: bool, now: Instant) {
        if self.started.is_none() {
            self.pause_began = None;
            return;
        }
        match (paused, self.pause_began) {
            (true, None) => self.pause_began = Some(now),
            (false, Some(b)) => {
                self.paused += now.saturating_duration_since(b);
                self.pause_began = None;
            }
            _ => {}
        }
    }

    /// Seconds of marking in the current iteration at `now` (paused time excluded), never less than last reported.
    fn elapsed(&mut self, now: Instant) -> f64 {
        let Some(s) = self.started else { return 0.0 };
        let held = self.pause_began.map_or(Duration::ZERO, |b| now.saturating_duration_since(b));
        let e = now.saturating_duration_since(s).saturating_sub(self.paused).saturating_sub(held).as_secs_f64();
        self.last_elapsed = self.last_elapsed.max(e);
        self.last_elapsed
    }
}

/// The most a running job shows: 100 % only once it has ended.
pub const MAX_RUNNING_PROGRESS: f64 = 0.99;

/// Share of the estimate from which the progress by the clock slows down ([`time_progress`]).
const SLOW_FROM: f64 = 0.9;

/// Below this many seconds left the job is shown as finishing rather than as a time ("0 s left").
pub const FINISHING_S: f64 = 1.0;

/// Progress (0..1) by the clock: `elapsed / estimate` up to 90 %, then slowing down toward 100 % without reaching
/// it (`0.9 + 0.1 × (1 - e^-(x - 0.9) / 0.1)` for `x = elapsed / estimate`, smooth at 90 %), so a job that runs
/// longer than estimated keeps moving instead of sitting at the end. 0 without an estimate.
pub fn time_progress(elapsed_s: f64, estimated_s: f64) -> f64 {
    if !(estimated_s > 0.0 && elapsed_s > 0.0) {
        return 0.0;
    }
    let x = elapsed_s / estimated_s;
    if x <= SLOW_FROM {
        return x;
    }
    let tail = 1.0 - SLOW_FROM;
    SLOW_FROM + tail * (1.0 - (-(x - SLOW_FROM) / tail).exp())
}

/// Job progress (0..1) from the packets and the clock (JOB-050). The packets bound it: at least the ones the board
/// has certainly finished (`done`, `machine::RunProgress::packets_done`: known only once the board has shown how
/// many packets it holds, else 0) and at most the sent ones; the clock ([`time_progress`]) places it between them.
/// With no packet count (`total` 0) it is by the clock alone. Never above [`MAX_RUNNING_PROGRESS`] while the job
/// runs, and never lower than before for growing `done`, `sent` and `elapsed_s` (the bounds and the clock only grow).
pub fn blended_progress(done: usize, sent: usize, total: usize, elapsed_s: f64, estimated_s: f64) -> (f64, &'static str) {
    let by_time = time_progress(elapsed_s, estimated_s);
    if total == 0 {
        return (by_time.min(MAX_RUNNING_PROGRESS), "time");
    }
    let sent = sent.min(total);
    let low = done.min(sent) as f64 / total as f64;
    let high = sent as f64 / total as f64;
    let p = by_time.clamp(low, high);
    let source = if p == by_time { "time" } else { "packets" };
    (p.min(MAX_RUNNING_PROGRESS), source)
}

/// Seconds left of a running job at `progress` (from [`blended_progress`]), or `None` when it should show as
/// finishing. By the clock (`estimate - elapsed`); when the packets put the job further on than the clock, by the
/// progress (`estimate × (1 - progress)`); when they hold it back (the board is slower than estimated), from the
/// rate measured so far (`elapsed × (1 - p) / p`), also past the estimate: packets still to be sent are not
/// "finishing". Otherwise finishing once the clock, or the progress, leaves less than [`FINISHING_S`]: both only
/// shrink, so a job on the clock never comes back from finishing to a time.
pub fn remaining_s(progress: f64, elapsed_s: f64, estimated_s: f64) -> Option<f64> {
    if estimated_s.is_nan() || estimated_s <= 0.0 {
        return None;
    }
    let by_clock = (estimated_s - elapsed_s).max(0.0);
    let clock = time_progress(elapsed_s, estimated_s);
    if progress < clock && progress > 0.0 {
        let by_rate = by_clock.max(elapsed_s * (1.0 - progress) / progress);
        return (by_rate >= FINISHING_S).then_some(by_rate);
    }
    let at_least = by_clock.min(estimated_s * (1.0 - progress));
    (at_least >= FINISHING_S).then_some(at_least)
}

/// Counter, Stop At and the active job. See the module docs.
#[derive(Debug, Default)]
pub struct JobTracker {
    /// What runs really took per device, and the estimate correction it gives (module `marktime`).
    pub times: crate::marktime::MarkTimes,
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

    /// A job starts: `estimated_time_s` per iteration, with the learned `estimate_correction` already in it.
    pub(crate) fn begin(&self, run_id: &str, device: &str, iterations: Option<u32>, estimated_time_s: f64, estimate_correction: f64) {
        self.end_after_current.store(false, Ordering::SeqCst);
        *lock(&self.active) = Some(Active {
            run_id: run_id.to_owned(),
            device: device.to_owned(),
            iteration: 1,
            iterations,
            completed: 0,
            waiting_for_input: false,
            estimated_time_s,
            estimate_correction: if estimate_correction.is_finite() && estimate_correction > 0.0 { estimate_correction } else { 1.0 },
            started: None,
            paused: Duration::ZERO,
            pause_began: None,
            last_elapsed: 0.0,
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
            a.pause_began = None;
            a.last_elapsed = 0.0;
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

    /// Whether the current iteration was paused (then its board time is not a fair measure; module `marktime`).
    pub(crate) fn iteration_paused(&self) -> bool {
        lock(&self.active).as_ref().is_some_and(|a| a.paused > Duration::ZERO || a.pause_began.is_some())
    }

    pub(crate) fn active_run(&self) -> Option<String> {
        lock(&self.active).as_ref().map(|a| a.run_id.clone())
    }

    /// Account paused time (called by the progress ticker; status reads do it too).
    pub(crate) fn tick(&self, paused: bool) {
        if let Some(a) = lock(&self.active).as_mut() {
            let now = Instant::now();
            a.account(paused, now);
            a.last_tick = Some(now);
        }
    }

    /// `GET /api/job` for `device` (the active device): `{active, run_id, state, iteration, iterations, completed,
    /// packets_sent, packets_acknowledged, packets_total, part, parts, paused, elapsed_s, estimated_time_s,
    /// remaining_s, finishing, estimate_correction, progress, progress_source, count, stop_at, end_after_current}`.
    /// `progress` blends the packets acknowledged with the clock ([`blended_progress`]), stays below 1 while the job
    /// runs and never goes back; `finishing` is true (and `remaining_s` 0) once the estimate is used up or less than
    /// [`FINISHING_S`] is left ([`remaining_s`]); `estimated_time_s` includes the device's learned correction
    /// (`estimate_correction`, module `marktime`); `elapsed_s` stands still while paused.
    pub fn status(&self, machine: &Machine, device: &str) -> Value {
        let count = self.count(device);
        let stop_at = self.stop_at();
        let p = machine.progress();
        // A pause asked for between iterations holds the job too (module `approve`).
        let paused = p.paused || machine.pause_requested();
        let snapshot = {
            let mut guard = lock(&self.active);
            guard.as_mut().map(|a| {
                let now = Instant::now();
                a.account(paused, now);
                let elapsed = a.elapsed(now);
                (a.clone(), elapsed)
            })
        };
        let Some((a, elapsed)) = snapshot else {
            return json!({
                "active": false, "run_id": null, "state": "idle", "iteration": 0, "iterations": null, "completed": 0,
                "packets_sent": 0, "packets_acknowledged": 0, "packets_total": 0, "part": 0, "parts": 0, "paused": false,
                "elapsed_s": 0.0, "estimated_time_s": 0.0, "remaining_s": 0.0, "finishing": false, "progress": 0.0,
                "progress_source": "time",
                "count": count, "stop_at": stop_at, "end_after_current": false,
            });
        };
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
        let sent = if a.started.is_some() { p.packets_sent } else { 0 };
        let done = if a.started.is_some() { p.packets_done.min(sent) } else { 0 };
        let (progress, source) =
            if a.started.is_some() { blended_progress(done, sent, p.packets_total, elapsed, est) } else { (0.0, "time") };
        // What is left ([`remaining_s`]); `None` while running: finishing (never "0 s left" while the job runs).
        let left = if a.started.is_some() { remaining_s(progress, elapsed, est) } else { Some(est) };
        let finishing = a.started.is_some() && left.is_none();
        let remaining = left.unwrap_or(0.0);
        json!({
            "active": true,
            "run_id": a.run_id,
            "device": a.device,
            "state": state,
            "iteration": a.iteration,
            "iterations": a.iterations,
            "completed": a.completed,
            "packets_sent": sent,
            "packets_acknowledged": done,
            "packets_total": p.packets_total,
            "part": p.part,
            "parts": p.parts,
            "paused": paused,
            "elapsed_s": elapsed,
            "estimated_time_s": est,
            "remaining_s": remaining,
            "finishing": finishing,
            "estimate_correction": a.estimate_correction,
            "progress": progress,
            "progress_source": source,
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
    fn progress_blends_packets_and_time() {
        // No packet count: by time, never done.
        assert_eq!(blended_progress(0, 0, 0, 5.0, 10.0), (0.5, "time"));
        assert_eq!(blended_progress(0, 0, 0, 50.0, 10.0).0, 0.99);
        // The board has finished more than the clock says: the packets pull it ahead.
        let (p, s) = blended_progress(78, 80, 100, 0.1, 10.0);
        assert!((p - 0.78).abs() < 1e-12 && s == "packets", "{p} {s}");
        // Sent, but nothing known to be finished (a board that took the whole list): the clock.
        assert_eq!(blended_progress(0, 100, 100, 2.0, 10.0), (0.2, "time"));
        // The clock ahead of the packets (a slow board): held back to what was sent.
        let (p, s) = blended_progress(8, 10, 100, 9.0, 10.0);
        assert!((p - 0.10).abs() < 1e-12 && s == "packets");
        // Between the bounds: the clock.
        assert_eq!(blended_progress(50, 52, 100, 5.0, 10.0), (0.5, "time"));
        assert!(blended_progress(100, 100, 100, 10.0, 10.0).0 < 1.0, "done only when the job ends");
        assert!(blended_progress(0, 100, 100, 1e6, 10.0).0 < 1.0, "done only when the job ends");
        assert_eq!(blended_progress(0, 500, 100, 0.0, 0.0).0, 0.0, "sent beyond the total is capped");
        assert_eq!(blended_progress(500, 500, 100, 0.0, 0.0).0, 0.99, "finished beyond the total is capped");
    }

    #[test]
    fn time_progress_slows_down_past_the_estimate_and_never_arrives() {
        assert_eq!(time_progress(0.0, 10.0), 0.0);
        assert_eq!(time_progress(5.0, 0.0), 0.0);
        assert!((time_progress(4.5, 10.0) - 0.45).abs() < 1e-12);
        assert!((time_progress(9.0, 10.0) - 0.9).abs() < 1e-12);
        // Smooth at 90 %: the slope is still 1 / estimate just after it.
        let slope = (time_progress(9.001, 10.0) - time_progress(9.0, 10.0)) / 0.001;
        assert!((slope - 0.1).abs() < 1e-3, "{slope}");
        // Up to twice the estimate it still moves (beyond that it rounds to 1; a running job is capped anyway).
        let mut last = 0.0;
        for i in 1..=200 {
            let p = time_progress(f64::from(i) * 0.1, 10.0);
            assert!(p > last && p < 1.0, "{i}: {p}");
            last = p;
        }
        assert!(blended_progress(0, 0, 0, 1e9, 10.0).0 <= MAX_RUNNING_PROGRESS);
    }

    /// The owner's B6, 2026-11-01: a 30 x 30 mm fill estimated at 18.3 s (before the motion model) that took
    /// 23.6 s, 8 list packets all sent within the first second. Seen every half second as the UI polls it, the
    /// progress never goes back, stays below 100 % until the end and no "0 s left" shows while it runs.
    #[test]
    fn a_job_longer_than_its_estimate_shows_finishing_not_0_s_left() {
        // The board finishes a packet every 2.95 s. Either it takes the whole list at once (nothing known to be done
        // before the end), or it holds three and takes the next as one finishes (then all but three are done).
        for (estimate, holds_all) in [(18.28, true), (18.28, false), (23.57, true), (23.57, false), (30.0, false)] {
            let (mut last, mut finishing_seen) = (0.0, false);
            for i in 0..=47 {
                let elapsed = f64::from(i) * 0.5;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let finished = (elapsed / 2.95) as usize;
                let (done, sent) = if holds_all { (0, 8) } else { (finished.min(5), (finished + 3).min(8)) };
                let (p, _) = blended_progress(done, sent, 8, elapsed, estimate);
                assert!(p >= last && p <= MAX_RUNNING_PROGRESS, "{estimate} s at {elapsed} s: {p} after {last}");
                last = p;
                match remaining_s(p, elapsed, estimate) {
                    Some(left) => {
                        assert!(left >= FINISHING_S, "{estimate} s at {elapsed} s: {left} s left");
                        assert!(!finishing_seen, "{estimate} s: back from finishing at {elapsed} s");
                    }
                    None => finishing_seen = true,
                }
            }
            if estimate < 23.5 {
                assert!(finishing_seen, "{estimate} s: past the estimate the job shows as finishing");
            }
        }
    }

    #[test]
    fn remaining_follows_the_clock_or_the_measured_rate() {
        // On time: the clock.
        assert_eq!(remaining_s(0.5, 5.0, 10.0), Some(5.0));
        // Packets ahead (the simulator): what the progress says is left.
        assert!((remaining_s(0.8, 1.0, 10.0).unwrap() - 2.0).abs() < 1e-9);
        // Packets behind (a slow board, 10 % done after 5 s): from the measured rate, 45 s.
        assert!((remaining_s(0.1, 5.0, 10.0).unwrap() - 45.0).abs() < 1e-9);
        // Past the estimate with most of the list not even sent: still a time, from the rate (12 s for 20 %).
        let (p, _) = blended_progress(10, 20, 100, 12.0, 10.0);
        assert!((p - 0.2).abs() < 1e-12);
        assert!((remaining_s(p, 12.0, 10.0).unwrap() - 48.0).abs() < 1e-9);
        // Past the estimate or under a second to go: finishing.
        assert_eq!(remaining_s(0.95, 9.5, 10.0), None);
        assert_eq!(remaining_s(0.99, 12.0, 10.0), None);
        assert_eq!(remaining_s(0.0, 0.0, 0.0), None);
    }

    #[test]
    fn elapsed_stands_still_while_paused_and_never_goes_back() {
        let t0 = Instant::now();
        let mut a = Active {
            run_id: "r".into(),
            device: "d".into(),
            iteration: 1,
            iterations: Some(1),
            completed: 0,
            waiting_for_input: false,
            estimated_time_s: 10.0,
            estimate_correction: 1.0,
            started: Some(t0),
            paused: Duration::ZERO,
            pause_began: None,
            last_elapsed: 0.0,
            last_tick: None,
        };
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        a.account(false, at(1000));
        assert!((a.elapsed(at(1000)) - 1.0).abs() < 1e-9);
        // Paused at 1.2 s (first seen then), read at 1.7 s and 2.9 s: stands at 1.2 s.
        a.account(true, at(1200));
        assert!((a.elapsed(at(1700)) - 1.2).abs() < 1e-9);
        a.account(true, at(2900));
        assert!((a.elapsed(at(2900)) - 1.2).abs() < 1e-9);
        // Resumed (seen at 3.0 s): counts on from 1.2 s.
        a.account(false, at(3000));
        assert!((a.elapsed(at(3000)) - 1.2).abs() < 1e-9);
        assert!((a.elapsed(at(3500)) - 1.7).abs() < 1e-9);
        // A read with an earlier instant never reports less than before.
        assert!((a.elapsed(at(3100)) - 1.7).abs() < 1e-9);
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
        t.begin("run-1", "d", None, 1.0, 1.0);
        assert!(t.end_after_current() && t.ending());
        t.end("run-1");
        assert!(!t.ending() && t.active_run().is_none());
        // The end of an older job never clears a newer one.
        t.begin("run-2", "d", None, 1.0, 1.0);
        t.end("run-1");
        assert_eq!(t.active_run().as_deref(), Some("run-2"));
    }
}
