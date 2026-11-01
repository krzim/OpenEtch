//! Planning with Offset Fill, kerf offset and user start points.

use doc::{CutOptions, CutSettings, Document, FillMode, Geometry, Object, ObjectOptions, StartPoint};
use kurbo::Affine;

use crate::target::tests::target;
use crate::*;

fn square(id: &str, side: f64, at: (f64, f64)) -> Object {
    Object::new(id.into(), id, "layer-1".into(), Geometry::Rect { width_mm: side, height_mm: side, corner_radius_mm: 0.0 }, Affine::translate(at))
}

fn doc_with(objects: Vec<Object>, settings: CutSettings) -> Document {
    let mut doc = Document::default();
    doc.layers[0].settings = settings;
    doc.objects = objects;
    doc
}

fn extent(poly: &[Point]) -> f64 {
    let xs = poly.iter().map(|p| p[0]);
    xs.clone().fold(f64::NEG_INFINITY, f64::max) - xs.fold(f64::INFINITY, f64::min)
}

#[test]
fn offset_fill_marks_concentric_closed_rings() {
    let mut s = CutSettings { mode: FillMode::OffsetFill, ..CutSettings::default() };
    s.hatch.interval_mm = 1.0;
    let plan = plan_job(&doc_with(vec![square("obj-1", 10.0, (0.0, 0.0))], s), &target()).unwrap();
    let polys = &plan.passes[0].polylines;
    assert_eq!(polys.len(), 5, "10, 8, 6, 4 and 2 mm squares");
    let mut sizes: Vec<f64> = polys.iter().map(|p| extent(p)).collect();
    sizes.sort_by(f64::total_cmp);
    for (got, want) in sizes.iter().zip([2.0, 4.0, 6.0, 8.0, 10.0]) {
        assert!((got - want).abs() < 1e-6, "{sizes:?}");
    }
    for p in polys {
        assert_eq!(p.first(), p.last(), "every ring is closed");
    }
}

#[test]
fn offset_fill_respects_the_segment_budget() {
    let mut s = CutSettings { mode: FillMode::OffsetFill, ..CutSettings::default() };
    s.hatch.interval_mm = 0.001;
    let d = doc_with(vec![square("obj-1", 100.0, (0.0, 0.0))], s);
    assert!(matches!(crate::job::plan_job_within(&d, &target(), 10_000, &JobOptions::default()), Err(PlanError::TooManySegments { .. })));
}

#[test]
fn kerf_offset_grows_closed_outlines_only() {
    let s = CutSettings { options: CutOptions { kerf_offset_mm: 0.5, ..CutOptions::default() }, ..CutSettings::default() };
    let mut path = kurbo::BezPath::new();
    path.move_to((20.0, 0.0));
    path.line_to((30.0, 0.0));
    let line = Object::new("obj-2".into(), "line", "layer-1".into(), Geometry::Path { path }, Affine::IDENTITY);
    let plan = plan_job(&doc_with(vec![square("obj-1", 10.0, (0.0, 0.0)), line], s), &target()).unwrap();
    let polys = &plan.passes[0].polylines;
    let widths: Vec<f64> = polys.iter().map(|p| extent(p)).collect();
    assert!(widths.iter().any(|w| (w - 11.0).abs() < 1e-6), "the square grew by 0.5 on each side: {widths:?}");
    assert!(widths.iter().any(|w| (w - 10.0).abs() < 1e-6), "the open line is not offset: {widths:?}");
    let inward = CutSettings { options: CutOptions { kerf_offset_mm: -1.0, ..CutOptions::default() }, ..CutSettings::default() };
    let plan = plan_job(&doc_with(vec![square("obj-1", 10.0, (0.0, 0.0))], inward), &target()).unwrap();
    assert!((extent(&plan.passes[0].polylines[0]) - 8.0).abs() < 1e-6);
}

#[test]
fn start_point_is_used_when_best_start_is_off() {
    let mut d = doc_with(vec![square("obj-1", 10.0, (0.0, 0.0))], CutSettings::default());
    // The rectangle's local corner (5, 5) is the field point (5, 5).
    d.object_options.insert(
        "obj-1".into(),
        ObjectOptions { start: Some(StartPoint { node: 2, local: [5.0, 5.0], reverse: false }), ..ObjectOptions::default() },
    );
    d.optimization.best_start = false;
    let plan = plan_job(&d, &target()).unwrap();
    let p = &plan.passes[0].polylines[0];
    assert_eq!(p[0], [5.0, 5.0]);
    assert_eq!(p.last(), Some(&[5.0, 5.0]));
    // Reversed: the same start, the other way round.
    d.object_options.get_mut("obj-1").unwrap().start.as_mut().unwrap().reverse = true;
    let r = plan_job(&d, &target()).unwrap();
    let q = &r.passes[0].polylines[0];
    assert_eq!(q[0], [5.0, 5.0]);
    assert_eq!(q[1], p[p.len() - 2], "reversed direction");
    // With "choose best starting point" on (LightBurn: it may override the start point) the optimizer decides.
    d.optimization.best_start = true;
    let o = plan_job(&d, &target()).unwrap();
    assert_eq!(o.passes[0].polylines[0][0], [-5.0, -5.0], "nearest corner to the field centre's start position");
}

#[test]
fn status_lights_ride_along_with_every_port_write() {
    let d = doc_with(vec![square("obj-1", 10.0, (0.0, 0.0))], CutSettings::default());
    let t = Target { port_outputs: 0x18 | 0x01, ..target() };
    let plan = plan_job(&d, &t).unwrap();
    let ports: Vec<u16> = lmc::decode(&encode(&plan, &t).unwrap())
        .into_iter()
        .filter_map(|c| if let lmc::ListCommand::WritePort { bits } = c { Some(bits) } else { None })
        .collect();
    assert_eq!(ports, [0x19, 0x18], "laser enable + lights, then lights only (the enable bit is never held)");
}
