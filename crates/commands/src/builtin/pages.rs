//! Pages (LightBurn 2.2's Pages window) and project notes (File > Show Notes).
//!
//! A document with pages keeps the active page's shapes in its own `objects`/`groups` (so every other command,
//! previews and jobs work on the active page only) and the other pages' shapes in `Document::pages`. Layers and cut
//! settings are shared by all pages, as in LightBurn. A document without `pages` is one implicit page ("Page 1");
//! the first page command makes it explicit.

use doc::{Document, Group, Object, ObjectId, Page};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{CommandError, Command, Result, Risk};

/// Make the implicit single page explicit (`pages` empty → one page holding nothing, active).
fn ensure_pages(doc: &mut Document) -> Result<()> {
    if doc.pages.is_empty() {
        let id = doc.alloc_id("page")?;
        doc.pages.push(Page {
            id: id.clone(),
            name: "Page 1".into(),
            objects: vec![],
            groups: vec![],
            locked: Default::default(),
            object_options: Default::default(),
            image_masks: Default::default(),
        });
        doc.active_page = Some(id);
    }
    if doc.active_page.as_ref().is_none_or(|a| !doc.pages.iter().any(|p| &p.id == a)) {
        doc.active_page = Some(doc.pages[0].id.clone());
    }
    Ok(())
}

fn index_of(cmd: &str, doc: &Document, page: &str) -> Result<usize> {
    doc.pages.iter().position(|p| p.id == page || p.name == page).ok_or_else(|| CommandError::NotFound {
        message: format!("{cmd}: no page {page}"),
        candidates: doc.pages.iter().map(|p| format!("{} ({})", p.id, p.name)).collect(),
    })
}

fn active_index(doc: &Document) -> usize {
    doc.active_page.as_ref().and_then(|a| doc.pages.iter().position(|p| &p.id == a)).unwrap_or(0)
}

/// Make page `k` active: the active page's shapes go into its entry, page `k`'s come out of it.
fn switch_to(doc: &mut Document, k: usize) {
    let a = active_index(doc);
    if a == k {
        return;
    }
    let cur = &mut doc.pages[a];
    cur.objects = std::mem::take(&mut doc.objects);
    cur.groups = std::mem::take(&mut doc.groups);
    cur.locked = std::mem::take(&mut doc.locked);
    cur.object_options = std::mem::take(&mut doc.object_options);
    cur.image_masks = std::mem::take(&mut doc.image_masks);
    let next = &mut doc.pages[k];
    doc.objects = std::mem::take(&mut next.objects);
    doc.groups = std::mem::take(&mut next.groups);
    doc.locked = std::mem::take(&mut next.locked);
    doc.object_options = std::mem::take(&mut next.object_options);
    doc.image_masks = std::mem::take(&mut next.image_masks);
    doc.active_page = Some(next.id.clone());
}

fn pages_json(doc: &Document) -> Value {
    if doc.pages.is_empty() {
        return json!({ "active": null, "pages": [{ "id": null, "name": "Page 1", "objects": doc.objects.len(), "active": true }] });
    }
    let a = active_index(doc);
    let pages: Vec<Value> = doc
        .pages
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let n = if k == a { doc.objects.len() } else { p.objects.len() };
            json!({ "id": p.id, "name": p.name, "objects": n, "active": k == a })
        })
        .collect();
    json!({ "active": doc.pages[a].id, "pages": pages })
}

