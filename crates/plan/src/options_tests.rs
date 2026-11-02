//! Tests of the cut options, optimization settings, job options, parts (rotary, Z), start-here and preview timing.

use doc::{
    CutOptions, CutSettings, Document, DotMode, FillGrouping, FillMode, Geometry, Group, Object, ObjectOptions, OrderBy,
    Rotary, Tabs,
};
use kurbo::Affine;
use lmc::{ListCommand, decode};

use crate::target::tests::target;
use crate::*;

fn rect(id: &str, w: f64, h: f64, at: (f64, f64)) -> Object {
    Object::new(
        id.into(),
        id,
        "layer-1".into(),
        Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: 0.0 },
        Affine::translate(at),
    )
}

fn doc_with(objects: Vec<Object>, settings: CutSettings) -> Document {
    let mut doc = Document::default();
    doc.layers[0].settings = settings;
    doc.objects = objects;
    doc
}

fn fill(interval: f64) -> CutSettings {
    let mut s = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
    s.hatch.interval_mm = interval;
    s
}

fn with_options(mut s: CutSettings, o: CutOptions) -> CutSettings {
    s.options = o;
    s
}

fn cmds(plan: &JobPlan) -> Vec<ListCommand> {
    decode(&encode(plan, &target()).unwrap())
}

#[test]
fn selection_plans_only_the_selected_objects() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (20.0, 0.0))], CutSettings::default());
    let opts = JobOptions { selection: Some(vec!["obj-2".into()]), ..JobOptions::default() };
    let plan = plan_job_with(&d, &target(), &opts).unwrap();
    assert_eq!(plan.stats.polyline_count, 1);
    assert!(plan.passes[0].polylines[0].iter().all(|p| p[0] >= 15.0));
    let bad = JobOptions { selection: Some(vec!["obj-404".into()]), ..JobOptions::default() };
    assert!(matches!(plan_job_with(&d, &target(), &bad), Err(PlanError::Selection(_))));
    // A group id selects its members.
    let mut g = d.clone();
    g.groups.push(Group::new("group-1".into(), "g", None));
    g.objects[0].group = Some("group-1".into());
    let opts = JobOptions { selection: Some(vec!["group-1".into()]), ..JobOptions::default() };
    assert_eq!(plan_job_with(&g, &target(), &opts).unwrap().stats.polyline_count, 1);
}

#[test]
fn global_passes_repeat_the_layer_and_keep_turning() {
    let mut s = fill(1.0);
    s.hatch.angle_increment_deg = 10.0;
    s.passes = 2;
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s);
    d.layers[0].global_passes = 3;
    let plan = plan_job(&d, &target()).unwrap();
    assert_eq!(plan.passes.len(), 6);
    let angles: Vec<f64> = plan.passes.iter().map(|p| p.settings.hatch_angle_deg).collect();
    assert_eq!(angles, [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
}

#[test]
fn sub_layer_output_off_is_skipped() {
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default());
    d.layers[0].sub_layers.push(with_options(fill(1.0), CutOptions { output: false, ..CutOptions::default() }));
    d.layers[0].sub_layers.push(fill(1.0));
    let plan = plan_job(&d, &target()).unwrap();
    assert_eq!(plan.passes.iter().map(|p| p.sub_layer).collect::<Vec<_>>(), [0, 2]);
}

#[test]
fn pauses_become_delays_and_count_in_the_time() {
    let base = plan_job(&doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default()), &target()).unwrap();
    let o = CutOptions { pause_before_ms: 1000.0, pause_after_ms: 20.0, ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), o));
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.estimated_time_s - base.stats.estimated_time_s - 1.02).abs() < 1e-9);
    let delays: Vec<u16> = cmds(&plan).iter().filter_map(|c| if let ListCommand::DelayTime { time_10us } = c { Some(*time_10us) } else { None }).collect();
    // open MO (800), 1000 ms as 65535 + 34465, 20 ms.
    assert!(delays.contains(&65535) && delays.contains(&34465) && delays.contains(&2000), "{delays:?}");
}

