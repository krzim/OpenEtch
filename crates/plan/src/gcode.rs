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
//! G1 X.. Y.. S.. F..   the first cut of a polyline with its power and feed, later cuts only X/Y
//! M5 / G4 P.. / M4 a pause before or after a pass, with the laser off
//! M5               laser off at the end
//! ```
//!
//! Coordinates are machine mm from the origin corner, positive into the bed: `S · field + [W/2, H/2]` with `S`
//! negating the mirrored axes (the rule `lbrn` uses for gantry projects); feeds are mm/min (`speed_mm_s × 60`); S is
//! `power % / 100 × $30`. Nothing is clamped: a feed above the machine's limit, a power above its limit, a point off the
//! bed, a dot-mode pass (a galvo feature) or a multi-part job (rotary/Z) are refused. Unverified on hardware.

use crate::{JobPlan, PlanError, Result, Target};

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
    let spindle = if s.dynamic_power { "M4" } else { "M3" };
    let mut out = vec!["G90 G21".to_owned(), format!("{spindle} S0")];
    let mut on = true;
    let mut pos: Option<(f64, f64)> = None;
    let (mut cur_s, mut cur_f): (Option<f64>, Option<f64>) = (Some(0.0), None);
    let mut marked = false;
    let pause = |out: &mut Vec<String>, us: f64, on: &mut bool| {
        if us > 0.0 {
            out.push("M5".into());
            out.push(format!("G4 P{}", gcode_number((us / 1e6).min(60.0))));
            *on = false;
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
        pause(&mut out, pass.pause_before_us, &mut on);
        for (i, poly) in pass.polylines.iter().enumerate() {
            if poly.len() < 2 {
                continue;
            }
            let power = pass.power_of(i);
            if !(power.is_finite() && (0.0..=s.max_power_pct + 1e-9).contains(&power)) {
                return Err(PlanError::PowerTooHigh { layer: pass.layer_id.clone(), power_pct: power, max_pct: s.max_power_pct });
            }
            let sv = (power / 100.0 * s.s_max * 1000.0).round() / 1000.0;
            let start = s.to_machine(poly[0])?;
            if pos.is_none_or(|p| (p.0 - start.0).abs() > 1e-6 || (p.1 - start.1).abs() > 1e-6) {
                out.push(format!("G0 X{} Y{}", gcode_number(start.0), gcode_number(start.1)));
            }
            if !on {
                out.push(spindle.to_owned());
                on = true;
            }
            for (k, p) in poly[1..].iter().enumerate() {
                let (x, y) = s.to_machine(*p)?;
                let mut line = format!("G1 X{} Y{}", gcode_number(x), gcode_number(y));
                if k == 0 && cur_s != Some(sv) {
                    line.push_str(&format!(" S{}", gcode_number(sv)));
                    cur_s = Some(sv);
                }
                if k == 0 && cur_f != Some(feed) {
                    line.push_str(&format!(" F{}", gcode_number(feed)));
                    cur_f = Some(feed);
                }
                out.push(line);
                pos = Some((x, y));
            }
            marked = true;
        }
        pause(&mut out, pass.pause_after_us, &mut on);
    }
    if !marked {
        return Err(PlanError::Empty);
    }
    out.push("M5".into());
    Ok(out)
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
        assert_eq!(lines, ["G90 G21", "M3 S0", "M5", "G4 P0.5", "G0 X400 Y0", "M3", "G1 X390 Y10 S125 F600", "M5"]);
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

    #[test]
    fn the_target_is_the_bed() {
        let t = settings().target();
        assert_eq!((t.field_mm, t.field_height_mm, t.gantry, t.timing.jump_speed_mm_s), (400.0, Some(300.0), true, 100.0));
        t.validate().unwrap();
    }
}
