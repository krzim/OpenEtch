//! Fill order of jobs made from LightBurn files: LightBurn's default Fill Grouping ("Fill all shapes at once",
//! `scanOpt` absent) scans each fill pass of a layer as one sweep of scanlines across the whole layer, bottom to top
//! (or back), not shape by shape. A synthetic file checks it on every run; `ghost_files_fill_in_one_sweep` checks the
//! owner's real Halloween ghost files in place when `OPENETCH_LBRN_SAMPLES` names the folders that hold them (OS
//! path-list form; without it the test prints a note and passes). Run with `--nocapture` to see each pass's numbers.

use std::path::PathBuf;

use doc::{Document, FillGrouping, FillMode, GalvoTiming};
use plan::{JobPlan, PassPlan, Target, plan_job};

/// A 150 mm B6-like target (the field LightBurn files are imported with here).
fn target() -> Target {
    Target {
        field_mm: 150.0,
        field_height_mm: None,
        curve_tolerance_mm: None,
        swap_xy: false,
        negate_x: false,
        negate_y: false,
        max_power_pct: 100.0,
        pulse_widths_ns: lmc::JPT_M7_PULSE_WIDTHS_NS.to_vec(),
        timing: GalvoTiming {
            laser_on_delay_us: 100.0,
            laser_off_delay_us: 100.0,
            polygon_delay_us: 10.0,
            end_delay_us: 0.0,
            jump_speed_mm_s: 4000.0,
            min_jump_delay_us: 100.0,
            max_jump_delay_us: 400.0,
            jump_distance_limit_mm: 10.0,
        },
        open_mo_delay_us: 8000.0,
        calibration: lmc::Calibration::default(),
        pulse_width_enabled: true,
        z_axis: false,
        rotary_axis: false,
        port_outputs: 0,
    }
}

fn import(text: &str) -> Document {
    let p = lbrn::parse(text).expect("parse");
    let i = lbrn::to_document(&p, 150.0).expect("import");
    let mut d = Document { layers: i.layers, objects: i.objects, groups: i.groups, ..Document::default() };
    d.field_mm = Some(150.0);
    d
}

/// How one fill pass runs: per hatch direction (in order of appearance), the scanlines' positions across it.
#[derive(Debug)]
struct Sweep {
    /// Hatch lines of the pass.
    lines: usize,
    /// Runs of lines with one direction; one sweep per hatch angle has as many runs as angles.
    runs: usize,
    /// Times the scanline position turns back within a run (0 for one monotonic sweep).
    reversals: usize,
    /// Jumps between the pass's lines, mm.
    jump_mm: f64,
}

fn sweep(pass: &PassPlan, interval: f64) -> Sweep {
    let lines: Vec<[[f64; 2]; 2]> = pass.polylines.iter().filter(|p| p.len() == 2).map(|p| [p[0], p[1]]).collect();
    let dir = |l: &[[f64; 2]; 2]| {
        let a = (l[1][1] - l[0][1]).atan2(l[1][0] - l[0][0]).to_degrees().rem_euclid(180.0);
        (a.round() as i64) % 180
    };
    let (mut runs, mut reversals, mut jump_mm) = (0, 0, 0.0);
    let mut prev: Option<([[f64; 2]; 2], i64)> = None;
    let mut heading = 0.0f64;
    for l in &lines {
        let d = dir(l);
        let (s, c) = (d as f64).to_radians().sin_cos();
        let across = |p: [f64; 2]| -p[0] * s + p[1] * c;
        match prev {
            Some((q, pd)) if pd == d => {
                jump_mm += (l[0][0] - q[1][0]).hypot(l[0][1] - q[1][1]);
                let step = across(l[0]) - across(q[0]);
                // Several segments of one scanline share its position; a new scanline is an interval further.
                if step.abs() > interval / 4.0 {
                    if heading != 0.0 && step.signum() != heading {
                        reversals += 1;
                    }
                    heading = step.signum();
                }
            }
            Some((q, _)) => {
                jump_mm += (l[0][0] - q[1][0]).hypot(l[0][1] - q[1][1]);
                runs += 1;
                heading = 0.0;
            }
            None => runs += 1,
        }
        prev = Some((*l, d));
    }
    Sweep { lines: lines.len(), runs, reversals, jump_mm }
}

/// Every fill pass of `plan` (main settings and sub-layers) with its sweep, for the layers of `d` that fill.
fn fill_sweeps(d: &Document, plan: &JobPlan) -> Vec<(String, usize, u32, Sweep, usize)> {
    plan.passes
        .iter()
        .filter_map(|p| {
            let layer = d.layers.iter().find(|l| l.id == p.layer_id)?;
            let cut = if p.sub_layer == 0 { &layer.settings } else { &layer.sub_layers[p.sub_layer - 1] };
            if cut.mode != FillMode::Fill {
                return None;
            }
            let angles = if cut.hatch.cross_hatch { 2 } else { 1 };
            Some((layer.name.clone(), p.sub_layer, p.repeat, sweep(p, cut.hatch.interval_mm), angles))
        })
        .collect()
}

