//! End-to-end tests through [`Session`]: undo/redo, batch atomicity, history, and the registry's schemas.

use doc::Document;
use serde_json::{Value, json};

use crate::{CommandError, Invocation, Origin, Risk, Session};

fn inv(id: &str, params: Value) -> Invocation {
    Invocation { id: id.into(), params }
}

fn exec(s: &mut Session, id: &str, params: Value) -> Value {
    match s.execute(inv(id, params), Origin::Agent("test".into())) {
        Ok(o) => o.result,
        Err(e) => panic!("{id} failed: {e}"),
    }
}

const ALL_IDS: &[&str] = &[
    "doc.inspect",
    "doc.rename",
    "doc.new",
    "doc.load",
    "doc.save",
    "doc.import_lbrn2",
    "doc.export_lbrn2",
    "stock.define",
    "stock.clear",
    "layer.add",
    "layer.update",
    "layer.delete",
    "layer.move",
    "layer.sub_add",
    "layer.sub_update",
    "layer.sub_delete",
    "layer.sub_move",
    "layer.sub_duplicate",
    "shape.rect",
    "shape.ellipse",
    "shape.path",
    "art.import_svg",
    "object.transform",
    "object.set_layer",
    "object.rename",
    "object.delete",
    "object.duplicate",
    "art.place",
    "art.array",
    "stock.move",
    "object.move",
    "object.align",
    "object.distribute",
    "group.create",
    "group.ungroup",
    "group.rename",
    "art.import_image",
    "image.adjust",
    "job.set_optimization",
    "object.set_cut_properties",
    "job.set_rotary",
    "art.import_dxf",
    "path.boolean",
    "path.weld",
    "path.cut_shapes",
    "path.offset",
    "path.break_apart",
    "path.auto_join",
    "path.close",
    "path.delete_duplicates",
    "path.convert",
    "path.optimize",
    "path.fillet",
    "path.trim",
    "path.reverse",
    "node.list",
    "node.move",
    "node.insert",
    "node.delete",
    "node.break",
    "node.join",
    "node.set_type",
    "node.segment_type",
    "shape.polygon",
    "shape.line",
    "shape.circle",
    "art.circular_array",
    "art.along_path",
    "art.nest",
    "object.order",
    "object.lock",
    "object.make_same",
    "object.rotate_to",
    "object.scale_to",
    // Text, variable text, barcodes, fonts.
    "shape.text",
    "text.update",
    "text.apply_path",
    "text.detach_path",
    "text.convert_to_path",
    "text.refresh",
    "var.set_serial",
    "var.advance",
    "var.load_csv",
    "var.clear_csv",
    "var.preview_row",
    "shape.barcode",
    "font.list",
    "layer.apply_preset",
    "layer.unlink_preset",
    "generate.material_test",
    "generate.interval_test",
    "generate.focus_test",
    "object.set_start_point",
    "object.clear_start_point",
    "object.align_to_targets",
    // Files and image tools.
    "doc.export",
    "doc.print",
    "art.import_pdf",
    "art.import_ai",
    "art.import_eps",
    "art.import_ezd",
    "art.import_hpgl",
    "art.import_gerber",
    "art.import_model",
    "image.trace",
    "image.trace_preview",
    "art.convert_to_bitmap",
    "image.apply_mask",
    "image.remove_mask",
    "image.flatten_mask",
    "image.replace",
    "image.export",
    "image.processed",
    "page.list",
    "page.add",
    "page.duplicate",
    "page.delete",
    "page.rename",
    "page.activate",
    "page.move",
    "page.send_objects",
    "doc.set_notes",
];

#[test]
fn registry_lists_every_command_with_documented_schema() {
    let s = Session::new(Document::default());
    let ids: Vec<&str> = s.registry().specs().map(|c| c.id).collect();
    assert_eq!(ids, ALL_IDS);
    for spec in s.registry().specs() {
        assert!(!spec.description.is_empty() && spec.description.len() < 700, "{}", spec.id);
        assert!(!spec.description.contains("  "), "{}: a line continuation lost its backslash", spec.id);
        let query = matches!(
            spec.id,
            "doc.inspect" | "doc.save" | "doc.export_lbrn2" | "node.list" | "var.preview_row" | "font.list"
                | "doc.export" | "doc.print" | "image.trace_preview" | "image.export" | "image.processed" | "page.list"
        );
        assert_eq!(spec.risk == Risk::Query, query, "{}", spec.id);
        let props = spec.input_schema["properties"].as_object();
        for (name, p) in props.into_iter().flatten() {
            // Every field carries a description, directly or through a referenced definition.
            let documented = p.get("description").is_some()
                || p.get("$ref").is_some()
                || p.get("anyOf").is_some()
                || p.get("allOf").is_some();
            assert!(documented, "{}.{name} has no description: {p}", spec.id);
        }
    }
}

