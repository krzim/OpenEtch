//! What runs really took on each device, and the correction that gives the time estimate (JOB-050).
//!
//! After every iteration that finished on a real board (USB) the engine records the planner's estimate (before any
//! correction, `plan::Stats::estimated_time_s`) next to the board's own list time (`GetMarkTime`,
//! `machine::RunReport::mark_time_s`) in `<config-dir>/mark-times.json`, per device id, the last [`KEPT_RUNS`]. Runs of
//! several lists (rotary strips, Z steps) and runs that were paused are not recorded: the board's count may cover only
//! the last list, or the pause.
//!
//! The estimate shown before and during a run (the preview, the run summary, the job progress) is the planner's
//! estimate times [`MarkTimes::correction`], computed by [`correction_of`]:
//!
//! - only runs estimated at [`MIN_ESTIMATE_S`] or more count (short jobs are mostly fixed costs and rounding), and only
//!   ratios measured / estimated within [`RATIO_RANGE`] (anything else is taken as a misreading, not a lesson);
//! - nothing until there are [`MIN_RUNS`] of them, so one odd run never moves the estimate on its own;
//! - of those, the last [`USED_RUNS`]: their median `m`, pulled toward 1 by the number of runs `n`:
//!   `1 + (m - 1) × n / (n + 2)` (60 % of the way at 3 runs, 83 % at 10); one odd run among them moves it not at all
//!   (it is never the median);
//! - kept within [`CORRECTION_RANGE`].
//!
//! The profile's `motion_time_scale` is the physical model (the board moves slower than commanded); this correction
//! only takes up what is left, so it stays near 1. A sample keeps the estimate of its time: after changing the
//! profile's model the older samples wash out over the next [`USED_RUNS`] runs.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The run-time file in the config directory.
pub const MARK_TIMES_FILE: &str = "mark-times.json";
const FORMAT: &str = "openetch-mark-times";
/// Runs kept per device.
pub const KEPT_RUNS: usize = 20;
/// Runs the correction is computed from (the most recent usable ones).
pub const USED_RUNS: usize = 10;
/// Usable runs needed before any correction: with fewer, one odd run could be the median.
pub const MIN_RUNS: usize = 3;
/// Shortest estimate a run must have to count, s.
pub const MIN_ESTIMATE_S: f64 = 1.0;
/// Ratios measured / estimated that count; anything outside is ignored.
pub const RATIO_RANGE: std::ops::RangeInclusive<f64> = 0.5..=2.0;
/// The correction is kept within this.
pub const CORRECTION_RANGE: std::ops::RangeInclusive<f64> = 0.8..=1.25;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One finished run: what the planner estimated and what the board measured.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub estimated_s: f64,
    pub measured_s: f64,
    #[serde(default)]
    pub unix_s: u64,
}

impl Sample {
    /// measured / estimated, when the run counts (see the module docs).
    pub fn ratio(&self) -> Option<f64> {
        if !(self.estimated_s.is_finite() && self.measured_s.is_finite() && self.estimated_s >= MIN_ESTIMATE_S) {
            return None;
        }
        Some(self.measured_s / self.estimated_s).filter(|r| RATIO_RANGE.contains(r))
    }
}

/// The correction of a device's estimates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Correction {
    /// Multiply the planner's estimate by this.
    pub factor: f64,
    /// Runs it was computed from (0: no correction yet, fewer than [`MIN_RUNS`] usable runs).
    pub runs: usize,
}

impl Default for Correction {
    fn default() -> Self {
        Correction { factor: 1.0, runs: 0 }
    }
}

/// The correction `samples` (oldest first) give. See the module docs.
pub fn correction_of(samples: &[Sample]) -> Correction {
    let mut ratios: Vec<f64> = samples.iter().rev().filter_map(Sample::ratio).take(USED_RUNS).collect();
    let n = ratios.len();
    if n < MIN_RUNS {
        return Correction::default();
    }
    ratios.sort_by(f64::total_cmp);
    let median = if n % 2 == 1 { ratios[n / 2] } else { (ratios[n / 2 - 1] + ratios[n / 2]) / 2.0 };
    #[allow(clippy::cast_precision_loss)]
    let weight = n as f64 / (n as f64 + 2.0);
    let factor = (1.0 + (median - 1.0) * weight).clamp(*CORRECTION_RANGE.start(), *CORRECTION_RANGE.end());
    Correction { factor, runs: n }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    devices: BTreeMap<String, Vec<Sample>>,
}

