//! A planned job as G-code for a GRBL 1.1 laser (and `.gcode` export).
//!
//! The planner's output is the same as for a galvo: passes of polylines in field-frame mm (origin at the bed centre,
//! +Y away from the operator), each with a power (or one per polyline for grayscale images and power-scaled shapes)
//! and a speed. [`gcode_lines`] turns it into:
//!
//! ```text
//! G90 G21          absolute millimetres
//! M4 S0            dynamic power (M3 with constant power), laser off until an S
//! G0 X.. Y..       travel to each polyline (GRBL's laser mode keeps the laser off during G0)
//! M8 / M9          air assist on / off where a pass's setting changes (layer option `air_assist`), after the travel
//! M3 S0 / M4 S0    a change of spindle mode (layer option `constant_power`), always with S0
//! G1 X.. Y.. S.. F..   the first cut of a polyline with its power and feed, later cuts only X/Y
//! M5 / G4 P.. / M4 S0  a pause before or after a pass, with the laser off
//! M5 (M9)          laser off at the end (and air assist, when it was on)
//! ```
//!
//! The GRBL layer options ([`crate::GantryPass`], from `doc::CutOptions`): `constant_power` runs a pass with `M3`
//! even when the device's default is `M4`; `air_assist` turns air on for the passes that ask for it; `overscan_mm`
//! moves the head that far before and past every straight scan line with `G1 ... S0` (laser off, at the cut's feed,
//! shortened at the bed's edge); `overcut_mm` cuts a closed outline on past its start point (at most once around).
//! S changes ride on the `G1` line they apply to, so the laser never gets a power while the head stands still.
//!
//! Coordinates are machine mm from the origin corner, positive into the bed: `S · field + [W/2, H/2]` with `S`
//! negating the mirrored axes (the rule `lbrn` uses for gantry projects); feeds are mm/min (`speed_mm_s × 60`); S is
//! `power % / 100 × $30`. Nothing is clamped: a feed above the machine's limit, a power above its limit, a point off the
//! bed, a dot-mode pass (a galvo feature) or a multi-part job (rotary/Z) are refused. Unverified on hardware.

use crate::{JobPlan, PassPlan, PlanError, Point, Result, Target};

/// The machine facts a G-code job is made for (from the engine's GRBL profile).
#[derive(Debug, Clone, PartialEq)]
pub struct GcodeSettings {
    /// The bed, `[width, height]` mm.
    pub work_area_mm: [f64; 2],
    /// The origin is on the right (`mirror_x`) / at the rear (`mirror_y`).
    pub mirror_x: bool,
    pub mirror_y: bool,
    /// `$30`: S of 100 % power.
    pub s_max: f64,
    /// Hard power limit, % of `s_max`.
    pub max_power_pct: f64,
    /// Fastest feed allowed, mm/min.
    pub max_feed_mm_min: f64,
    /// `M4` (dynamic power) rather than `M3`.
    pub dynamic_power: bool,
}

impl GcodeSettings {
    /// The planning target of this bed: the field is the bed (centred), no galvo transform, the power limit, rapid
    /// moves at the feed limit for time estimates, and `gantry` set so LMC list limits do not apply.
    pub fn target(&self) -> Target {
        let [w, h] = self.work_area_mm;
        Target {
            field_mm: w,
            field_height_mm: (h != w).then_some(h),
            curve_tolerance_mm: None,
            swap_xy: false,
            negate_x: false,
            negate_y: false,
            max_power_pct: self.max_power_pct,
            pulse_widths_ns: Vec::new(),
            timing: doc::GalvoTiming {
                laser_on_delay_us: 0.0,
                laser_off_delay_us: 0.0,
                polygon_delay_us: 0.0,
                end_delay_us: 0.0,
                jump_speed_mm_s: self.max_feed_mm_min / 60.0,
                min_jump_delay_us: 0.0,
                max_jump_delay_us: 0.0,
                jump_distance_limit_mm: 0.0,
            },
            open_mo_delay_us: 0.0,
            // Moves timed as commanded (no acceleration model yet).
            motion_time_scale: 1.0,
            calibration: lmc::Calibration::default(),
            pulse_width_enabled: false,
            z_axis: false,
            rotary_axis: false,
            port_outputs: 0,
            gantry: true,
        }
    }

