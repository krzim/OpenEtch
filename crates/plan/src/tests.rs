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
        group: None,
        extras: Default::default(),
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

/// A PNG of `w` x `h` grey pixels (`grey(x, y)`, row 0 at the top) as base64.
fn png(w: u32, h: u32, grey: impl Fn(u32, u32) -> u8) -> String {
    use base64::Engine;
    let img = ::image::GrayImage::from_fn(w, h, |x, y| ::image::Luma([grey(x, y)]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), ::image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// An image object `w` x `h` mm of `data`, placed by `transform`.
fn image_object(id: &str, w: f64, h: f64, data: String, transform: Affine) -> Object {
    let g = Geometry::Image { width_mm: w, height_mm: h, mime: "image/png".into(), data_base64: data.into(), adjust: Default::default() };
    Object::new(id.into(), id, "layer-1".into(), g, transform)
}

/// Image settings at 1 mm, one-way, threshold: one run per dark stretch of a row.
fn coarse(dither: doc::DitherMode) -> CutSettings {
    let image = doc::ImageSettings { dither, interval_mm: 1.0, bidirectional: false, ..doc::ImageSettings::default() };
    CutSettings { image, ..CutSettings::default() }
}

#[test]
fn images_engrave_in_image_mode_next_to_vectors() {
    // 4 x 2 px at 1 mm: left half black. Rows burn bottom up, before the layer's vectors (traced as lines).
    let img = image_object("obj-7", 4.0, 2.0, png(4, 2, |x, _| if x < 2 { 0 } else { 255 }), Affine::IDENTITY);
    let d = doc_with(vec![img.clone(), rect("obj-1", 10.0, 10.0, (20.0, 0.0))], coarse(doc::DitherMode::Threshold));
    let plan = plan_job(&d, &target()).unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    let pass = &plan.passes[0];
    assert_eq!(&pass.polylines[..2], [vec![[-2.0, -0.5], [0.0, -0.5]], vec![[-2.0, 0.5], [0.0, 0.5]]]);
    assert_eq!(pass.polylines.len(), 3, "two runs and the rectangle");
    assert!(pass.powers.is_empty(), "every run at the layer's power");
    // Negative engraves the other half.
    let mut neg = d.clone();
    neg.layers[0].settings.image.negative = true;
    let plan = plan_job(&neg, &target()).unwrap();
    assert_eq!(plan.passes[0].polylines[0], [[0.0, -0.5], [2.0, -0.5]]);
    // The scan angle turns per pass with its increment; 90° scans along +Y.
    let mut turning = doc_with(vec![img.clone()], coarse(doc::DitherMode::Threshold));
    turning.layers[0].settings.passes = 2;
    turning.layers[0].settings.image.angle_increment_deg = 90.0;
    let plan = plan_job(&turning, &target()).unwrap();
    let first = &plan.passes[1].polylines[0];
    assert_eq!(first[0][0], first[1][0], "the second pass scans vertically: {first:?}");
    // Rotated and mirrored with the object: a 90° turn puts the black half at the bottom.
    let turned = image_object("obj-8", 4.0, 2.0, png(4, 2, |x, _| if x < 2 { 0 } else { 255 }), Affine::rotate(std::f64::consts::FRAC_PI_2));
    let plan = plan_job(&doc_with(vec![turned], coarse(doc::DitherMode::Threshold)), &target()).unwrap();
    assert!(plan.passes[0].polylines.iter().flatten().all(|p| p[1] <= 0.0), "{:?}", plan.passes[0].polylines);
    // An image outside the field is refused like any object.
    let far = image_object("obj-9", 4.0, 2.0, png(1, 1, |_, _| 0), Affine::translate((60.0, 0.0)));
    assert!(matches!(plan_job(&doc_with(vec![far], CutSettings::default()), &target()), Err(PlanError::OutOfField { .. })));
}

#[test]
fn grayscale_runs_change_power_mid_list() {
    // Black, mid grey, white: two runs at 60 % and about 35 % (min 10 %), white never fires.
    let img = image_object("obj-7", 3.0, 1.0, png(3, 1, |x, _| [0, 128, 255][x as usize]), Affine::translate((0.5, 0.5)));
    let mut s = coarse(doc::DitherMode::Grayscale);
    s.power_pct = 60.0;
    s.image.min_power_pct = 10.0;
    let plan = plan_job(&doc_with(vec![img.clone()], s.clone()), &target()).unwrap();
    let pass = &plan.passes[0];
    assert_eq!(pass.polylines.len(), 2);
    assert_eq!(pass.powers.len(), 2);
    assert_eq!(pass.powers[0], 60.0);
    let mid = 10.0 + 50.0 * 127.0 / 255.0;
    assert!((pass.powers[1] - mid).abs() < 1e-9, "{:?}", pass.powers);
    let preview = preview_plan(&plan);
    let marks: Vec<Option<f64>> = preview.segments.iter().filter(|s| s.kind == SegmentKind::Mark).map(|s| s.power_pct).collect();
    assert_eq!(marks, [None, Some(pass.powers[1])], "only marks off the pass's power carry one");
    // Encoded: the pass header's MarkCurrent, then one more before the grey run.
    let cmds: Vec<ListCommand> = decode(&encode(&plan, &target()).unwrap());
    let currents: Vec<u16> = cmds.iter().filter_map(|c| if let ListCommand::MarkCurrent { current } = c { Some(*current) } else { None }).collect();
    let mc = |p: f64| lmc::units::power_to_mark_current(p).unwrap();
    assert_eq!(currents, [mc(60.0), mc(pass.powers[1])]);
    let grey_at = cmds.iter().position(|c| matches!(c, ListCommand::MarkCurrent { current } if *current == mc(pass.powers[1]))).unwrap();
    let marks_before = cmds[..grey_at].iter().filter(|c| matches!(c, ListCommand::MarkTo { .. })).count();
    assert_eq!(marks_before, 1, "the power changes between the two runs");
    // A minimum power above the machine's limit is refused (the target allows 80 %).
    s.image.min_power_pct = 85.0;
    assert!(matches!(plan_job(&doc_with(vec![img.clone()], s.clone()), &target()), Err(PlanError::PowerTooHigh { power_pct, .. }) if power_pct == 85.0));
    // A plan whose run power was tampered with past the limit is refused by the encoder.
    let mut bad = plan.clone();
    bad.passes[0].powers[1] = 95.0;
    assert!(matches!(encode(&bad, &target()), Err(PlanError::PowerTooHigh { .. })));
}

#[test]
fn image_budgets_and_bad_images_are_refused_with_clear_errors() {
    // A checkerboard at 1 mm: 8 runs per pass of 4 x 4.
    let checker = image_object("obj-7", 4.0, 4.0, png(4, 4, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 }), Affine::IDENTITY);
    let d = doc_with(vec![checker], coarse(doc::DitherMode::Threshold));
    assert_eq!(crate::job::plan_job_within(&d, &target(), 8).unwrap().stats.segment_count, 8);
    let err = crate::job::plan_job_within(&d, &target(), 7).unwrap_err();
    assert_eq!(err, PlanError::ImageSegments { object: "obj-7".into(), max: 7 });
    assert!(err.to_string().contains("image obj-7"), "{err}");
    // Too many dots: 100 mm at 0.001 mm.
    let mut fine = doc_with(vec![image_object("obj-7", 100.0, 100.0, png(1, 1, |_, _| 0), Affine::IDENTITY)], CutSettings::default());
    fine.layers[0].settings.image.interval_mm = 0.001;
    let target = Target { field_mm: 150.0, ..target() };
    assert!(matches!(plan_job(&fine, &target), Err(PlanError::ImageTooLarge { .. })));
    // Data that is not an image.
    let junk = doc_with(vec![image_object("obj-7", 4.0, 4.0, "iVBORw0KGgo=".into(), Affine::IDENTITY)], CutSettings::default());
    let err = plan_job(&junk, &target).unwrap_err();
    assert!(matches!(&err, PlanError::Image { object, .. } if object == "obj-7"), "{err}");
    // Image settings are checked on layers with images.
    let mut wide = d.clone();
    wide.layers[0].settings.image.dot_width_mm = 2.0;
    assert!(matches!(plan_job(&wide, &target), Err(PlanError::Setting { what: "image.dot_width_mm", .. })));
    wide.objects.clear();
    wide.objects.push(rect("obj-1", 10.0, 10.0, (0.0, 0.0)));
    assert!(plan_job(&wide, &target).is_ok(), "vector-only layers ignore their image settings");
}

#[test]
fn every_dither_mode_plans_a_photo_like_ramp() {
    // A ramp with a hard edge in the middle (for sketch).
    let ramp = image_object("obj-7", 20.0, 10.0, png(40, 20, |x, y| if x < 20 { (x * 6 + y) as u8 } else { 255 }), Affine::IDENTITY);
    for m in doc::DitherMode::ALL {
        let mut s = CutSettings::default();
        s.image.dither = m;
        s.image.interval_mm = 0.1;
        s.image.min_power_pct = 5.0;
        let plan = plan_job(&doc_with(vec![ramp.clone()], s), &target()).unwrap_or_else(|e| panic!("{m:?}: {e}"));
        let runs = plan.passes.first().map_or(0, |p| p.polylines.len());
        assert!(runs > 0, "{m:?} marks something");
        assert!(plan.passes.iter().flat_map(|p| &p.polylines).flatten().all(|p| p[0].abs() <= 10.0 + 1e-9 && p[1].abs() <= 5.0 + 1e-9), "{m:?} stays inside the image");
        encode(&plan, &target()).unwrap_or_else(|e| panic!("{m:?}: {e}"));
    }
}

/// A main fill of `main` passes with an ordinary sub-layer and a cleanup sub-layer (every `n` main passes).
fn with_cleanup(main: u32, n: u32, cleanup_passes: u32) -> Document {
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings { passes: main, ..fill(0.5, false) });
    d.layers[0].sub_layers.push(CutSettings { name: Some("Outline".into()), ..CutSettings::default() });
    let mut cleanup = CutSettings { passes: cleanup_passes, power_pct: 5.0, cleanup: Some(doc::Cleanup { every_n_passes: n }), ..fill(0.5, false) };
    cleanup.hatch.angle_deg = 45.0;
    cleanup.hatch.angle_increment_deg = 10.0;
    d.layers[0].sub_layers.push(cleanup);
    d
}

