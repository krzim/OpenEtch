//! Tests of the cut options, optimization settings, job options, parts (rotary, Z), start-here and preview timing.

use doc::{
    CutOptions, CutSettings, Document, DotMode, FillGrouping, FillMode, Geometry, Group, Object, ObjectOptions, OrderBy,
    Rotary, TabGeneration, Tabs,
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
    let o = CutOptions { tabs: Some(Tabs { count: 4, spacing_mm: 0.0, size_mm: 1.0, ..Tabs::default() }), ..CutOptions::default() };
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
fn hand_placed_tabs_skip_inner_and_tab_cut_power() {
    // A 10 x 10 square at the origin (local frame centred) with a hole: a 4 x 4 square inside it.
    let mut outer = rect("obj-1", 10.0, 10.0, (0.0, 0.0));
    outer.geometry = Geometry::Path {
        path: {
            let mut p = kurbo::BezPath::new();
            for ring in [[(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)], [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)]] {
                p.move_to(ring[0]);
                for q in &ring[1..] {
                    p.line_to(*q);
                }
                p.close_path();
            }
            p
        },
    };
    let base = CutSettings { power_pct: 40.0, ..CutSettings::default() };
    let marked = |d: &Document| plan_job(d, &target()).unwrap().stats.mark_length_mm;
    // Automatic: 4 tabs of 1 mm on both outlines (40 + 16 mm of outline).
    let tabs = Tabs { count: 4, size_mm: 1.0, ..Tabs::default() };
    let d = doc_with(vec![outer.clone()], with_options(base.clone(), CutOptions { tabs: Some(tabs.clone()), ..CutOptions::default() }));
    assert!((marked(&d) - 48.0).abs() < 1e-6, "{}", marked(&d));
    // Skip Inner Shapes: the hole is cut through.
    let skip = Tabs { skip_inner: true, ..tabs.clone() };
    let d = doc_with(vec![outer.clone()], with_options(base.clone(), CutOptions { tabs: Some(skip), ..CutOptions::default() }));
    assert!((marked(&d) - 52.0).abs() < 1e-6, "{}", marked(&d));
    // The switch off cuts everything; the settings stay.
    let off = Tabs { enabled: false, ..tabs.clone() };
    let d = doc_with(vec![outer.clone()], with_options(base.clone(), CutOptions { tabs: Some(off), ..CutOptions::default() }));
    assert!((marked(&d) - 56.0).abs() < 1e-6);
    // Hand-placed tabs (moved with the object): a shape with them gets only those, on the outline nearest each.
    let mut moved = outer.clone();
    moved.transform = Affine::translate((20.0, 0.0));
    let mut d = doc_with(vec![moved], with_options(base.clone(), CutOptions { tabs: Some(tabs.clone()), ..CutOptions::default() }));
    d.object_options.insert("obj-1".into(), ObjectOptions { tabs: vec![[0.0, -5.2], [2.1, 0.0]], ..ObjectOptions::default() });
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 54.0).abs() < 1e-6, "{}", plan.stats.mark_length_mm);
    let ends: Vec<[f64; 2]> = plan.passes[0].polylines.iter().flat_map(|p| [p[0], p[p.len() - 1]]).collect();
    assert!(ends.iter().any(|e| (e[0] - 19.5).abs() < 1e-9 && (e[1] + 5.0).abs() < 1e-9), "a gap centred under the bottom edge's middle: {ends:?}");
    // Manual generation: shapes without hand-placed tabs are cut through.
    let manual = Tabs { generation: TabGeneration::Manual, ..tabs.clone() };
    let mut d = doc_with(vec![outer.clone(), rect("obj-2", 10.0, 10.0, (30.0, 0.0))], with_options(base.clone(), CutOptions { tabs: Some(manual), ..CutOptions::default() }));
    d.object_options.insert("obj-1".into(), ObjectOptions { tabs: vec![[5.0, 0.0]], ..ObjectOptions::default() });
    assert!((marked(&d) - 95.0).abs() < 1e-6, "{}", marked(&d));
    // Tab Cut Power: the tabs are marked at that share of the power.
    let powered = Tabs { cut_power_pct: 25.0, ..tabs };
    let d = doc_with(vec![rect("obj-2", 10.0, 10.0, (0.0, 0.0))], with_options(base, CutOptions { tabs: Some(powered), ..CutOptions::default() }));
    let plan = plan_job(&d, &target()).unwrap();
    let pass = &plan.passes[0];
    assert!((plan.stats.mark_length_mm - 40.0).abs() < 1e-6);
    let tab_len: f64 = (0..pass.polylines.len()).filter(|&i| pass.power_of(i) == 10.0).map(|i| crate::geom::polyline_length(&pass.polylines[i])).sum();
    assert!((tab_len - 4.0).abs() < 1e-6, "4 tabs of 1 mm at 25 % of 40 %: {tab_len}");
}