    /// The overscan run-in and run-out points of the scan line `a` -> `b`: `length` mm before `a` and past `b` along
    /// the line, each shortened so it stays on the bed (field frame).
    fn overscan(&self, a: Point, b: Point, length: f64) -> (Point, Point) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let d = (dx * dx + dy * dy).sqrt();
        if d <= 1e-9 || !d.is_finite() {
            return (a, b);
        }
        let u = [dx / d, dy / d];
        let half = [self.work_area_mm[0] / 2.0, self.work_area_mm[1] / 2.0];
        // How far from `p` the bed reaches in direction `v`.
        let room = |p: Point, v: [f64; 2]| {
            (0..2)
                .filter(|&k| v[k].abs() > 1e-12)
                .map(|k| (v[k].signum() * half[k] - p[k]) / v[k])
                .fold(f64::INFINITY, f64::min)
                .max(0.0)
        };
        let back = length.min(room(a, [-u[0], -u[1]]));
        let on = length.min(room(b, u));
        ([a[0] - back * u[0], a[1] - back * u[1]], [b[0] + on * u[0], b[1] + on * u[1]])
    }

    fn to_machine(&self, p: [f64; 2]) -> Result<(f64, f64)> {
        let [w, h] = self.work_area_mm;
        if !(p[0].is_finite() && p[1].is_finite() && p[0].abs() <= w / 2.0 + 1e-9 && p[1].abs() <= h / 2.0 + 1e-9) {
            return Err(PlanError::Gcode(format!("point ({:.3}, {:.3}) mm is off the {w} x {h} mm bed", p[0], p[1])));
        }
        let x = if self.mirror_x { -p[0] } else { p[0] } + w / 2.0;
        let y = if self.mirror_y { -p[1] } else { p[1] } + h / 2.0;
        Ok((x.clamp(0.0, w), y.clamp(0.0, h)))
    }
}

