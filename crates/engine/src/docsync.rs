//! Document change sets, so a client keeps its copy of the document current without fetching the whole document after
//! every edit (`GET /api/document` is megabytes for a large LightBurn import; an edit usually changes a few objects).
//!
//! The engine keeps a [`DocLog`]: the last document state a client may have seen (its revision, its top-level members
//! as JSON and a copy of its objects) and a short chain of [`ChangeSet`]s that led to it. Whenever a client is about to
//! see the document (`GET /api/document`, `GET /api/document/sync`, the `sync` member of a mutating answer), the log is
//! brought up to the current document first ([`DocLog::observe`]): the difference between the logged state and the
//! current one becomes the next change set. So every revision a client can hold is in the chain until it falls off
//! the end ([`MAX_CHANGESETS`], [`MAX_LOG_BYTES`]), however the document was changed (UI, MCP, timeline, another open
//! document becoming active).
//!
//! `GET /api/document/sync?since=<revision>&epoch=<epoch>` answers [`SyncPayload`]:
//!
//! ```json
//! { "epoch": "9f…", "document_id": "default", "revision": 42,
//!   "changes": { "from": 40, "to": 42,
//!                "set": { "revision": 42, "next_id": 230, "layers": [ … ] },
//!                "unset": [ "rotary" ],
//!                "objects": { "upsert": [ { "id": "obj-3", … } ], "remove": [ "obj-7" ], "order": [ "obj-1", … ] } },
//!   "history": { "marker": 5, "steps": [ … ] },
//!   "project": { "name": null, "display_name": null, "saved_revision": null, "revision": 42, "dirty": true } }
//! ```
//!
//! - `changes` applies to the client's document at revision `from` (= `since`): `set` replaces top-level members (as
//!   `GET /api/document` would have them), `unset` removes members the engine leaves out of JSON, `objects.upsert`
//!   replaces or adds whole objects by id, `objects.remove` drops ids (an id the client does not have is ignored),
//!   and `objects.order`, present only when the sequence of ids changed, is the complete new order of `objects`.
//! - `document` (instead of `changes`) is the whole document: the client gave no `since`, an `epoch` of another engine
//!   run, or a revision the log no longer holds (or never held: another open document, a document before a restart).
//! - `epoch` names this engine run; a client sends it back with `since`, so a revision of an earlier run (revisions
//!   start again after a restart) is never mistaken for one of this run.
//!
//! A `POST` under `/api/` with the header `X-OpenEtch-Since: <epoch>:<revision>` gets the same payload as a `sync`
//! member of its JSON answer (see `api::attach_sync`), so an edit, undo, redo, timeline move or import needs no
//! further request to show its result.
//!
//! The change sets are display data. Planning, runs and framing always use the engine's own document; nothing a
//! client sends back is derived from them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use doc::{Document, Object};
use serde::Serialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

/// Change sets kept (older ones fall off; a client that far behind gets the whole document).
pub const MAX_CHANGESETS: usize = 64;
/// JSON bytes of change sets kept (an image import can add megabytes in one step).
pub const MAX_LOG_BYTES: usize = 24 * 1024 * 1024;

/// The difference between two revisions of one document.
#[derive(Debug)]
pub struct ChangeSet {
    pub from: u64,
    pub to: u64,
    /// Top-level members (other than `objects`) that changed or appeared, as JSON.
    set: Vec<(String, Box<RawValue>)>,
    /// Top-level members that are no longer in the JSON (left out while empty or default).
    unset: Vec<String>,
    /// Objects added or changed, by id, as JSON.
    upsert: Vec<(String, Box<RawValue>)>,
    /// Ids of objects removed.
    remove: Vec<String>,
    /// The complete order of object ids, when the sequence changed.
    order: Option<Vec<String>>,
    /// JSON bytes held (for [`MAX_LOG_BYTES`]).
    bytes: usize,
}

/// The document state the log is at.
struct Base {
    document_id: String,
    revision: u64,
    /// Top-level members other than `objects`, as `GET /api/document` serialises them.
    meta: Map<String, Value>,
    objects: HashMap<String, Object>,
    order: Vec<String>,
}