#[test]
fn hostile_tab_settings_neither_hang_nor_cut_where_unintended() {
    // A spacing of a micrometre with smaller tabs would be 40 million gaps on a 10 mm square: refused at once.
    let tiny = Tabs { count: 0, spacing_mm: 1e-6, size_mm: 1e-7, ..Tabs::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), CutOptions { tabs: Some(tiny), ..CutOptions::default() }));
    assert!(matches!(plan_job(&d, &target()), Err(PlanError::TooManySegments { .. })));
    // A hand-edited document: tabs that are not numbers are ignored; the others still leave their gaps, and only
    // there (the bottom edge's middle, at x = 0 of the centred local frame).
    let manual = Tabs { generation: TabGeneration::Manual, size_mm: 1.0, ..Tabs::default() };
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))], with_options(CutSettings::default(), CutOptions { tabs: Some(manual), ..CutOptions::default() }));
    d.object_options.insert("obj-1".into(), ObjectOptions { tabs: vec![[f64::NAN, 0.0], [0.0, f64::INFINITY], [0.0, -5.0]], ..ObjectOptions::default() });
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 39.0).abs() < 1e-6, "{}", plan.stats.mark_length_mm);
    let pts: Vec<[f64; 2]> = plan.passes[0].polylines.iter().flatten().copied().collect();
    assert!(!pts.iter().any(|p| p[1] < -4.999 && p[0].abs() < 0.499), "nothing is cut inside the tab: {pts:?}");
    assert!(pts.iter().any(|p| (p[0] - 0.5).abs() < 1e-9 && (p[1] + 5.0).abs() < 1e-9));
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

#[test]
fn gantry_layer_options_reach_the_passes_and_never_the_galvo_list() {
    let o = CutOptions { constant_power: true, air_assist: true, overscan_mm: 2.0, overcut_mm: 1.5, ..CutOptions::default() };
    let objects = || vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))];
    let line = plan_job(&doc_with(objects(), with_options(CutSettings::default(), o.clone())), &target()).unwrap();
    // Overscan is for scan lines only: a line layer has none.
    assert_eq!(line.passes[0].gantry, GantryPass { constant_power: true, air_assist: true, overscan_mm: 0.0, overcut_mm: 1.5, ..GantryPass::default() });
    let filled = plan_job(&doc_with(objects(), with_options(fill(1.0), o.clone())), &target()).unwrap();
    assert_eq!(filled.passes[0].gantry.overscan_mm, 2.0);
    // The galvo's list is the same with or without them.
    let plain = plan_job(&doc_with(objects(), fill(1.0)), &target()).unwrap();
    assert_eq!(encode(&filled, &target()).unwrap(), encode(&plain, &target()).unwrap());
    assert!(plain.passes[0].gantry.is_default());
    assert!(serde_json::to_value(&plain.passes[0]).unwrap().get("gantry").is_none(), "left out of JSON while off");
}