fn page_name(cmd: &str, s: &str) -> Result<String> {
    util::name(cmd, "name", s)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageListInput {}

/// `page.list`.
pub struct PageList;

impl Command for PageList {
    type Input = PageListInput;
    const ID: &'static str = "page.list";
    const TITLE: &'static str = "List pages";
    const DESCRIPTION: &'static str = "The document's pages in order (LightBurn 2.2's Pages window): {active, pages: \
        [{id, name, objects, active}]}. A document without pages is one page (id null). Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _: PageListInput) -> Result<(Value, String)> {
        Ok((pages_json(doc), String::new()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageAddInput {
    /// Default "Page <n>".
    pub name: Option<String>,
    /// 0-based position (default after the active page).
    pub index: Option<usize>,
    /// Make it the active page (default true).
    pub activate: Option<bool>,
}

/// `page.add`.
pub struct PageAdd;

impl Command for PageAdd {
    type Input = PageAddInput;
    const ID: &'static str = "page.add";
    const TITLE: &'static str = "Add page";
    const DESCRIPTION: &'static str = "Add a blank page (LightBurn's Pages > Add); layers and cut settings are shared \
        by all pages. Jobs, previews and every shape command work on the active page. name, index, activate (default \
        true). Returns {id, pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageAddInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        transact(doc, |doc| {
            ensure_pages(doc)?;
            let name = match &i.name {
                Some(n) => page_name(cmd, n)?,
                None => format!("Page {}", doc.pages.len() + 1),
            };
            let at = i.index.unwrap_or(active_index(doc) + 1);
            if at > doc.pages.len() {
                return Err(invalid(cmd, format!("index {at} is past the end ({} pages)", doc.pages.len())));
            }
            let id = doc.alloc_id("page")?;
            doc.pages.insert(
                at,
                Page {
                    id: id.clone(),
                    name: name.clone(),
                    objects: vec![],
                    groups: vec![],
                    locked: Default::default(),
                    object_options: Default::default(),
                    image_masks: Default::default(),
                },
            );
            if i.activate.unwrap_or(true) {
                switch_to(doc, at);
            }
            let mut r = pages_json(doc);
            r["id"] = json!(id);
            r["created"] = json!([]);
            r["affected"] = json!([]);
            Ok((r, format!("added page \"{name}\"")))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageRefInput {
    /// Page id (or name).
    pub page: Option<String>,
    /// page.rename: the new name.
    pub name: Option<String>,
    /// page.move: the new 0-based position.
    pub to_index: Option<usize>,
}

/// Copies of `objects` and `groups` with new ids (group links remapped).
fn copy_shapes(doc: &mut Document, objects: &[Object], groups: &[Group]) -> Result<(Vec<Object>, Vec<Group>)> {
    let mut map = std::collections::HashMap::new();
    let mut gs = Vec::with_capacity(groups.len());
    for g in groups {
        let id = doc.alloc_id("group")?;
        map.insert(g.id.clone(), id.clone());
        gs.push(Group { id, ..g.clone() });
    }
    for g in &mut gs {
        g.parent = g.parent.as_ref().and_then(|p| map.get(p).cloned());
    }
    let mut os = Vec::with_capacity(objects.len());
    for o in objects {
        let id = doc.alloc_id("obj")?;
        os.push(Object { id, group: o.group.as_ref().and_then(|g| map.get(g).cloned()), ..o.clone() });
    }
    Ok((os, gs))
}

/// `page.duplicate`.
pub struct PageDuplicate;

impl Command for PageDuplicate {
    type Input = PageRefInput;
    const ID: &'static str = "page.duplicate";
    const TITLE: &'static str = "Duplicate page";
    const DESCRIPTION: &'static str = "A new page right after `page` (default the active one) holding copies of all its \
        objects (new ids), made active (LightBurn's Pages > Duplicate). Returns {id, pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageRefInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        transact(doc, |doc| {
            ensure_pages(doc)?;
            let k = match &i.page {
                Some(p) => index_of(cmd, doc, p)?,
                None => active_index(doc),
            };
            let (objects, groups) =
                if k == active_index(doc) { (doc.objects.clone(), doc.groups.clone()) } else { (doc.pages[k].objects.clone(), doc.pages[k].groups.clone()) };
            let (objects, groups) = copy_shapes(doc, &objects, &groups)?;
            let name = format!("{} copy", doc.pages[k].name);
            let id = doc.alloc_id("page")?;
            doc.pages.insert(
                k + 1,
                Page { id: id.clone(), name: name.clone(), objects, groups, locked: Default::default(), object_options: Default::default(), image_masks: Default::default() },
            );
            switch_to(doc, k + 1);
            let mut r = pages_json(doc);
            r["id"] = json!(id);
            r["created"] = json!(doc.objects.iter().map(|o| o.id.clone()).collect::<Vec<ObjectId>>());
            r["affected"] = json!([]);
            Ok((r, format!("duplicated page as \"{name}\"")))
        })
    }
}

/// `page.delete`.
pub struct PageDelete;

impl Command for PageDelete {
    type Input = PageRefInput;
    const ID: &'static str = "page.delete";
    const TITLE: &'static str = "Delete page";
    const DESCRIPTION: &'static str = "Delete `page` (default the active one) and its objects; the last page cannot be \
        deleted. When the active page goes, the next one (else the previous) becomes active. Returns {pages}. \
        Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageRefInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        transact(doc, |doc| {
            ensure_pages(doc)?;
            if doc.pages.len() < 2 {
                return Err(invalid(cmd, "the last page cannot be deleted"));
            }
            let k = match &i.page {
                Some(p) => index_of(cmd, doc, p)?,
                None => active_index(doc),
            };
            if k == active_index(doc) {
                switch_to(doc, if k + 1 < doc.pages.len() { k + 1 } else { k - 1 });
            }
            let gone = doc.pages.remove(k);
            let mut r = pages_json(doc);
            r["created"] = json!([]);
            r["affected"] = json!([]);
            Ok((r, format!("deleted page \"{}\"", gone.name)))
        })
    }
}

/// `page.rename`.
pub struct PageRename;

impl Command for PageRename {
    type Input = PageRefInput;
    const ID: &'static str = "page.rename";
    const TITLE: &'static str = "Rename page";
    const DESCRIPTION: &'static str = "Rename `page` (default the active one) to `name`. Returns {pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageRefInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let name = page_name(cmd, i.name.as_deref().ok_or_else(|| invalid(cmd, "name is required"))?)?;
        transact(doc, |doc| {
            ensure_pages(doc)?;
            let k = match &i.page {
                Some(p) => index_of(cmd, doc, p)?,
                None => active_index(doc),
            };
            doc.pages[k].name = name.clone();
            let mut r = pages_json(doc);
            r["created"] = json!([]);
            r["affected"] = json!([]);
            Ok((r, format!("renamed a page to \"{name}\"")))
        })
    }
}

/// `page.activate`.
pub struct PageActivate;

impl Command for PageActivate {
    type Input = PageRefInput;
    const ID: &'static str = "page.activate";
    const TITLE: &'static str = "Show page";
    const DESCRIPTION: &'static str = "Make `page` the active page: the canvas, previews, jobs and shape commands then \
        work on its objects. Returns {pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageRefInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let page = i.page.as_deref().ok_or_else(|| invalid(cmd, "page is required"))?;
        if doc.pages.is_empty() {
            return Err(invalid(cmd, "the document has one page only"));
        }
        let k = index_of(cmd, doc, page)?;
        if k == active_index(doc) {
            return Err(invalid(cmd, format!("{page} is already the active page")));
        }
        switch_to(doc, k);
        let mut r = pages_json(doc);
        r["created"] = json!([]);
        r["affected"] = json!([]);
        Ok((r, format!("showed page \"{}\"", doc.pages[k].name)))
    }
}

/// `page.move`.
pub struct PageMove;

impl Command for PageMove {
    type Input = PageRefInput;
    const ID: &'static str = "page.move";
    const TITLE: &'static str = "Move page";
    const DESCRIPTION: &'static str = "Move `page` (default the active one) to the 0-based `to_index` in the page \
        order. Returns {pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PageRefInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let to = i.to_index.ok_or_else(|| invalid(cmd, "to_index is required"))?;
        if doc.pages.is_empty() {
            return Err(invalid(cmd, "the document has one page only"));
        }
        let k = match &i.page {
            Some(p) => index_of(cmd, doc, p)?,
            None => active_index(doc),
        };
        if to >= doc.pages.len() || to == k {
            return Err(invalid(cmd, format!("to_index must be another position below {}", doc.pages.len())));
        }
        let p = doc.pages.remove(k);
        doc.pages.insert(to, p);
        let mut r = pages_json(doc);
        r["created"] = json!([]);
        r["affected"] = json!([]);
        Ok((r, "moved a page".to_string()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendInput {
    /// Objects or groups of the active page.
    pub ids: Vec<ObjectId>,
    /// The page to move them to (id or name).
    pub page: String,
}

/// `page.send_objects`.
pub struct PageSendObjects;

impl Command for PageSendObjects {
    type Input = SendInput;
    const ID: &'static str = "page.send_objects";
    const TITLE: &'static str = "Move objects to page";
    const DESCRIPTION: &'static str = "Move objects (or groups) of the active page to another page, keeping their ids, \
        groups, lock and cut properties. Returns {affected, pages}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SendInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let ids = util::object_ids(cmd, doc, &i.ids)?;
        util::check_unlocked(cmd, doc, &ids)?;
        transact(doc, |doc| {
            ensure_pages(doc)?;
            let k = index_of(cmd, doc, &i.page)?;
            if k == active_index(doc) {
                return Err(invalid(cmd, "the objects are already on that page"));
            }
            send_to(doc, &ids, k);
            let mut r = pages_json(doc);
            r["created"] = json!([]);
            r["affected"] = json!(ids);
            Ok((r, format!("moved {} object(s) to page {}", ids.len(), i.page)))
        })
    }
}

/// A new page for `art.nest`'s Multipage: added after the last page (named "Page <n>", not activated) with the
/// active page's objects `ids` moved onto it. Returns its id and name.
pub(super) fn nest_sheet(doc: &mut Document, ids: &[ObjectId]) -> Result<(String, String)> {
    ensure_pages(doc)?;
    let id = doc.alloc_id("page")?;
    let name = format!("Page {}", doc.pages.len() + 1);
    doc.pages.push(Page {
        id: id.clone(),
        name: name.clone(),
        objects: vec![],
        groups: vec![],
        locked: Default::default(),
        object_options: Default::default(),
        image_masks: Default::default(),
    });
    send_to(doc, ids, doc.pages.len() - 1);
    Ok((id, name))
}

/// Move the active page's objects `ids` (with their groups, lock, cut properties and masks) onto page `k` (not the
/// active one).
fn send_to(doc: &mut Document, ids: &[ObjectId], k: usize) {
    let (moved, kept): (Vec<Object>, Vec<Object>) = std::mem::take(&mut doc.objects).into_iter().partition(|o| ids.contains(&o.id));
    doc.objects = kept;
    // The groups the moved objects belong to (with their parents) go along.
    let mut needed: Vec<String> = Vec::new();
    for o in &moved {
        if let Some(g) = &o.group {
            for c in doc.group_chain(g) {
                if !needed.contains(&c.id) {
                    needed.push(c.id.clone());
                }
            }
        }
    }
    let groups: Vec<Group> = doc.groups.iter().filter(|g| needed.contains(&g.id)).cloned().collect();
    let page = &mut doc.pages[k];
    for g in groups {
        if !page.groups.iter().any(|x| x.id == g.id) {
            page.groups.push(g);
        }
    }
    for o in &moved {
        if let Some(v) = doc.object_options.remove(&o.id) {
            page.object_options.insert(o.id.clone(), v);
        }
        if doc.locked.remove(&o.id) {
            page.locked.insert(o.id.clone());
        }
        if let Some(m) = doc.image_masks.remove(&o.id) {
            page.image_masks.insert(o.id.clone(), m);
        }
    }
    page.objects.extend(moved);
    doc.prune_groups();
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NotesInput {
    /// The notes text (default: unchanged).
    pub text: Option<String>,
    /// Show the notes when the project is opened (LightBurn's "Show on File Open"; default: unchanged).
    pub show_on_load: Option<bool>,
}

/// `doc.set_notes`.
pub struct SetNotes;

impl Command for SetNotes {
    type Input = NotesInput;
    const ID: &'static str = "doc.set_notes";
    const TITLE: &'static str = "Project notes";
    const DESCRIPTION: &'static str = "Set the project notes (LightBurn's File > Show Notes, Ctrl+Alt+N): text and \
        show_on_load (show them when the project opens). Saved in the project and in LightBurn exports (the Notes \
        element). Returns {notes}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NotesInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let mut n = doc.notes.clone();
        if let Some(t) = i.text {
            if t.len() > 1_000_000 {
                return Err(invalid(cmd, "the notes are longer than 1 MB"));
            }
            n.text = t.replace('\0', "");
        }
        if let Some(s) = i.show_on_load {
            n.show_on_load = s;
        }
        if n == doc.notes {
            return Err(CommandError::NothingTo("change"));
        }
        doc.notes = n;
        Ok((json!({ "notes": doc.notes, "created": [], "affected": [], "bounds": null }), "changed the project notes".into()))
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::{Value, json};

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: Value) -> Value {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).unwrap_or_else(|e| panic!("{id}: {e}")).result
    }

    #[test]
    fn pages_hold_their_own_objects_and_jobs_see_the_active_one() {
        let mut s = Session::new(Document::default());
        run(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}));
        let l = run(&mut s, "page.list", json!({}));
        assert_eq!(l["pages"][0]["objects"], 1);
        let p2 = run(&mut s, "page.add", json!({"name": "Back"}));
        assert_eq!(p2["pages"].as_array().unwrap().len(), 2);
        assert!(s.document().objects.is_empty(), "a new page is blank and active");
        run(&mut s, "shape.ellipse", json!({"rx_mm": 3, "ry_mm": 3}));
        let first = s.document().pages[0].id.clone();
        run(&mut s, "page.activate", json!({"page": first}));
        assert_eq!(s.document().objects[0].id, "obj-2");
        // A LightBurn export holds the active page only and says so.
        let lb = run(&mut s, "doc.export_lbrn2", json!({"field_mm": 100}));
        assert!(lb["warnings"].to_string().contains("1 other page(s)"), "{}", lb["warnings"]);
        // Duplicate, rename, move, delete.
        let d = run(&mut s, "page.duplicate", json!({}));
        assert_eq!(d["created"].as_array().unwrap().len(), 1);
        assert_ne!(s.document().objects[0].id, "obj-2");
        run(&mut s, "page.rename", json!({"name": "Front copy 2"}));
        run(&mut s, "page.move", json!({"to_index": 0}));
        assert_eq!(s.document().pages[0].name, "Front copy 2");
        run(&mut s, "page.delete", json!({}));
        assert_eq!(s.document().pages.len(), 2);
        // Send the active page's rectangle to "Back".
        let active = run(&mut s, "page.list", json!({}));
        assert_eq!(active["pages"][0]["active"], true);
        run(&mut s, "page.send_objects", json!({"ids": ["obj-2"], "page": "Back"}));
        assert!(s.document().objects.is_empty());
        assert_eq!(s.document().pages[1].objects.len(), 2);
        // The file keeps it all.
        let back = doc::file::from_openetch_json(&doc::file::to_openetch_json(s.document())).unwrap();
        assert_eq!(back.pages, s.document().pages);
        assert!(back.next_id > 3);
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 1);
    }

    #[test]
    fn notes() {
        let mut s = Session::new(Document::default());
        let r = run(&mut s, "doc.set_notes", json!({"text": "Slate, 2 passes", "show_on_load": true}));
        assert_eq!(r["notes"]["show_on_load"], true);
        let lb = run(&mut s, "doc.export_lbrn2", json!({"field_mm": 100}));
        assert!(lb["content"].as_str().unwrap().contains("Slate, 2 passes"));
        let mut t = Session::new(Document::default());
        run(&mut t, "doc.import_lbrn2", json!({"content": lb["content"], "field_mm": 100}));
        assert_eq!(t.document().notes.text, "Slate, 2 passes");
        assert!(t.document().notes.show_on_load);
        assert!(s.execute(Invocation { id: "doc.set_notes".into(), params: json!({"show_on_load": true}) }, Origin::Ui).is_err());
    }
}