#[test]
fn dot_mode_fires_points() {
    let o = CutOptions { dot_mode: Some(DotMode { dot_time_ms: 2.0, dot_spacing_mm: 5.0 }), ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), o));
    let plan = plan_job(&d, &target()).unwrap();
    let pass = &plan.passes[0];
    assert!(pass.polylines.iter().all(|p| p.len() == 1));
    assert_eq!(pass.polylines.len(), 9, "40 mm every 5 mm, both ends");
    assert_eq!(pass.dot_time_us, Some(2000.0));
    let c = cmds(&plan);
    assert_eq!(c.iter().filter(|c| matches!(c, ListCommand::LaserOnPoint { time_10us: 200 })).count(), 9);
    assert!(!c.iter().any(|c| matches!(c, ListCommand::MarkTo { .. })));
    let preview = preview_plan(&plan);
    assert_eq!(preview.segments.iter().filter(|s| s.kind == SegmentKind::Mark && s.from == s.to).count(), 9);
}

#[test]
fn tabs_perforation_and_wobble_shape_the_lines() {
    let o = CutOptions { tabs: Some(Tabs { count: 4, spacing_mm: 0.0, size_mm: 1.0 }), ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), o));
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 36.0).abs() < 1e-6);
    let o = CutOptions { perforation: Some(doc::Perforation { cut_mm: 1.0, skip_mm: 1.0 }), ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), o));
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 20.0).abs() < 1e-6 && plan.stats.polyline_count == 20);
    let o = CutOptions { wobble: Some(doc::Wobble { step_mm: 0.5, size_mm: 0.4 }), ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), o));
    assert!(plan_job(&d, &target()).unwrap().stats.mark_length_mm > 40.0 * 2.0);
}

#[test]
fn power_scale_lowers_a_shapes_power() {
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (20.0, 0.0))], CutSettings { power_pct: 40.0, ..CutSettings::default() });
    d.object_options.insert("obj-2".into(), ObjectOptions { priority: None, power_scale_pct: 50.0, ..ObjectOptions::default() });
    let plan = plan_job(&d, &target()).unwrap();
    let pass = &plan.passes[0];
    let powers: Vec<f64> = (0..pass.polylines.len()).map(|i| pass.power_of(i)).collect();
    assert!(powers.contains(&40.0) && powers.contains(&20.0), "{powers:?}");
    let currents: Vec<u16> = cmds(&plan).iter().filter_map(|c| if let ListCommand::MarkCurrent { current } = c { Some(*current) } else { None }).collect();
    assert!(currents.contains(&lmc::units::power_to_mark_current(20.0).unwrap()));
}

#[test]
fn power_scale_holds_for_fills_filled_together() {
    // New layers fill all at once; a shape with its own power scale still marks at its own power (shapes of one scale
    // share a hatch), for all at once and by group alike.
    let objects = vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (20.0, 0.0)), rect("obj-3", 10.0, 10.0, (40.0, 0.0))];
    for grouping in [FillGrouping::AllAtOnce, FillGrouping::ByGroup] {
        let o = CutOptions { fill_grouping: grouping, ..CutOptions::default() };
        let mut d = doc_with(objects.clone(), with_options(CutSettings { power_pct: 40.0, ..fill(0.5) }, o));
        d.object_options.insert("obj-2".into(), ObjectOptions { priority: None, power_scale_pct: 50.0, ..ObjectOptions::default() });
        let plan = plan_job(&d, &target()).unwrap();
        let pass = &plan.passes[0];
        let power_at = |x: f64| -> Vec<f64> {
            (0..pass.polylines.len()).filter(|&i| pass.polylines[i].iter().all(|p| (p[0] - x).abs() <= 5.0)).map(|i| pass.power_of(i)).collect()
        };
        for (x, power) in [(0.0, 40.0), (20.0, 20.0), (40.0, 40.0)] {
            let p = power_at(x);
            assert!(!p.is_empty() && p.iter().all(|&v| v == power), "{grouping:?}: the square at x {x} marks at {p:?}, not {power}");
        }
    }
}

