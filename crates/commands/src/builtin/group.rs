//! Groups, as in LightBurn: `group.create` (Ctrl+G), `group.ungroup` (Ctrl+U, one level) and `group.rename`. A group
//! selects and moves as one unit; groups nest. Objects point at their innermost group (`Object::group`), groups at
//! their parent (`Group::parent`). A group never stays empty: removing its last member removes it.

use std::collections::BTreeSet;

use doc::{Document, Group, ObjectId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// Exact object and/or group ids to group (at least two items). A listed group becomes a nested group of the new
    /// one; an object or group already inside another listed group is covered by it.
    pub ids: Vec<String>,
    /// Display name. Default "Group <n>".
    pub name: Option<String>,
}

/// `group.create`.
pub struct GroupCreate;

/// An object or a group, by id.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    Object(ObjectId),
    Group(String),
}

/// A free "Group <n>" name.
fn next_group_name(doc: &Document) -> String {
    let used: BTreeSet<&str> = doc.groups.iter().map(|g| g.name.as_str()).collect();
    (doc.groups.len() + 1..).map(|n| format!("Group {n}")).find(|n| !used.contains(n.as_str())).unwrap_or_default()
}

/// Look up a group by exact id, or fail with close matches among the groups.
fn group<'a>(cmd: &str, doc: &'a Document, id: &str) -> Result<&'a Group> {
    doc.group(id).ok_or_else(|| {
        let known: Vec<String> = doc.groups.iter().map(|g| format!("{} (\"{}\")", g.id, g.name)).take(10).collect();
        let message = if known.is_empty() {
            format!("{cmd}: no group with id '{id}'; the document has no groups")
        } else {
            format!("{cmd}: no group with id '{id}'; groups: {}", known.join(", "))
        };
        CommandError::NotFound { message, candidates: doc.groups.iter().map(|g| g.id.clone()).take(10).collect() }
    })
}

