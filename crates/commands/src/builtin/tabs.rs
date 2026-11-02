//! Tabs placed by hand (LightBurn's Add Tabs tool and Clear Tabs, DES-024): `tabs.add`, `tabs.move`, `tabs.remove`,
//! `tabs.clear` and `tabs.list`. A tab is a point on a shape's closed outline, kept in the object's own frame in
//! `object_options[id].tabs` so it follows the shape when it moves; the planner leaves a gap of the layer's tab size
//! centred there (or marks it at the tab cut power) while the layer's tabs are on (`settings.options.tabs`, see
//! `doc::Tabs`). Automatic tabs come from the layer's settings at plan time and are not stored.

use doc::{Document, FillMode, Geometry, Layer, MAX_MANUAL_TABS, ObjectId, TabGeneration, Tabs};
use kurbo::Point;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::pathutil;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Whether some pass of the layer cuts outlines (only those passes get tabs).
fn cuts_lines(l: &Layer) -> bool {
    std::iter::once(&l.settings).chain(&l.sub_layers).any(|s| matches!(s.mode, FillMode::Line | FillMode::FillThenLine))
}

/// The point of the object's closed outlines nearest `p` (field mm), and its distance.
fn nearest_on_outline(cmd: &str, doc: &Document, id: &str, p: Point) -> Result<(Point, f64)> {
    let o = util::object(doc, id)?;
    if matches!(o.geometry, Geometry::Image { .. }) {
        return Err(invalid(cmd, format!("{} (\"{}\") is an image; tabs go on closed vector shapes", o.id, o.name)));
    }
    let path = pathutil::vector_path(cmd, o)?;
    let lines = geom::flatten(&path, geom::TOLERANCE).map_err(pathutil::geom_err(cmd))?;
    let mut best: Option<(Point, f64)> = None;
    for l in lines.iter().filter(|l| l.closed && l.points.len() > 2) {
        let n = l.points.len();
        for k in 0..n {
            let (a, b) = (l.points[k], l.points[(k + 1) % n]);
            let d = b - a;
            let l2 = d.hypot2();
            let t = if l2 > 0.0 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let q = a + d * t;
            let dist = q.distance(p);
            if best.is_none_or(|(_, b)| dist < b) {
                best = Some((q, dist));
            }
        }
    }
    best.ok_or_else(|| invalid(cmd, format!("{} (\"{}\") has no closed outline to put a tab on", o.id, o.name)))
}

/// The hand-placed tabs of an object in field mm.
fn field_tabs(doc: &Document, id: &str) -> Vec<Point> {
    let Some(o) = doc.object(id) else { return Vec::new() };
    doc.object_options.get(id).map_or_else(Vec::new, |opts| opts.tabs.iter().map(|t| o.transform * Point::new(t[0], t[1])).collect())
}

/// Store field-mm tabs on an object (in its own frame), dropping its options entry when nothing else is in it.
fn set_tabs(cmd: &str, doc: &mut Document, id: &str, field: &[Point]) -> Result<()> {
    let t = util::object(doc, id)?.transform;
    if t.determinant() == 0.0 {
        return Err(CommandError::Failed(format!("{cmd}: object {id} has a transform that cannot be inverted")));
    }
    let inv = t.inverse();
    let local: Vec<[f64; 2]> = field.iter().map(|p| inv * *p).map(|p| [p.x, p.y]).collect();
    let opts = doc.object_options.entry(id.to_owned()).or_default();
    opts.tabs = local;
    if opts.is_default() {
        doc.object_options.remove(id);
    }
    Ok(())
}

/// The layer's tab size (mm), for spacing checks: its settings', else LightBurn's manual default of 0.5 mm.
fn tab_size(doc: &Document, layer: &str) -> f64 {
    doc.layer(layer).and_then(|l| l.settings.options.tabs.as_ref()).map_or(0.5, |t| t.size_mm)
}

/// Turn the layer's tabs on (LightBurn: adding a tab by hand switches them on): new tab settings are manual ones,
/// existing ones are switched on. Returns whether anything changed.
fn switch_layer_on(doc: &mut Document, layer: &str) -> bool {
    let Some(l) = doc.layers.iter_mut().find(|l| l.id == layer) else { return false };
    match &mut l.settings.options.tabs {
        Some(t) if t.enabled => false,
        Some(t) => {
            t.enabled = true;
            true
        }
        None => {
            l.settings.options.tabs = Some(Tabs { generation: TabGeneration::Manual, ..Tabs::default() });
            true
        }
    }
}

