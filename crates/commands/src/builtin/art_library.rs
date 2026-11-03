//! Art Library items (LightBurn's Art Library window, docs "Art Library"): reusable artwork, without cut settings,
//! that is added from a selection and inserted into a document. The engine keeps the libraries (module `artlib`);
//! this module owns the item content, an [`ArtFragment`]: the objects (geometry, transform, name, extras), the groups
//! they belong to and the name and colour of each layer they were on.
//!
//! `art.insert {content, name?, center?, layer?}` inserts a fragment: every object and group gets a new id, groups
//! keep their nesting, objects go to the document layer of the same colour (LightBurn: shapes keep their colour), a
//! new layer of that name and colour when the document has none (default settings: libraries store art only), or all
//! to `layer` when given. Several top-level pieces are wrapped in one new group named `name`. The art is moved so the
//! centre of its bounds lands on `center` (default the field centre; LightBurn's Add Graphic to Project uses the
//! centre of the view, a drag the drop point). Text on a path keeps its path when the path is part of the item, else
//! it is detached (its outline stays). A fragment's `object_options` (the app's clipboard copy carries them; library
//! items never do) go to the new ids, so a pasted copy keeps its tabs and start point. One undo step; returns
//! `created` (objects), `groups`, `layers_created`.

use std::collections::{BTreeMap, HashMap, HashSet};

use doc::{Document, Geometry, Group, Layer, Object};
use kurbo::{Affine, Vec2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The format tag of [`ArtFragment`] JSON.
pub const ART_FORMAT: &str = "openetch-art";
/// Most objects one item holds.
pub const MAX_ART_OBJECTS: usize = 20_000;

/// A layer an item's objects were on: only its name and colour are kept (an Art Library stores no cut settings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtLayer {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// The content of one Art Library item (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtFragment {
    /// Always [`ART_FORMAT`].
    pub format: String,
    pub objects: Vec<Object>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub layers: Vec<ArtLayer>,
    /// Cut properties by object id (priority, power scale, start point, hand-placed tabs; `Document::object_options`):
    /// carried by a copy on the clipboard so a pasted copy keeps its tabs and start point. Art Library items keep none
    /// (libraries store art only); ids that name no object of the item are ignored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub object_options: BTreeMap<String, doc::ObjectOptions>,
}

impl ArtFragment {
    /// Parse fragment JSON (as the engine stores it).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let f: ArtFragment = serde_json::from_str(text).map_err(|e| format!("not an art item: {e}"))?;
        f.check()?;
        Ok(f)
    }

    fn check(&self) -> Result<(), String> {
        if self.format != ART_FORMAT {
            return Err(format!("not an art item (format '{}', expected '{ART_FORMAT}')", self.format));
        }
        if self.objects.is_empty() {
            return Err("the art item holds no objects".into());
        }
        if self.objects.len() > MAX_ART_OBJECTS {
            return Err(format!("the art item holds {} objects (at most {MAX_ART_OBJECTS})", self.objects.len()));
        }
        for (id, o) in &self.object_options {
            let finite = |p: &[f64; 2]| p.iter().all(|v| v.is_finite());
            let ok = (0.0..=100.0).contains(&o.power_scale_pct)
                && o.tabs.len() <= doc::MAX_MANUAL_TABS
                && o.tabs.iter().all(finite)
                && o.start.as_ref().is_none_or(|p| finite(&p.local));
            if !ok {
                return Err(format!("the cut properties of '{id}' are not usable (power scale 0..=100, at most {} tabs, finite points)", doc::MAX_MANUAL_TABS));
            }
        }
        // The rules of a loaded project (unique ids, positive sizes, names, image data, extras keys, groups that do
        // not contain themselves, usable numbers): a hand-made library file or `art.insert` content must not put into
        // a document what no project file could hold. Checked on the document `art.insert` would add, before new ids.
        doc::file::check_document(&self.as_document()).map_err(|e| format!("not a usable art item: {e}"))
    }

    /// The fragment as a document of its own: its layers (a stand-in for a layer it names but does not list), and
    /// its groups and objects linked the way `art.insert` links them (a parent or group outside the item dropped,
    /// text detached from a path outside it).
    fn as_document(&self) -> Document {
        let mut d = Document { layers: Vec::new(), ..Document::default() };
        for l in &self.layers {
            d.layers.push(Layer::new(l.id.clone(), &l.name, &l.color));
        }
        for o in &self.objects {
            if d.layer(&o.layer).is_none() {
                d.layers.push(Layer::new(o.layer.clone(), "Art", "#888888"));
            }
        }
        if d.layers.is_empty() {
            d.layers.push(Layer::new("layer-1".into(), "Art", "#888888"));
        }
        let groups: HashSet<&str> = self.groups.iter().map(|g| g.id.as_str()).collect();
        let objects: HashSet<&str> = self.objects.iter().map(|o| o.id.as_str()).collect();
        d.groups = self
            .groups
            .iter()
            .map(|g| {
                let mut n = g.clone();
                n.parent = g.parent.clone().filter(|p| groups.contains(p.as_str()) && *p != g.id);
                n
            })
            .collect();
        d.objects = self
            .objects
            .iter()
            .map(|o| {
                let mut n = o.clone();
                n.group = o.group.clone().filter(|g| groups.contains(g.as_str()));
                if let Geometry::Text(t) = &mut n.geometry
                    && t.path.as_ref().is_some_and(|p| !objects.contains(p.object.as_str()))
                {
                    t.path = None;
                }
                n
            })
            .collect();
        d
    }

    /// The bounds of every object, field mm.
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        self.objects.iter().map(Object::bounds).reduce(|a, b| a.union(b))
    }

    /// The layer colour of an object (for thumbnails).
    pub fn color_of(&self, o: &Object) -> &str {
        self.layers.iter().find(|l| l.id == o.layer).map_or("#888888", |l| l.color.as_str())
    }
}

