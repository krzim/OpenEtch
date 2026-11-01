//! Text, variable text and barcode commands. They use the bundled Hershey fonts (always present); the system-font
//! checks run only where a common font is installed.

use doc::{Document, Geometry};
use serde_json::{Value, json};

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

fn fails(s: &mut Session, id: &str, params: Value) -> String {
    match s.execute(Invocation { id: id.to_string(), params: params.clone() }, Origin::Ui) {
        Ok(o) => panic!("{id} {params} should fail, got {}", o.result),
        Err(e) => e.to_string(),
    }
}

fn hershey() -> Value {
    json!({ "family": "Hershey Sans", "kind": "stroke" })
}

fn text_of<'a>(s: &'a Session, id: &str) -> &'a doc::Text {
    match &s.document().object(id).unwrap().geometry {
        Geometry::Text(t) => t,
        g => panic!("{id} is {}", g.kind_name()),
    }
}

fn width(r: &Value) -> f64 {
    r["bounds"]["size"][0].as_f64().unwrap()
}

#[test]
fn text_is_added_edited_and_converted() {
    let mut s = session();
    let r = exec(&mut s, "shape.text", json!({ "text": "Hello", "font": hershey(), "height_mm": 10, "center": [5, 5] }));
    let id = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(s.document().object(&id).unwrap().name, "Hello");
    let w0 = width(&r);
    assert!(w0 > 20.0 && w0 < 60.0, "{r}");
    assert!(!text_of(&s, &id).outline.elements().is_empty());
    // Partial update: only the height changes.
    let r = exec(&mut s, "text.update", json!({ "id": id, "height_mm": 20 }));
    assert!((width(&r) - 2.0 * w0).abs() < 1e-6, "{r}");
    assert_eq!(text_of(&s, &id).text, "Hello");
    exec(&mut s, "text.update", json!({ "id": id, "text": "Hi\nthere", "align_h": "left", "upper": true }));
    let t = text_of(&s, &id);
    assert_eq!((t.text.as_str(), t.upper), ("Hi\nthere", true));
    assert!(fails(&mut s, "text.update", json!({ "id": id, "height_mm": -1 })).contains("height_mm"));
    assert!(fails(&mut s, "text.update", json!({ "id": id, "text": "  " })).contains("empty"));
    assert!(fails(&mut s, "shape.text", json!({ "text": "x", "font": { "family": "Nope", "kind": "stroke" } })).contains("not installed"));
    // Undo restores the previous outline.
    let before = text_of(&s, &id).outline.clone();
    exec(&mut s, "text.update", json!({ "id": id, "bend_deg": 90 }));
    assert_ne!(text_of(&s, &id).outline, before);
    s.undo().unwrap();
    assert_eq!(text_of(&s, &id).outline, before);
    // Convert to path keeps the outline.
    exec(&mut s, "text.convert_to_path", json!({ "ids": [id] }));
    assert!(matches!(&s.document().object(&id).unwrap().geometry, Geometry::Path { path } if *path == before));
    assert!(fails(&mut s, "text.convert_to_path", json!({ "ids": [id] })).contains("none of the objects is text"));
}

#[test]
fn locked_text_cannot_be_edited_and_path_convert_takes_text() {
    let mut s = session();
    let id = exec(&mut s, "shape.text", json!({ "text": "A", "font": hershey() }))["created"][0].as_str().unwrap().to_string();
    exec(&mut s, "object.lock", json!({ "ids": [id], "locked": true }));
    assert!(fails(&mut s, "text.update", json!({ "id": id, "text": "B" })).contains("locked"));
    exec(&mut s, "object.lock", json!({ "ids": [id], "locked": false }));
    exec(&mut s, "path.convert", json!({ "ids": [id] }));
    assert_eq!(s.document().object(&id).unwrap().geometry.kind_name(), "path");
}