#[test]
fn a_gantry_fill_ramps_its_power_near_the_inner_edges_only() {
    let gantry = Target { gantry: true, ..target() };
    // A 20 mm square with a 4 mm hole, filled at 50 %; ramp 1 mm from 10 % at the hole's edge.
    let objects = || vec![rect("obj-1", 20.0, 20.0, (0.0, 0.0)), rect("obj-2", 4.0, 4.0, (0.0, 0.0))];
    let mut settings = fill(0.5);
    settings.power_pct = 50.0;
    let o = CutOptions { ramp_length_mm: 1.0, ramp_min_power_pct: 10.0, ..CutOptions::default() };
    let ramped = plan_job(&doc_with(objects(), with_options(settings.clone(), o.clone())), &gantry).unwrap();
    let plain = plan_job(&doc_with(objects(), settings.clone()), &gantry).unwrap();
    let p = &ramped.passes[0];
    assert!(p.polylines.len() > plain.passes[0].polylines.len(), "hatch lines split into ramp pieces");
    let powers: Vec<f64> = (0..p.polylines.len()).map(|i| p.power_of(i)).collect();
    assert!(powers.iter().all(|w| (10.0 - 1e-9..=50.0 + 1e-9).contains(w)), "{powers:?}");
    assert!(powers.iter().any(|w| (*w - 10.0).abs() < 1e-9) && powers.contains(&50.0), "{powers:?}");
    // The ramp is only near the hole: a hatch line far from it (near the outer edge) keeps the full power.
    for (i, q) in p.polylines.iter().enumerate() {
        if q.iter().all(|pt| pt[1].abs() > 4.0) {
            assert_eq!(p.power_of(i), 50.0, "{q:?}");
        }
    }
    // Ramp Outer Edge ramps the outer square's sides too.
    let outer = plan_job(&doc_with(objects(), with_options(settings.clone(), CutOptions { ramp_outer_edge: true, ..o.clone() })), &gantry).unwrap();
    let q = &outer.passes[0];
    assert!(q.polylines.len() > p.polylines.len());
    // A galvo job ignores the ramp: the same list as without it.
    let galvo = plan_job(&doc_with(objects(), with_options(settings.clone(), o)), &target()).unwrap();
    assert_eq!(encode(&galvo, &target()).unwrap(), encode(&plan_job(&doc_with(objects(), settings), &target()).unwrap(), &target()).unwrap());
}

#[test]
fn leads_and_path_pauses_reach_line_passes_only() {
    let lead = doc::Lead { style: doc::LeadStyle::Arc, length_mm: 2.0, angle_deg: 45.0 };
    let o = CutOptions { lead_in: Some(lead), lead_out: Some(lead), start_pause_ms: 5.0, end_pause_ms: 2.0, ..CutOptions::default() };
    let objects = || vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))];
    let line = plan_job(&doc_with(objects(), with_options(CutSettings::default(), o.clone())), &target()).unwrap();
    let g = line.passes[0].gantry;
    assert_eq!((g.lead_in, g.lead_out, g.start_pause_us, g.end_pause_us), (Some(lead), Some(lead), 5000.0, 2000.0));
    let filled = plan_job(&doc_with(objects(), with_options(fill(1.0), o.clone())), &target()).unwrap();
    assert!(filled.passes[0].gantry.is_default(), "fills get no leads");
    // A lead longer than 50 mm or steeper than 90 degrees is refused.
    let bad = CutOptions { lead_in: Some(doc::Lead { angle_deg: 91.0, ..lead }), ..CutOptions::default() };
    assert!(plan_job(&doc_with(objects(), with_options(CutSettings::default(), bad)), &target()).is_err());
    let bad = CutOptions { lead_out: Some(doc::Lead { length_mm: 0.0, ..lead }), ..CutOptions::default() };
    assert!(plan_job(&doc_with(objects(), with_options(CutSettings::default(), bad)), &target()).is_err());
}