/// The selected objects (`ids`: objects or groups) of `doc` as an Art Library item: the objects in document order,
/// every group they sit in (with its ancestors) and their layers' names and colours. Refuses an empty selection.
pub fn art_fragment(doc: &Document, ids: &[String]) -> Result<ArtFragment> {
    let ids = util::object_ids("art.add_to_library", doc, ids)?;
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let objects: Vec<Object> = doc.objects.iter().filter(|o| wanted.contains(o.id.as_str())).cloned().collect();
    if objects.is_empty() {
        return Err(CommandError::Failed("nothing selected to add to the library".into()));
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for o in &objects {
        let mut g = o.group.clone();
        let mut depth = 0;
        while let Some(id) = g {
            if !seen.insert(id.clone()) || depth > 1000 {
                break;
            }
            depth += 1;
            let Some(group) = doc.group(&id) else { break };
            groups.push(group.clone());
            g = group.parent.clone();
        }
    }
    // Groups in document order (parents first is not needed: insertion maps every id first).
    groups.sort_by_key(|g| doc.groups.iter().position(|d| d.id == g.id));
    let mut layers: Vec<ArtLayer> = Vec::new();
    for o in &objects {
        if layers.iter().any(|l| l.id == o.layer) {
            continue;
        }
        if let Some(l) = doc.layer(&o.layer) {
            layers.push(ArtLayer { id: l.id.clone(), name: l.name.clone(), color: l.color.clone() });
        }
    }
    Ok(ArtFragment { format: ART_FORMAT.into(), objects, groups, layers, object_options: BTreeMap::new() })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtInsertInput {
    /// The item: Art Library fragment JSON (`{"format": "openetch-art", "objects": [...], "groups": [...],
    /// "layers": [...]}`), as the engine's Art Library stores it.
    pub content: String,
    /// Name of the group that holds several top-level pieces (default "Art").
    pub name: Option<String>,
    /// Field point (mm) the centre of the art's bounds goes to; default [0, 0], the field centre.
    pub center: Option<[f64; 2]>,
    /// Put every object on this layer instead of matching layers by colour.
    pub layer: Option<String>,
}

/// `art.insert`.
pub struct ArtInsert;

impl Command for ArtInsert {
    type Input = ArtInsertInput;
    const ID: &'static str = "art.insert";
    const TITLE: &'static str = "Insert art";
    const DESCRIPTION: &'static str = "Insert an Art Library item (LightBurn's Add Graphic to Project / drag from the Art \
        Library): every object and group gets a new id; objects go to the layer of the same colour (a new layer of \
        that name and colour with default settings when there is none), or all to `layer`; several pieces are wrapped \
        in one group named `name`; the art's bounds are centred on `center` (field mm, default the field centre). \
        Returns created object ids, groups and layers_created.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: ArtInsertInput) -> Result<(Value, String)> {
        let frag = ArtFragment::from_json(&input.content).map_err(|e| invalid(Self::ID, e))?;
        let center = match input.center {
            Some(c) => util::finite2(Self::ID, "center", c)?,
            None => [0.0, 0.0],
        };
        let name = match &input.name {
            Some(n) => util::name(Self::ID, "name", n)?,
            None => "Art".to_string(),
        };
        let target = input.layer.as_deref().map(|l| util::layer(doc, l).map(|l| l.id.clone())).transpose()?;
        transact(doc, |doc| {
            // Layers: by colour, else a new one.
            let mut layer_map: HashMap<String, String> = HashMap::new();
            let mut layers_created = Vec::new();
            if target.is_none() {
                for l in &frag.layers {
                    let found = doc.layers.iter().find(|d| d.color.eq_ignore_ascii_case(&l.color)).map(|d| d.id.clone());
                    let id = match found {
                        Some(id) => id,
                        None => {
                            let color = util::color(Self::ID, &l.color).unwrap_or_else(|_| "#888888".into());
                            let lname = util::name(Self::ID, "name", &l.name).unwrap_or_else(|_| "Art".into());
                            let id = doc.alloc_id("layer")?;
                            doc.layers.push(Layer::new(id.clone(), &lname, &color));
                            layers_created.push(id.clone());
                            id
                        }
                    };
                    layer_map.insert(l.id.clone(), id);
                }
            }
            let fallback = match &target {
                Some(t) => t.clone(),
                None => doc.layers.first().map(|l| l.id.clone()).ok_or_else(|| CommandError::Failed("the document has no layer".into()))?,
            };
            // Groups and objects: new ids.
            let known_groups: HashSet<&str> = frag.groups.iter().map(|g| g.id.as_str()).collect();
            let mut group_map: BTreeMap<String, String> = BTreeMap::new();
            for g in &frag.groups {
                if !group_map.contains_key(&g.id) {
                    group_map.insert(g.id.clone(), doc.alloc_id("group")?);
                }
            }
            let mut object_map: HashMap<String, String> = HashMap::new();
            for o in &frag.objects {
                object_map.insert(o.id.clone(), doc.alloc_id("obj")?);
            }
            let shift = match frag.bounds() {
                Some(b) => Vec2::new(center[0] - b.center().x, center[1] - b.center().y),
                None => Vec2::ZERO,
            };
            let mut new_groups: Vec<Group> = frag
                .groups
                .iter()
                .map(|g| {
                    let mut n = g.clone();
                    n.id = group_map[&g.id].clone();
                    n.parent = g.parent.as_deref().filter(|p| known_groups.contains(p) && *p != g.id).map(|p| group_map[p].clone());
                    n
                })
                .collect();
            let mut created = Vec::new();
            let mut new_objects: Vec<Object> = Vec::new();
            for o in &frag.objects {
                let mut n = o.clone();
                n.id = object_map[&o.id].clone();
                n.layer = target.clone().or_else(|| layer_map.get(&o.layer).cloned()).unwrap_or_else(|| fallback.clone());
                n.group = o.group.as_deref().filter(|g| known_groups.contains(g)).map(|g| group_map[g].clone());
                n.transform = Affine::translate(shift) * o.transform;
                if let Geometry::Text(t) = &mut n.geometry
                    && let Some(p) = t.path.as_mut()
                {
                    match object_map.get(&p.object) {
                        Some(id) => p.object = id.clone(),
                        None => t.path = None,
                    }
                }
                created.push(n.id.clone());
                new_objects.push(n);
            }
            // Several top-level pieces: one group around them.
            let top = new_groups.iter().filter(|g| g.parent.is_none()).count()
                + new_objects.iter().filter(|o| o.group.is_none()).count();
            let mut group_ids: Vec<String> = new_groups.iter().map(|g| g.id.clone()).collect();
            if top > 1 {
                let wrap = doc.alloc_id("group")?;
                for g in new_groups.iter_mut().filter(|g| g.parent.is_none()) {
                    g.parent = Some(wrap.clone());
                }
                for o in new_objects.iter_mut().filter(|o| o.group.is_none()) {
                    o.group = Some(wrap.clone());
                }
                new_groups.push(Group::new(wrap.clone(), name.clone(), None));
                group_ids.push(wrap);
            }
            let count = new_objects.len();
            doc.groups.extend(new_groups);
            doc.objects.extend(new_objects);
            // A pasted copy keeps its cut properties (tabs and start points are in the object's own coordinates).
            for (from, opt) in &frag.object_options {
                if let Some(to) = object_map.get(from)
                    && !opt.is_default()
                {
                    doc.object_options.insert(to.clone(), opt.clone());
                }
            }
            let mut result = util::edit_result(doc, &created, &[]);
            result["groups"] = json!(group_ids);
            result["layers_created"] = json!(layers_created);
            Ok((result, format!("inserted art \"{name}\" ({count} object(s))")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: Value) -> Result<Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    #[test]
    fn a_selection_round_trips_through_an_item() {
        let mut s = Session::new(Document::default());
        let red = run(&mut s, "layer.add", json!({"name": "Red", "color": "#ff0000"})).unwrap()["id"].as_str().unwrap().to_owned();
        let a = run(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "layer": red})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let b = run(&mut s, "shape.ellipse", json!({"rx_mm": 5, "ry_mm": 5})).unwrap()["created"][0].as_str().unwrap().to_owned();
        run(&mut s, "object.transform", json!({"ids": [b], "translate": [30, 0]})).unwrap();
        let g = run(&mut s, "group.create", json!({"ids": [a, b], "name": "Pair"})).unwrap();
        let gid = g["group"].as_str().or(g["id"].as_str()).unwrap().to_owned();
        let frag = art_fragment(s.document(), &[gid]).unwrap();
        assert_eq!((frag.objects.len(), frag.groups.len(), frag.layers.len()), (2, 1, 2));
        let content = serde_json::to_string(&frag).unwrap();
        assert!(ArtFragment::from_json(&content).is_ok());

        // Into a fresh document: the red layer is created, the default layer matched by colour.
        let mut t = Session::new(Document::default());
        let r = run(&mut t, "art.insert", json!({"content": content, "center": [10, 20]})).unwrap();
        let created: Vec<String> = serde_json::from_value(r["created"].clone()).unwrap();
        assert_eq!(created.len(), 2);
        assert_eq!(r["layers_created"].as_array().unwrap().len(), 1);
        let d = t.document();
        assert_eq!(d.layers.len(), 2);
        let objs: Vec<&Object> = created.iter().map(|id| d.object(id).unwrap()).collect();
        assert_eq!(objs[0].layer, d.layers[1].id, "the red layer");
        assert_eq!(d.layers[1].color, "#ff0000");
        assert_eq!(objs[1].layer, d.layers[0].id, "matched by colour");
        // One top-level group (the copy of "Pair"): no extra wrapper.
        assert_eq!(d.groups.len(), 1);
        assert_eq!(d.groups[0].name, "Pair");
        assert!(objs.iter().all(|o| o.group.as_deref() == Some(d.groups[0].id.as_str())));
        let b = d.bounds_of(&created).unwrap();
        assert!((b.center().x - 10.0).abs() < 1e-9 && (b.center().y - 20.0).abs() < 1e-9, "{b:?}");

        // Inserted again: new ids, same layers; all on one layer when asked.
        let first = t.document().layers[0].id.clone();
        let r2 = run(&mut t, "art.insert", json!({"content": serde_json::to_string(&frag).unwrap(), "layer": first})).unwrap();
        let again: Vec<String> = serde_json::from_value(r2["created"].clone()).unwrap();
        assert!(again.iter().all(|id| !created.contains(id)));
        assert!(again.iter().all(|id| t.document().object(id).unwrap().layer == t.document().layers[0].id));
        assert_eq!(t.document().layers.len(), 2);
        // Undo removes it in one step.
        t.undo().unwrap();
        assert_eq!(t.document().objects.len(), 2);
    }

    #[test]
    fn loose_pieces_are_wrapped_and_bad_items_refused() {
        let mut s = Session::new(Document::default());
        let a = run(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let b = run(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let frag = art_fragment(s.document(), &[a, b]).unwrap();
        let content = serde_json::to_string(&frag).unwrap();
        let r = run(&mut s, "art.insert", json!({"content": content, "name": "Stars"})).unwrap();
        let groups = r["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(s.document().group(groups[0].as_str().unwrap()).unwrap().name, "Stars");
        assert!(run(&mut s, "art.insert", json!({"content": "{}"})).is_err());
        assert!(run(&mut s, "art.insert", json!({"content": json!({"format": "x", "objects": []}).to_string()})).is_err());
        assert!(run(&mut s, "art.insert", json!({"content": content, "layer": "nope"})).is_err());
        assert!(art_fragment(s.document(), &[]).is_err());
    }

    /// Every real LightBurn file's art makes a usable item (the item check is a project file's, which every
    /// imported document meets), and the item inserts again.
    #[test]
    fn real_lightburn_art_makes_usable_items() {
        let Some(files) = super::super::dxf::sample_files(&["lbrn2", "lbrn"]) else {
            println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
            return;
        };
        let mut items = 0;
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let mut s = Session::new(Document::default());
            if run(&mut s, "doc.import_lbrn2", json!({"content": text.trim_start_matches('\u{feff}'), "field_mm": 150})).is_err() {
                continue;
            }
            let ids: Vec<String> = s.document().objects.iter().map(|o| o.id.clone()).collect();
            if ids.is_empty() || ids.len() > MAX_ART_OBJECTS {
                continue;
            }
            let frag = art_fragment(s.document(), &ids).unwrap();
            let content = serde_json::to_string(&frag).unwrap();
            ArtFragment::from_json(&content).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let mut t = Session::new(Document::default());
            run(&mut t, "art.insert", json!({"content": content})).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            items += 1;
        }
        println!("{items} real LightBurn file(s) made usable art items");
    }

    /// A hand-made item (a hostile `.oeart`, or `art.insert` content from a client) is held to a project file's rules:
    /// groups inside each other in a loop, sizes that are not positive, control characters in names, image data
    /// that is not base64 and duplicate ids are refused before anything reaches the document.
    #[test]
    fn hostile_items_are_refused_like_hostile_project_files() {
        let mut s = Session::new(Document::default());
        let a = run(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let b = run(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let c = run(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let inner = run(&mut s, "group.create", json!({"ids": [a, b], "name": "In"})).unwrap();
        let inner = inner["group"].as_str().or(inner["id"].as_str()).unwrap().to_owned();
        let outer = run(&mut s, "group.create", json!({"ids": [inner, c], "name": "Out"})).unwrap();
        let outer = outer["group"].as_str().or(outer["id"].as_str()).unwrap().to_owned();
        let good = serde_json::to_value(art_fragment(s.document(), &[outer]).unwrap()).unwrap();
        assert_eq!(good["groups"].as_array().unwrap().len(), 2, "{good}");
        assert!(ArtFragment::from_json(&good.to_string()).is_ok());
        let refused = |edit: &dyn Fn(&mut Value)| {
            let mut v = good.clone();
            edit(&mut v);
            let text = v.to_string();
            let e = ArtFragment::from_json(&text).expect_err("refused");
            let before = s.document().clone();
            let mut t = Session::new(before.clone());
            assert!(run(&mut t, "art.insert", json!({"content": text})).is_err(), "{e}");
            assert_eq!(t.document().objects, before.objects);
            e
        };
        // Each group the other's parent: inserted, walking up from either would never end.
        let e = refused(&|v| {
            let ids: Vec<String> = v["groups"].as_array().unwrap().iter().map(|g| g["id"].as_str().unwrap().to_owned()).collect();
            v["groups"][0]["parent"] = json!(ids[1]);
            v["groups"][1]["parent"] = json!(ids[0]);
        });
        assert!(e.contains("inside itself"), "{e}");
        refused(&|v| v["objects"][0]["geometry"]["width_mm"] = json!(0.0));
        refused(&|v| v["objects"][0]["name"] = json!("bad\u{7}name"));
        refused(&|v| v["layers"][0]["name"] = json!("bad\u{1b}[31m"));
        refused(&|v| {
            let id = v["objects"][0]["id"].clone();
            v["objects"][1]["id"] = id;
        });
        refused(&|v| {
            v["objects"][0]["geometry"] =
                json!({"kind": "image", "width_mm": 5, "height_mm": 5, "mime": "text/html", "data_base64": "PGgxPg=="});
        });
    }
}