#[test]
fn serial_numbers_advance_and_render() {
    let mut s = session();
    let r = exec(
        &mut s,
        "shape.text",
        json!({ "text": "'SN '0ddd", "font": hershey(), "variable": { "mode": "serial", "offset": 1 } }),
    );
    let id = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["texts"][0]["shown"], "SN 001");
    exec(&mut s, "var.set_serial", json!({ "start": 10, "end": 12, "current": 10, "advance_by": 1 }));
    assert_eq!(text_of(&s, &id).shown.as_deref(), Some("SN 011"));
    let r = exec(&mut s, "var.advance", json!({}));
    assert_eq!(r["variables"]["current"], 11);
    assert_eq!(text_of(&s, &id).shown.as_deref(), Some("SN 012"));
    exec(&mut s, "var.advance", json!({}));
    assert_eq!(s.document().variables.current, 12);
    exec(&mut s, "var.advance", json!({}));
    assert_eq!(s.document().variables.current, 10, "wraps after end");
    // Auto-advance is off: the engine's after-run advance does nothing.
    let r = exec(&mut s, "var.advance", json!({ "auto": true }));
    assert_eq!(r["advanced"], false);
    assert!(!crate::auto_advance_due(s.document()));
    exec(&mut s, "var.set_serial", json!({ "auto_advance": true }));
    assert!(crate::auto_advance_due(s.document()));
    let r = exec(&mut s, "var.advance", json!({ "auto": true }));
    assert_eq!((r["advanced"].clone(), s.document().variables.current), (json!(true), 11));
    // Preview shows a value without changing anything.
    let rev = s.document().revision;
    let r = exec(&mut s, "var.preview_row", json!({ "value": 499 }));
    assert_eq!(r["items"][0]["shown"], "SN 500");
    assert_eq!(s.document().revision, rev);
    assert!(fails(&mut s, "var.set_serial", json!({ "start": 5, "end": 1 })).contains("after end"));
    exec(&mut s, "var.set_serial", json!({ "reset": true }));
    assert_eq!(s.document().variables.current, 10);
}

