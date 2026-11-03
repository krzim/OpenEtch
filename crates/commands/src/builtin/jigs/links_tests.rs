//! The links between jigs and the production and text features: jig pockets feed the serial slots (`slots`), and the
//! pre-flight check sees the run's document and the contact sheet's checks of the next row.

use doc::Document;
use serde_json::{Value, json};

use crate::builtin::slots::{apply_slot_plan, slot_plan};
use crate::{Invocation, Origin, Session};

fn exec(s: &mut Session, id: &str, params: Value) -> Value {
    match s.execute(Invocation { id: id.into(), params: params.clone() }, Origin::Ui) {
        Ok(o) => o.result,
        Err(e) => panic!("{id} {params}: {e}"),
    }
}

fn created(r: &Value) -> String {
    r["created"][0].as_str().unwrap().to_string()
}

/// A 2 x 3 coin jig (pockets 1-3 at y = +15, 4-6 at y = -15, 30 mm apart, 25 mm discs) with a serial text and a
/// disc design placed in every pocket. Returns the session, the jig id, the text id and the disc id.
fn placed() -> (Session, String, String, String) {
    let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
    let j = exec(&mut s, "jig.create", json!({"rows": 2, "cols": 3, "pitch_x_mm": 30, "pitch_y_mm": 30, "name": "Coins",
        "pocket": {"kind": "circle", "diameter_mm": 25}, "center": [0, 0]}))["jig"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    let text = created(&exec(&mut s, "shape.text", json!({ "text": "'SN '0ddd", "font": font, "height_mm": 3, "variable": { "mode": "serial" }, "center": [0, 0] })));
    let disc = created(&exec(&mut s, "shape.ellipse", json!({ "rx_mm": 10, "ry_mm": 10, "center": [0, 0] })));
    exec(&mut s, "jig.place", json!({"jig": j, "ids": [text, disc]}));
    exec(&mut s, "var.set_serial", json!({ "start": 1, "end": 100, "current": 1, "advance_by": 6 }));
    (s, j, text, disc)
}

#[test]
fn jig_pockets_are_the_slots_and_hold_their_whole_design() {
    let (mut s, j, text, disc) = placed();
    let d = s.document().clone();
    let places = doc::jig::object_pockets(&d);
    assert_eq!(places[&text].number, 1, "the originals sit in the source pocket");
    assert_eq!(places[&disc].pocket, "p1");
    let plan = slot_plan(&d);
    assert_eq!(plan.slots.len(), 6);
    for (k, slot) in plan.slots.iter().enumerate() {
        let p = slot.pocket.as_ref().expect("every slot sits in a pocket");
        assert_eq!((p.jig.as_str(), p.number), (j.as_str(), k as u32 + 1));
        assert_eq!(slot.objects.len(), 2, "the text and the disc of pocket {}", k + 1);
        assert_eq!(slot.label, format!("Coins P{}", k + 1));
    }
    // Pocket 2 marked empty: its disc goes with its number, the later pockets count on.
    exec(&mut s, "var.set_serial", json!({ "empty_slots": [1] }));
    let d = s.document().clone();
    let plan = slot_plan(&d);
    let p2: Vec<String> = plan.slots[1].objects.clone();
    let out = apply_slot_plan(&d, &plan);
    assert!(p2.iter().all(|id| out.object(id).is_none()), "nothing of pocket 2 is planned");
    assert_eq!(out.objects.len(), 10);
    assert_eq!(plan.slots[2].values[0].shown, "SN 002", "pocket 3 takes the next value");
    assert_eq!(plan.slots[2].pocket.as_ref().unwrap().number, 3);
}

#[test]
fn hand_laid_designs_find_their_pocket_and_the_plate_does_not() {
    let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
    exec(&mut s, "jig.create", json!({"rows": 1, "cols": 2, "pitch_x_mm": 40, "pocket": {"kind": "circle", "diameter_mm": 30}, "center": [0, 0]}));
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    let a = created(&exec(&mut s, "shape.text", json!({ "text": "0ddd", "font": font, "height_mm": 3, "variable": { "mode": "serial" }, "center": [-20, 0] })));
    let b = created(&exec(&mut s, "shape.text", json!({ "text": "0ddd", "font": font, "height_mm": 3, "variable": { "mode": "serial", "offset": 1 }, "center": [20, 0] })));
    let logo = created(&exec(&mut s, "shape.rect", json!({ "width_mm": 8, "height_mm": 4, "center": [20, 6] })));
    let plate = created(&exec(&mut s, "shape.rect", json!({ "width_mm": 90, "height_mm": 40, "center": [0, 0] })));
    let d = s.document().clone();
    let places = doc::jig::object_pockets(&d);
    assert_eq!(places[&a].number, 1);
    assert_eq!(places[&b].number, 2);
    assert_eq!(places[&logo].number, 2);
    assert!(!places.contains_key(&plate), "the plate spans both pockets");
    let plan = slot_plan(&d);
    assert_eq!(plan.slots[1].objects, vec![b.clone(), logo.clone()], "the logo goes with pocket 2's number");
    assert!(!plan.slots[0].objects.contains(&plate));
}

fn checks(r: &Value) -> Vec<(String, String)> {
    r["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| (w["check"].as_str().unwrap().to_string(), w["params"]["kind"].as_str().unwrap_or("").to_string()))
        .collect()
}

#[test]
fn preflight_runs_the_contact_sheet_on_the_next_row_and_knows_the_slots() {
    let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
    let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
    // A tag outline with a serial number too long for it.
    exec(&mut s, "shape.rect", json!({ "width_mm": 12, "height_mm": 6, "center": [0, 0], "name": "Tag" }));
    let t = created(&exec(&mut s, "shape.text", json!({ "text": "'SERIAL '0ddddd", "font": font, "height_mm": 4, "variable": { "mode": "serial" }, "center": [0, 0], "name": "SN" })));
    exec(&mut s, "var.set_serial", json!({ "start": 1, "end": 100, "current": 5 }));
    let r = exec(&mut s, "job.preflight", json!({}));
    let w = r["warnings"].as_array().unwrap().iter().find(|w| w["check"] == "contact_sheet").expect("the row check reports it");
    assert_eq!(w["params"]["kind"], "overflow", "{w}");
    assert_eq!(w["params"]["value"], 5);
    assert_eq!(w["objects"], json!([t]));
    assert!(w["text"].as_str().unwrap().contains("\"SN\" at 5"), "{w}");
    // Each problem as a code with its values, for the app to word in its language.
    assert_eq!(w["params"]["problems"][0], json!({ "kind": "overflow", "code": "outside_object", "params": { "name": "Tag" } }), "{w}");
    assert!(r["checks"].as_array().unwrap().contains(&json!("contact_sheet")));
    // Switched off for the project like the other warnings.
    exec(&mut s, "job.preflight_set", json!({ "check": "contact_sheet", "enabled": false }));
    let r = exec(&mut s, "job.preflight", json!({}));
    assert!(!checks(&r).iter().any(|(c, _)| c == "contact_sheet"), "{r}");
}

#[test]
fn preflight_sees_empty_pockets_and_stop_at_end() {
    let (mut s, _, _, _) = placed();
    exec(&mut s, "var.set_serial", json!({ "current": 98, "end": 100, "stop_at_end": true }));
    let r = exec(&mut s, "job.preflight", json!({}));
    let found = checks(&r);
    assert!(found.contains(&("unfilled_variable".into(), "stop_at_end".into())), "{r}");
    assert!(!found.iter().any(|(_, k)| k == "serial_wrap"), "Stop at End leaves them out, nothing wraps: {r}");
    let w = r["warnings"].as_array().unwrap().iter().find(|w| w["params"]["kind"] == "stop_at_end").unwrap();
    assert_eq!(w["params"]["count"], 3, "pockets 4-6 would get 101-103");
    assert!(w["params"]["names"].as_str().unwrap().contains("Coins P4"), "{w}");
    // Without Stop at End they would wrap.
    exec(&mut s, "var.set_serial", json!({ "stop_at_end": false }));
    let r = exec(&mut s, "job.preflight", json!({}));
    assert!(checks(&r).contains(&("unfilled_variable".into(), "serial_wrap".into())), "{r}");
    // The texts themselves fit their pockets.
    assert!(!checks(&r).iter().any(|(c, _)| c == "contact_sheet"), "{r}");
}
