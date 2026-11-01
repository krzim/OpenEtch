//! File commands: `doc.new`, `doc.load` and `doc.save` (the native `.openetch` project), `doc.import_lbrn2` and
//! `doc.export_lbrn2` (LightBurn). File contents travel as text in the parameters and results; the engine reads and
//! writes the actual files. Replacing the document is one undo step like any other edit.

use std::collections::HashMap;

use doc::file::{from_openetch_json, try_to_openetch_json};
use doc::{Document, ObjectId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::validate_settings;
use super::util::{self, invalid};
use crate::{Command, CommandError, Result, Risk};

/// Largest accepted file content (the engine's upload limit is lower).
const MAX_CONTENT_BYTES: usize = lbrn::MAX_BYTES;
/// Largest galvo field accepted, in mm.
const MAX_FIELD_MM: f64 = 2000.0;

fn content<'a>(cmd: &str, text: &'a str) -> Result<&'a str> {
    if text.len() > MAX_CONTENT_BYTES {
        return Err(invalid(cmd, format!("content is larger than {} MB", MAX_CONTENT_BYTES / (1024 * 1024))));
    }
    Ok(text)
}

/// The field size to convert with: the argument, else the document's, else an error saying what to pass.
fn field(cmd: &str, given: Option<f64>, doc: &Document) -> Result<f64> {
    let f = match given.or(doc.field_mm) {
        Some(f) => f,
        None => {
            return Err(invalid(
                cmd,
                "field_mm is required: the document has no field size; pass the galvo field size in mm the \
                 LightBurn project is laid out for (e.g. 150)",
            ));
        }
    };
    util::positive(cmd, "field_mm", f)?;
    if f > MAX_FIELD_MM {
        return Err(invalid(cmd, format!("field_mm must be at most {MAX_FIELD_MM} (got {f})")));
    }
    Ok(f)
}

/// Raise `next_id` past every numbered id, so later allocations never collide.
fn fix_next_id(doc: &mut Document) {
    let ids = doc.layers.iter().map(|l| &l.id).chain(doc.objects.iter().map(|o| &o.id)).chain(doc.groups.iter().map(|g| &g.id));
    if let Some(max) = ids.filter_map(|id| id.rsplit_once('-').and_then(|(_, n)| n.parse::<u64>().ok())).max() {
        doc.next_id = doc.next_id.max(max.saturating_add(1));
    }
}