#[test]
fn grbl_leads_and_path_pauses_refuse_tabs_and_perforation() {
    // Tabs and perforation cut each path into pieces: a lead at every piece would cut across the bridges and skips.
    let gantry = Target { gantry: true, ..target() };
    let lead = Some(doc::Lead { style: doc::LeadStyle::Line, length_mm: 2.0, angle_deg: 90.0 });
    let objects = || vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))];
    let tabs = Some(Tabs { count: 4, spacing_mm: 0.0, size_mm: 1.0, ..Tabs::default() });
    let perf = Some(doc::Perforation { cut_mm: 1.0, skip_mm: 1.0 });
    for o in [
        CutOptions { lead_in: lead, tabs: tabs.clone(), ..CutOptions::default() },
        CutOptions { lead_out: lead, perforation: perf.clone(), ..CutOptions::default() },
        CutOptions { start_pause_ms: 5.0, tabs: tabs.clone(), ..CutOptions::default() },
    ] {
        let d = doc_with(objects(), with_options(CutSettings::default(), o));
        assert!(matches!(plan_job(&d, &gantry), Err(PlanError::Target(m)) if m.contains("tabs or perforation")));
        // A galvo ignores the GRBL-only settings.
        plan_job(&d, &target()).unwrap();
    }
    plan_job(&doc_with(objects(), with_options(CutSettings::default(), CutOptions { tabs: tabs.clone(), ..CutOptions::default() })), &gantry).unwrap();
    // Hand-placed tabs (manual generation) split the outline as well: refused with a lead.
    let manual = Some(Tabs { generation: TabGeneration::Manual, size_mm: 1.0, ..Tabs::default() });
    let mut d = doc_with(objects(), with_options(CutSettings::default(), CutOptions { lead_in: lead, tabs: manual, ..CutOptions::default() }));
    d.object_options.insert("obj-1".into(), ObjectOptions { tabs: vec![[0.0, -5.0]], ..ObjectOptions::default() });
    assert!(matches!(plan_job(&d, &gantry), Err(PlanError::Target(m)) if m.contains("tabs or perforation")));
    // Tab settings kept with the Tabs / Bridges switch off cut no tabs: the lead is allowed and the outline is whole.
    let off = Some(Tabs { enabled: false, count: 4, size_mm: 1.0, ..Tabs::default() });
    let d = doc_with(objects(), with_options(CutSettings::default(), CutOptions { lead_in: lead, tabs: off, ..CutOptions::default() }));
    let plan = plan_job(&d, &gantry).unwrap();
    assert_eq!((plan.passes[0].polylines.len(), plan.passes[0].gantry.lead_in), (1, lead));
}

#[test]
fn grbl_wobble_refuses_leads_and_overcut() {
    // Wobble makes each outline a run of small circles: a lead or an overcut would follow a circle, not the outline.
    let gantry = Target { gantry: true, ..target() };
    let lead = Some(doc::Lead { style: doc::LeadStyle::Line, length_mm: 2.0, angle_deg: 90.0 });
    let wobble = || Some(doc::Wobble { step_mm: 0.5, size_mm: 0.4 });
    let objects = || vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0))];
    for o in [
        CutOptions { wobble: wobble(), lead_in: lead, ..CutOptions::default() },
        CutOptions { wobble: wobble(), lead_out: lead, ..CutOptions::default() },
        CutOptions { wobble: wobble(), overcut_mm: 2.0, ..CutOptions::default() },
    ] {
        let d = doc_with(objects(), with_options(CutSettings::default(), o.clone()));
        assert!(matches!(plan_job(&d, &gantry), Err(PlanError::Target(m)) if m.contains("wobble")), "{o:?}");
        // A galvo ignores the GRBL-only leads and overcut and wobbles as before.
        plan_job(&d, &target()).unwrap();
    }
    // Wobble with path pauses only is fine (one pause per wobbled outline), and so is a fill layer with a stray wobble.
    let d = doc_with(objects(), with_options(CutSettings::default(), CutOptions { wobble: wobble(), start_pause_ms: 5.0, ..CutOptions::default() }));
    plan_job(&d, &gantry).unwrap();
    let d = doc_with(objects(), with_options(fill(1.0), CutOptions { wobble: wobble(), overcut_mm: 2.0, ..CutOptions::default() }));
    plan_job(&d, &gantry).unwrap();
}

