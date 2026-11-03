//! Production slots (`var.slots`, `slot_plan`, `apply_slot_plan`): a jig of pockets, each a disc outline grouped
//! with its serial-number text, and one barcode slot.

use doc::{Document, Geometry};
use serde_json::{Value, json};

use super::{apply_slot_plan, slot_plan};
use crate::{Invocation, Origin, Session};

fn session() -> Session {
    Session::new(Document { field_mm: Some(150.0), ..Document::default() })
}

fn exec(s: &mut Session, id: &str, params: Value) -> Value {
    match s.execute(Invocation { id: id.to_string(), params: params.clone() }, Origin::Ui) {
        Ok(o) => o.result,
        Err(e) => panic!("{id} {params}: {e}"),
    }
}

/// `n` pockets: a disc and a serial text `SN 0ddd` with offset k, grouped as "Pocket k+1".
fn jig(s: &mut Session, n: i64) -> Vec<String> {
    let mut texts = Vec::new();
    for k in 0..n {
        let x = -50.0 + 20.0 * k as f64;
        let disc = exec(s, "shape.ellipse", json!({ "center": [x, 0], "rx_mm": 7.5, "ry_mm": 7.5 }))["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        let r = exec(
            s,
            "shape.text",
            json!({ "text": "'SN '0ddd", "font": { "family": "Hershey Sans", "kind": "stroke" }, "height_mm": 3,
                    "center": [x, 0], "variable": { "mode": "serial", "offset": k } }),
        );
        let text = r["created"][0].as_str().unwrap().to_owned();
        exec(s, "group.create", json!({ "ids": [disc, text], "name": format!("Pocket {}", k + 1) }));
        texts.push(text);
    }
    exec(s, "var.set_serial", json!({ "start": 1, "end": 100, "current": 7, "advance_by": n, "auto_advance": true }));
    texts
}

fn shown(doc: &Document, id: &str) -> String {
    match &doc.object(id).unwrap().geometry {
        Geometry::Text(t) => t.shown.clone().unwrap_or_else(|| t.text.clone()),
        _ => panic!("{id} is not a text"),
    }
}

#[test]
fn every_pocket_is_a_slot_with_its_value() {
    let mut s = session();
    jig(&mut s, 4);
    let plan = slot_plan(s.document());
    assert_eq!(plan.slots.len(), 4);
    let labels: Vec<&str> = plan.slots.iter().map(|x| x.label.as_str()).collect();
    assert_eq!(labels, ["Pocket 1", "Pocket 2", "Pocket 3", "Pocket 4"]);
    assert!(plan.slots.iter().all(|x| x.objects.len() == 2 && x.marks()));
    assert_eq!(plan.slots[2].values[0].shown, "SN 009");
    assert_eq!((plan.advance, plan.next_current(), plan.refusal()), (4, 11, None));
    // Nothing skipped: the run plans the document as it is.
    assert_eq!(apply_slot_plan(s.document(), &plan), *s.document());
    let r = exec(&mut s, "var.slots", json!({}));
    assert_eq!(r["slots"][3]["values"][0]["shown"], "SN 010");
    assert_eq!(r["refusal"], Value::Null);
}

#[test]
fn empty_slots_drop_their_pockets_and_the_rest_take_consecutive_values() {
    let mut s = session();
    let texts = jig(&mut s, 4);
    let r = exec(&mut s, "var.set_serial", json!({ "empty_slots": [1, 1] }));
    assert_eq!(r["variables"]["empty_slots"], json!([1]));
    let plan = slot_plan(s.document());
    assert!(plan.slots[1].empty && !plan.slots[1].marks());
    let values: Vec<i64> = plan.marked().map(|x| x.value).collect();
    assert_eq!(values, [7, 8, 9], "pockets 1, 3 and 4 get 7, 8 and 9");
    assert_eq!((plan.advance, plan.next_current()), (3, 10));
    let out = commands_prepare(&apply_slot_plan(s.document(), &plan));
    assert_eq!(out.objects.len(), 6, "pocket 2's disc and text are left out");
    assert!(out.object(&texts[1]).is_none());
    assert_eq!(shown(&out, &texts[2]), "SN 008");
    assert_eq!(shown(&out, &texts[3]), "SN 009");
    assert_eq!(out.groups.len(), 3, "the emptied group is gone");
    // The document itself is untouched.
    assert_eq!(shown(s.document(), &texts[2]), "SN 009");
}

#[test]
fn stop_at_end_marks_up_to_end_then_refuses() {
    let mut s = session();
    jig(&mut s, 4);
    exec(&mut s, "var.set_serial", json!({ "end": 9, "stop_at_end": true }));
    let plan = slot_plan(s.document());
    let marked: Vec<usize> = plan.marked().map(|x| x.number).collect();
    assert_eq!(marked, [1, 2, 3], "slot 4 would be 10, past End 9");
    assert!(plan.slots[3].past_end);
    assert_eq!((plan.advance, plan.next_current()), (3, 10));
    assert!(plan.refusal().is_none());
    // The engine's after-run step: past End the counter stays one past it, and every run is refused.
    exec(&mut s, "var.set_serial", json!({ "current": plan.next_current() }));
    let used = slot_plan(s.document());
    assert!(used.exhausted);
    assert!(used.refusal().unwrap().contains("up to End (9)"), "{:?}", used.refusal());
    // Raising End lets the next run mark again.
    exec(&mut s, "var.set_serial", json!({ "end": 20 }));
    assert!(slot_plan(s.document()).refusal().is_none());
    // Without Stop at End nothing is refused and nothing is skipped.
    exec(&mut s, "var.set_serial", json!({ "end": 9, "stop_at_end": false }));
    let wrap = slot_plan(s.document());
    assert!(wrap.refusal().is_none() && !wrap.skips());
}

#[test]
fn a_barcode_slot_takes_its_compacted_offset_too() {
    let mut s = session();
    for k in 0..3 {
        exec(
            &mut s,
            "shape.barcode",
            json!({ "kind": "qr", "data": "0ddd", "module_mm": 0.5, "center": [-40 + 30 * k, 0],
                    "variable": { "mode": "serial", "offset": k } }),
        );
    }
    exec(&mut s, "var.set_serial", json!({ "current": 40, "advance_by": 3, "empty_slots": [0] }));
    let plan = slot_plan(s.document());
    assert_eq!(plan.slots.iter().map(|x| x.values[0].shown.as_str()).collect::<Vec<_>>(), ["040", "040", "041"]);
    let out = apply_slot_plan(s.document(), &plan);
    assert_eq!(out.objects.len(), 2);
    let offsets: Vec<i64> = out
        .objects
        .iter()
        .map(|o| crate::builtin::text::barcode_params(o).unwrap().variable.unwrap().offset)
        .collect();
    assert_eq!(offsets, [0, 1]);
}

/// The run's rendering step (`prepare_output`).
fn commands_prepare(d: &Document) -> Document {
    crate::prepare_output(d).unwrap()
}