#[test]
fn every_edit_is_one_undo_step() {
    let mut s = Session::new(Document::default());
    let r = exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}));
    let id = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(s.document().revision, 1);
    exec(&mut s, "object.transform", json!({"ids": [id], "translate": [5, 0]}));
    assert_eq!(s.document().object(&id).unwrap().bounds().x0, 0.0);

    s.undo().unwrap();
    assert_eq!(s.document().object(&id).unwrap().bounds().x0, -5.0);
    s.undo().unwrap();
    assert!(s.document().objects.is_empty());
    assert!(matches!(s.undo(), Err(CommandError::NothingTo("undo"))));
    s.redo().unwrap();
    s.redo().unwrap();
    assert_eq!(s.document().object(&id).unwrap().bounds().x0, 0.0);
    assert!(s.redo().is_err());
    assert_eq!(s.history().len(), 2);
}

#[test]
fn query_does_not_create_undo_step() {
    let mut s = Session::new(Document::default());
    let r = exec(&mut s, "doc.inspect", json!({}));
    assert_eq!(r["revision"], json!(0));
    assert_eq!(s.document().revision, 0);
    assert!(s.undo().is_err());
}

#[test]
fn batch_is_atomic() {
    let mut s = Session::new(Document::default());
    let before = s.document().clone();
    let err = s.batch(
        vec![
            inv("shape.rect", json!({"width_mm": 10, "height_mm": 10})),
            inv("layer.add", json!({"name": "Cut"})),
            inv("object.delete", json!({"ids": ["obj-404"]})),
        ],
        Origin::Ui,
    );
    assert!(matches!(err, Err(CommandError::NotFound { .. })));
    assert_eq!(s.document(), &before);
    assert!(s.history().is_empty());

    let ok = s
        .batch(
            vec![
                inv("shape.rect", json!({"width_mm": 10, "height_mm": 10})),
                inv("layer.add", json!({"name": "Cut"})),
                inv("object.set_layer", json!({"ids": ["obj-2"], "layer": "layer-3"})),
            ],
            Origin::Ui,
        )
        .unwrap();
    assert_eq!(ok.len(), 3);
    assert!(ok.iter().all(|o| o.revision == 1));
    assert_eq!(s.document().objects[0].layer, "layer-3");
    s.undo().unwrap();
    assert_eq!(s.document().objects.len(), 0);
    assert_eq!(s.document().layers.len(), 1);
}

#[test]
fn layer_lifecycle_with_partial_settings_and_undo() {
    let mut s = Session::new(Document::default());
    let r = exec(&mut s, "layer.add", json!({"name": "Engrave", "settings": {"power_pct": 30, "mode": "fill"}}));
    let lid = r["id"].as_str().unwrap().to_string();
    let r = exec(&mut s, "layer.update", json!({"id": lid, "settings": {"power_pct": 40}}));
    assert_eq!(r["settings"]["power_pct"], json!(40.0));
    assert_eq!(r["settings"]["mode"], json!("fill"), "unrelated fields kept");
    assert_eq!(r["settings"]["speed_mm_s"], json!(1000.0));
    s.undo().unwrap();
    assert_eq!(s.document().layer(&lid).unwrap().settings.power_pct, 30.0);

    exec(&mut s, "shape.ellipse", json!({"rx_mm": 2, "ry_mm": 2, "layer": lid}));
    let e = s.execute(inv("layer.delete", json!({"id": lid})), Origin::Ui).unwrap_err();
    assert!(e.to_string().contains("reassign_to"), "{e}");
    let r = exec(&mut s, "layer.delete", json!({"id": lid, "reassign_to": "layer-1"}));
    assert_eq!(r["affected"].as_array().unwrap().len(), 1);
    assert_eq!(s.document().objects[0].layer, "layer-1");
    assert!(s.execute(inv("layer.delete", json!({"id": "layer-1"})), Origin::Ui).is_err(), "last layer");
    s.undo().unwrap();
    assert!(s.document().layer(&lid).is_some());
}

#[test]
fn layer_passes_above_the_cap_are_refused() {
    let mut s = Session::new(Document::default());
    let e = s.execute(inv("layer.add", json!({"name": "X", "settings": {"passes": 1001}})), Origin::Ui).unwrap_err();
    assert!(matches!(e, CommandError::InvalidInput { .. }) && e.to_string().contains("passes"), "{e}");
    let e = s
        .execute(inv("layer.update", json!({"id": "layer-1", "settings": {"passes": 4_000_000_000u32}})), Origin::Ui)
        .unwrap_err();
    assert!(e.to_string().contains("1..=1000"), "{e}");
    assert_eq!(s.document().layers.len(), 1);
    assert_eq!(s.document().layers[0].settings.passes, 1);
    exec(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"passes": 1000}}));
    assert_eq!(s.document().layers[0].settings.passes, 1000);
}

#[test]
fn unknown_fields_and_commands_are_errors() {
    let mut s = Session::new(Document::default());
    assert!(matches!(
        s.execute(inv("shape.rect", json!({"width_mm": 1, "height_mm": 1, "colour": "red"})), Origin::Ui),
        Err(CommandError::InvalidInput { .. })
    ));
    assert!(matches!(s.execute(inv("shape.square", json!({})), Origin::Ui), Err(CommandError::Unknown(_))));
    // `params: null` is treated as `{}`.
    assert!(s.execute(inv("doc.inspect", Value::Null), Origin::Ui).is_ok());
}

