//! Field and workspace layout of new documents and LightBurn imports (moved from the engine's `state` module): which
//! `field_mm` / `workspace_mm` / `origin` a `doc.new`, `doc.import_lbrn2` or `doc.export_lbrn2` gets when the caller
//! gives none.

use commands::Invocation;
use doc::Document;
use serde_json::{Value, json};

/// Commands that convert with (or start at) a field size and get one filled in when the caller gives none.
pub const NEEDS_FIELD: [&str; 3] = ["doc.new", "doc.import_lbrn2", "doc.export_lbrn2"];

/// An empty document laid out for `device`: a galvo's field (`machine_field` when it drives the machine; a
/// non-square field as its `workspace_mm` with the centre origin), a gantry's bed with its origin corner (the
/// smaller side as `field_mm`).
pub fn new_document_for(device: &machine::Device, machine_field: f64) -> Document {
    let mut d = Document::default();
    if device.kind.is_galvo() {
        let [w, h] = galvo_field(device, machine_field);
        d.field_mm = Some(w);
        if (w - h).abs() > 1e-9 {
            d.workspace_mm = Some([w, h]);
            d.extras.insert(commands::ORIGIN_KEY.into(), "center".into());
        }
    } else {
        d.field_mm = Some(device.field_mm());
        d.workspace_mm = Some(device.workspace_mm);
        d.extras.insert(commands::ORIGIN_KEY.into(), device.origin.name().into());
    }
    d
}

/// Whether an invocation's params have no (or a null) `field_mm`.
pub fn lacks_field(inv: &Invocation) -> bool {
    lacks(inv, "field_mm")
}

/// Whether an invocation's params have no (or a null) `key`.
pub fn lacks(inv: &Invocation, key: &str) -> bool {
    inv.params.get(key).is_none_or(Value::is_null)
}

/// A galvo device's field `[width, height]`: `machine_field` (the machine profile's width) when it drives the
/// machine, else the device's workspace width, with the device's height (LightBurn's separate field Height; the
/// width again for a square field).
pub fn galvo_field(device: &machine::Device, machine_field: f64) -> [f64; 2] {
    let [w, h] = device.workspace_mm;
    let width = if device.controllable { machine_field } else { w };
    // A square device keeps a square field even when the machine's width differs from the record's.
    if (w - h).abs() < 1e-9 { [width, width] } else { [width, h] }
}

/// `doc.new` parameters for `device`: a galvo's `field_mm` (`field`) or, for a non-square field, its
/// `workspace_mm` with the centre origin; a gantry's bed and origin corner.
pub fn new_layout(device: &machine::Device, field: f64) -> Value {
    if device.kind.is_galvo() {
        let [w, h] = galvo_field(device, field);
        if (w - h).abs() > 1e-9 {
            json!({ "workspace_mm": [w, h], "origin": "center" })
        } else {
            json!({ "field_mm": w })
        }
    } else {
        json!({ "workspace_mm": device.workspace_mm, "origin": device.origin.name() })
    }
}

/// `doc.import_lbrn2` parameters for a file laid out on `device`: a galvo's field as LightBurn knows it (its
/// `Width`, e.g. 153.8 for the B6, whose profile keeps EzCad's 153.75), a gantry's bed and origin corner.
pub fn lightburn_layout(device: &machine::Device) -> Value {
    if let Some(field) = device.lightburn_field_mm() {
        let h = device.workspace_mm[1];
        // A non-square galvo field (LightBurn's Height more than its 0.1 mm display step from the Width).
        if (h - device.workspace_mm[0]).abs() > 0.05 {
            json!({ "workspace_mm": [field, h], "origin": "center" })
        } else {
            json!({ "field_mm": field })
        }
    } else {
        json!({ "workspace_mm": device.workspace_mm, "origin": device.origin.name() })
    }
}

/// Add every key of `layout` the invocation does not set.
pub fn fill_layout(inv: &mut Invocation, layout: &Value) {
    if inv.params.is_null() {
        inv.params = json!({});
    }
    if let (Some(obj), Some(l)) = (inv.params.as_object_mut(), layout.as_object()) {
        for (k, v) in l {
            if obj.get(k).is_none_or(Value::is_null) {
                obj.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Give a `doc.new` / `doc.import_lbrn2` / `doc.export_lbrn2` invocation `field_mm` when it has none.
pub fn fill_field_mm(inv: &mut Invocation, field_mm: f64) {
    if !NEEDS_FIELD.contains(&inv.id.as_str()) {
        return;
    }
    if inv.params.is_null() {
        inv.params = json!({});
    }
    if let Some(obj) = inv.params.as_object_mut()
        && obj.get("field_mm").is_none_or(Value::is_null)
    {
        obj.insert("field_mm".into(), json!(field_mm));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A galvo with a separate field Height lays new documents (and LightBurn imports) out on its whole field, so the
    /// canvas can draw it; a square galvo keeps the plain field_mm.
    #[test]
    fn non_square_galvo_fields_lay_documents_out_on_their_workspace() {
        let profile = machine::MachineProfile { field_height_mm: Some(120.0), ..machine::MachineProfile::generic_jcz_galvo() };
        let w = profile.field_mm;
        let device = machine::Device::builtin(profile);
        let d = new_document_for(&device, w);
        assert_eq!((d.field_mm, d.workspace(), d.extras.get(commands::ORIGIN_KEY).map(String::as_str)), (Some(w), Some([w, 120.0]), Some("center")));
        assert_eq!(new_layout(&device, w), json!({"workspace_mm": [w, 120.0], "origin": "center"}));
        assert_eq!(lightburn_layout(&device), json!({"workspace_mm": [w, 120.0], "origin": "center"}));
        // doc.new with that layout makes the same document.
        let mut s = commands::Session::new(Document::default());
        s.execute(Invocation { id: "doc.new".into(), params: new_layout(&device, w) }, commands::Origin::Ui).unwrap();
        assert_eq!((s.document().field_mm, s.document().workspace()), (Some(w), Some([w, 120.0])));

        let square = machine::Device::builtin(machine::MachineProfile::generic_jcz_galvo());
        let d = new_document_for(&square, 150.0);
        assert_eq!((d.field_mm, d.workspace_mm, d.extras.get(commands::ORIGIN_KEY)), (Some(150.0), None, None));
        assert_eq!(new_layout(&square, 150.0), json!({"field_mm": 150.0}));
    }

    #[test]
    fn field_is_filled_only_when_missing() {
        let mut inv = Invocation { id: "doc.export_lbrn2".into(), params: Value::Null };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params, json!({"field_mm": 150.0}));
        let mut inv = Invocation { id: "doc.import_lbrn2".into(), params: json!({"content": "x", "field_mm": 110}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params["field_mm"], json!(110));
        let mut inv = Invocation { id: "doc.import_lbrn2".into(), params: json!({"content": "x", "field_mm": null}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params["field_mm"], json!(150.0));
        let mut inv = Invocation { id: "shape.rect".into(), params: json!({}) };
        fill_field_mm(&mut inv, 150.0);
        assert_eq!(inv.params, json!({}));
        let mut inv = Invocation { id: "doc.new".into(), params: json!({"name": "x"}) };
        fill_field_mm(&mut inv, 153.75);
        assert_eq!(inv.params, json!({"name": "x", "field_mm": 153.75}));
    }
}
