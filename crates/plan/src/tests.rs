//! End-to-end tests: document -> plan -> packets -> simulation. Nothing here talks to hardware.

use doc::{CutSettings, Document, FillMode, Geometry, Object};
use kurbo::Affine;
use lmc::{LASER_ENABLE_BIT, ListCommand, decode, emits_light_only};

use crate::target::tests::target;
use crate::*;

fn rect(id: &str, w: f64, h: f64, at: (f64, f64)) -> Object {
    Object {
        id: id.into(),
        name: id.into(),
        layer: "layer-1".into(),
        geometry: Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: 0.0 },
        transform: Affine::translate(at),
    }
}

fn doc_with(objects: Vec<Object>, settings: CutSettings) -> Document {
    let mut doc = Document::default();
    doc.layers[0].settings = settings;
    doc.objects = objects;
    doc
}

fn fill(interval: f64, cross: bool) -> CutSettings {
    let mut s = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
    s.hatch.interval_mm = interval;
    s.hatch.cross_hatch = cross;
    s
}

fn line_count(plan: &JobPlan) -> usize {
    plan.passes.iter().map(|p| p.polylines.len()).sum()
}

#[test]
fn rectangle_line_job_marks_its_perimeter() {
    let doc = doc_with(vec![rect("obj-1", 20.0, 10.0, (5.0, -3.0))], CutSettings::default());
    let plan = plan_job(&doc, &target()).unwrap();
    assert_eq!(plan.passes.len(), 1);
    assert_eq!(plan.passes[0].polylines.len(), 1);
    let poly = &plan.passes[0].polylines[0];
    assert_eq!(poly.first(), poly.last(), "a closed outline ends where it starts");
    assert!((plan.stats.mark_length_mm - 60.0).abs() < 0.1, "{}", plan.stats.mark_length_mm);
    assert_eq!(plan.stats.segment_count, 4);
    assert!(plan.stats.estimated_time_s > 0.0);
}

#[test]
fn line_job_starts_at_the_corner_nearest_the_centre() {
    let doc = doc_with(vec![rect("obj-1", 10.0, 10.0, (20.0, 20.0))], CutSettings::default());
    let plan = plan_job(&doc, &target()).unwrap();
    assert_eq!(plan.passes[0].polylines[0][0], [15.0, 15.0]);
    assert!((plan.stats.jump_length_mm - 15.0 * 2f64.sqrt()).abs() < 1e-9);
}

#[test]
fn hatch_of_a_10mm_square_at_0_1mm_has_about_100_lines() {
    let doc = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], fill(0.1, false));
    let n = line_count(&plan_job(&doc, &target()).unwrap());
    assert!((98..=102).contains(&n), "{n} lines");
}

#[test]
fn cross_hatch_doubles_the_lines() {
    let single = line_count(&plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], fill(0.1, false)), &target()).unwrap());
    let cross = line_count(&plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], fill(0.1, true)), &target()).unwrap());
    assert_eq!(cross, 2 * single);
}

#[test]
fn fill_then_line_hatches_first() {
    let mut s = fill(1.0, false);
    s.mode = FillMode::FillThenLine;
    let plan = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap();
    let polys = &plan.passes[0].polylines;
    assert_eq!(polys.len(), 11);
    assert_eq!(polys.last().map(Vec::len), Some(5), "the outline comes last");
}

#[test]
fn unidirectional_hatch_runs_one_way() {
    let mut s = fill(1.0, false);
    s.hatch.bidirectional = false;
    let plan = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap();
    assert!(plan.passes[0].polylines.iter().all(|l| l[1][0] > l[0][0]));
}

#[test]
fn passes_repeat_and_rotate_the_hatch() {
    let mut s = fill(1.0, false);
    s.passes = 3;
    s.hatch.angle_increment_deg = 90.0;
    let mut doc = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s);
    doc.layers[0].sub_layers.push(CutSettings::default());
    let plan = plan_job(&doc, &target()).unwrap();
    let angles: Vec<f64> = plan.passes.iter().map(|p| p.settings.hatch_angle_deg).collect();
    assert_eq!(angles, vec![0.0, 90.0, 180.0, 0.0]);
    assert_eq!(plan.passes[3].sub_layer, 1);
    // The 90° pass hatches vertically.
    assert!(plan.passes[1].polylines.iter().all(|l| (l[0][0] - l[1][0]).abs() < 1e-9));
}

#[test]
fn layers_without_output_are_skipped() {
    let mut doc = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default());
    doc.layers[0].output = false;
    let plan = plan_job(&doc, &target()).unwrap();
    assert!(plan.passes.is_empty());
    assert_eq!(encode(&plan, &target()), Err(PlanError::Empty));
}

#[test]
fn out_of_field_is_refused_naming_the_objects() {
    let doc = doc_with(
        vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (52.0, 0.0)), rect("obj-3", 2.0, 2.0, (0.0, -60.0))],
        CutSettings::default(),
    );
    let err = plan_job(&doc, &target()).unwrap_err();
    assert_eq!(err, PlanError::OutOfField { objects: vec!["obj-2".into(), "obj-3".into()], field_mm: 110.0 });
    let msg = err.to_string();
    assert!(msg.contains("obj-2") && msg.contains("obj-3") && !msg.contains("obj-1"), "{msg}");
}