/// See the module docs.
#[derive(Default)]
pub struct DocLog {
    base: Option<Base>,
    /// Contiguous: `ring[i].to == ring[i + 1].from`, and the last one ends at `base.revision`.
    ring: VecDeque<Arc<ChangeSet>>,
    bytes: usize,
}

/// The document without its objects (they are compared one by one). Destructured exhaustively, so a new member of
/// [`Document`] does not compile until it is listed here, and it is then diffed like the others.
fn meta_of(d: &Document) -> Map<String, Value> {
    let Document {
        revision,
        name,
        field_mm,
        workspace_mm,
        stock,
        layers,
        objects: _,
        groups,
        next_id,
        extras,
        locked,
        variables,
        optimization,
        object_options,
        rotary,
        notes,
        pages,
        active_page,
        image_masks,
        guides,
        curve_tolerance_mm,
        virtual_arrays,
    } = d;
    let shell = Document {
        revision: *revision,
        name: name.clone(),
        field_mm: *field_mm,
        workspace_mm: *workspace_mm,
        stock: stock.clone(),
        layers: layers.clone(),
        objects: Vec::new(),
        groups: groups.clone(),
        next_id: *next_id,
        extras: extras.clone(),
        locked: locked.clone(),
        variables: variables.clone(),
        optimization: optimization.clone(),
        object_options: object_options.clone(),
        rotary: rotary.clone(),
        notes: notes.clone(),
        pages: pages.clone(),
        active_page: active_page.clone(),
        image_masks: image_masks.clone(),
        guides: guides.clone(),
        curve_tolerance_mm: *curve_tolerance_mm,
        virtual_arrays: virtual_arrays.clone(),
    };
    match serde_json::to_value(&shell) {
        Ok(Value::Object(mut m)) => {
            m.remove("objects");
            m
        }
        // A document always serialises to an object (it is what GET /api/document sends).
        _ => Map::new(),
    }
}

fn raw<T: Serialize + ?Sized>(v: &T) -> Box<RawValue> {
    serde_json::value::to_raw_value(v).unwrap_or_else(|_| RawValue::from_string("null".into()).expect("valid JSON"))
}

/// Object ids in order, or `None` when an id repeats (no document the engine commits has one; such a document is not
/// diffed, clients get it whole).
fn ids_of(d: &Document) -> Option<Vec<String>> {
    let mut seen = HashSet::with_capacity(d.objects.len());
    d.objects.iter().map(|o| seen.insert(o.id.as_str()).then(|| o.id.clone())).collect()
}

impl Base {
    fn new(d: &Document, document_id: &str) -> Option<Self> {
        let order = ids_of(d)?;
        Some(Base {
            document_id: document_id.to_owned(),
            revision: d.revision,
            meta: meta_of(d),
            objects: d.objects.iter().map(|o| (o.id.clone(), o.clone())).collect(),
            order,
        })
    }
}

impl DocLog {
    /// Bring the log up to `d`, the active document `document_id`: a change set from the logged revision to `d`'s,
    /// or a fresh start (empty chain) for another document, an older revision or a document that cannot be diffed.
    pub fn observe(&mut self, d: &Document, document_id: &str) {
        let same_doc = self.base.as_ref().is_some_and(|b| b.document_id == document_id);
        match self.base.as_mut() {
            Some(b) if same_doc && b.revision == d.revision => {}
            Some(b) if same_doc && b.revision < d.revision => match diff(b, d) {
                Some(c) => self.push(c),
                None => self.restart(d, document_id),
            },
            _ => self.restart(d, document_id),
        }
    }

    fn restart(&mut self, d: &Document, document_id: &str) {
        self.ring.clear();
        self.bytes = 0;
        self.base = Base::new(d, document_id);
    }

    fn push(&mut self, c: ChangeSet) {
        self.bytes += c.bytes;
        self.ring.push_back(Arc::new(c));
        while self.ring.len() > MAX_CHANGESETS || (self.bytes > MAX_LOG_BYTES && self.ring.len() > 1) {
            if let Some(old) = self.ring.pop_front() {
                self.bytes -= old.bytes;
            }
        }
    }