fn object_ids(doc: &Document) -> Vec<ObjectId> {
    doc.objects.iter().map(|o| o.id.clone()).collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewInput {
    /// Name of the new document. Default "Untitled".
    pub name: Option<String>,
}

/// `doc.new`.
pub struct DocNew;

impl Command for DocNew {
    type Input = NewInput;
    const ID: &'static str = "doc.new";
    const TITLE: &'static str = "New document";
    const DESCRIPTION: &'static str = "Replace the document with an empty one: one default layer, no stock, no \
        objects. Keeps field_mm. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: NewInput) -> Result<(Value, String)> {
        let name = input.name.as_deref().map(|n| util::name(Self::ID, "name", n)).transpose()?;
        let mut fresh = Document { revision: doc.revision, field_mm: doc.field_mm, ..Document::default() };
        if let Some(n) = name {
            fresh.name = n;
        }
        let summary = format!("new document \"{}\"", fresh.name);
        *doc = fresh;
        Ok((json!({ "created": [], "affected": [], "name": doc.name, "bounds": Value::Null }), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadInput {
    /// The .openetch project as text (the JSON from doc.save or a .openetch file), not a path.
    pub content: String,
}

/// `doc.load`.
pub struct DocLoad;

impl Command for DocLoad {
    type Input = LoadInput;
    const ID: &'static str = "doc.load";
    const TITLE: &'static str = "Load project";
    const DESCRIPTION: &'static str = "Replace the document with a saved .openetch project given as text (JSON \
        {\"format\": \"openetch\", \"version\": 1, \"document\": ...}). Other formats and newer versions are refused. \
        Keeps the current field_mm when the file has none. Undoable. Returns the object ids as `created`, the layer \
        ids, the name and field-frame bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LoadInput) -> Result<(Value, String)> {
        let text = content(Self::ID, &input.content)?;
        let mut loaded = from_openetch_json(text)
            .map_err(|e| invalid(Self::ID, format!("content is not a usable .openetch project: {e}")))?;
        for l in &loaded.layers {
            for s in std::iter::once(&l.settings).chain(&l.sub_layers) {
                validate_settings(Self::ID, s)
                    .map_err(|e| invalid(Self::ID, format!("layer {} (\"{}\"): {e}", l.id, l.name)))?;
            }
        }
        loaded.field_mm = loaded.field_mm.or(doc.field_mm);
        loaded.revision = doc.revision;
        let summary = format!("loaded project \"{}\" ({} object(s))", loaded.name, loaded.objects.len());
        *doc = loaded;
        let mut r = util::edit_result(doc, &object_ids(doc), &[]);
        r["name"] = json!(doc.name);
        r["layers"] = json!(doc.layers.iter().map(|l| &l.id).collect::<Vec<_>>());
        r["field_mm"] = json!(doc.field_mm);
        Ok((r, summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveInput {}

/// `doc.save`.
pub struct DocSave;

impl Command for DocSave {
    type Input = SaveInput;
    const ID: &'static str = "doc.save";
    const TITLE: &'static str = "Save project";
    const DESCRIPTION: &'static str = "Serialize the document as a .openetch project: returns {content (JSON text), \
        revision}. Read-only; the engine's project routes write it to disk.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _input: SaveInput) -> Result<(Value, String)> {
        let content = try_to_openetch_json(doc).map_err(|e| CommandError::Failed(e.to_string()))?;
        Ok((json!({ "content": content, "revision": doc.revision }), String::new()))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// Swap the whole document (keeping field_mm).
    #[default]
    Replace,
    /// Add the file's layers and objects to the document.
    Merge,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportLbrn2Input {
    /// The LightBurn .lbrn2 file contents as text (XML), not a path.
    pub content: String,
    /// "replace" (default) swaps the whole document except field_mm; "merge" adds layers (reusing one whose name
    /// and settings match) and objects with fresh ids.
    #[serde(default)]
    pub mode: ImportMode,
    /// Galvo field size in mm the LightBurn project was laid out for (LightBurn files do not record it). Default:
    /// the document's field_mm.
    pub field_mm: Option<f64>,
    /// File name (without extension) used to name the group a merge creates. Default "Imported LightBurn".
    pub name: Option<String>,
}

/// The name of the group holding a merged file's objects.
fn merged_group_name(name: Option<&str>) -> String {
    let clean: String = name.unwrap_or_default().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let clean = clean.trim();
    if clean.is_empty() { "Imported LightBurn".into() } else { clean.chars().take(200).collect() }
}

/// `doc.import_lbrn2`.
pub struct ImportLbrn2;

impl Command for ImportLbrn2 {
    type Input = ImportLbrn2Input;
    const ID: &'static str = "doc.import_lbrn2";
    const TITLE: &'static str = "Import LightBurn project";
    const DESCRIPTION: &'static str = "Import a LightBurn .lbrn2 project or .lbt template given as text. Cut \
        settings become layers; shapes become objects and groups in the field frame (via field_mm). Text is drawn \
        and burned as LightBurn's saved outline, bitmaps become images (not engraved yet); everything else is kept \
        for doc.export_lbrn2. replace swaps the document; merge adds the file's objects as one group. Returns \
        created object, group and layer ids, reused layer ids, warnings and bounds. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: ImportLbrn2Input) -> Result<(Value, String)> {
        let id = Self::ID;
        let text = content(id, &input.content)?;
        let field_mm = field(id, input.field_mm, doc)?;
        let project = lbrn::parse(text).map_err(|e| invalid(id, e.to_string()))?;
        let lbrn::Imported { layers, objects, groups, extras, warnings } =
            lbrn::to_document(&project, field_mm).map_err(|e| invalid(id, e.to_string()))?;
        // Nothing below can fail, so the document is never left half-changed.
        let (mut created_layers, mut reused_layers, mut created) = (Vec::new(), Vec::new(), Vec::new());
        let mut created_groups = Vec::new();
        match input.mode {
            ImportMode::Replace => {
                let mut fresh = Document {
                    revision: doc.revision,
                    field_mm: doc.field_mm.or(Some(field_mm)),
                    layers,
                    objects,
                    groups,
                    extras,
                    ..Document::default()
                };
                fix_next_id(&mut fresh);
                created_layers = fresh.layers.iter().map(|l| l.id.clone()).collect();
                created = object_ids(&fresh);
                created_groups = fresh.groups.iter().map(|g| g.id.clone()).collect();
                *doc = fresh;
            }
            ImportMode::Merge => {
                // Check the ids up front, so the allocations below cannot fail half-way.
                let needed = (layers.len() + objects.len() + groups.len() + 1) as u64;
                if doc.next_id.saturating_add(needed) > doc::MAX_ID {
                    return Err(doc::IdsExhausted(doc.next_id).into());
                }
                doc.field_mm = doc.field_mm.or(Some(field_mm));
                let before = doc.layers.len();
                let mut map: HashMap<String, String> = HashMap::new();
                for mut l in layers {
                    let same = doc.layers[..before]
                        .iter()
                        .find(|e| e.name == l.name && e.settings == l.settings && e.sub_layers == l.sub_layers)
                        .map(|e| e.id.clone());
                    let old = l.id.clone();
                    let target = match same {
                        Some(e) => {
                            if !reused_layers.contains(&e) {
                                reused_layers.push(e.clone());
                            }
                            e
                        }
                        None => {
                            l.id = doc.alloc_id("layer")?;
                            created_layers.push(l.id.clone());
                            let new_id = l.id.clone();
                            doc.layers.push(l);
                            new_id
                        }
                    };
                    map.insert(old, target);
                }
                let mut objects = objects;
                for o in &mut objects {
                    if let Some(layer) = map.get(&o.layer) {
                        o.layer = layer.clone();
                    }
                    o.id = doc.alloc_id("obj")?;
                    created.push(o.id.clone());
                }
                // The file's objects come in as one group when there is more than one item (LightBurn's "group
                // imported shapes"), its own groups nested inside. The file's document-level LightBurn data
                // (root attributes, notes, thumbnail) stays with the file.
                let top_items = objects.iter().filter(|o| o.group.is_none()).count()
                    + groups.iter().filter(|g| g.parent.is_none()).count();
                let outer = (top_items > 1).then(|| doc.alloc_id("group")).transpose()?;
                let mut group_map: HashMap<String, String> = HashMap::new();
                for g in &groups {
                    group_map.insert(g.id.clone(), doc.alloc_id("group")?);
                }
                if let Some(outer) = &outer {
                    doc.groups.push(doc::Group::new(outer.clone(), merged_group_name(input.name.as_deref()), None));
                    created_groups.push(outer.clone());
                }
                for mut g in groups {
                    g.id = group_map[&g.id].clone();
                    g.parent = match &g.parent {
                        Some(p) => group_map.get(p).cloned(),
                        None => outer.clone(),
                    };
                    created_groups.push(g.id.clone());
                    doc.groups.push(g);
                }
                for mut o in objects {
                    o.group = match &o.group {
                        Some(g) => group_map.get(g).cloned(),
                        None => outer.clone(),
                    };
                    doc.objects.push(o);
                }
                doc.prune_groups();
            }
        }
        let mode = match input.mode {
            ImportMode::Replace => "replace",
            ImportMode::Merge => "merge",
        };
        let summary = format!(
            "imported LightBurn project ({mode}): {} object(s), {} new layer(s), {} reused",
            created.len(),
            created_layers.len(),
            reused_layers.len()
        );
        let mut r = util::edit_result(doc, &created, &[]);
        r["created_groups"] = json!(created_groups);
        r["created_layers"] = json!(created_layers);
        r["reused_layers"] = json!(reused_layers);
        r["warnings"] = json!(warnings);
        r["mode"] = json!(mode);
        r["field_mm"] = json!(field_mm);
        Ok((r, summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportLbrn2Input {
    /// Galvo field size in mm to lay the LightBurn project out for. Default: the document's field_mm.
    pub field_mm: Option<f64>,
}

/// `doc.export_lbrn2`.
pub struct ExportLbrn2;

impl Command for ExportLbrn2 {
    type Input = ExportLbrn2Input;
    const ID: &'static str = "doc.export_lbrn2";
    const TITLE: &'static str = "Export LightBurn project";
    const DESCRIPTION: &'static str = "Convert the document to a LightBurn .lbrn2 project: returns {content (XML \
        text), warnings, field_mm}. Layers keep the LightBurn index and settings they were imported with; the stock \
        outline goes on tool layer T1; non-output and hidden layers stay so. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, input: ExportLbrn2Input) -> Result<(Value, String)> {
        let id = Self::ID;
        let field_mm = field(id, input.field_mm, doc)?;
        let (project, warnings) = lbrn::from_document(doc, field_mm).map_err(|e| invalid(id, e.to_string()))?;
        Ok((json!({ "content": lbrn::write(&project), "warnings": warnings, "field_mm": field_mm }), String::new()))
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::{Value, json};

    use crate::{CommandError, Invocation, Origin, Session};

    const LBRN: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="1.4.00" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <CutSetting type="Scan">
        <index Value="1"/>
        <name Value="Fill"/>
        <maxPower Value="30"/>
        <speed Value="2000"/>
        <frequency Value="100000"/>
        <QPulseWidth Value="200"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Rect" CutIndex="1" W="20" H="10" Cr="0">
        <XForm>1 0 0 1 80 75</XForm>
    </Shape>
    <Shape Type="Path" CutIndex="1" VertID="0" PrimID="0">
        <XForm>1 0 0 1 75 75</XForm>
        <VertList>V0 0c0x1c1x1V5 0c0x1c1x1V5 5c0x1c1x1</VertList>
        <PrimList>L0 1L1 2L2 0</PrimList>
    </Shape>
    <Shape Type="Bitmap" CutIndex="1" W="1" H="1" Data="">
        <XForm>1 0 0 1 75 75</XForm>
    </Shape>
</LightBurnProject>"#;

    fn session(field_mm: Option<f64>) -> Session {
        Session::new(Document { field_mm, ..Document::default() })
    }

    fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, CommandError> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn ok(s: &mut Session, id: &str, params: Value) -> Value {
        run(s, id, params).unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    fn strip_rev(d: &Document) -> Document {
        Document { revision: 0, ..d.clone() }
    }

    #[test]
    fn save_then_load_round_trips_and_undoes() {
        let mut s = session(Some(153.75));
        ok(&mut s, "layer.add", json!({"name": "Cut", "settings": {"pulse_width_ns": 50}}));
        ok(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 5, "layer": "layer-2"}));
        let saved = s.document().clone();
        let r = ok(&mut s, "doc.save", json!({}));
        assert_eq!(r["revision"], json!(2));
        assert_eq!(s.document().revision, 2, "save is read-only");
        let content = r["content"].as_str().unwrap().to_string();
        assert!(content.starts_with(r#"{"format":"openetch","version":1,"document":"#), "{content}");

        ok(&mut s, "doc.new", json!({"name": "Blank"}));
        assert_eq!(s.document().name, "Blank");
        assert!(s.document().objects.is_empty());
        assert_eq!(s.document().layers.len(), 1);
        assert_eq!(s.document().field_mm, Some(153.75));

        let r = ok(&mut s, "doc.load", json!({"content": content}));
        assert_eq!(r["created"], json!(["obj-3"]));
        assert_eq!(r["layers"], json!(["layer-1", "layer-2"]));
        assert_eq!(s.document().revision, 4, "revisions keep counting up");
        assert_eq!(strip_rev(s.document()), strip_rev(&saved));
        s.undo().unwrap();
        assert_eq!(s.document().name, "Blank");
        s.undo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&saved));
    }

    #[test]
    fn load_keeps_field_when_the_file_has_none() {
        let mut s = session(Some(153.75));
        let without = doc::file::to_openetch_json(&Document::default());
        ok(&mut s, "doc.load", json!({"content": without}));
        assert_eq!(s.document().field_mm, Some(153.75));
        let with = doc::file::to_openetch_json(&Document { field_mm: Some(110.0), ..Document::default() });
        let r = ok(&mut s, "doc.load", json!({"content": with}));
        assert_eq!(r["field_mm"], json!(110.0));
        assert_eq!(s.document().field_mm, Some(110.0));
    }

    #[test]
    fn load_refuses_other_formats_and_bad_settings() {
        let mut s = session(None);
        let before = s.document().clone();
        let e = run(&mut s, "doc.load", json!({"content": LBRN})).unwrap_err();
        assert!(matches!(e, CommandError::InvalidInput { .. }) && e.to_string().contains("LightBurn"), "{e}");
        let newer = json!({"format": "openetch", "version": 9, "document": Document::default()}).to_string();
        let e = run(&mut s, "doc.load", json!({"content": newer})).unwrap_err();
        assert!(e.to_string().contains("update OpenEtch"), "{e}");
        let mut d = Document::default();
        d.layers[0].settings.passes = 5000;
        let e = run(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&d)})).unwrap_err();
        assert!(e.to_string().contains("passes"), "{e}");
        assert!(run(&mut s, "doc.load", json!({})).is_err());
        assert_eq!(s.document(), &before);
        assert!(s.history().is_empty());
    }

    #[test]
    fn import_replace_swaps_everything_but_field() {
        let mut s = session(None);
        ok(&mut s, "stock.define", json!({"shape": {"kind": "circle", "diameter_mm": 30}, "thickness_mm": 1}));
        let before = s.document().clone();
        let e = run(&mut s, "doc.import_lbrn2", json!({"content": LBRN})).unwrap_err();
        assert!(e.to_string().contains("field_mm is required"), "{e}");
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "append", "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": "<svg/>", "field_mm": 150})).is_err());
        assert!(run(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": -1})).is_err());

        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150}));
        assert_eq!(r["mode"], json!("replace"));
        assert_eq!(r["created"], json!(["obj-2", "obj-3"]));
        assert_eq!(r["created_layers"], json!(["layer-1"]));
        assert!(r["warnings"].to_string().contains("bitmap with no usable image data was kept"), "{}", r["warnings"]);
        assert_eq!(r["created_groups"], json!([]));
        assert_eq!(r["bounds"]["min"], json!([-5.0, -5.0]));
        assert_eq!(r["bounds"]["max"], json!([15.0, 5.0]));
        let d = s.document();
        assert_eq!(d.field_mm, Some(150.0), "set from the import when the document had none");
        assert!(d.stock.is_none());
        assert_eq!(d.layers[0].name, "Fill");
        assert_eq!(d.layers[0].settings.frequency_khz, 100.0);
        assert_eq!(d.layers[0].settings.pulse_width_ns, Some(200.0));
        assert_eq!(d.next_id, 4);
        s.undo().unwrap();
        assert_eq!(strip_rev(s.document()), strip_rev(&before));

        // The document's own field size is kept even when another one is used to read the file.
        let mut s = session(Some(153.75));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "field_mm": 150}));
        assert_eq!(r["field_mm"], json!(150.0));
        assert_eq!(s.document().field_mm, Some(153.75));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
        assert_eq!(r["bounds"]["min"], json!([-6.875, -6.875]), "uses the document's field");
    }

    #[test]
    fn import_merge_reuses_matching_layers_with_fresh_ids() {
        let mut s = session(Some(150.0));
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4}));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge", "name": "badge"}));
        assert_eq!(r["created_layers"], json!(["layer-3"]));
        assert_eq!(r["reused_layers"], json!([]));
        assert_eq!(r["created"], json!(["obj-4", "obj-5"]));
        assert_eq!(r["created_groups"], json!(["group-6"]), "the file's objects come in as one group");
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN, "mode": "merge"}));
        assert_eq!(r["created_layers"], json!([]));
        assert_eq!(r["reused_layers"], json!(["layer-3"]));
        assert_eq!(r["created"], json!(["obj-7", "obj-8"]));
        let d = s.document();
        assert_eq!(d.layers.len(), 2);
        assert_eq!(d.objects.len(), 5);
        assert!(d.objects[1..].iter().all(|o| o.layer == "layer-3"));
        assert_eq!(d.objects[0].layer, "layer-1");
        let names: Vec<&str> = d.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["badge", "Imported LightBurn"]);
        assert_eq!(d.group_members("group-6"), ["obj-4", "obj-5"]);
        assert!(d.extras.is_empty(), "a merge keeps the document's own file data");
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 3);
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 1);
        assert_eq!(s.document().layers.len(), 1);
        assert!(s.document().groups.is_empty());
    }

    #[test]
    fn import_keeps_lightburn_groups_text_and_file_data_for_export() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<LightBurnProject AppVersion="2.1.04" FormatVersion="1" MaterialHeight="0" MirrorX="False" MirrorY="False">
    <Notes ShowOnLoad="1" Notes="keep me"/>
    <CutSetting type="Cut">
        <index Value="0"/>
        <name Value="C00"/>
        <maxPower Value="20"/>
        <speed Value="100"/>
        <priority Value="0"/>
    </CutSetting>
    <Shape Type="Group">
        <XForm>1 0 0 1 0 0</XForm>
        <Children>
            <Shape Type="Rect" CutIndex="0" W="4" H="2" Cr="0">
                <XForm>1 0 0 1 70 75</XForm>
            </Shape>
            <Shape Type="Text" CutIndex="0" Font="Arial,-1,100,5,50,0,0,0,0,0" Str="Hi" H="2" HasBackupPath="1">
                <BackupPath Type="Path" CutIndex="0">
                    <XForm>1 0 0 1 80 75</XForm>
                    <VertList>V0 0c0x1c1x1V1 0c0x1c1x1V1 1c0x1c1x1</VertList>
                    <PrimList>L0 1L1 2L2 0</PrimList>
                </BackupPath>
                <XForm>1 0 0 1 80 75</XForm>
            </Shape>
        </Children>
    </Shape>