#[test]
fn start_here_part_way_along_a_grbl_path_resumes_on_the_planned_leads() {
    // A gantry plan has its leads in its paths (`GantryPass::baked`), so the preview numbers them and Start Here
    // resumes on the very path the head would have cut: part-way along an outline, the rest of it with its lead-out,
    // and no new lead measured from the resume point (that one would land on the wrong side, into the part).
    let gantry = Target { gantry: true, ..target() };
    let lead = Some(doc::Lead { style: doc::LeadStyle::Line, length_mm: 2.0, angle_deg: 90.0 });
    let o = CutOptions { lead_in: lead, lead_out: lead, ..CutOptions::default() };
    let d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (30.0, 0.0))], with_options(CutSettings::default(), o));
    let plan = plan_job(&d, &gantry).unwrap();
    assert!(plan.passes[0].gantry.baked);
    assert_eq!(plan.passes[0].polylines.iter().map(Vec::len).collect::<Vec<_>>(), [7, 7], "lead-in, 4 sides, lead-out");
    assert!((plan.stats.mark_length_mm - 88.0).abs() < 1e-9, "{}", plan.stats.mark_length_mm);
    // Segment 3: the second side of the first outline (0 the jump, 1 the lead-in).
    let mid = start_at(&plan, 3, &gantry).unwrap();
    assert_eq!((mid.passes.len(), mid.passes[0].polylines.len()), (1, 2));
    let rest = &mid.passes[0].polylines[0];
    assert_eq!((rest[0], *rest.last().unwrap()), ([5.0, -5.0], [-7.0, -5.0]), "on the outline, to its lead-out: {rest:?}");
    assert!((mid.stats.mark_length_mm - 76.0).abs() < 1e-9, "{}", mid.stats.mark_length_mm);
    // From the second path's jump: that path whole, lead-in first.
    let from_second = start_at(&plan, 7, &gantry).unwrap();
    assert_eq!(from_second.passes[0].polylines.len(), 1);
    assert_eq!(from_second.passes[0].polylines[0][0], [25.0, -7.0]);
}

#[test]
fn tabs_survive_remove_overlapping_lines() {
    // Two 10 mm squares sharing an edge: the second loses that edge to the first and is cut as an open line, but
    // keeps its tabs. Without overlaps removed: 80 mm, 8 tabs of 1 mm.
    let o = CutOptions { tabs: Some(Tabs { count: 4, size_mm: 1.0, ..Tabs::default() }), ..CutOptions::default() };
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (10.0, 0.0))], with_options(CutSettings::default(), o));
    assert!((plan_job(&d, &target()).unwrap().stats.mark_length_mm - 72.0).abs() < 1e-6);
    // Overlaps removed: 70 mm of line, less the first square's 4 tabs and the 3 of the second's not on the shared
    // edge (the tabs sit mid-edge, so one of the second square's is on the line the first one cuts).
    d.optimization.remove_overlaps = true;
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 63.0).abs() < 1e-6, "{}", plan.stats.mark_length_mm);
    // A tab placed by hand on the second square's far edge is honoured too.
    let manual = CutOptions { tabs: Some(Tabs { generation: TabGeneration::Manual, size_mm: 2.0, ..Tabs::default() }), ..CutOptions::default() };
    let mut d = doc_with(vec![rect("obj-1", 10.0, 10.0, (0.0, 0.0)), rect("obj-2", 10.0, 10.0, (10.0, 0.0))], with_options(CutSettings::default(), manual));
    d.optimization.remove_overlaps = true;
    d.object_options.insert("obj-2".into(), ObjectOptions { tabs: vec![[5.0, 0.0]], ..ObjectOptions::default() });
    let plan = plan_job(&d, &target()).unwrap();
    assert!((plan.stats.mark_length_mm - 68.0).abs() < 1e-6, "{}", plan.stats.mark_length_mm);
    let pts: Vec<[f64; 2]> = plan.passes[0].polylines.iter().flatten().copied().collect();
    assert!(!pts.iter().any(|p| (p[0] - 15.0).abs() < 0.999 && p[1].abs() < 0.999), "the gap at x = 15 is uncut: {pts:?}");
}