    /// The revision the log is at (tests).
    #[cfg(test)]
    pub fn revision(&self) -> Option<u64> {
        self.base.as_ref().map(|b| b.revision)
    }

    /// The changes from revision `since` to the logged revision, or `None` when the chain does not reach back to it.
    /// Call [`DocLog::observe`] first.
    pub fn since(&self, since: u64) -> Option<Changes> {
        let b = self.base.as_ref()?;
        if since == b.revision {
            return Some(Changes { from: since, to: since, sets: Vec::new() });
        }
        let i = self.ring.iter().position(|c| c.from == since)?;
        Some(Changes { from: since, to: b.revision, sets: self.ring.iter().skip(i).cloned().collect() })
    }
}

/// The change set from `b` to `d` (and `b` moved to `d`), or `None` when `d` cannot be diffed.
fn diff(b: &mut Base, d: &Document) -> Option<ChangeSet> {
    let order = ids_of(d)?;
    let meta = meta_of(d);
    let mut bytes = 0;
    let mut set = Vec::new();
    for (k, v) in &meta {
        if b.meta.get(k) != Some(v) {
            let r = raw(v);
            bytes += r.get().len();
            set.push((k.clone(), r));
        }
    }
    let unset: Vec<String> = b.meta.keys().filter(|k| !meta.contains_key(*k)).cloned().collect();
    let mut upsert = Vec::new();
    for o in &d.objects {
        if b.objects.get(&o.id) != Some(o) {
            let r = raw(o);
            bytes += r.get().len();
            upsert.push((o.id.clone(), r));
            b.objects.insert(o.id.clone(), o.clone());
        }
    }
    let now: HashSet<&str> = order.iter().map(String::as_str).collect();
    let remove: Vec<String> = b.order.iter().filter(|id| !now.contains(id.as_str())).cloned().collect();
    for id in &remove {
        b.objects.remove(id);
    }
    let order = (order != b.order).then_some(order);
    if let Some(o) = &order {
        bytes += o.iter().map(|id| id.len() + 3).sum::<usize>();
        b.order = o.clone();
    }
    let c = ChangeSet { from: b.revision, to: d.revision, set, unset, upsert, remove, order, bytes };
    b.meta = meta;
    b.revision = d.revision;
    Some(c)
}

/// The change sets from one revision to another, merged when serialised.
#[derive(Debug, Clone)]
pub struct Changes {
    pub from: u64,
    pub to: u64,
    sets: Vec<Arc<ChangeSet>>,
}

/// Members as an object, their values spliced in as they are.
struct RawMap<'a>(Vec<(&'a str, &'a RawValue)>);

impl Serialize for RawMap<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter().map(|(k, v)| (*k, *v)))
    }
}

#[derive(Serialize)]
struct ObjectsView<'a> {
    upsert: Vec<&'a RawValue>,
    remove: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<&'a [String]>,
}

#[derive(Serialize)]
struct ChangesView<'a> {
    from: u64,
    to: u64,
    set: RawMap<'a>,
    unset: Vec<&'a str>,
    objects: ObjectsView<'a>,
}