/// A number for G-code: up to 3 decimals, no trailing zeros, no `-0`.
pub fn gcode_number(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// The program lines (no comments, no line endings): what a GRBL run streams. See the module docs.
pub fn gcode_lines(job: &JobPlan, s: &GcodeSettings) -> Result<Vec<String>> {
    if !(s.s_max.is_finite() && s.s_max > 0.0) {
        return Err(PlanError::Target(format!("s_max must be positive (got {})", s.s_max)));
    }
    if job.parts.len() > 1 || job.passes.iter().any(|p| p.part != 0) {
        return Err(PlanError::Gcode("the job moves an extension axis (rotary or Z), which GRBL output does not support".into()));
    }
    let mode_of = |p: &PassPlan| if p.gantry.constant_power || !s.dynamic_power { "M3" } else { "M4" };
    let first = job
        .passes
        .iter()
        .find(|p| p.polylines.iter().any(|q| q.len() >= 2))
        .map_or(if s.dynamic_power { "M4" } else { "M3" }, mode_of);
    let mut out = vec!["G90 G21".to_owned(), format!("{first} S0")];
    // The spindle mode in force (`None` after an `M5`) and whether air assist is on (`None`: not known yet). A job that
    // uses air assist does not assume it starts off: the console may have left it on (`M8`), so its first pass sets
    // it either way and the end turns it off. A job without air assist never touches it.
    let mut spindle = Some(first);
    let mut air = if job.passes.iter().any(|p| p.gantry.air_assist) { None } else { Some(false) };
    let mut pos: Option<(f64, f64)> = None;
    let (mut cur_s, mut cur_f): (Option<f64>, Option<f64>) = (Some(0.0), None);
    let mut marked = false;
    let pause = |out: &mut Vec<String>, us: f64, spindle: &mut Option<&str>| {
        if us > 0.0 {
            out.push("M5".into());
            out.push(format!("G4 P{}", gcode_number((us / 1e6).min(60.0))));
            *spindle = None;
        }
    };
    for pass in &job.passes {
        if pass.dot_time_us.is_some() {
            return Err(PlanError::Gcode(format!("layer {}: dot mode is a galvo feature; GRBL output has no dots", pass.layer_id)));
        }
        let feed = pass.settings.speed_mm_s * 60.0;
        if !(feed.is_finite() && feed > 0.0 && feed <= s.max_feed_mm_min + 1e-9) {
            return Err(PlanError::Gcode(format!(
                "layer {}: speed {} mm/s is {} mm/min, above the machine's {} mm/min",
                pass.layer_id,
                pass.settings.speed_mm_s,
                gcode_number(feed),
                s.max_feed_mm_min
            )));
        }
        let g = &pass.gantry;
        for (name, v) in [("overscan", g.overscan_mm), ("overcut", g.overcut_mm)] {
            if !(v.is_finite() && (0.0..=doc::MAX_GANTRY_EXTRA_MM).contains(&v)) {
                return Err(PlanError::Gcode(format!(
                    "layer {}: {name} {v} mm is outside 0..={} mm",
                    pass.layer_id,
                    doc::MAX_GANTRY_EXTRA_MM
                )));
            }
        }
        let want = mode_of(pass);
        pause(&mut out, pass.pause_before_us, &mut spindle);
        for (i, poly) in pass.polylines.iter().enumerate() {
            if poly.len() < 2 {
                continue;
            }
            let power = pass.power_of(i);
            if !(power.is_finite() && (0.0..=s.max_power_pct + 1e-9).contains(&power)) {
                return Err(PlanError::PowerTooHigh { layer: pass.layer_id.clone(), power_pct: power, max_pct: s.max_power_pct });
            }
            let sv = (power / 100.0 * s.s_max * 1000.0).round() / 1000.0;
            // Where the head goes with the laser off first, then each move with its S.
            let (approach, moves) = if g.overscan_mm > 0.0 && poly.len() == 2 {
                let (pre, post) = s.overscan(poly[0], poly[1], g.overscan_mm);
                let mut m = Vec::with_capacity(3);
                if !same(pre, poly[0]) {
                    m.push((poly[0], 0.0));
                }
                m.push((poly[1], sv));
                if !same(post, poly[1]) {
                    m.push((post, 0.0));
                }
                (pre, m)
            } else {
                let mut m: Vec<(Point, f64)> = poly[1..].iter().map(|p| (*p, sv)).collect();
                if g.overcut_mm > 0.0 {
                    m.extend(overcut(poly, g.overcut_mm).into_iter().map(|p| (p, sv)));
                }
                (poly[0], m)
            };
            let start = s.to_machine(approach)?;
            if pos.is_none_or(|p| (p.0 - start.0).abs() > 1e-6 || (p.1 - start.1).abs() > 1e-6) {
                out.push(format!("G0 X{} Y{}", gcode_number(start.0), gcode_number(start.1)));
                pos = Some(start);
            }
            // After the travel: `M8`/`M9` make GRBL wait until the head stands still, and in constant power (`M3`) the
            // laser keeps its last S while it stands; after a `G0` that S is off, after a cut it would burn a dot.
            if air != Some(g.air_assist) {
                out.push(if g.air_assist { "M8" } else { "M9" }.to_owned());
                air = Some(g.air_assist);
            }
            if spindle != Some(want) {
                // Always with S0: the power comes with the next cut, never while the head stands still.
                out.push(format!("{want} S0"));
                spindle = Some(want);
                cur_s = Some(0.0);
            }
            for (p, sval) in moves {
                let (x, y) = s.to_machine(p)?;
                let mut line = format!("G1 X{} Y{}", gcode_number(x), gcode_number(y));
                if cur_s != Some(sval) {
                    line.push_str(&format!(" S{}", gcode_number(sval)));
                    cur_s = Some(sval);
                }
                if cur_f != Some(feed) {
                    line.push_str(&format!(" F{}", gcode_number(feed)));
                    cur_f = Some(feed);
                }
                out.push(line);
                pos = Some((x, y));
            }
            marked = true;
        }
        pause(&mut out, pass.pause_after_us, &mut spindle);
    }
    if !marked {
        return Err(PlanError::Empty);
    }
    out.push("M5".into());
    if air != Some(false) {
        out.push("M9".into());
    }
    Ok(out)
}

fn same(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() <= 1e-9 && (a[1] - b[1]).abs() <= 1e-9
}

/// The first `length` mm of a closed polyline (first point = last, at least two segments) from its start, at most
/// once around: the overcut's points after the polyline's own. Empty for an open polyline.
fn overcut(poly: &[Point], length: f64) -> Vec<Point> {
    let (Some(first), Some(last)) = (poly.first(), poly.last()) else { return Vec::new() };
    if poly.len() < 3 || !same(*first, *last) {
        return Vec::new();
    }
    let mut left = length;
    let mut out = Vec::new();
    for w in poly.windows(2) {
        let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt();
        if d >= left {
            if d > 0.0 {
                let t = left / d;
                out.push([w[0][0] + t * (w[1][0] - w[0][0]), w[0][1] + t * (w[1][1] - w[0][1])]);
            }
            return out;
        }
        left -= d;
        out.push(w[1]);
    }
    out
}

/// The job as a `.gcode` file: `header` lines as `;` comments, then [`gcode_lines`], CRLF-free (`\n`).
pub fn gcode_file(job: &JobPlan, s: &GcodeSettings, header: &[String]) -> Result<String> {
    let mut text = String::new();
    for h in header {
        for l in h.lines() {
            text.push_str("; ");
            text.push_str(&l.replace(['\r'], ""));
            text.push('\n');
        }
    }
    for l in gcode_lines(job, s)? {
        text.push_str(&l);
        text.push('\n');
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PassPlan, PassSettings};

    fn settings() -> GcodeSettings {
        GcodeSettings {
            work_area_mm: [400.0, 300.0],
            mirror_x: false,
            mirror_y: false,
            s_max: 1000.0,
            max_power_pct: 100.0,
            max_feed_mm_min: 6000.0,
            dynamic_power: true,
        }
    }

    fn pass(polys: Vec<Vec<[f64; 2]>>, power: f64, speed: f64) -> PassPlan {
        PassPlan {
            layer_id: "layer-1".into(),
            sub_layer: 0,
            repeat: 0,
            settings: PassSettings {
                power_pct: power,
                speed_mm_s: speed,
                frequency_khz: 30.0,
                pulse_width_ns: None,
                timing: settings().target().timing,
                hatch_angle_deg: 0.0,
                motion: crate::job::MotionTime::default(),
            },
            polylines: polys,
            powers: Vec::new(),
            part: 0,
            dot_time_us: None,
            pause_before_us: 0.0,
            pause_after_us: 0.0,
            z_mm: None,
            gantry: crate::GantryPass::default(),
        }
    }

    #[test]
    fn polylines_become_travel_and_cuts() {
        let job = JobPlan {
            passes: vec![
                pass(vec![vec![[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0]], vec![[10.0, 10.0], [0.0, 20.0]]], 50.0, 20.0),
                pass(vec![vec![[0.0, 0.0], [5.0, 0.0]]], 25.5, 10.0),
            ],
            ..JobPlan::default()
        };
        let lines = gcode_lines(&job, &settings()).unwrap();
        assert_eq!(
            lines,
            [
                "G90 G21",
                "M4 S0",
                "G0 X190 Y140",
                "G1 X210 Y140 S500 F1200",
                "G1 X210 Y160",
                "G1 X200 Y170",
                "G0 X200 Y150",
                "G1 X205 Y150 S255 F600",
                "M5"
            ]
        );
    }

    #[test]
    fn corners_powers_pauses_and_refusals() {
        let mut p = pass(vec![vec![[-200.0, 150.0], [-190.0, 140.0]]], 40.0, 10.0);
        p.pause_before_us = 500_000.0;
        p.powers = vec![12.5];
        let job = JobPlan { passes: vec![p.clone()], ..JobPlan::default() };
        let rear_right = GcodeSettings { mirror_x: true, mirror_y: true, dynamic_power: false, ..settings() };
        let lines = gcode_lines(&job, &rear_right).unwrap();
        assert_eq!(lines, ["G90 G21", "M3 S0", "M5", "G4 P0.5", "G0 X400 Y0", "M3 S0", "G1 X390 Y10 S125 F600", "M5"]);
        let fast = JobPlan { passes: vec![pass(vec![vec![[0.0, 0.0], [1.0, 0.0]]], 10.0, 101.0)], ..JobPlan::default() };
        assert!(gcode_lines(&fast, &settings()).unwrap_err().to_string().contains("6060 mm/min"));
        let hot = JobPlan { passes: vec![pass(vec![vec![[0.0, 0.0], [1.0, 0.0]]], 60.0, 10.0)], ..JobPlan::default() };
        assert!(gcode_lines(&hot, &GcodeSettings { max_power_pct: 50.0, ..settings() }).is_err());
        let off = JobPlan { passes: vec![pass(vec![vec![[0.0, 0.0], [201.0, 0.0]]], 10.0, 10.0)], ..JobPlan::default() };
        assert!(gcode_lines(&off, &settings()).is_err());
        let mut dot = pass(vec![vec![[0.0, 0.0]]], 10.0, 10.0);
        dot.dot_time_us = Some(100.0);
        assert!(gcode_lines(&JobPlan { passes: vec![dot], ..JobPlan::default() }, &settings()).is_err());
        assert!(matches!(gcode_lines(&JobPlan::default(), &settings()), Err(PlanError::Empty)));
        let file = gcode_file(&job, &settings(), &["OpenEtch".into(), "two\nlines".into()]).unwrap();
        assert!(file.starts_with("; OpenEtch\n; two\n; lines\nG90 G21\n") && file.ends_with("M5\n"));
    }

    fn gantry(constant_power: bool, air_assist: bool, overscan_mm: f64, overcut_mm: f64) -> crate::GantryPass {
        crate::GantryPass { constant_power, air_assist, overscan_mm, overcut_mm }
    }

    #[test]
    fn constant_power_and_air_assist_per_pass() {
        let mut a = pass(vec![vec![[0.0, 0.0], [10.0, 0.0]]], 50.0, 20.0);
        a.gantry = gantry(true, true, 0.0, 0.0);
        let b = pass(vec![vec![[0.0, 10.0], [10.0, 10.0]]], 50.0, 20.0);
        let mut c = pass(vec![vec![[0.0, 20.0], [10.0, 20.0]]], 40.0, 20.0);
        c.gantry = gantry(false, true, 0.0, 0.0);
        let job = JobPlan { passes: vec![a, b, c], ..JobPlan::default() };
        let lines = gcode_lines(&job, &settings()).unwrap();
        assert_eq!(
            lines,
            [
                "G90 G21",
                "M3 S0",
                "G0 X200 Y150",
                "M8",
                "G1 X210 Y150 S500 F1200",
                "G0 X200 Y160",
                "M9",
                "M4 S0",
                "G1 X210 Y160 S500",
                "G0 X200 Y170",
                "M8",
                "G1 X210 Y170 S400",
                "M5",
                "M9"
            ]
        );
        // M8/M9 wait for the head to stand still, and in M3 the laser keeps its S while it stands: they never come
        // straight after a cut.
        for w in lines.windows(2) {
            assert!(!(w[0].starts_with("G1") && !w[0].ends_with(" S0") && (w[1] == "M8" || w[1] == "M9")), "{lines:?}");
        }
        // A device that is constant power anyway never switches.
        let m3 = gcode_lines(&job, &GcodeSettings { dynamic_power: false, ..settings() }).unwrap();
        assert!(!m3.iter().any(|l| l.starts_with("M4")), "{m3:?}");
        assert_eq!(m3.iter().filter(|l| l.starts_with("M3")).count(), 1, "{m3:?}");
    }

    #[test]
    fn air_assist_does_not_assume_it_starts_off() {
        // The console may have left air on (M8): a job with an air layer sets it for its first pass either way and
        // turns it off at the end; a job without air assist never touches it.
        let a = pass(vec![vec![[0.0, 0.0], [10.0, 0.0]]], 50.0, 20.0);
        let mut b = pass(vec![vec![[0.0, 10.0], [10.0, 10.0]]], 50.0, 20.0);
        b.gantry = gantry(false, true, 0.0, 0.0);
        let lines = gcode_lines(&JobPlan { passes: vec![a.clone(), b], ..JobPlan::default() }, &settings()).unwrap();
        assert_eq!(lines[..4], ["G90 G21", "M4 S0", "G0 X200 Y150", "M9"], "{lines:?}");
        assert!(lines.contains(&"M8".to_owned()) && lines.last().unwrap() == "M9", "{lines:?}");
        let plain = gcode_lines(&JobPlan { passes: vec![a], ..JobPlan::default() }, &settings()).unwrap();
        assert!(!plain.iter().any(|l| l.starts_with("M7") || l.starts_with("M8") || l.starts_with("M9")), "{plain:?}");
    }

    #[test]
    fn overscan_runs_in_and_out_with_the_laser_off() {
        let mut p = pass(vec![vec![[-10.0, 0.0], [10.0, 0.0]], vec![[10.0, 1.0], [-10.0, 1.0]]], 50.0, 20.0);
        p.gantry = gantry(false, false, 2.5, 0.0);
        let job = JobPlan { passes: vec![p], ..JobPlan::default() };
        let lines = gcode_lines(&job, &settings()).unwrap();
        assert_eq!(
            lines,
            [
                "G90 G21",
                "M4 S0",
                "G0 X187.5 Y150",
                "G1 X190 Y150 F1200",
                "G1 X210 Y150 S500",
                "G1 X212.5 Y150 S0",
                "G0 X212.5 Y151",
                "G1 X210 Y151",
                "G1 X190 Y151 S500",
                "G1 X187.5 Y151 S0",
                "M5"
            ]
        );
        // At the bed's edge the run-in is shortened, never off the bed; a diagonal line runs in along itself.
        let mut edge = pass(vec![vec![[-199.0, 0.0], [-190.0, 0.0]], vec![[0.0, 0.0], [3.0, 4.0]]], 50.0, 20.0);
        edge.gantry = gantry(false, false, 5.0, 0.0);
        let lines = gcode_lines(&JobPlan { passes: vec![edge], ..JobPlan::default() }, &settings()).unwrap();
        assert_eq!(lines[2], "G0 X0 Y150");
        assert!(lines.contains(&"G0 X197 Y146".to_owned()) && lines.contains(&"G1 X206 Y158 S0".to_owned()), "{lines:?}");
        // Every S > 0 is on a G1 between the run-in and the run-out: the S0 moves frame each cut.
        let mut img = pass(vec![vec![[0.0, 0.0], [1.0, 0.0]], vec![[1.0, 0.1], [0.0, 0.1]]], 30.0, 10.0);
        img.powers = vec![10.0, 20.0];
        img.gantry = gantry(true, false, 1.0, 0.0);
        let lines = gcode_lines(&JobPlan { passes: vec![img], ..JobPlan::default() }, &settings()).unwrap();
        for (k, l) in lines.iter().enumerate().filter(|(_, l)| l.starts_with("G1") && l.contains(" S") && !l.contains(" S0")) {
            assert!(lines[k - 1].starts_with("G1") && lines[k + 1].ends_with(" S0"), "cut {l} without run-in and run-out: {lines:?}");
        }
        assert!(lines.contains(&"G1 X201 Y150 S100".to_owned()) && lines.contains(&"G1 X200 Y150.1 S200".to_owned()), "{lines:?}");
    }

    #[test]
    fn overcut_retraces_the_start_of_closed_outlines_only() {
        let square = vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0]];
        let mut p = pass(vec![square, vec![[0.0, 20.0], [5.0, 20.0], [5.0, 25.0]]], 50.0, 20.0);
        p.gantry = gantry(false, false, 0.0, 12.0);
        let lines = gcode_lines(&JobPlan { passes: vec![p.clone()], ..JobPlan::default() }, &settings()).unwrap();
        assert_eq!(
            lines[2..9],
            ["G0 X200 Y150", "G1 X210 Y150 S500 F1200", "G1 X210 Y160", "G1 X200 Y160", "G1 X200 Y150", "G1 X210 Y150", "G1 X210 Y152"]
        );
        assert_eq!(lines[9..], ["G0 X200 Y170", "G1 X205 Y170", "G1 X205 Y175", "M5"], "an open path gets no overcut");
        p.gantry.overcut_mm = 50.0;
        let lines = gcode_lines(&JobPlan { passes: vec![p.clone()], ..JobPlan::default() }, &settings()).unwrap();
        assert_eq!(lines.iter().filter(|l| l.starts_with("G1")).count(), 4 + 4 + 2, "at most once around: {lines:?}");
        p.gantry.overcut_mm = 51.0;
        assert!(gcode_lines(&JobPlan { passes: vec![p], ..JobPlan::default() }, &settings()).is_err());
    }

    #[test]
    fn the_target_is_the_bed() {
        let t = settings().target();
        assert_eq!((t.field_mm, t.field_height_mm, t.gantry, t.timing.jump_speed_mm_s), (400.0, Some(300.0), true, 100.0));
        t.validate().unwrap();
    }
}