</LightBurnProject>
"#;
        let mut s = session(Some(150.0));
        let r = ok(&mut s, "doc.import_lbrn2", json!({"content": text}));
        assert_eq!(r["created_groups"], json!(["group-2"]));
        let d = s.document();
        assert_eq!(d.group_members("group-2"), ["obj-3", "obj-4"]);
        assert_eq!(d.objects[1].name, "Hi");
        assert_eq!(d.objects[1].extras.get("lbrn2.Str").map(String::as_str), Some("Hi"));
        assert!(d.extras.contains_key("lbrn2._root_attrs"));
        // Saved and opened again, then exported: the file comes back as it was.
        let saved = ok(&mut s, "doc.save", json!({}))["content"].as_str().unwrap().to_string();
        ok(&mut s, "doc.new", json!({}));
        ok(&mut s, "doc.load", json!({"content": saved}));
        let out = ok(&mut s, "doc.export_lbrn2", json!({}))["content"].as_str().unwrap().to_string();
        assert_eq!(out, text);
    }

    #[test]
    fn load_refuses_documents_the_edit_commands_would_never_make() {
        let mut s = session(None);
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2}));
        let good = s.document().clone();
        let before = s.document().clone();
        type Spoil = fn(&mut Document);
        let cases: [(&str, Spoil); 8] = [
            ("does not exist", |d| d.objects[0].layer = "layer-404".into()),
            ("no layers", |d| {
                d.layers.clear();
                d.objects.clear();
            }),
            ("not positive", |d| {
                d.objects[0].geometry = doc::Geometry::Rect { width_mm: -4.0, height_mm: 2.0, corner_radius_mm: 0.0 }
            }),
            ("cannot be inverted", |d| d.objects[0].transform = kurbo::Affine::new([0.0; 6])),
            ("control character", |d| d.name = "x\u{0}".into()),
            ("control character", |d| d.layers[0].name = "\u{1}".into()),
            ("too large", |d| d.objects[0].id = "obj-18446744073709551615".into()),
            ("extras key", |d| {
                d.layers[0].extras.insert("lbrn2.evil a=\"1\"/><Shape/><x".into(), "1".into());
            }),
        ];
        for (what, f) in cases {
            let mut d = good.clone();
            f(&mut d);
            let e = run(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&d)})).unwrap_err();
            assert!(e.to_string().contains(what), "{what}: {e}");
            assert_eq!(s.document(), &before);
        }
        ok(&mut s, "doc.load", json!({"content": doc::file::to_openetch_json(&good)}));
    }

    #[test]
    fn edits_that_overflow_to_non_finite_geometry_are_refused() {
        let mut s = session(Some(150.0));
        ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2}));
        ok(&mut s, "object.transform", json!({"ids": ["obj-2"], "scale": 1e100, "about": "origin"}));
        let before = s.document().clone();
        let history = s.history().len();
        // 1e100 x 1e250 overflows to infinity.
        let e = run(&mut s, "object.transform", json!({"ids": ["obj-2"], "scale": 1e250, "about": "origin"}))
            .unwrap_err();
        assert!(matches!(e, CommandError::Failed(_)) && e.to_string().contains("unusable geometry"), "{e}");
        assert_eq!(s.document(), &before);
        assert_eq!(s.history().len(), history);
        // What is committed can always be saved and loaded again.
        let saved = ok(&mut s, "doc.save", json!({}));
        ok(&mut s, "doc.load", json!({"content": saved["content"]}));
    }

    #[test]
    fn save_refuses_a_document_that_would_not_load_again() {
        let mut d = Document::default();
        d.layers[0].settings.speed_mm_s = f64::INFINITY;
        let mut s = Session::new(d);
        let e = run(&mut s, "doc.save", json!({})).unwrap_err();
        assert!(e.to_string().contains("cannot save"), "{e}");
    }

    #[test]
    fn exhausted_ids_are_an_error_not_a_panic() {
        let mut s = Session::new(Document { next_id: doc::MAX_ID, field_mm: Some(150.0), ..Document::default() });
        let before = s.document().clone();
        for (id, params) in [
            ("shape.rect", json!({"width_mm": 4, "height_mm": 2})),
            ("layer.add", json!({"name": "x"})),
            ("doc.import_lbrn2", json!({"content": LBRN, "mode": "merge"})),
        ] {
            let e = run(&mut s, id, params).unwrap_err();
            assert!(matches!(e, CommandError::Failed(_)) && e.to_string().contains("used up"), "{id}: {e}");
            assert_eq!(s.document(), &before, "{id}");
        }
        // A replacing import starts a fresh id range.
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
    }

    #[test]
    fn setting_the_frequency_clears_the_lightburn_no_frequency_marker() {
        let no_freq = LBRN.replace("        <frequency Value=\"100000\"/>\n", "");
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": no_freq}));
        let marker = |s: &Session| s.document().layers[0].extras.contains_key(lbrn::FREQUENCY_ABSENT_KEY);
        assert!(marker(&s));
        let export = |s: &mut Session| ok(s, "doc.export_lbrn2", json!({}))["content"].as_str().unwrap().to_string();
        assert!(!export(&mut s).contains("<frequency"), "the source had none");
        ok(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"power_pct": 40}}));
        assert!(marker(&s), "other settings leave it");
        ok(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"frequency_khz": 60}}));
        assert!(!marker(&s));
        assert!(export(&mut s).contains("<frequency Value=\"60000\"/>"));
    }

    #[test]
    fn names_with_control_characters_are_refused() {
        let mut s = session(None);
        assert!(run(&mut s, "layer.add", json!({"name": "a\u{1}b"})).is_err());
        assert!(run(&mut s, "doc.rename", json!({"name": "x\ny"})).is_err());
    }

    #[test]
    fn export_needs_a_field_and_writes_lightburn_xml() {
        let mut s = session(None);
        ok(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 2, "center": [10, 0]}));
        let e = run(&mut s, "doc.export_lbrn2", json!({})).unwrap_err();
        assert!(e.to_string().contains("field_mm is required"), "{e}");
        let r = ok(&mut s, "doc.export_lbrn2", json!({"field_mm": 100}));
        assert_eq!(r["warnings"], json!([]));
        let p = lbrn::parse(r["content"].as_str().unwrap()).unwrap();
        assert_eq!(p.shapes.len(), 1);
        assert_eq!(p.shapes[0].xform.translation(), kurbo::Vec2::new(60.0, 50.0));
        assert_eq!(s.document().revision, 1, "export is read-only");

        // Import, then export: the LightBurn index and kept values come back.
        let mut s = session(Some(150.0));
        ok(&mut s, "doc.import_lbrn2", json!({"content": LBRN}));
        let r = ok(&mut s, "doc.export_lbrn2", json!({}));
        let p = lbrn::parse(r["content"].as_str().unwrap()).unwrap();
        assert_eq!(p.cut_settings[0].index, 1);
        assert_eq!(p.cut_settings[0].params.q_pulse_width, Some(200.0));
        assert_eq!(p.shapes.len(), 3, "the unusable bitmap is kept for LightBurn");
    }
}