impl Serialize for Changes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // Later change sets win: a member set and then left out is unset, an object changed twice is sent once (in its
        // last form, at the place it was first changed), an object removed and added back is an upsert.
        let mut set: Vec<(&str, &RawValue)> = Vec::new();
        let mut set_at: HashMap<&str, usize> = HashMap::new();
        let mut unset: Vec<&str> = Vec::new();
        let mut upsert: Vec<(&str, &RawValue)> = Vec::new();
        let mut upsert_at: HashMap<&str, usize> = HashMap::new();
        let mut remove: Vec<&str> = Vec::new();
        let mut order: Option<&[String]> = None;
        for c in &self.sets {
            for (k, v) in &c.set {
                unset.retain(|u| u != k);
                match set_at.get(k.as_str()) {
                    Some(&i) => set[i].1 = v,
                    None => {
                        set_at.insert(k, set.len());
                        set.push((k, v));
                    }
                }
            }
            for k in &c.unset {
                if let Some(i) = set_at.remove(k.as_str()) {
                    set[i].0 = "";
                }
                if !unset.contains(&k.as_str()) {
                    unset.push(k);
                }
            }
            for id in &c.remove {
                if let Some(i) = upsert_at.remove(id.as_str()) {
                    upsert[i].0 = "";
                }
                if !remove.contains(&id.as_str()) {
                    remove.push(id);
                }
            }
            for (id, v) in &c.upsert {
                remove.retain(|r| r != id);
                match upsert_at.get(id.as_str()) {
                    Some(&i) => upsert[i].1 = v,
                    None => {
                        upsert_at.insert(id, upsert.len());
                        upsert.push((id, v));
                    }
                }
            }
            if let Some(o) = &c.order {
                order = Some(o);
            }
        }
        let members = RawMap(set.into_iter().filter(|(k, _)| !k.is_empty()).collect());
        ChangesView {
            from: self.from,
            to: self.to,
            set: members,
            unset,
            objects: ObjectsView {
                upsert: upsert.into_iter().filter(|(id, _)| !id.is_empty()).map(|(_, v)| v).collect(),
                remove,
                order,
            },
        }
        .serialize(s)
    }
}

/// `GET /api/document/sync`'s answer (and the `sync` member of mutating answers). See the module docs.
#[derive(Serialize)]
pub struct SyncPayload {
    pub epoch: Arc<str>,
    pub document_id: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Changes>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<Document>,
    pub history: commands::HistoryView,
    pub project: Value,
}