#[test]
fn cleanup_sub_layers_run_between_main_passes_then_the_other_sub_layers() {
    let order = |d: &Document| -> Vec<(usize, u32)> {
        plan_job(d, &target()).unwrap().passes.iter().map(|p| (p.sub_layer, p.repeat)).collect()
    };
    // Every 2 of 5 main passes and after the last; the ordinary sub-layer (1) runs after all of that.
    assert_eq!(order(&with_cleanup(5, 2, 1)), [(0, 0), (0, 1), (2, 0), (0, 2), (0, 3), (2, 1), (0, 4), (2, 2), (1, 0)]);
    // A long interval cleans once, after the main passes; each run makes the cleanup's own passes, counted on.
    assert_eq!(order(&with_cleanup(2, 10, 2)), [(0, 0), (0, 1), (2, 0), (2, 1), (1, 0)]);
    assert_eq!(layer_schedule(&with_cleanup(3, 1, 2).layers[0]).len(), 3 + 3 * 2 + 1);
    // The cleanup's hatch keeps turning across runs, from its own angle.
    let plan = plan_job(&with_cleanup(4, 2, 1), &target()).unwrap();
    let angles: Vec<f64> = plan.passes.iter().filter(|p| p.sub_layer == 2).map(|p| p.settings.hatch_angle_deg).collect();
    assert_eq!(angles, [45.0, 55.0]);
    assert!(plan.passes.iter().filter(|p| p.sub_layer == 2).all(|p| p.settings.power_pct == 5.0));
}