#[test]
fn fill_grouping_all_at_once_cancels_overlaps() {
    let objects = vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (5.0, 0.0))];
    let o = CutOptions { fill_grouping: FillGrouping::Individually, ..CutOptions::default() };
    let individually = plan_job(&doc_with(objects.clone(), with_options(fill(0.5), o)), &target()).unwrap();
    // New settings fill all at once, as LightBurn does.
    assert_eq!(fill(0.5).options.fill_grouping, FillGrouping::AllAtOnce);
    let at_once = plan_job(&doc_with(objects, fill(0.5)), &target()).unwrap();
    // Individually: 200 mm² of hatch; at once (even-odd): 100 mm² (the 50 mm² overlap cancels, twice).
    assert!((individually.stats.mark_length_mm / at_once.stats.mark_length_mm - 2.0).abs() < 0.1);
}

#[test]
fn flood_fill_and_dot_width_change_the_hatch() {
    // A U shape: two posts joined at the bottom.
    let mut path = kurbo::BezPath::new();
    for (x, y) in [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (7.0, 10.0), (7.0, 3.0), (3.0, 3.0), (3.0, 10.0), (0.0, 10.0)] {
        if path.elements().is_empty() { path.move_to((x, y)) } else { path.line_to((x, y)) }
    }
    path.close_path();
    let u = Object::new("obj-1".into(), "u", "layer-1".into(), Geometry::Path { path }, Affine::IDENTITY);
    let plain = plan_job(&doc_with(vec![u.clone()], fill(0.5)), &target()).unwrap();
    let o = CutOptions { flood_fill: true, ..CutOptions::default() };
    let flood = plan_job(&doc_with(vec![u.clone()], with_options(fill(0.5), o)), &target()).unwrap();
    assert!((plain.stats.mark_length_mm - flood.stats.mark_length_mm).abs() < 1e-6, "same lines");
    assert!(flood.stats.jump_length_mm < plain.stats.jump_length_mm, "less travel across the gap");
    let o = CutOptions { fill_dot_width_mm: 0.2, ..CutOptions::default() };
    let trimmed = plan_job(&doc_with(vec![u], with_options(fill(0.5), o)), &target()).unwrap();
    assert!(trimmed.stats.mark_length_mm < plain.stats.mark_length_mm - 0.1 * plain.stats.polyline_count as f64);
}

#[test]
fn auto_rotate_turns_with_the_job_counter() {
    let mut s = fill(1.0);
    s.hatch.angle_increment_deg = 15.0;
    s.options.auto_rotate = true;
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s);
    let at = |run| plan_job_with(&d, &target(), &JobOptions { run_index: run, ..JobOptions::default() }).unwrap().passes[0].settings.hatch_angle_deg;
    assert_eq!((at(0), at(1), at(3)), (0.0, 15.0, 45.0));
}

#[test]
fn optimization_orders_by_priority_and_cuts_last() {
    let mut d = doc_with(vec![rect("obj-1", 4.0, 4.0, (0.0, 0.0)), rect("obj-2", 4.0, 4.0, (40.0, 0.0))], CutSettings::default());
    d.object_options.insert("obj-2".into(), ObjectOptions { priority: Some(0), power_scale_pct: 100.0, ..ObjectOptions::default() });
    d.object_options.insert("obj-1".into(), ObjectOptions { priority: Some(1), power_scale_pct: 100.0, ..ObjectOptions::default() });
    let plan = plan_job(&d, &target()).unwrap();
    assert!(plan.passes[0].polylines[0][0][0] < 10.0, "nearest first by default");
    d.optimization.order_by = vec![OrderBy::Priority];
    let plan = plan_job(&d, &target()).unwrap();
    assert!(plan.passes[0].polylines[0][0][0] > 30.0, "priority 0 first");
    // Cuts last: a line layer listed first runs after the fill layer.
    let mut d = doc_with(vec![rect("obj-1", 4.0, 4.0, (0.0, 0.0))], CutSettings::default());
    let mut fill_layer = doc::Layer::new("layer-2".into(), "Fill", "#f00");
    fill_layer.settings = fill(1.0);
    d.layers.push(fill_layer);
    d.objects.push(Object { layer: "layer-2".into(), ..rect("obj-2", 4.0, 4.0, (10.0, 0.0)) });
    assert_eq!(plan_job(&d, &target()).unwrap().passes[0].layer_id, "layer-1");
    d.optimization.cuts_last = true;
    assert_eq!(plan_job(&d, &target()).unwrap().passes[0].layer_id, "layer-2");
}