fn point_json(p: Point) -> Value {
    json!([util::r4(p.x), util::r4(p.y)])
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddTabsInput {
    /// Exact object id (a closed vector shape on a layer that cuts lines).
    pub id: ObjectId,
    /// Field points (mm); each tab goes on the nearest point of the shape's closed outlines.
    pub points: Vec<[f64; 2]>,
    /// Refuse points farther than this from the outline, mm (default: no limit).
    pub max_distance_mm: Option<f64>,
}

/// `tabs.add`.
pub struct AddTabs;

impl Command for AddTabs {
    type Input = AddTabsInput;
    const ID: &'static str = "tabs.add";
    const TITLE: &'static str = "Add tabs";
    const DESCRIPTION: &'static str = "LightBurn's Add Tabs tool: place tabs (bridges left uncut so a \
        part stays in the stock) on a shape by hand. Each field point becomes a tab on the nearest point of the \
        shape's closed outlines; one closer than the layer's tab size to another tab is refused. Adding a tab turns \
        the layer's tabs on (new settings: generation manual, 0.5 mm), as in LightBurn. Size, Tab Cut Power and \
        automatic tabs are layer settings (layer.update settings.options.tabs). Returns {affected, tabs (field mm), \
        layer_tabs_switched_on}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AddTabsInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        if i.points.is_empty() {
            return Err(invalid(cmd, "give at least one point"));
        }
        let max = match i.max_distance_mm {
            Some(m) => util::non_negative(cmd, "max_distance_mm", m)?,
            None => f64::INFINITY,
        };
        let o = util::object(doc, &i.id)?.clone();
        util::check_unlocked(cmd, doc, std::slice::from_ref(&o.id))?;
        let layer = util::layer(doc, &o.layer)?;
        if !cuts_lines(layer) {
            return Err(invalid(cmd, format!("layer {} does not cut lines (mode line or fill then line); tabs only apply to cut lines", layer.id)));
        }
        let size = tab_size(doc, &o.layer);
        let mut tabs = field_tabs(doc, &o.id);
        for p in &i.points {
            let [x, y] = util::finite2(cmd, "points", *p)?;
            let (q, d) = nearest_on_outline(cmd, doc, &o.id, Point::new(x, y))?;
            if d > max {
                return Err(invalid(cmd, format!("[{x}, {y}] is {} mm from the outline of {} (more than {max} mm)", util::r4(d), o.id)));
            }
            if let Some(t) = tabs.iter().find(|t| t.distance(q) < size) {
                return Err(invalid(cmd, format!("a tab at [{}, {}] is closer than the tab size ({size} mm) to the tab at [{}, {}]", util::r4(q.x), util::r4(q.y), util::r4(t.x), util::r4(t.y))));
            }
            tabs.push(q);
        }
        if tabs.len() > MAX_MANUAL_TABS {
            return Err(invalid(cmd, format!("a shape holds at most {MAX_MANUAL_TABS} tabs")));
        }
        transact(doc, |doc| {
            set_tabs(cmd, doc, &o.id, &tabs)?;
            let switched = switch_layer_on(doc, &o.layer);
            let mut r = util::edit_result(doc, &[], std::slice::from_ref(&o.id));
            r["tabs"] = Value::Array(tabs.iter().map(|p| point_json(*p)).collect());
            r["layer_tabs_switched_on"] = json!(switched);
            Ok((r, format!("added {} tab(s) to {}", i.points.len(), o.id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveTabInput {
    /// Exact object id holding the tab.
    pub id: ObjectId,
    /// The tab's index as tabs.list gives it.
    pub index: usize,
    /// Field point (mm) to move it to (onto the nearest outline point).
    pub to: [f64; 2],
    /// Move it onto this other shape instead (LightBurn: drag a tab onto another shape). Default: the same one.
    pub to_id: Option<ObjectId>,
}

/// `tabs.move`.
pub struct MoveTab;

impl Command for MoveTab {
    type Input = MoveTabInput;
    const ID: &'static str = "tabs.move";
    const TITLE: &'static str = "Move tab";
    const DESCRIPTION: &'static str = "Move a hand-placed tab (LightBurn: drag its red dot) to the outline point \
        nearest `to`, on the same shape or on `to_id`. Returns {affected, tabs}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MoveTabInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let from = util::object(doc, &i.id)?.id.clone();
        let mut tabs = field_tabs(doc, &from);
        if i.index >= tabs.len() {
            return Err(invalid(cmd, format!("{from} has {} tab(s); there is no tab {}", tabs.len(), i.index)));
        }
        let target = match &i.to_id {
            Some(t) => util::object(doc, t)?.id.clone(),
            None => from.clone(),
        };
        util::check_unlocked(cmd, doc, &[from.clone(), target.clone()])?;
        let target_layer = util::object(doc, &target)?.layer.clone();
        if !cuts_lines(util::layer(doc, &target_layer)?) {
            return Err(invalid(cmd, format!("layer {target_layer} does not cut lines; tabs only apply to cut lines")));
        }
        let [x, y] = util::finite2(cmd, "to", i.to)?;
        let (q, _) = nearest_on_outline(cmd, doc, &target, Point::new(x, y))?;
        let size = tab_size(doc, &target_layer);
        tabs.remove(i.index);
        transact(doc, |doc| {
            let mut dest = if target == from { tabs.clone() } else { field_tabs(doc, &target) };
            if dest.iter().any(|t| t.distance(q) < size) {
                return Err(invalid(cmd, format!("another tab is closer than the tab size ({size} mm) there")));
            }
            dest.push(q);
            if dest.len() > MAX_MANUAL_TABS {
                return Err(invalid(cmd, format!("a shape holds at most {MAX_MANUAL_TABS} tabs")));
            }
            if target != from {
                set_tabs(cmd, doc, &from, &tabs)?;
                switch_layer_on(doc, &target_layer);
            }
            set_tabs(cmd, doc, &target, &dest)?;
            let affected: Vec<ObjectId> = if target == from { vec![from.clone()] } else { vec![from.clone(), target.clone()] };
            let mut r = util::edit_result(doc, &[], &affected);
            r["tabs"] = Value::Array(dest.iter().map(|p| point_json(*p)).collect());
            Ok((r, format!("moved tab {} of {from}", i.index)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveTabsInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// Remove only the tab nearest this field point (within `within_mm`); default: every hand-placed tab of the ids.
    pub near: Option<[f64; 2]>,
    /// With `near`: how far the tab may be, mm (default 2).
    pub within_mm: Option<f64>,
}

/// `tabs.remove`.
pub struct RemoveTabs;

impl Command for RemoveTabs {
    type Input = RemoveTabsInput;
    const ID: &'static str = "tabs.remove";
    const TITLE: &'static str = "Remove tabs";
    const DESCRIPTION: &'static str = "Remove hand-placed tabs (LightBurn: double-click or Shift+click a tab's red \
        dot): with `near`, the one tab of the ids nearest that field point (within within_mm, default 2 mm); \
        without, every hand-placed tab of the ids. The layer's tab settings stay. Returns {affected, removed}. \
        Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RemoveTabsInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let ids = util::object_ids(cmd, doc, &i.ids)?;
        util::check_unlocked(cmd, doc, &ids)?;
        transact(doc, |doc| {
            let mut affected = Vec::new();
            let mut removed = 0usize;
            match i.near {
                Some(p) => {
                    let [x, y] = util::finite2(cmd, "near", p)?;
                    let within = util::non_negative(cmd, "within_mm", i.within_mm.unwrap_or(2.0))?;
                    let at = Point::new(x, y);
                    let best = ids
                        .iter()
                        .flat_map(|id| field_tabs(doc, id).into_iter().enumerate().map(move |(k, t)| (id.clone(), k, t.distance(at))))
                        .filter(|(_, _, d)| *d <= within)
                        .min_by(|a, b| a.2.total_cmp(&b.2));
                    let Some((id, k, _)) = best else {
                        return Err(CommandError::Failed(format!("{cmd}: no tab within {within} mm of [{x}, {y}]")));
                    };
                    let mut tabs = field_tabs(doc, &id);
                    tabs.remove(k);
                    set_tabs(cmd, doc, &id, &tabs)?;
                    affected.push(id);
                    removed = 1;
                }
                None => {
                    for id in &ids {
                        let n = field_tabs(doc, id).len();
                        if n > 0 {
                            set_tabs(cmd, doc, id, &[])?;
                            affected.push(id.clone());
                            removed += n;
                        }
                    }
                }
            }
            if removed == 0 {
                return Err(CommandError::Failed(format!("{cmd}: the shapes have no tabs placed by hand")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["removed"] = json!(removed);
            Ok((r, format!("removed {removed} tab(s)")))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearTabsInput {
    /// Exact layer id.
    pub layer: String,
}

/// `tabs.clear`.
pub struct ClearTabs;

impl Command for ClearTabs {
    type Input = ClearTabsInput;
    const ID: &'static str = "tabs.clear";
    const TITLE: &'static str = "Clear tabs";
    const DESCRIPTION: &'static str = "LightBurn's Clear Tabs (Cut Settings Editor): remove every hand-placed tab \
        from every shape on a layer. Automatic tabs are not stored; they stop when the layer's tabs are switched off \
        or set to generation manual (layer.update settings.options.tabs). Returns {affected, removed}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ClearTabsInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let layer = util::layer(doc, &i.layer)?.id.clone();
        let ids: Vec<ObjectId> =
            doc.objects.iter().filter(|o| o.layer == layer && doc.object_options.get(&o.id).is_some_and(|x| !x.tabs.is_empty())).map(|o| o.id.clone()).collect();
        if ids.is_empty() {
            return Err(CommandError::Failed(format!("{cmd}: no shape on layer {layer} has tabs placed by hand")));
        }
        util::check_unlocked(cmd, doc, &ids)?;
        transact(doc, |doc| {
            let mut removed = 0;
            for id in &ids {
                removed += field_tabs(doc, id).len();
                set_tabs(cmd, doc, id, &[])?;
            }
            let mut r = util::edit_result(doc, &[], &ids);
            r["removed"] = json!(removed);
            Ok((r, format!("cleared {removed} tab(s) on layer {layer}")))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListTabsInput {
    /// Exact object or group ids; default every object with hand-placed tabs.
    #[serde(default)]
    pub ids: Vec<ObjectId>,
}

/// `tabs.list`.
pub struct ListTabs;

impl Command for ListTabs {
    type Input = ListTabsInput;
    const ID: &'static str = "tabs.list";
    const TITLE: &'static str = "List tabs";
    const DESCRIPTION: &'static str = "The hand-placed tabs of shapes (field mm, by index for tabs.move) and their \
        layer's tab settings (null when the layer has none): {objects: [{id, layer, tabs: [[x, y]], layer_tabs}]}. \
        Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ListTabsInput) -> Result<(Value, String)> {
        let ids = if i.ids.is_empty() {
            doc.objects.iter().filter(|o| doc.object_options.get(&o.id).is_some_and(|x| !x.tabs.is_empty())).map(|o| o.id.clone()).collect()
        } else {
            util::object_ids(Self::ID, doc, &i.ids)?
        };
        let objects: Vec<Value> = ids
            .iter()
            .filter_map(|id| doc.object(id))
            .map(|o| {
                let layer_tabs = doc.layer(&o.layer).and_then(|l| l.settings.options.tabs.as_ref());
                json!({
                    "id": o.id,
                    "layer": o.layer,
                    "tabs": field_tabs(doc, &o.id).into_iter().map(point_json).collect::<Vec<_>>(),
                    "layer_tabs": layer_tabs,
                })
            })
            .collect();
        Ok((json!({ "objects": objects }), String::new()))
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::json;

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: serde_json::Value) -> crate::Result<serde_json::Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn session() -> Session {
        Session::new(Document { field_mm: Some(150.0), ..Document::default() })
    }

    fn square(s: &mut Session) -> String {
        let r = run(s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [20, 0]})).unwrap();
        r["created"][0].as_str().unwrap().to_owned()
    }

    #[test]
    fn hand_placed_tabs_follow_the_shape_and_switch_the_layer_on() {
        let mut s = session();
        let id = square(&mut s);
        let r = run(&mut s, "tabs.add", json!({"id": id, "points": [[20, -6], [26, 0]]})).unwrap();
        assert_eq!(r["tabs"], json!([[20.0, -5.0], [25.0, 0.0]]), "on the nearest outline points");
        assert_eq!(r["layer_tabs_switched_on"], true);
        let layer = s.document().objects[0].layer.clone();
        let tabs = s.document().layer(&layer).unwrap().settings.options.tabs.clone().unwrap();
        assert_eq!(tabs.generation, doc::TabGeneration::Manual);
        // Too close to another tab; too far with a limit.
        assert!(run(&mut s, "tabs.add", json!({"id": id, "points": [[20.2, -5]]})).is_err());
        assert!(run(&mut s, "tabs.add", json!({"id": id, "points": [[20, 20]], "max_distance_mm": 5})).is_err());
        // They move with the shape.
        run(&mut s, "object.transform", json!({"ids": [id], "translate": [5, 0]})).unwrap();
        let l = run(&mut s, "tabs.list", json!({})).unwrap();
        assert_eq!(l["objects"][0]["tabs"], json!([[25.0, -5.0], [30.0, 0.0]]), "{l}");
        // Move one onto the left side; remove the nearest; clear the layer.
        let r = run(&mut s, "tabs.move", json!({"id": id, "index": 0, "to": [19, 2]})).unwrap();
        assert_eq!(r["tabs"], json!([[30.0, 0.0], [20.0, 2.0]]));
        let r = run(&mut s, "tabs.remove", json!({"ids": [id], "near": [20.5, 2.5]})).unwrap();
        assert_eq!(r["removed"], 1);
        run(&mut s, "tabs.clear", json!({"layer": layer})).unwrap();
        assert!(s.document().object_options.is_empty(), "no options left behind");
        assert!(run(&mut s, "tabs.clear", json!({"layer": layer})).is_err(), "nothing to clear");
        // Undo brings the tab back.
        s.undo().unwrap();
        assert_eq!(run(&mut s, "tabs.list", json!({})).unwrap()["objects"][0]["tabs"], json!([[30.0, 0.0]]));
    }

    #[test]
    fn every_tab_and_joinery_edit_undoes_and_redoes_whole() {
        let mut s = session();
        let pts = [[30, -5], [50, -5], [50, 5], [42, 5], [42, 0], [38, 0], [38, 5], [30, 5]];
        let id = run(&mut s, "shape.line", json!({"points": pts, "closed": true})).unwrap()["created"][0].as_str().unwrap().to_owned();
        // The document without its revision counter (which only ever counts up).
        let snapshot = |s: &Session| {
            let mut v = serde_json::to_value(s.document()).unwrap();
            v.as_object_mut().unwrap().remove("revision");
            v
        };
        let mut states = vec![snapshot(&s)];
        for (cmd, params) in [
            ("tabs.add", json!({"id": id, "points": [[40, -6], [51, 0]]})),
            ("tabs.move", json!({"id": id, "index": 0, "to": [35, -6]})),
            ("path.resize_slots", json!({"ids": [id], "old_thickness_mm": 5, "new_thickness_mm": 6})),
            ("path.dogbone", json!({"ids": [id], "diameter_mm": 1})),
            ("tabs.remove", json!({"ids": [id], "near": [50, 0]})),
            ("tabs.clear", json!({"layer": s.document().objects[0].layer})),
        ] {
            run(&mut s, cmd, params).unwrap_or_else(|e| panic!("{cmd}: {e}"));
            states.push(snapshot(&s));
            assert_ne!(states[states.len() - 1], states[states.len() - 2], "{cmd} changed something");
        }
        // The layer's tabs were switched on by the first tabs.add and stay on through the others.
        for k in (0..states.len() - 1).rev() {
            s.undo().unwrap();
            assert_eq!(snapshot(&s), states[k], "undo to state {k}");
        }
        for (k, state) in states.iter().enumerate().skip(1) {
            s.redo().unwrap();
            assert_eq!(&snapshot(&s), state, "redo to state {k}");
        }
        assert!(s.document().object_options.is_empty(), "tabs.clear leaves no options behind");
    }

    #[test]
    fn tabs_need_a_closed_outline_on_a_cutting_layer() {
        let mut s = session();
        let id = square(&mut s);
        let layer = s.document().objects[0].layer.clone();
        run(&mut s, "layer.update", json!({"id": layer, "settings": {"mode": "fill"}})).unwrap();
        let e = run(&mut s, "tabs.add", json!({"id": id, "points": [[20, -5]]})).unwrap_err().to_string();
        assert!(e.contains("does not cut lines"), "{e}");
        run(&mut s, "layer.update", json!({"id": layer, "settings": {"mode": "line"}})).unwrap();
        let line = run(&mut s, "shape.line", json!({"points": [[0, 0], [10, 0]]})).unwrap()["created"][0].as_str().unwrap().to_owned();
        let e = run(&mut s, "tabs.add", json!({"id": line, "points": [[5, 0]]})).unwrap_err().to_string();
        assert!(e.contains("no closed outline"), "{e}");
    }
}