/// Parse `X-OpenEtch-Since` / the `since` and `epoch` query: `<epoch>:<revision>`.
pub fn parse_since(v: &str) -> Option<(String, u64)> {
    let (epoch, rev) = v.trim().rsplit_once(':')?;
    Some((epoch.to_owned(), rev.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::{Geometry, Layer};
    use kurbo::Affine;

    fn rect(id: &str, x: f64) -> Object {
        let g = Geometry::Rect { width_mm: 10.0, height_mm: 10.0, corner_radius_mm: 0.0 };
        Object::new(id.into(), id, "layer-1".into(), g, Affine::translate((x, 0.0)))
    }

    /// The client's side: apply serialised changes to a document's JSON (the web app does the same in TypeScript).
    fn apply(doc: &Value, changes: &Value) -> Value {
        let mut out = doc.as_object().unwrap().clone();
        assert_eq!(out["revision"], changes["from"]);
        for k in changes["unset"].as_array().unwrap() {
            out.remove(k.as_str().unwrap());
        }
        for (k, v) in changes["set"].as_object().unwrap() {
            out.insert(k.clone(), v.clone());
        }
        let mut by_id: HashMap<String, Value> =
            doc["objects"].as_array().unwrap().iter().map(|o| (o["id"].as_str().unwrap().to_owned(), o.clone())).collect();
        for id in changes["objects"]["remove"].as_array().unwrap() {
            by_id.remove(id.as_str().unwrap());
        }
        for o in changes["objects"]["upsert"].as_array().unwrap() {
            by_id.insert(o["id"].as_str().unwrap().to_owned(), o.clone());
        }
        let order: Vec<String> = match changes["objects"].get("order") {
            Some(o) => o.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect(),
            None => doc["objects"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap().to_owned()).collect(),
        };
        let objects: Vec<Value> = order.iter().map(|id| by_id[id].clone()).collect();
        out.insert("objects".into(), Value::Array(objects));
        Value::Object(out)
    }

    fn json(d: &Document) -> Value {
        serde_json::to_value(d).unwrap()
    }

    #[test]
    fn changes_rebuild_the_document_across_merged_steps() {
        let mut log = DocLog::default();
        let mut d = Document { objects: vec![rect("obj-1", 0.0), rect("obj-2", 10.0), rect("obj-3", 20.0)], ..Document::default() };
        log.observe(&d, "default");
        let v0 = json(&d);

        // Move one object, add one, remove one, set a top-level member, reorder.
        d.revision = 1;
        d.objects[1].transform = Affine::translate((11.0, 1.0));
        d.objects.push(rect("obj-4", 30.0));
        d.rotary = Some(doc::Rotary::default());
        log.observe(&d, "default");
        let v1 = json(&d);
        d.revision = 2;
        d.objects.remove(0);
        d.objects.swap(0, 1);
        d.layers.push(Layer::new("layer-9".into(), "Nine", "#00ff00"));
        log.observe(&d, "default");
        let v2 = json(&d);
        d.revision = 3;
        d.rotary = None;
        d.objects.push(rect("obj-1", 5.0));
        log.observe(&d, "default");
        let v3 = json(&d);
        assert_eq!(log.revision(), Some(3));

        let ser = |c: Changes| serde_json::to_value(c).unwrap();
        assert_eq!(apply(&v0, &ser(log.since(0).unwrap())), v3);
        assert_eq!(apply(&v1, &ser(log.since(1).unwrap())), v3);
        assert_eq!(apply(&v2, &ser(log.since(2).unwrap())), v3);
        let none = ser(log.since(3).unwrap());
        assert_eq!(apply(&v3, &none), v3);
        assert!(none["objects"]["upsert"].as_array().unwrap().is_empty() && none["set"].as_object().unwrap().is_empty());
        // Only what changed travels: one step back sends the re-added object, no unchanged ones.
        let last = ser(log.since(2).unwrap());
        assert_eq!(last["objects"]["upsert"].as_array().unwrap().len(), 1);
        assert_eq!(last["unset"], serde_json::json!(["rotary"]));
        // An unknown revision cannot be served from the chain.
        assert!(log.since(7).is_none());
    }

    #[test]
    fn unchanged_order_is_left_out_and_moves_send_one_object() {
        let mut log = DocLog::default();
        let mut d = Document { objects: (0..50).map(|i| rect(&format!("obj-{i}"), f64::from(i))).collect(), ..Document::default() };
        log.observe(&d, "default");
        d.revision = 1;
        d.objects[7].transform = Affine::translate((99.0, 0.0));
        log.observe(&d, "default");
        let c = serde_json::to_value(log.since(0).unwrap()).unwrap();
        assert!(c["objects"].get("order").is_none());
        assert_eq!(c["objects"]["upsert"].as_array().unwrap().len(), 1);
        assert_eq!(c["set"].as_object().unwrap().keys().collect::<Vec<_>>(), ["revision"]);
    }

    #[test]
    fn another_document_or_an_older_revision_starts_the_chain_again() {
        let mut log = DocLog::default();
        let mut d = Document::default();
        log.observe(&d, "default");
        d.revision = 5;
        log.observe(&d, "default");
        assert!(log.since(0).is_some());
        d.revision = 9;
        log.observe(&d, "doc-2");
        assert!(log.since(5).is_none(), "another document's revisions never chain");
        assert!(log.since(9).is_some());
        d.revision = 3;
        log.observe(&d, "doc-2");
        assert_eq!(log.revision(), Some(3));
        assert!(log.since(9).is_none());
    }

    #[test]
    fn the_chain_is_bounded() {
        let mut log = DocLog::default();
        let mut d = Document { objects: vec![rect("obj-1", 0.0)], ..Document::default() };
        log.observe(&d, "default");
        for r in 1..=(MAX_CHANGESETS as u64 + 10) {
            d.revision = r;
            d.objects[0].transform = Affine::translate((r as f64, 0.0));
            log.observe(&d, "default");
        }
        assert!(log.since(0).is_none());
        assert!(log.since(20).is_some());
    }

    #[test]
    fn since_parses() {
        assert_eq!(parse_since("ab12:42"), Some(("ab12".into(), 42)));
        assert_eq!(parse_since(" ab12:7 "), Some(("ab12".into(), 7)));
        assert_eq!(parse_since("ab12"), None);
        assert_eq!(parse_since("ab12:x"), None);
    }
}