#[test]
fn shared_edges_are_cut_once() {
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (10.0, 0.0))], CutSettings::default());
    assert!((plan_job(&d, &target()).unwrap().stats.mark_length_mm - 80.0).abs() < 1e-6);
    d.optimization.remove_overlaps = true;
    assert!((plan_job(&d, &target()).unwrap().stats.mark_length_mm - 70.0).abs() < 1e-6);
}

#[test]
fn disabled_optimization_keeps_drawn_order() {
    let mut d = doc_with(vec![rect("obj-1", 4.0, 4.0, (40.0, 0.0)), rect("obj-2", 4.0, 4.0, (0.0, 0.0))], CutSettings::default());
    d.optimization.enabled = false;
    let plan = plan_job(&d, &target()).unwrap();
    assert!(plan.passes[0].polylines[0][0][0] > 30.0);
}

#[test]
fn z_steps_make_parts_with_a_z_axis_only() {
    let mut s = CutSettings { passes: 3, ..CutSettings::default() };
    s.options.z_step_mm = -0.5;
    s.options.z_offset_mm = 1.0;
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], s);
    let flat = plan_job(&d, &target()).unwrap();
    assert!(flat.parts.is_empty() && flat.warnings.iter().any(|w| w.contains("Z")));
    let t = Target { z_axis: true, ..target() };
    let plan = plan_job(&d, &t).unwrap();
    assert_eq!(plan.parts.iter().map(|p| p.z_mm).collect::<Vec<_>>(), [Some(1.0), Some(0.5), Some(0.0)]);
    let parts = encode_parts(&plan, &t).unwrap();
    assert_eq!(parts.len(), 3);
    assert!(encode(&plan, &t).is_err(), "one list cannot hold the parts");
}

#[test]
fn pulse_widths_are_dropped_without_q_pulse() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings { pulse_width_ns: Some(200.0), ..CutSettings::default() });
    let t = Target { pulse_width_enabled: false, ..target() };
    let plan = plan_job(&d, &t).unwrap();
    assert_eq!(plan.passes[0].settings.pulse_width_ns, None);
    assert!(!plan.warnings.is_empty());
    assert!(!decode(&encode(&plan, &t).unwrap()).iter().any(|c| matches!(c, ListCommand::FiberPulseWidth { .. })));
}