#[test]
fn objects_exactly_on_the_field_edge_are_refused() {
    let doc = doc_with(vec![rect("edge", 10.0, 10.0, (50.0, 0.0))], CutSettings::default());
    assert!(matches!(plan_job(&doc, &target()), Err(PlanError::OutOfField { .. })));
}

#[test]
fn power_above_the_machine_limit_is_refused() {
    let s = CutSettings { power_pct: 85.0, ..CutSettings::default() };
    let err = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap_err();
    assert_eq!(err, PlanError::PowerTooHigh { layer: "layer-1".into(), power_pct: 85.0, max_pct: 80.0 });
}

#[test]
fn power_above_the_limit_in_a_hand_made_plan_is_refused_by_encode() {
    let mut plan = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default()), &target()).unwrap();
    plan.passes[0].settings.power_pct = 90.0;
    assert!(matches!(encode(&plan, &target()), Err(PlanError::PowerTooHigh { .. })));
}

#[test]
fn pulse_widths_snap_or_are_refused() {
    let obj = || vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))];
    let s = CutSettings { pulse_width_ns: Some(199.8), ..CutSettings::default() };
    let plan = plan_job(&doc_with(obj(), s), &target()).unwrap();
    assert_eq!(plan.passes[0].settings.pulse_width_ns, Some(200));
    let s = CutSettings { pulse_width_ns: Some(210.0), ..CutSettings::default() };
    let err = plan_job(&doc_with(obj(), s), &target()).unwrap_err();
    assert_eq!(err, PlanError::PulseWidth { layer: "layer-1".into(), requested_ns: 210.0, nearest_ns: 200 });
}

#[test]
fn unencodable_settings_are_refused_at_plan_time() {
    let s = CutSettings { frequency_khz: 0.1, ..CutSettings::default() };
    let err = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap_err();
    assert!(matches!(err, PlanError::Encode { .. }), "{err:?}");
    let s = CutSettings { speed_mm_s: 0.0, ..CutSettings::default() };
    let err = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap_err();
    assert!(matches!(err, PlanError::Setting { what: "speed_mm_s", .. }), "{err:?}");
}

#[test]
fn list_opens_and_closes_emission_in_order() {
    let s = CutSettings { pulse_width_ns: Some(200.0), ..CutSettings::default() };
    let plan = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s), &target()).unwrap();
    let packets = encode(&plan, &target()).unwrap();
    assert!(!emits_light_only(&packets));
    let cmds = decode(&packets);
    assert_eq!(cmds[0], ListCommand::ReadyMark);
    assert_eq!(cmds[1], ListCommand::DelayTime { time_10us: 800 });
    assert_eq!(cmds[2], ListCommand::WritePort { bits: LASER_ENABLE_BIT });
    assert!(matches!(cmds[3], ListCommand::JumpSpeed { .. }));
    assert_eq!(cmds[4], ListCommand::FiberPulseWidth { ns: 200 });
    assert_eq!(cmds[5], ListCommand::MarkCurrent { current: lmc::units::power_to_mark_current(20.0).unwrap() });
    assert_eq!(cmds[6], ListCommand::QSwitchPeriod { period: 667 });
    assert!(matches!(cmds[7], ListCommand::MarkSpeed { .. }));
    // The target's 10 µs polygon delay is one 10 µs word (meerk40t's encoding).
    assert_eq!(cmds[10], ListCommand::PolygonDelay { time_10us: 1 });
    let n = cmds.len();
    assert_eq!(cmds[n - 2..], [ListCommand::WritePort { bits: 0 }, ListCommand::EndOfList]);
    assert_eq!(cmds.iter().filter(|c| matches!(c, ListCommand::MarkTo { .. })).count(), 4);
    // The MO is opened and closed only by the machine's single commands, never in-list.
    assert!(!cmds.iter().any(|c| matches!(c, ListCommand::FiberOpenMo { .. })));
}

/// Mark segments of a preview, for comparing plan and simulation.
fn marks(p: &Preview) -> Vec<(Point, Point)> {
    p.segments.iter().filter(|s| s.kind == SegmentKind::Mark).map(|s| (s.from, s.to)).collect()
}