/// Three separate squares on one fill layer (with a cleanup-style sub-layer at 45°) and the same squares on a layer
/// that fills shapes individually (`scanOpt individual`).
const SYNTHETIC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Hand-written OpenEtch test fixture (not made by LightBurn): separate shapes on fill layers. -->
<LightBurnProject AppVersion="2.1.04" DeviceName="JCZFiber (Unknown)" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Scan">
        <index Value="0"/>
        <name Value="C00"/>
        <maxPower Value="50"/>
        <speed Value="1000"/>
        <interval Value="0.5"/>
        <priority Value="0"/>
        <SubLayer type="Scan" index="1">
            <maxPower Value="20"/>
            <speed Value="3000"/>
            <interval Value="0.5"/>
            <angle Value="45"/>
            <subname Value="cleanup"/>
        </SubLayer>
    </CutSetting>
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="C01"/>
        <maxPower Value="50"/>
        <speed Value="1000"/>
        <interval Value="0.5"/>
        <scanOpt Value="individual"/>
        <priority Value="1"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 50 90</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 75 60</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="0" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 100 84</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="1" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 50 40</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="1" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 75 20</XForm>
    </Shape>
    <Shape Type="Rect" CutIndex="1" W="10" H="10" Cr="0">
        <XForm>1 0 0 1 100 34</XForm>
    </Shape>
</LightBurnProject>
"#;

#[test]
fn separate_shapes_on_a_lightburn_fill_layer_plan_as_one_sweep() {
    let d = import(SYNTHETIC);
    let at_once = &d.layers[0];
    assert_eq!(at_once.settings.options.fill_grouping, FillGrouping::AllAtOnce, "no scanOpt: LightBurn's default");
    assert_eq!(at_once.sub_layers[0].options.fill_grouping, FillGrouping::AllAtOnce, "sub-layers too");
    assert_eq!(d.layers[1].settings.options.fill_grouping, FillGrouping::Individually, "scanOpt individual");
    let plan = plan_job(&d, &target()).expect("plan");
    let sweeps = fill_sweeps(&d, &plan);
    assert_eq!(sweeps.len(), 3, "{sweeps:#?}");
    for (name, sub, _, s, angles) in &sweeps {
        if name == "C00" {
            assert_eq!((s.runs, s.reversals), (*angles, 0), "{name} sub-layer {sub} is not one sweep: {s:?}");
        } else {
            // Shape by shape: the scanlines turn back between the squares.
            assert!(s.reversals >= 1, "{name}: {s:?}");
        }
    }
    // Every line of the three squares is marked in the one sweep (none lost to grouping): 20 per square at 0°.
    assert_eq!(sweeps[0].3.lines, 60);
}

fn sample(name: &str) -> Option<PathBuf> {
    let dirs = std::env::var_os("OPENETCH_LBRN_SAMPLES")?;
    std::env::split_paths(&dirs).map(|d| d.join(name)).find(|p| p.is_file())
}

#[test]
fn ghost_files_fill_in_one_sweep() {
    let files: Vec<PathBuf> = ["ghost cleaned - cleanup pass.lbrn2", "ghost cleaned.lbrn2"].iter().filter_map(|n| sample(n)).collect();
    if files.is_empty() {
        println!("OPENETCH_LBRN_SAMPLES is not set or lacks the ghost files; skipping the real-file test");
        return;
    }
    let mut bad = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).expect("read");
        let d = import(text.trim_start_matches('\u{feff}'));
        let plan = plan_job(&d, &target()).expect("plan");
        let sweeps = fill_sweeps(&d, &plan);
        println!(
            "{}: {} fill passes, job jumps {:.1} mm, about {:.1} s",
            f.display(),
            sweeps.len(),
            plan.stats.jump_length_mm,
            plan.stats.estimated_time_s
        );
        assert!(!sweeps.is_empty());
        for (name, sub, repeat, s, angles) in &sweeps {
            println!(
                "  {name} sub {sub} pass {repeat}: {} lines, {} direction runs ({angles} angle(s)), {} reversals, jumps {:.1} mm",
                s.lines, s.runs, s.reversals, s.jump_mm
            );
            if s.runs != *angles || s.reversals != 0 {
                bad.push(format!("{}: {name} sub {sub} pass {repeat}", f.display()));
            }
        }
    }
    assert!(bad.is_empty(), "not one sweep: {bad:#?}");
}