#[test]
fn rotary_jobs_are_split_into_strips() {
    // 60 mm tall in Y: more than the 110 mm field would allow if it were wrapped, but only X is bounded.
    let mut d = doc_with(vec![rect("obj-1", 10.0, 60.0, (0.0, 0.0))], CutSettings::default());
    d.rotary = Some(Rotary { enabled: true, split_width_mm: 10.0, object_diameter_mm: 80.0, steps_per_rotation: 1000, ..Rotary::default() });
    assert!(matches!(plan_job(&d, &target()), Err(PlanError::Target(_))), "no rotary axis");
    let preview = plan_job_with(&d, &target(), &JobOptions { preview: true, ..JobOptions::default() }).unwrap();
    assert!(preview.parts.is_empty() && preview.warnings.iter().any(|w| w.contains("rotary")));
    let t = Target { rotary_axis: true, ..target() };
    let plan = plan_job(&d, &t).unwrap();
    assert_eq!(plan.parts.len(), 6);
    let steps: Vec<i64> = plan.parts.iter().map(|p| p.rotary_steps.unwrap()).collect();
    // 10 mm of an 80 mm diameter is 1000 / (π·80) · 10 ≈ 39.8 steps per strip.
    assert_eq!(steps, [0, 40, 80, 119, 159, 199]);
    for pass in &plan.passes {
        for p in pass.polylines.iter().flatten() {
            assert!(p[1].abs() <= 5.0 + 1e-9, "every strip is marked around Y = 0: {p:?}");
        }
    }
    // With cylinder correction a strip edge lands at r·sin(5/r) < 5 mm.
    let max_y = plan.passes.iter().flat_map(|p| p.polylines.iter().flatten()).map(|p| p[1].abs()).fold(0.0, f64::max);
    assert!(max_y < 5.0 && max_y > 4.9, "{max_y}");
    assert_eq!(encode_parts(&plan, &t).unwrap().len(), 6);
    assert!(matches!(start_at(&plan, 0, &t), Err(PlanError::Target(_))));
    // Taller than one turn (π·15 ≈ 47 mm): it would mark over itself, so it is refused.
    d.rotary.as_mut().unwrap().object_diameter_mm = 15.0;
    assert!(matches!(plan_job(&d, &t), Err(PlanError::Target(m)) if m.contains("over itself")));
}

#[test]
fn start_here_cuts_the_plan_at_a_preview_segment() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (30.0, 0.0))], CutSettings::default());
    let plan = plan_job(&d, &target()).unwrap();
    let preview = preview_plan(&plan);
    // Segments: jump, 4 marks, jump, 4 marks.
    assert_eq!(preview.segments.len(), 10);
    let from_second = start_at(&plan, 5, &target()).unwrap();
    assert_eq!(from_second.stats.polyline_count, 1);
    assert!((from_second.stats.mark_length_mm - 40.0).abs() < 1e-9);
    let mid = start_at(&plan, 3, &target()).unwrap();
    assert!((mid.stats.mark_length_mm - 60.0).abs() < 1e-9, "two marks of the first square left: {}", mid.stats.mark_length_mm);
    assert!(start_at(&plan, 10, &target()).is_err());
    assert!(from_second.stats.estimated_time_s < plan.stats.estimated_time_s);
}

#[test]
fn preview_times_run_up_to_the_estimate() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (30.0, 0.0))], fill(0.5));
    let plan = plan_job(&d, &target()).unwrap();
    let preview = preview_plan(&plan);
    let times: Vec<f64> = preview.segments.iter().map(|s| s.t_s).collect();
    assert!(times.windows(2).all(|w| w[1] >= w[0]));
    assert!((times.last().unwrap() - plan.stats.estimated_time_s).abs() < 1e-9, "{} vs {}", times.last().unwrap(), plan.stats.estimated_time_s);
}

#[test]
fn estimate_times_motion_at_the_speed_words_and_the_motion_scale() {
    // 300 mm/s on a 153.75 mm field is the speed word 127 (truncated), 297.95 mm/s; 2000 mm/s jumps are 852, 1999.3.
    let t = Target { field_mm: 153.75, ..target() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (30.0, 0.0))], fill(0.5));
    let mut d300 = d.clone();
    d300.layers[0].settings.speed_mm_s = 300.0;
    let plan = plan_job(&d300, &t).unwrap();
    let s = &plan.passes[0].settings;
    let word_speed = 127.0 * 153.75 * 1000.0 / 65535.0;
    assert!((s.motion.mark_speed_mm_s - word_speed).abs() < 1e-9, "{:?}", s.motion);
    assert!((s.mark_us(word_speed) - 1e6).abs() < 1e-6, "one second of marking");
    assert_eq!(s.motion.scale, 1.0);
    // The board's motion takes 1.3 times as long: only the motion grows, the delays stay.
    let scaled = plan_job(&d300, &Target { motion_time_scale: 1.3, ..t.clone() }).unwrap();
    let motion_s = (s.mark_us(plan.stats.mark_length_mm) + s.jump_us(plan.stats.jump_length_mm)) / 1e6;
    let grown = scaled.stats.estimated_time_s - plan.stats.estimated_time_s;
    assert!((grown - 0.3 * motion_s).abs() < 1e-9, "{grown} vs {}", 0.3 * motion_s);
    // The timeline and the encoded list agree with the stats.
    let preview = preview_plan(&scaled);
    assert!((preview.segments.last().unwrap().t_s - scaled.stats.estimated_time_s).abs() < 1e-9);
    let t13 = Target { motion_time_scale: 1.3, ..t.clone() };
    let sim = simulate(&encode(&scaled, &t13).unwrap(), &t13);
    assert!((sim.stats.estimated_time_s / scaled.stats.estimated_time_s - 1.0).abs() < 0.01);
    // Out of range: refused.
    assert!(plan_job(&d300, &Target { motion_time_scale: 3.0, ..t }).is_err());
}