#[derive(Debug, Default)]
struct Store {
    path: Option<PathBuf>,
    devices: BTreeMap<String, Vec<Sample>>,
}

impl Store {
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let file = File { format: FORMAT.into(), version: 1, devices: self.devices.clone() };
        let Ok(text) = serde_json::to_string_pretty(&file) else { return };
        let Some(dir) = path.parent() else { return };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(MARK_TIMES_FILE);
        if let Err(e) = crate::devices::write_atomic(dir, name, &text) {
            tracing::warn!(error = %e, "cannot save the run times");
        }
    }
}

/// The run times of every device, persisted (when a path is set) after every run recorded.
#[derive(Debug, Default)]
pub struct MarkTimes(Mutex<Store>);

impl MarkTimes {
    /// Keep the run times in `path` (`<config-dir>/mark-times.json`), loading what is there. A file that cannot be
    /// used starts empty (and is reported).
    pub fn open(&self, path: PathBuf) {
        let devices = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<File>(&text) {
                Ok(f) if f.format == FORMAT => f.devices,
                _ => {
                    tracing::warn!(path = %path.display(), "run time file is not usable; estimates start uncorrected");
                    BTreeMap::new()
                }
            },
            Err(_) => BTreeMap::new(),
        };
        *lock(&self.0) = Store { path: Some(path), devices };
    }

    /// Record a finished run of `device`: the planner's estimate (before correction) and the board's time, both s.
    /// Values that are not finite and positive are not recorded. Returns the device's correction afterwards.
    pub fn record(&self, device: &str, estimated_s: f64, measured_s: f64) -> Correction {
        let mut s = lock(&self.0);
        if estimated_s.is_finite() && estimated_s > 0.0 && measured_s.is_finite() && measured_s > 0.0 {
            let unix_s = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let runs = s.devices.entry(device.to_owned()).or_default();
            runs.push(Sample { estimated_s, measured_s, unix_s });
            let over = runs.len().saturating_sub(KEPT_RUNS);
            runs.drain(..over);
            s.save();
        }
        correction_of(s.devices.get(device).map_or(&[], Vec::as_slice))
    }

    /// The correction of `device`'s estimates (1 with no usable runs).
    pub fn correction(&self, device: &str) -> Correction {
        correction_of(lock(&self.0).devices.get(device).map_or(&[], Vec::as_slice))
    }

    /// The recorded runs of `device`, oldest first.
    pub fn samples(&self, device: &str) -> Vec<Sample> {
        lock(&self.0).devices.get(device).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{CutSettings, Document, FillMode, Geometry, Object};
    use kurbo::Affine;

    fn rect(id: &str, w: f64, h: f64, at: (f64, f64)) -> Object {
        let g = Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: 0.0 };
        Object::new(id.into(), id, "layer-1".into(), g, Affine::translate(at))
    }

    /// The planner's estimate for `objects` with `settings` on the B6 fixture, with the profile's motion scale or
    /// `scale` instead.
    fn b6_estimate(objects: Vec<Object>, settings: CutSettings, scale: Option<f64>) -> plan::Stats {
        let mut profile = machine::MachineProfile::from_toml(include_str!("../../../fixtures/b6-mopa-60w.toml")).unwrap();
        if let Some(s) = scale {
            profile.motion_time_scale = s;
        }
        let mut d = Document::default();
        d.layers[0].settings = settings;
        d.objects = objects;
        plan::plan_job(&d, &crate::hw::target_from_profile(&profile)).unwrap().stats
    }

    /// The two runs on the owner's B6 (2026-11-01), with the board's own `GetMarkTime`. Before the motion model
    /// (scale 1) both estimates were 22-25 % short; with the profile's scale both are within 3 %.
    #[test]
    fn b6_estimate_matches_the_owners_runs() {
        // Run A: an "F" of three outlines (4 x 40, 20 x 4, 12 x 4 mm), Line, 40 %, 300 mm/s, 30 kHz, 200 ns.
        // Where the owner placed it is not known; this layout jumps 23 mm in all.
        let f = || vec![rect("stem", 4.0, 40.0, (-8.0, -20.0)), rect("top", 20.0, 4.0, (-8.0, 16.0)), rect("mid", 12.0, 4.0, (-8.0, 0.0))];
        let line = CutSettings { power_pct: 40.0, speed_mm_s: 300.0, frequency_khz: 30.0, pulse_width_ns: Some(200.0), ..CutSettings::default() };
        // Run B: a 30 x 30 mm square, Fill, 0.05 mm, 0°, bidirectional, 30 %, 1000 mm/s, 30 kHz, 200 ns.
        let square = || vec![rect("sq", 30.0, 30.0, (-15.0, -15.0))];
        let mut fill = CutSettings { mode: FillMode::Fill, speed_mm_s: 1000.0, power_pct: 30.0, ..line.clone() };
        fill.hatch.interval_mm = 0.05;
        fill.hatch.angle_deg = 0.0;
        fill.hatch.bidirectional = true;
        fill.hatch.cross_hatch = false;
        let measured_a = lmc::mark_time_s([0, 12182, 1, 544]).unwrap();
        let measured_b = lmc::mark_time_s([0, 63040, 35, 544]).unwrap();

        let b = b6_estimate(square(), fill.clone(), None);
        assert_eq!((b.polyline_count, b.mark_length_mm.round()), (600, 18000.0), "the owner's stats: 600 lines, 18000 mm");
        let a = b6_estimate(f(), line.clone(), None);
        let (err_a, err_b) = (a.estimated_time_s / measured_a - 1.0, b.estimated_time_s / measured_b - 1.0);
        assert!(err_a.abs() < 0.05, "F: {} s estimated, {measured_a} s measured", a.estimated_time_s);
        assert!(err_b.abs() < 0.02, "square: {} s estimated, {measured_b} s measured", b.estimated_time_s);

        let (a1, b1) = (b6_estimate(f(), line.clone(), Some(1.0)), b6_estimate(square(), fill.clone(), Some(1.0)));
        assert!(a1.estimated_time_s / measured_a < 0.8 && b1.estimated_time_s / measured_b < 0.8);
        assert!((b1.estimated_time_s - 18.28).abs() < 0.02, "the owner's planner said 18.283 s: {}", b1.estimated_time_s);

        // The alternatives: each run alone gives the scale (A 1.33, B 1.29); a fixed cost alone (scale 1) needs
        // 64 ms per polyline on A but 8.8 ms on B, so it is out (A's real jump path, unknown, cannot close a 7x gap:
        // that would take ~300 mm more jumping). A fixed cost per polyline or per mark segment on top of a scale
        // solves to a negative cost with this layout, but ~35 mm more jumping in A (an "F" placed ~20 mm off this
        // spot) makes it 0 and a scale of 1.29 fit both exactly: a small per-polyline cost (|c| of 1-2 ms) cannot be
        // told apart from none with these two runs. Either way the extra is mostly proportional to the motion.
        let (a2, b2) = (b6_estimate(f(), line, Some(2.0)), b6_estimate(square(), fill, Some(2.0)));
        // (motion at scale 1, fixed delays, measured) per run.
        let run = |s1: &plan::Stats, s2: &plan::Stats, m: f64| (s2.estimated_time_s - s1.estimated_time_s, 2.0 * s1.estimated_time_s - s2.estimated_time_s, m);
        let (ra, rb) = (run(&a1, &a2, measured_a), run(&b1, &b2, measured_b));
        let scale = |(motion, fixed, m): (f64, f64, f64)| (m - fixed) / motion;
        assert!((scale(ra) - 1.33).abs() < 0.01 && (scale(rb) - 1.29).abs() < 0.01, "{} {}", scale(ra), scale(rb));
        // measured = fixed + k × motion + c × count, solved for c over both runs.
        #[allow(clippy::cast_precision_loss)]
        let per_item = |na: usize, nb: usize| {
            let (na, nb) = (na as f64, nb as f64);
            ((rb.2 - rb.1) - rb.0 / ra.0 * (ra.2 - ra.1)) / (nb - rb.0 / ra.0 * na)
        };
        assert!(per_item(a1.polyline_count, b1.polyline_count).abs() < 0.002, "per polyline");
        assert!(per_item(a1.segment_count, b1.segment_count) < 0.0, "per segment (this layout)");
        #[allow(clippy::cast_precision_loss)]
        let fixed_only = |(motion, fixed, m): (f64, f64, f64), n: usize| (m - fixed - motion) / n as f64;
        assert!(fixed_only(ra, a1.polyline_count) > 0.06 && fixed_only(rb, b1.polyline_count) < 0.009);
    }

    fn s(estimated_s: f64, measured_s: f64) -> Sample {
        Sample { estimated_s, measured_s, unix_s: 0 }
    }

    #[test]
    fn no_runs_no_correction() {
        assert_eq!(correction_of(&[]), Correction { factor: 1.0, runs: 0 });
        // Too short, or a ratio that is a misreading: ignored.
        assert_eq!(correction_of(&[s(0.5, 0.7), s(10.0, 30.0), s(10.0, 1.0), s(f64::NAN, 1.0)]).runs, 0);
    }

    #[test]
    fn nothing_before_three_runs_then_most_of_the_way() {
        assert_eq!(correction_of(&[s(10.0, 11.5)]), Correction::default());
        assert_eq!(correction_of(&[s(10.0, 11.5), s(10.0, 11.5)]), Correction::default());
        let three = correction_of(&[s(10.0, 11.5); 3]);
        assert!((three.factor - 1.09).abs() < 1e-9 && three.runs == 3, "{three:?}");
        let many = correction_of(&vec![s(10.0, 11.5); 10]);
        assert!((many.factor - (1.0 + 0.15 * 10.0 / 12.0)).abs() < 1e-9, "{many:?}");
    }

    #[test]
    fn one_odd_run_does_not_swing_it() {
        let mut runs = vec![s(20.0, 21.0); 6];
        let before = correction_of(&runs).factor;
        runs.push(s(20.0, 39.0)); // a 1.95 ratio, still within RATIO_RANGE
        let after = correction_of(&runs).factor;
        assert!((after - before).abs() < 0.01, "{before} -> {after}");
        // An odd run alone, or with one other, moves nothing; at the third run it is not the median.
        assert_eq!(correction_of(&[s(20.0, 39.0)]).factor, 1.0);
        assert_eq!(correction_of(&[s(20.0, 20.0), s(20.0, 39.0)]).factor, 1.0);
        assert!((correction_of(&[s(20.0, 20.0), s(20.0, 39.0), s(20.0, 20.0)]).factor - 1.0).abs() < 1e-12);
        // Hostile values: never counted, whatever their number.
        let hostile = [s(20.0, 0.0), s(20.0, -5.0), s(20.0, f64::INFINITY), s(f64::INFINITY, 20.0), s(20.0, 1e12), s(-20.0, -20.0), s(0.0, 0.0)];
        assert_eq!(correction_of(&hostile.repeat(3)).runs, 0);
        // Never past the bounds.
        assert!(correction_of(&vec![s(20.0, 10.5); 10]).factor >= *CORRECTION_RANGE.start());
        assert_eq!(correction_of(&vec![s(20.0, 39.0); 10]).factor, *CORRECTION_RANGE.end());
    }

    #[test]
    fn only_the_recent_runs_count() {
        let mut runs = vec![s(10.0, 15.0); 10];
        runs.extend(vec![s(10.0, 10.0); USED_RUNS]);
        assert_eq!(correction_of(&runs).factor, 1.0);
    }

    #[test]
    fn runs_persist_per_device_and_are_bounded() {
        let dir = crate::pin::temp_dir("marktimes");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(MARK_TIMES_FILE);
        let t = MarkTimes::default();
        t.open(path.clone());
        assert_eq!(t.correction("b6"), Correction::default());
        for _ in 0..(KEPT_RUNS + 5) {
            t.record("b6", 10.0, 11.0);
        }
        for _ in 0..3 {
            t.record("other", 10.0, 9.0);
        }
        assert_eq!(t.record("b6", -1.0, 3.0).runs, USED_RUNS, "nonsense is not recorded");
        for (e, m) in [(10.0, 0.0), (10.0, -1.0), (f64::NAN, 10.0), (10.0, f64::INFINITY), (0.0, 10.0)] {
            t.record("b6", e, m);
        }
        assert_eq!(t.samples("b6").len(), KEPT_RUNS, "nor is any other nonsense");
        let again = MarkTimes::default();
        again.open(path.clone());
        assert_eq!(again.samples("b6").len(), KEPT_RUNS);
        assert!((again.correction("b6").factor - (1.0 + 0.1 * 10.0 / 12.0)).abs() < 1e-9);
        assert!((again.correction("other").factor - (1.0 - 0.1 * 0.6)).abs() < 1e-9);
        assert_eq!(again.correction("new").runs, 0);
        // A broken file starts empty.
        std::fs::write(&path, "{not json").unwrap();
        let broken = MarkTimes::default();
        broken.open(path);
        assert_eq!(broken.samples("b6").len(), 0);
    }
}