fn assert_round_trip(doc: &Document, t: &Target) {
    let plan = plan_job(doc, t).unwrap();
    let packets = encode(&plan, t).unwrap();
    let sim = simulate(&packets, t);
    let pre = preview_plan(&plan);
    let (a, b) = (marks(&pre), marks(&sim));
    assert_eq!(a.len(), b.len());
    let unit = t.galvo_unit_mm();
    for ((pf, pt), (sf, st)) in a.iter().zip(&b) {
        for (p, s) in [(pf, sf), (pt, st)] {
            assert!((p[0] - s[0]).abs() <= unit && (p[1] - s[1]).abs() <= unit, "plan {p:?} vs simulated {s:?}");
        }
    }
    assert!(sim.segments.iter().filter(|s| s.kind == SegmentKind::Mark).all(|s| s.emits), "every mark fires");
    assert!(sim.segments.iter().filter(|s| s.kind == SegmentKind::Jump).all(|s| !s.emits));
    assert!((sim.stats.mark_length_mm - plan.stats.mark_length_mm).abs() < 0.01 * plan.stats.segment_count as f64);
    assert_eq!(sim.stats.segment_count, plan.stats.segment_count);
    let rel = (sim.stats.estimated_time_s - plan.stats.estimated_time_s).abs() / plan.stats.estimated_time_s;
    assert!(rel < 0.02, "time estimates differ: plan {} s, simulated {} s", plan.stats.estimated_time_s, sim.stats.estimated_time_s);
}

fn mixed_doc() -> Document {
    let mut s = fill(0.25, true);
    s.mode = FillMode::FillThenLine;
    let mut circle = rect("obj-2", 0.0, 0.0, (-20.0, 10.0));
    circle.geometry = Geometry::Ellipse { rx_mm: 8.0, ry_mm: 5.0 };
    doc_with(vec![rect("obj-1", 12.0, 7.0, (15.0, -10.0)), circle], s)
}

#[test]
fn encode_then_simulate_matches_the_plan() {
    assert_round_trip(&mixed_doc(), &target());
}

#[test]
fn swap_and_negate_are_respected() {
    let doc = mixed_doc();
    for bits in 0..8u8 {
        let t = Target { swap_xy: bits & 1 != 0, negate_x: bits & 2 != 0, negate_y: bits & 4 != 0, ..target() };
        assert_round_trip(&doc, &t);
    }
    // The galvo words themselves move: field (+20, 0) is galvo X high normally and galvo Y low when X-negated (in
    // the field frame) and then swapped.
    let s = CutSettings { mode: FillMode::Line, ..CutSettings::default() };
    let mut obj = rect("obj-1", 0.0, 0.0, (0.0, 0.0));
    let mut path = kurbo::BezPath::new();
    path.move_to((20.0, 0.0));
    path.line_to((20.0, 5.0));
    obj.geometry = Geometry::Path { path };
    let doc = doc_with(vec![obj], s);
    let words = |t: &Target| {
        let packets = encode(&plan_job(&doc, t).unwrap(), t).unwrap();
        decode(&packets).into_iter().find_map(|c| if let ListCommand::JumpTo { x, y, .. } = c { Some((x, y)) } else { None }).unwrap()
    };
    let plain = words(&target());
    assert!(plain.0 > 0x8000 && plain.1 == 0x8000);
    let swapped = words(&Target { swap_xy: true, negate_x: true, ..target() });
    assert!(swapped.0 == 0x8000 && swapped.1 < 0x8000);
    // Every combination encodes the job's first point exactly as `lmc::units::field_to_galvo` (the transform
    // `machine` frames with) does.
    for bits in 0..8u8 {
        let t = Target { swap_xy: bits & 1 != 0, negate_x: bits & 2 != 0, negate_y: bits & 4 != 0, ..target() };
        assert_eq!(words(&t), lmc::units::field_to_galvo(20.0, 0.0, &t.transform()).unwrap(), "{bits:03b}");
    }
}

#[test]
fn passes_above_the_limit_are_refused() {
    let doc = |passes| doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings { passes, ..CutSettings::default() });
    assert!(plan_job(&doc(MAX_PASSES), &target()).is_ok());
    let err = plan_job(&doc(MAX_PASSES + 1), &target()).unwrap_err();
    assert_eq!(err, PlanError::TooManyPasses { layer: "layer-1".into(), passes: MAX_PASSES + 1, max: MAX_PASSES });
    // A huge value is refused before anything is generated.
    assert!(matches!(plan_job(&doc(u32::MAX), &target()), Err(PlanError::TooManyPasses { .. })));
}

#[test]
fn segment_budget_is_enforced() {
    // 100 hatch lines per pass: one pass fits a 150-segment budget, two do not; a 50-line budget stops the hatcher.
    let one = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], fill(0.1, false));
    assert_eq!(crate::job::plan_job_within(&one, &target(), 150).map(|p| p.stats.segment_count), Ok(100));
    let two = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings { passes: 2, ..fill(0.1, false) });
    assert_eq!(crate::job::plan_job_within(&two, &target(), 150), Err(PlanError::TooManySegments { max: 150 }));
    assert_eq!(crate::job::plan_job_within(&one, &target(), 50), Err(PlanError::TooManySegments { max: 50 }));
    assert_eq!(MAX_SEGMENTS, 2_000_000);
}

#[test]
fn preview_serialises_with_lowercase_kinds() {
    let plan = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default()), &target()).unwrap();
    let json = serde_json::to_value(preview_plan(&plan)).unwrap();
    assert_eq!(json["segments"][0]["kind"], "jump");
    assert_eq!(json["segments"][1]["kind"], "mark");
    assert_eq!(json["segments"][1]["layer"], "layer-1");
    assert!(json["stats"]["mark_length_mm"].as_f64().is_some());
}