#[test]
fn scale_times_scales_every_time_of_a_preview() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], fill(0.5));
    let plan = plan_job(&d, &target()).unwrap();
    let mut p = preview_document(&d, &plan);
    let before = p.clone();
    scale_times(&mut p, 1.25);
    assert!((p.stats.estimated_time_s - 1.25 * before.stats.estimated_time_s).abs() < 1e-12);
    assert!((p.layers[0].estimated_time_s - 1.25 * before.layers[0].estimated_time_s).abs() < 1e-12);
    assert!((p.passes[0].estimated_time_s - 1.25 * before.passes[0].estimated_time_s).abs() < 1e-12);
    assert!((p.segments.last().unwrap().t_s - p.stats.estimated_time_s).abs() < 1e-9);
    scale_times(&mut p, f64::NAN);
    assert!((p.stats.estimated_time_s - 1.25 * before.stats.estimated_time_s).abs() < 1e-12);
}

#[test]
fn calibration_changes_the_encoded_words() {
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], CutSettings::default());
    let plan = plan_job(&d, &target()).unwrap();
    let mut t = target();
    t.calibration.offset_x_mm = 1.0;
    let first = |t: &Target| decode(&encode(&plan, t).unwrap()).into_iter().find_map(|c| if let ListCommand::JumpTo { x, .. } = c { Some(x) } else { None }).unwrap();
    assert_eq!(first(&t), lmc::units::field_to_galvo(-5.0, -5.0, &t.transform()).unwrap().0);
    assert!(first(&t) > first(&target()));
}

#[test]
fn curve_tolerance_from_the_document_or_device_sets_how_finely_curves_are_marked() {
    let circle = Object::new("obj-1".into(), "c", "layer-1".into(), Geometry::Ellipse { rx_mm: 20.0, ry_mm: 20.0 }, Affine::IDENTITY);
    let mut d = doc_with(vec![circle], CutSettings::default());
    let points = |d: &Document, t: &Target| plan_job(d, t).unwrap().passes[0].polylines[0].len();
    let fine = points(&d, &target());
    let device = Target { curve_tolerance_mm: Some(0.5), ..target() };
    let coarse = points(&d, &device);
    assert!(coarse * 3 < fine, "{coarse} vs {fine}");
    d.curve_tolerance_mm = Some(0.01);
    assert_eq!(points(&d, &device), fine, "the document's setting wins over the device's");
    // A non-square field bounds Y by its height.
    let tall = Object::new("obj-2".into(), "r", "layer-1".into(), Geometry::Rect { width_mm: 2.0, height_mm: 2.0, corner_radius_mm: 0.0 }, Affine::translate((0.0, 40.0)));
    let d = doc_with(vec![tall], CutSettings::default());
    assert!(plan_job(&d, &target()).is_ok());
    assert!(matches!(plan_job(&d, &Target { field_height_mm: Some(60.0), ..target() }), Err(PlanError::OutOfField { .. })));
}