#[test]
fn cleanup_and_layer_pass_limits_are_enforced() {
    let mut zero = with_cleanup(2, 1, 1);
    zero.layers[0].sub_layers[1].cleanup = Some(doc::Cleanup { every_n_passes: 0 });
    assert!(matches!(plan_job(&zero, &target()), Err(PlanError::Setting { what: "cleanup.every_n_passes", .. })));
    // 1000 main passes with a 200-pass cleanup after each: refused before anything is generated.
    let huge = with_cleanup(MAX_PASSES, 1, 200);
    let err = plan_job(&huge, &target()).unwrap_err();
    assert_eq!(err, PlanError::TooManyPasses { layer: "layer-1".into(), passes: 1000 + 1000 * 200 + 1, max: MAX_LAYER_PASSES });
    // Settings are checked even on a layer with nothing to mark.
    let mut empty = with_cleanup(1, 1, 1);
    empty.objects.clear();
    empty.layers[0].sub_layers[0].power_pct = 95.0;
    assert!(matches!(plan_job(&empty, &target()), Err(PlanError::PowerTooHigh { .. })));
}

#[test]
fn preview_lists_every_pass_and_every_sub_layer_with_its_share() {
    let d = with_cleanup(2, 1, 1);
    let plan = plan_job(&d, &target()).unwrap();
    let p = preview_document(&d, &plan);
    assert_eq!(p.passes.len(), plan.passes.len());
    for s in &p.segments {
        let pass = &p.passes[s.pass.unwrap()];
        assert_eq!(Some(pass.layer_id.as_str()), s.layer.as_deref());
    }
    let layer = &p.layers[0];
    let labels: Vec<&str> = layer.sub_layers.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Main", "Outline", "Cleanup"]);
    let planned: Vec<usize> = layer.sub_layers.iter().map(|s| s.planned_passes).collect();
    assert_eq!(planned, [2, 1, 2]);
    assert_eq!(layer.sub_layers[2].cleanup, Some(doc::Cleanup { every_n_passes: 1 }));
    assert_eq!((layer.sub_layers[2].power_pct, layer.sub_layers[2].passes), (5.0, 1));
    let sum = |f: fn(&SubLayerPreview) -> f64| layer.sub_layers.iter().map(f).sum::<f64>();
    assert!((sum(|s| s.mark_length_mm) - layer.mark_length_mm).abs() < 1e-9);
    assert!((sum(|s| s.estimated_time_s) - layer.estimated_time_s).abs() < 1e-9);
    assert_eq!(layer.sub_layers.iter().map(|s| s.segments).sum::<usize>(), layer.segments);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["passes"][1]["sub_layer"], 2, "the cleanup right after the first main pass");
    assert_eq!(v["layers"][0]["sub_layers"][1]["name"], "Outline");
}