#[test]
fn agent_workflow_import_place_array() {
    let mut s = Session::new(Document::default());
    exec(
        &mut s,
        "stock.define",
        json!({"name": "tag", "shape": {"kind": "circle", "diameter_mm": 30}, "thickness_mm": 1, "material": "stainless 304"}),
    );
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10"><path d="M0 0 H10 V10 Z"/></svg>"#;
    let r = exec(&mut s, "art.import_svg", json!({"svg": svg}));
    let id = r["created"][0].as_str().unwrap().to_string();
    let r = exec(&mut s, "art.place", json!({"ids": [id], "fit": {"mode": "contain", "margin_mm": 1.5}}));
    assert_eq!(r["bbox_inside_stock"], json!(true));
    // A square in a 30 mm circle with 1.5 mm margin: diagonal 27 mm.
    let side = 27.0 / 2f64.sqrt();
    assert!((r["bounds"]["size"][0].as_f64().unwrap() - side).abs() < 1e-3);
    let warnings = exec(&mut s, "doc.inspect", json!({}))["warnings"].clone();
    assert_eq!(warnings, json!([]), "{warnings}");

    exec(&mut s, "art.array", json!({"ids": [id], "rows": 1, "cols": 2, "pitch_x_mm": 40, "pitch_y_mm": 0}));
    let warnings = exec(&mut s, "doc.inspect", json!({}))["warnings"].clone();
    assert!(warnings.to_string().contains("entirely outside"), "{warnings}");
    assert_eq!(
        s.history().iter().map(|h| h.command.as_str()).collect::<Vec<_>>(),
        ["stock.define", "art.import_svg", "art.place", "art.array"]
    );
    assert_eq!(s.history()[0].origin, Origin::Agent("test".into()));
}

const LBRN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.00" FormatVersion="1">
    <CutSetting type="Cut"><index Value="0"/><maxPower Value="20"/><speed Value="500"/><frequency Value="30000"/></CutSetting>
    <Shape Type="Ellipse" CutIndex="0" Rx="3" Ry="2"><XForm>1 0 0 1 75 75</XForm></Shape>
</LightBurnProject>"#;

#[test]
fn undo_redo_for_each_edit_command() {
    let loaded = Document { name: "Loaded".into(), ..Document::default() };
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><circle cx="10" cy="10" r="5"/></svg>"#;
    let steps: Vec<(&str, Value)> = vec![
        ("doc.rename", json!({"name": "Coasters"})),
        (
            "stock.define",
            json!({"shape": {"kind": "rect", "width_mm": 80, "height_mm": 80, "corner_radius_mm": 5}, "thickness_mm": 3}),
        ),
        ("layer.add", json!({"name": "Cut", "color": "#ff0000"})),
        ("layer.update", json!({"id": "layer-2", "output": false})),
        ("shape.rect", json!({"width_mm": 10, "height_mm": 5})),
        ("shape.ellipse", json!({"rx_mm": 3, "ry_mm": 2, "center": [10, 10]})),
        ("shape.path", json!({"svg_path_data": "M0 0 L5 5"})),
        ("art.import_svg", json!({"svg": svg, "layer": "layer-2"})),
        ("object.transform", json!({"ids": ["obj-3", "obj-4"], "rotate_deg": 45})),
        ("object.set_layer", json!({"ids": ["obj-5"], "layer": "layer-2"})),
        ("object.rename", json!({"id": "obj-3", "name": "Frame"})),
        ("object.duplicate", json!({"ids": ["obj-3"], "offset": [0, -20]})),
        ("art.place", json!({"ids": ["obj-6"], "anchor": "bottom_right"})),
        ("art.array", json!({"ids": ["obj-4"], "rows": 2, "cols": 2, "pitch_x_mm": 8, "pitch_y_mm": 8})),
        ("object.delete", json!({"ids": ["obj-5"]})),
        ("layer.delete", json!({"id": "layer-2", "reassign_to": "layer-1"})),
        ("stock.clear", json!({})),
        ("doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "field_mm": 150})),
        ("doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150})),
        ("doc.load", json!({"content": doc::file::to_openetch_json(&loaded)})),
        ("doc.new", json!({"name": "Fresh"})),
    ];
    let mut s = Session::new(Document::default());
    for (id, params) in steps {
        let before = s.document().clone();
        let r = exec(&mut s, id, params);
        assert!(r.get("created").is_some() && r.get("affected").is_some() && r.get("bounds").is_some(), "{id}: {r}");
        let after = s.document().clone();
        assert_ne!(before, after, "{id} changed nothing");
        s.undo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&before), "{id} undo");
        s.redo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&after), "{id} redo");
    }
}

fn strip_rev(d: &Document) -> Document {
    Document { revision: 0, ..d.clone() }
}