impl Command for GroupCreate {
    type Input = CreateInput;
    const ID: &'static str = "group.create";
    const TITLE: &'static str = "Group";
    const DESCRIPTION: &'static str = "Group objects and/or groups (LightBurn Ctrl+G) so they select and move as one \
        unit. ids lists at least two objects or groups; listed groups are nested in the new group. The new group \
        sits where its items sat when they shared a parent group, else at the top level. Returns {group, affected \
        (every object now in it), bounds}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CreateInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let name = i.name.as_deref().map(|n| util::name(id, "name", n)).transpose()?;
        transact(doc, |doc| {
            let mut items: Vec<Item> = Vec::new();
            for x in &i.ids {
                let item = if doc.object(x).is_some() {
                    Item::Object(x.clone())
                } else if doc.group(x).is_some() {
                    Item::Group(x.clone())
                } else {
                    // The lookup error names close matches among objects and groups.
                    let e = util::object_ids(id, doc, std::slice::from_ref(x)).err();
                    return Err(e.unwrap_or_else(|| invalid(id, format!("no object or group with id '{x}'"))));
                };
                if !items.contains(&item) {
                    items.push(item);
                }
            }
            let groups: Vec<String> =
                items.iter().filter_map(|i| if let Item::Group(g) = i { Some(g.clone()) } else { None }).collect();
            // Leave out items inside another listed group: that group carries them.
            let within_listed = |g: Option<&str>, own: Option<&str>| {
                g.is_some_and(|g| groups.iter().any(|l| Some(l.as_str()) != own && doc.group_is_within(g, l)))
            };
            items.retain(|item| match item {
                Item::Object(o) => !within_listed(doc.object(o).and_then(|o| o.group.as_deref()), None),
                Item::Group(g) => !within_listed(doc.group(g).and_then(|g| g.parent.as_deref()), Some(g)),
            });
            if items.len() < 2 {
                return Err(invalid(id, "ids must name at least two objects or groups that are not inside each other"));
            }
            let parent_of = |item: &Item| match item {
                Item::Object(o) => doc.object(o).and_then(|o| o.group.clone()),
                Item::Group(g) => doc.group(g).and_then(|g| g.parent.clone()),
            };
            let first = parent_of(&items[0]);
            let parent = if items.iter().all(|i| parent_of(i) == first) { first } else { None };
            let gid = doc.alloc_id("group")?;
            let name = name.unwrap_or_else(|| next_group_name(doc));
            doc.groups.push(Group::new(gid.clone(), name.clone(), parent));
            for item in &items {
                match item {
                    Item::Object(o) => {
                        if let Some(o) = doc.object_mut(o) {
                            o.group = Some(gid.clone());
                        }
                    }
                    Item::Group(g) => {
                        if let Some(g) = doc.group_mut(g) {
                            g.parent = Some(gid.clone());
                        }
                    }
                }
            }
            doc.prune_groups();
            let members = doc.group_members(&gid);
            let mut r = util::edit_result(doc, &[], &members);
            r["group"] = json!(gid);
            r["name"] = json!(name);
            Ok((r, format!("grouped {} item(s) as \"{name}\"", items.len())))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UngroupInput {
    /// Exact id of the group to dissolve.
    pub group: String,
}

/// `group.ungroup`.
pub struct GroupUngroup;

impl Command for GroupUngroup {
    type Input = UngroupInput;
    const ID: &'static str = "group.ungroup";
    const TITLE: &'static str = "Ungroup";
    const DESCRIPTION: &'static str = "Dissolve one group level (LightBurn Ctrl+U): its objects and nested groups move \
        up to the group's parent (or the top level); nested groups stay grouped. Returns {affected (the objects that \
        were in it), groups (the nested groups now one level up), bounds}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: UngroupInput) -> Result<(Value, String)> {
        let id = Self::ID;
        transact(doc, |doc| {
            let g = group(id, doc, &i.group)?.clone();
            let members = doc.group_members(&g.id);
            for o in doc.objects.iter_mut().filter(|o| o.group.as_deref() == Some(g.id.as_str())) {
                o.group = g.parent.clone();
            }
            let mut children = Vec::new();
            for c in doc.groups.iter_mut().filter(|c| c.parent.as_deref() == Some(g.id.as_str())) {
                c.parent = g.parent.clone();
                children.push(c.id.clone());
            }
            doc.groups.retain(|x| x.id != g.id);
            doc.prune_groups();
            let mut r = util::edit_result(doc, &[], &members);
            r["groups"] = json!(children);
            Ok((r, format!("ungrouped \"{}\"", g.name)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameInput {
    /// Exact group id.
    pub group: String,
    /// New display name.
    pub name: String,
}

/// `group.rename`.
pub struct GroupRename;

impl Command for GroupRename {
    type Input = RenameInput;
    const ID: &'static str = "group.rename";
    const TITLE: &'static str = "Rename group";
    const DESCRIPTION: &'static str = "Rename a group. Returns {group, name}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RenameInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let name = util::name(id, "name", &i.name)?;
        group(id, doc, &i.group)?;
        let g = doc.group_mut(&i.group).expect("checked above");
        let old = std::mem::replace(&mut g.name, name.clone());
        Ok((json!({ "group": i.group, "name": name, "created": [], "affected": [] }), format!("renamed group \"{old}\" to \"{name}\"")))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::{CommandError, Invocation, Origin, Session};
    use doc::Document;

    fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, CommandError> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn ok(s: &mut Session, id: &str, params: Value) -> Value {
        run(s, id, params).unwrap_or_else(|e| panic!("{id} failed: {e}"))
    }

    /// Four rectangles obj-2..obj-5.
    fn session() -> Session {
        let mut s = Session::new(Document::default());
        for x in [0, 10, 20, 30] {
            ok(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4, "center": [x, 0]}));
        }
        s
    }

    #[test]
    fn group_nest_and_ungroup_one_level_at_a_time() {
        let mut s = session();
        let r = ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-3"]}));
        assert_eq!(r["group"], json!("group-6"));
        assert_eq!(r["name"], json!("Group 1"));
        assert_eq!(r["affected"], json!(["obj-2", "obj-3"]));
        assert_eq!(r["bounds"]["min"], json!([-2.0, -2.0]));
        // Nest it with another object.
        let r = ok(&mut s, "group.create", json!({"ids": ["group-6", "obj-4"], "name": "Outer"}));
        assert_eq!(r["group"], json!("group-7"));
        assert_eq!(r["affected"], json!(["obj-2", "obj-3", "obj-4"]));
        let d = s.document();
        assert_eq!(d.group("group-6").unwrap().parent.as_deref(), Some("group-7"));
        assert_eq!(d.top_group_of("obj-2").unwrap().id, "group-7");
        // Commands that take ids accept the group.
        assert_eq!(d.expand_group_ids(&["group-7".to_string()]), ["obj-2", "obj-3", "obj-4"]);

        let r = ok(&mut s, "group.ungroup", json!({"group": "group-7"}));
        assert_eq!(r["groups"], json!(["group-6"]));
        let d = s.document();
        assert!(d.group("group-7").is_none());
        assert_eq!(d.group("group-6").unwrap().parent, None, "the inner group stays, one level up");
        assert_eq!(d.object("obj-4").unwrap().group, None);
        s.undo().unwrap();
        assert!(s.document().group("group-7").is_some());
    }

    #[test]
    fn grouping_members_of_a_group_and_covered_items() {
        let mut s = session();
        ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-3", "obj-4"]}));
        // Two members of group-6 grouped again: the new group sits inside group-6.
        let r = ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-3"]}));
        assert_eq!(r["group"], json!("group-7"));
        assert_eq!(s.document().group("group-7").unwrap().parent.as_deref(), Some("group-6"));
        // An object inside a listed group is covered by it; items from different parents go to the top level.
        let r = ok(&mut s, "group.create", json!({"ids": ["group-6", "obj-2", "obj-5"]}));
        let g = r["group"].as_str().unwrap().to_string();
        let d = s.document();
        assert_eq!(d.group(&g).unwrap().parent, None);
        assert_eq!(d.object("obj-2").unwrap().group.as_deref(), Some("group-7"), "obj-2 stayed where it was");
        // Taking the last member out of a group removes that group.
        let mut s = session();
        ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-3"]}));
        ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-4"]}));
        assert!(s.document().group("group-6").is_some(), "obj-3 is still in it");
        assert_eq!(s.document().group("group-7").unwrap().parent, None, "items from different parents");
        ok(&mut s, "group.create", json!({"ids": ["obj-3", "obj-5"]}));
        assert!(s.document().group("group-6").is_none(), "group-6 lost its last member");
    }

    #[test]
    fn refusals_leave_the_document_alone() {
        let mut s = session();
        let before = s.document().clone();
        for params in [
            json!({"ids": ["obj-2"]}),
            json!({"ids": ["obj-2", "obj-2"]}),
            json!({"ids": []}),
            json!({"ids": ["obj-2", "obj-99"]}),
            json!({"ids": ["obj-2", "obj-3"], "name": "a\nb"}),
        ] {
            assert!(run(&mut s, "group.create", params.clone()).is_err(), "{params}");
        }
        assert!(matches!(run(&mut s, "group.ungroup", json!({"group": "group-1"})), Err(CommandError::NotFound { .. })));
        assert!(run(&mut s, "group.rename", json!({"group": "nope", "name": "x"})).is_err());
        assert_eq!(s.document(), &before);
        ok(&mut s, "group.create", json!({"ids": ["obj-2", "obj-3"]}));
        let r = ok(&mut s, "group.rename", json!({"group": "group-6", "name": " Logo "}));
        assert_eq!(r["name"], json!("Logo"));
        assert_eq!(s.document().group("group-6").unwrap().name, "Logo");
    }
}