#[test]
fn csv_merge_and_ignore_empty_vars() {
    let mut s = session();
    exec(&mut s, "var.load_csv", json!({ "content": "Ada,Lovelace\nAlan,\nGrace,Hopper\n", "name": "people.csv" }));
    let v = &s.document().variables;
    assert_eq!((v.start, v.end, v.current, v.csv.as_ref().unwrap().rows.len()), (0, 2, 0, 3));
    let r = exec(
        &mut s,
        "shape.text",
        json!({ "text": "%0\n%1", "font": hershey(), "variable": { "mode": "csv" }, "align_v": "bottom" }),
    );
    let id = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["texts"][0]["shown"], "Ada\nLovelace");
    let two_lines = r["bounds"]["size"][1].as_f64().unwrap();
    exec(&mut s, "var.set_serial", json!({ "current": 1 }));
    exec(&mut s, "text.update", json!({ "id": id, "ignore_empty_vars": true }));
    let one = s.document().bounds_of(std::slice::from_ref(&id)).unwrap().height();
    assert!(one < two_lines - 3.0, "the empty last name's line is dropped: {one} vs {two_lines}");
    exec(&mut s, "var.clear_csv", json!({}));
    assert_eq!(text_of(&s, &id).shown.as_deref(), Some("
"), "both columns empty");
    assert!(fails(&mut s, "var.clear_csv", json!({})).contains("no CSV"));
    assert!(fails(&mut s, "var.load_csv", json!({ "content": "\"open" })).contains("quoted"));
}

#[test]
fn cut_setting_text_reads_its_layer() {
    let mut s = session();
    exec(&mut s, "layer.update", json!({ "id": "layer-1", "settings": { "power_pct": 42, "frequency_khz": 60 } }));
    let r = exec(
        &mut s,
        "shape.text",
        json!({ "text": "P K", "font": hershey(), "variable": { "mode": "cut_setting" } }),
    );
    assert_eq!(r["texts"][0]["shown"], "42% 60kHz");
}

#[test]
fn prepare_output_renders_variables_for_the_run() {
    let mut s = session();
    let id = exec(&mut s, "shape.text", json!({ "text": "0dd", "font": hershey(), "variable": { "mode": "serial" } }))["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    // Change the counter behind the commands' back: the canvas is stale, the output is not.
    let mut d = s.document().clone();
    d.variables.current = 7;
    let out = crate::prepare_output(&d).unwrap();
    let Geometry::Text(t) = &out.object(&id).unwrap().geometry else { panic!() };
    assert_eq!(t.shown.as_deref(), Some("07"));
    // A missing font fails the output rather than burn a stale number.
    let mut d = s.document().clone();
    if let Geometry::Text(t) = &mut d.object_mut(&id).unwrap().geometry {
        t.font.family = "Gone".into();
    }
    assert!(crate::prepare_output(&d).unwrap_err().contains("not installed"));
}

#[test]
fn text_follows_a_path_and_detaches() {
    let mut s = session();
    let line = exec(&mut s, "shape.path", json!({ "svg_path_data": "M -40 0 L 40 0" }))["created"][0].as_str().unwrap().to_string();
    let id = exec(&mut s, "shape.text", json!({ "text": "ON A LINE", "font": hershey(), "center": [0, 30], "align_v": "bottom" }))
        ["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    let r = exec(&mut s, "text.apply_path", json!({ "id": id, "path": line }));
    let b = &r["bounds"];
    assert!(b["min"][1].as_f64().unwrap() > -0.5 && b["max"][1].as_f64().unwrap() < 10.0, "sits on the line: {b}");
    assert!(b["center"][0].as_f64().unwrap().abs() < 0.5, "middle alignment centres it on the line: {b}");
    exec(&mut s, "text.update", json!({ "id": id, "path_offset_mm": 5 }));
    let moved = s.document().bounds_of(std::slice::from_ref(&id)).unwrap();
    assert!((moved.center().x - 5.0).abs() < 0.5);
    // Moving the path: text.refresh follows it.
    exec(&mut s, "object.transform", json!({ "ids": [line], "translate": [0, -20] }));
    exec(&mut s, "text.refresh", json!({}));
    let after = s.document().bounds_of(std::slice::from_ref(&id)).unwrap();
    assert!((after.y0 - (moved.y0 - 20.0)).abs() < 1e-6, "{after:?} vs {moved:?}");
    exec(&mut s, "text.detach_path", json!({ "id": id }));
    assert!(text_of(&s, &id).path.is_none());
    assert!(fails(&mut s, "text.detach_path", json!({ "id": id })).contains("not on a path"));
    assert!(fails(&mut s, "text.apply_path", json!({ "id": id, "path": id })).contains("cannot be a text's path"));
}

#[test]
fn barcodes_are_paths_that_can_be_redrawn() {
    let mut s = session();
    let r = exec(&mut s, "shape.barcode", json!({ "kind": "qr", "data": "OpenEtch", "module_mm": 1, "center": [10, 10] }));
    let id = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["modules"], json!([21, 21]));
    assert_eq!(r["bounds"]["size"], json!([21.0, 21.0]));
    assert_eq!(r["bounds"]["center"], json!([10.0, 10.0]));
    let o = s.document().object(&id).unwrap();
    assert_eq!(o.geometry.kind_name(), "path");
    assert!(o.extras.contains_key(crate::BARCODE_KEY));
    let r = exec(&mut s, "shape.barcode", json!({ "id": id, "kind": "data_matrix", "data": "X", "module_mm": 0.5 }));
    assert_eq!(r["affected"][0], json!(id));
    assert!(fails(&mut s, "shape.barcode", json!({ "kind": "nope", "data": "x" })).contains("unknown barcode kind"));
    assert!(fails(&mut s, "shape.barcode", json!({ "kind": "ean13", "data": "12" })).contains("ean13"));
    // Variable barcodes follow the serial number.
    let r = exec(
        &mut s,
        "shape.barcode",
        json!({ "kind": "code128", "data": "0dddd", "variable": { "mode": "serial" }, "module_mm": 0.2 }),
    );
    assert_eq!(r["data"], "0000");
    let bc = r["created"][0].as_str().unwrap().to_string();
    let before = s.document().object(&bc).unwrap().geometry.clone();
    exec(&mut s, "var.set_serial", json!({ "current": 777 }));
    assert_ne!(s.document().object(&bc).unwrap().geometry, before);
    let r = exec(&mut s, "var.preview_row", json!({}));
    assert_eq!(r["items"][0]["shown"], "0777");
    // Show Bar Code Text adds the data under the bars; the outline stays centred on the object.
    let plain = exec(&mut s, "shape.barcode", json!({ "kind": "code128", "data": "AB12", "height_mm": 10 }));
    let with = exec(&mut s, "shape.barcode", json!({ "kind": "code128", "data": "AB12", "height_mm": 10, "show_text": true }));
    let h = |r: &Value| r["bounds"]["size"][1].as_f64().unwrap();
    assert!(h(&with) > h(&plain) + 1.0, "{with}");
    assert!(with["bounds"]["center"][1].as_f64().unwrap().abs() < 1e-6);
}

#[test]
fn fonts_are_listed() {
    let mut s = session();
    let r = exec(&mut s, "font.list", json!({ "kind": "stroke" }));
    let list = r.as_array().unwrap();
    assert!(list.iter().any(|f| f["family"] == "Hershey Sans"));
    assert!(list.iter().all(|f| f["kind"] == "stroke"));
}

#[test]
fn text_documents_round_trip_through_openetch_and_lightburn_files() {
    let mut s = session();
    let id = exec(&mut s, "shape.text", json!({ "text": "Tag 1", "font": hershey(), "variable": { "mode": "serial", "offset": 3 } }))
        ["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    exec(&mut s, "var.set_serial", json!({ "current": 5, "auto_advance": true }));
    let saved = exec(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
    let mut s2 = session();
    exec(&mut s2, "doc.load", json!({ "content": saved }));
    assert_eq!(s2.document().objects, s.document().objects);
    assert_eq!(s2.document().variables, s.document().variables);
    let r = exec(&mut s, "doc.export_lbrn2", json!({}));
    let lb = r["content"].as_str().unwrap();
    assert!(lb.contains("Type=\"Text\"") && lb.contains("Str=\"Tag 1\"") && lb.contains("VariableOffset=\"3\""), "{lb}");
    assert!(lb.contains("<BackupPath"), "{lb}");
    let mut s3 = session();
    exec(&mut s3, "doc.import_lbrn2", json!({ "content": lb, "field_mm": 150 }));
    let back = s3.document().objects.iter().find(|o| o.name == "Tag 1").unwrap();
    let Geometry::Text(t) = &back.geometry else { panic!("{}", back.geometry.kind_name()) };
    assert_eq!(t.text, "Tag 1");
    let _ = id;
}

/// With a common system font installed: a TrueType text renders filled glyph outlines.
#[test]
fn system_font_text_when_installed() {
    let mut s = session();
    let font = ["Arial", "DejaVu Sans", "Liberation Sans"]
        .into_iter()
        .find(|f| text::resolve(&doc::FontSpec::system(*f), false, false).is_ok());
    let Some(font) = font else { return };
    let r = exec(&mut s, "shape.text", json!({ "text": "Ag", "font": { "family": font }, "height_mm": 10 }));
    let h = r["bounds"]["size"][1].as_f64().unwrap();
    assert!(h > 7.0 && h < 11.0, "{r}");
}

/// A run burns a plain text on a path as the canvas shows it: moving the text (or deleting its path) must not make the
/// run render it somewhere else, nor block the run or the variable-text commands.
#[test]
fn runs_burn_texts_on_paths_as_shown() {
    let mut s = session();
    let line = exec(&mut s, "shape.path", json!({ "svg_path_data": "M -40 0 L 40 0" }))["created"][0].as_str().unwrap().to_string();
    let id = exec(&mut s, "shape.text", json!({ "text": "ON A LINE", "font": hershey() }))["created"][0].as_str().unwrap().to_string();
    let serial = exec(&mut s, "shape.text", json!({ "text": "0dd", "font": hershey(), "variable": { "mode": "serial" } }))
        ["created"][0]
        .as_str()
        .unwrap()
        .to_string();
    exec(&mut s, "text.apply_path", json!({ "id": id, "path": line }));
    exec(&mut s, "object.transform", json!({ "ids": [id], "translate": [0, 30] }));
    let shown = s.document().bounds_of(std::slice::from_ref(&id)).unwrap();
    let out = crate::prepare_output(s.document()).unwrap();
    let burned = out.bounds_of(std::slice::from_ref(&id)).unwrap();
    assert!((burned.y0 - shown.y0).abs() < 1e-9 && (burned.x0 - shown.x0).abs() < 1e-9, "{burned:?} vs {shown:?}");
    // The path object is deleted: the text keeps its outline; runs and the counter still work.
    exec(&mut s, "object.delete", json!({ "ids": [line] }));
    crate::prepare_output(s.document()).unwrap();
    exec(&mut s, "var.set_serial", json!({ "current": 4 }));
    let r = exec(&mut s, "var.advance", json!({}));
    assert_eq!(r["variables"]["current"], 5);
    assert_eq!(text_of(&s, &serial).shown.as_deref(), Some("05"));
    // text.refresh reports the text whose path is gone instead of failing.
    let r = exec(&mut s, "text.refresh", json!({}));
    assert_eq!(r["failed"][0]["id"], json!(id), "{r}");
}

#[test]
fn hostile_text_and_csv_input() {
    let mut s = session();
    // Characters the font lacks, zero-width ones, right-to-left and combining marks: warnings, never a panic.
    for t in ["\u{2603}", "\u{5e9}\u{5dc}\u{5d5}\u{5dd}", "e\u{301}", "A\tB"] {
        let r = s.execute(
            crate::Invocation { id: "shape.text".into(), params: json!({ "text": t, "font": hershey() }) },
            crate::Origin::Ui,
        );
        if let Ok(o) = r {
            assert!(o.result["warnings"].is_array(), "{t:?}");
        }
    }
    assert!(fails(&mut s, "shape.text", json!({ "text": "a\u{0}b", "font": hershey() })).contains("control"));
    assert!(fails(&mut s, "shape.text", json!({ "text": "x".repeat(10_001), "font": hershey() })).contains("longer"));
    assert!(fails(&mut s, "shape.text", json!({ "text": "x", "font": hershey(), "height_mm": 0 })).contains("height_mm"));
    assert!(fails(&mut s, "shape.text", json!({ "text": "x", "font": hershey(), "bend_deg": 1e9 })).contains("bend_deg"));
    // Malformed CSV is refused; a CSV of millions of empty cells is refused before it bloats every undo step.
    assert!(fails(&mut s, "var.load_csv", json!({ "content": "a,\"b\nc" })).contains("quoted"));
    assert!(fails(&mut s, "var.load_csv", json!({ "content": "" })).contains("no rows"));
    assert!(fails(&mut s, "var.load_csv", json!({ "content": ",".repeat(2_000_000) })).contains("cells"));
    assert!(fails(&mut s, "var.load_csv", json!({ "content": "\n".repeat(2_000_000) })).contains("cells"));
    let r = exec(&mut s, "var.load_csv", json!({ "content": "a,b\n\"c,\"\"d\"\"\",e\n" }));
    assert_eq!(r["variables"]["csv"]["rows"], 2);
    // Serial counter edges: start after end is refused; extreme values wrap without overflow.
    assert!(fails(&mut s, "var.set_serial", json!({ "start": 5, "end": 1 })).contains("after end"));
    exec(&mut s, "var.set_serial", json!({ "start": i64::MIN, "end": i64::MAX, "current": i64::MAX, "advance_by": i64::MAX }));
    exec(&mut s, "var.advance", json!({ "steps": i64::MAX }));
}

/// Settings LightBurn's file cannot carry are named in one readable export warning.
#[test]
fn lightburn_export_names_lost_text_settings() {
    let mut s = session();
    exec(&mut s, "shape.text", json!({ "text": "Arc", "font": hershey(), "bend_deg": 45 }));
    let r = exec(&mut s, "doc.export_lbrn2", json!({}));
    let w = r["warnings"].to_string();
    assert!(w.contains("bend, its single-line font not written"), "{w}");
    assert!(w.contains("its saved outline shows them"), "one space between the words: {w}");
}

#[test]
fn text_and_barcode_updates_refuse_no_ops_and_follow_auto_names() {
    let mut s = session();
    let r = exec(&mut s, "shape.text", json!({ "text": "Hello\nworld", "font": hershey(), "height_mm": 10 }));
    let id = r["created"][0].as_str().unwrap().to_string();
    let name = |s: &Session, id: &str| s.document().object(id).unwrap().name.clone();
    assert_eq!(name(&s, &id), "Hello");
    // Nothing changes: refused, and no undo step is recorded.
    let steps = s.history().len();
    assert!(fails(&mut s, "text.update", json!({ "id": id, "text": "Hello\nworld", "height_mm": 10 })).contains("nothing changed"));
    assert!(fails(&mut s, "text.update", json!({ "id": id })).contains("nothing changed"));
    assert_eq!(s.history().len(), steps, "no empty undo step");
    // The auto name follows the text.
    exec(&mut s, "text.update", json!({ "id": id, "text": "  Bye\nall" }));
    assert_eq!(name(&s, &id), "Bye");
    // A name the user chose stays.
    exec(&mut s, "object.rename", json!({ "id": id, "name": "Mine" }));
    exec(&mut s, "text.update", json!({ "id": id, "text": "Again" }));
    assert_eq!(name(&s, &id), "Mine");

    let r = exec(&mut s, "shape.barcode", json!({ "kind": "qr", "data": "A", "center": [5, 5] }));
    let bc = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(name(&s, &bc), "qr barcode");
    let steps = s.history().len();
    assert!(fails(&mut s, "shape.barcode", json!({ "id": bc, "kind": "qr", "data": "A" })).contains("nothing changed"));
    assert_eq!(s.history().len(), steps, "no empty undo step");
    exec(&mut s, "shape.barcode", json!({ "id": bc, "kind": "data_matrix", "data": "A" }));
    assert_eq!(name(&s, &bc), "data_matrix barcode");
    exec(&mut s, "shape.barcode", json!({ "id": bc, "kind": "data_matrix", "data": "B", "name": "Label" }));
    exec(&mut s, "shape.barcode", json!({ "id": bc, "kind": "aztec", "data": "B" }));
    assert_eq!(name(&s, &bc), "Label");
}

/// The `kind` field's description starts with every barcode kind, comma separated up to the first full stop: the web
/// UI builds its type list from it (web/src/text/textModel.ts barcodeKindsFrom).
#[test]
fn barcode_kind_list_names_every_kind() {
    let s = session();
    let spec = s.registry().specs().find(|c| c.id == "shape.barcode").unwrap();
    let d = spec.input_schema["properties"]["kind"]["description"].as_str().unwrap();
    let first = d.split(['.', ':', ';']).next().unwrap();
    let listed: Vec<&str> = first.split(',').map(str::trim).collect();
    for k in text::barcode::Kind::ALL {
        assert!(listed.contains(&k.name()), "{} missing from {first:?}", k.name());
    }
    assert_eq!(listed.len(), text::barcode::Kind::ALL.len());
}
