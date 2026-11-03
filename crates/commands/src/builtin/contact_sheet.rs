//! `var.contact_sheet`: every serial number or CSV row rendered before a batch (OpenEtch's contact sheet, short-list
//! #10), so a name that runs off a tag, a glyph the font lacks or an empty CSV cell is seen on the iPad before the
//! fifth coin, not after. Each row is laid out exactly as a run would burn it (`text::render_object` with the
//! counter at that value, as `prepare_output` does); nothing in the document changes.
//!
//! Checks per variable text (and variable barcode) and row:
//! - `overflow`: part of the outline leaves its container: the smallest closed shape (any layer, the text and other
//!   variable shapes aside) whose area holds the text's origin, else the stock when it holds it, else the workspace.
//!   Texts with an auto-fit (`fit`) fit by construction and are only checked against the workspace.
//! - `missing_glyph`: the font has no glyph for a character (the engine's render warning).
//! - `empty`: the text shows nothing, a CSV cell it uses is empty, or its row is past the end of the CSV file.
//! - `shrunk`: max width or auto-fit drew it at less than half its set height (readability).
//! - `error`: it cannot be drawn (a missing font, a bad serial format, data a barcode cannot encode).

use doc::{Document, Geometry, ObjectId, VarMode};
use kurbo::{BezPath, Point, Rect, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::font_preview::svg_d;
use super::text::{barcode_data, barcode_params, draw_barcode, now, rerender, var_of};
use super::util::{self, invalid};
use crate::{Command, CommandError, Result, Risk};

/// Rows one call returns at most.
pub const MAX_ROWS: u32 = 500;
/// Rows when `count` is not given.
pub const DEFAULT_ROWS: u32 = 60;
/// Points of an outline tested against its container at most (evenly picked).
const MAX_POINTS: usize = 600;
/// Below this share of its set height a text is reported as `shrunk`.
const SHRUNK_BELOW: f64 = 0.5;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContactSheetInput {
    /// First counter value (serial number or CSV row) to show. Default: the variable text's start.
    pub from: Option<i64>,
    /// How many rows, 1-500. Default 60. The rows run from `from` to the end value in steps of `advance_by`.
    pub count: Option<u32>,
    /// Include each row's outlines (SVG paths, y down) and the rest of the design (`base`), for a grid preview.
    /// Default false (the checks only).
    #[serde(default)]
    pub outlines: bool,
    /// Leave out rows without problems. Default false.
    #[serde(default)]
    pub problems_only: bool,
}

/// What a row of a variable shape is checked against.
#[derive(Debug, Clone)]
enum Container {
    Object { id: ObjectId, name: String, path: BezPath },
    Stock(BezPath),
    Workspace,
}

impl Container {
    fn json(&self) -> Value {
        match self {
            Container::Object { id, name, .. } => json!({ "kind": "object", "id": id, "name": name }),
            Container::Stock(_) => json!({ "kind": "stock" }),
            Container::Workspace => json!({ "kind": "workspace" }),
        }
    }
}

/// A variable text or barcode the sheet shows.
struct Target {
    id: ObjectId,
    name: String,
    barcode: bool,
    mode: VarMode,
    container: Container,
}

/// The variable shapes whose output changes with the counter (serial number and CSV modes).
fn targets(doc: &Document) -> Vec<(ObjectId, bool, VarMode)> {
    doc.objects
        .iter()
        .filter_map(|o| match &o.geometry {
            Geometry::Text(t) => t.variable.as_ref().filter(|v| matches!(v.mode, VarMode::Serial | VarMode::Csv)).map(|v| (o.id.clone(), false, v.mode)),
            _ => barcode_params(o)
                .and_then(|b| b.variable.as_ref().and_then(var_of))
                .filter(|v| matches!(v.mode, VarMode::Serial | VarMode::Csv))
                .map(|v| (o.id.clone(), true, v.mode)),
        })
        .collect()
}

/// Whether every subpath of `p` is closed (so it bounds an area): it ends with a close, or back at its start (kurbo
/// draws an ellipse that way).
fn closed(p: &BezPath) -> bool {
    let mut open: Option<(Point, Point)> = None;
    let mut any = false;
    let shut = |o: Option<(Point, Point)>| o.is_none_or(|(start, at)| (start - at).hypot() < 1e-6);
    for el in p.elements() {
        match *el {
            kurbo::PathEl::MoveTo(q) => {
                if !shut(open) {
                    return false;
                }
                open = Some((q, q));
                any = true;
            }
            kurbo::PathEl::LineTo(q) | kurbo::PathEl::QuadTo(_, q) | kurbo::PathEl::CurveTo(_, _, q) => {
                if let Some((_, at)) = &mut open {
                    *at = q;
                }
            }
            kurbo::PathEl::ClosePath => open = None,
        }
    }
    any && shut(open)
}

fn inside(p: &BezPath, pt: Point) -> bool {
    p.winding(pt) != 0
}

/// The container of variable shape `id` (see the module docs).
fn container_of(doc: &Document, id: &str, skip: &[ObjectId]) -> Container {
    let Some(o) = doc.object(id) else { return Container::Workspace };
    if matches!(&o.geometry, Geometry::Text(t) if t.fit.is_some()) {
        return Container::Workspace;
    }
    let at = o.transform * Point::ORIGIN;
    let best = doc
        .objects
        .iter()
        .filter(|c| !skip.contains(&c.id) && !matches!(c.geometry, Geometry::Text(_) | Geometry::Image { .. }))
        .filter(|c| barcode_params(c).is_none())
        .filter_map(|c| {
            let p = c.field_path();
            (closed(&p) && inside(&p, at)).then(|| (p.area().abs(), c, p))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, c, path)) = best {
        return Container::Object { id: c.id.clone(), name: c.name.clone(), path };
    }
    match &doc.stock {
        Some(s) if inside(&s.outline(), at) => Container::Stock(s.outline()),
        _ => Container::Workspace,
    }
}

/// Up to [`MAX_POINTS`] points of `p`, flattened.
fn sample(p: &BezPath) -> Vec<Point> {
    let mut pts = Vec::new();
    kurbo::flatten(p.elements().iter().copied(), 0.05, |el| match el {
        kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => pts.push(q),
        _ => {}
    });
    let step = pts.len().div_ceil(MAX_POINTS).max(1);
    pts.into_iter().step_by(step).collect()
}

/// The CSV columns a format uses (`%N`; `%%` is a percent sign), as `text::vars::format_csv` reads them.
fn csv_columns(format: &str) -> Vec<usize> {
    let chars: Vec<char> = format.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' {
            if chars.get(i + 1) == Some(&'%') {
                i += 2;
                continue;
            }
            let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if let Ok(c) = digits.parse::<usize>()
                    && !out.contains(&c)
                {
                    out.push(c);
                }
                i += 1 + digits.len();
                continue;
            }
        }
        i += 1;
    }
    out
}

fn flag(kind: &str, message: impl Into<String>) -> Value {
    json!({ "kind": kind, "message": message.into() })
}

/// The workspace rectangle (field frame), if the document has one.
fn workspace(doc: &Document) -> Option<Rect> {
    doc.workspace().map(|[w, h]| Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0))
}

/// The empty-field checks of a CSV text or barcode format at counter `value`.
fn empty_fields(doc: &Document, format: &str, value: i64, flags: &mut Vec<Value>) {
    let Some(csv) = doc.variables.csv.as_ref() else {
        flags.push(flag("empty", "no CSV file is loaded"));
        return;
    };
    let Some(row) = text::vars::csv_row(Some(csv), value) else {
        flags.push(flag("empty", format!("row {value} is not in the CSV file ({} rows)", csv.rows.len())));
        return;
    };
    for c in csv_columns(format) {
        match row.get(c) {
            Some(v) if !v.trim().is_empty() => {}
            Some(_) => flags.push(flag("empty", format!("column %{c} is empty in row {value}"))),
            None => flags.push(flag("empty", format!("row {value} has no column %{c}"))),
        }
    }
}

/// The outline of target `t` at the work document's current counter, and its problems.
fn check_row(work: &mut Document, t: &Target, value: i64, ws: Option<Rect>) -> (Option<BezPath>, String, Vec<Value>) {
    let cmd = ContactSheet::ID;
    let mut flags = Vec::new();
    let moment = now();
    let Some(o) = work.object(&t.id).cloned() else { return (None, String::new(), vec![flag("error", "the object is gone")]) };
    let (path, shown) = if t.barcode {
        let Some(b) = barcode_params(&o) else { return (None, String::new(), vec![flag("error", "barcode settings are unreadable")]) };
        let data = barcode_data(work, &o.layer, &b, moment.as_ref());
        let offset = b.variable.as_ref().map_or(0, |v| v.offset);
        if t.mode == VarMode::Csv {
            empty_fields(work, &b.data, value.saturating_add(offset), &mut flags);
        }
        if data.trim().is_empty() {
            flags.push(flag("empty", "the barcode has no data"));
            return (None, data, flags);
        }
        match draw_barcode(cmd, &b, &data) {
            Ok(d) => (Some(o.transform * d.path), data),
            Err(e) => {
                flags.push(flag("error", e.to_string()));
                return (None, data, flags);
            }
        }
    } else {
        let Geometry::Text(tx) = &o.geometry else { return (None, String::new(), vec![flag("error", "not a text")]) };
        let offset = tx.variable.as_ref().map_or(0, |v| v.offset);
        if t.mode == VarMode::Csv {
            empty_fields(work, &tx.text, value.saturating_add(offset), &mut flags);
        }
        let height = tx.height_mm;
        let scaled = tx.fit.is_some() || tx.max_width_mm > 0.0;
        match rerender(cmd, work, &t.id, moment.as_ref()) {
            Ok(w) => {
                for m in w {
                    let kind = if m.contains("has no glyph") { "missing_glyph" } else { "warning" };
                    flags.push(flag(kind, m));
                }
            }
            Err(e) => {
                flags.push(flag("error", e.to_string()));
                return (None, String::new(), flags);
            }
        }
        let o = work.object(&t.id).expect("rendered above");
        let Geometry::Text(nt) = &o.geometry else { unreachable!("still a text") };
        let shown = nt.shown.clone().unwrap_or_else(|| nt.text.clone());
        if shown == "bad serial format" {
            flags.push(flag("error", "the serial number format is not valid"));
        }
        if shown.trim().is_empty() {
            flags.push(flag("empty", "the text shows nothing"));
        }
        if scaled && !shown.trim().is_empty() {
            // How tall it came out against how tall the same text is without max width and fit.
            let mut plain = (**nt).clone();
            plain.fit = None;
            plain.max_width_mm = 0.0;
            if let (Ok(r), Some(now_b)) = (text::render_text(&plain, &shown, None), geom::bounds(&nt.outline))
                && let Some(b) = geom::bounds(&r.outline)
                && b.height() > 1e-9
            {
                let k = (now_b.height() / b.height()).min(if b.width() > 1e-9 { now_b.width() / b.width() } else { f64::INFINITY });
                if k < SHRUNK_BELOW {
                    flags.push(flag("shrunk", format!("drawn at {:.0} % of its {height} mm height", k * 100.0)));
                }
            }
        }
        (Some(o.field_path()), shown)
    };
    if let Some(p) = &path {
        flags.extend(leaves(p, &t.container, ws).into_iter().map(|m| flag("overflow", m)));
    }
    (path, shown, flags)
}

/// How field-frame outline `p` leaves `container` and the workspace `ws`, if it does.
fn leaves(p: &BezPath, container: &Container, ws: Option<Rect>) -> Vec<String> {
    let mut out = Vec::new();
    let off = match container {
        Container::Object { path: c, .. } | Container::Stock(c) => sample(p).iter().any(|q| !inside(c, *q)),
        Container::Workspace => false,
    };
    if off {
        out.push(match container {
            Container::Object { name, .. } => format!("runs outside \"{name}\""),
            _ => "runs outside the stock".to_string(),
        });
    }
    if let (Some(w), Some(b)) = (ws, geom::bounds(p))
        && (b.x0 < w.x0 || b.y0 < w.y0 || b.x1 > w.x1 || b.y1 > w.y1)
    {
        out.push("runs outside the workspace".into());
    }
    out
}

/// How text (or other shape) `id`, as it is drawn now, leaves its container (see the module docs) or the
/// workspace; empty when it fits. Used after a template fill.
pub(crate) fn overflow_of(doc: &Document, id: &str) -> Vec<String> {
    let Some(o) = doc.object(id) else { return Vec::new() };
    leaves(&o.field_path(), &container_of(doc, id, &[id.to_string()]), workspace(doc))
}

/// Most variable shapes the pre-flight row check draws (each is one text render).
pub(crate) const PREFLIGHT_MAX_TARGETS: usize = 200;

/// One problem of the next run's row ([`next_run_problems`]).
pub(crate) struct RowProblem {
    pub id: ObjectId,
    pub name: String,
    /// `overflow`, `missing_glyph`, `shrunk`, `empty` (a serial text that shows nothing), `error` or `warning`.
    pub kind: String,
    pub message: String,
}

/// The contact sheet's checks of the row the next run burns (pre-flight `contact_sheet`, `job.preflight`): every
/// serial / CSV text and barcode on an output layer of `doc` (the document as the run plans it) at the counter as it
/// is now. Empty CSV cells and rows past the file are `unfilled_variable`'s, so they are left out here. At most
/// [`PREFLIGHT_MAX_TARGETS`] shapes are drawn; `true` when some were left unchecked.
pub(crate) fn next_run_problems(doc: &Document) -> (Vec<RowProblem>, bool) {
    let emitting: Vec<&str> = doc.layers.iter().filter(|l| l.emits()).map(|l| l.id.as_str()).collect();
    let all = targets(doc);
    let skip: Vec<ObjectId> = all.iter().map(|(id, ..)| id.clone()).collect();
    let found: Vec<_> = all.into_iter().filter(|(id, ..)| doc.object(id).is_some_and(|o| emitting.contains(&o.layer.as_str()))).collect();
    let more = found.len() > PREFLIGHT_MAX_TARGETS;
    let ws = workspace(doc);
    let mut work = doc.clone();
    let value = doc.variables.current;
    let mut out = Vec::new();
    for (id, barcode, mode) in found.into_iter().take(PREFLIGHT_MAX_TARGETS) {
        let name = doc.object(&id).map(|o| o.name.clone()).unwrap_or_default();
        let t = Target { id: id.clone(), name: name.clone(), barcode, mode, container: container_of(doc, &id, &skip) };
        let (_, _, flags) = check_row(&mut work, &t, value, ws);
        for f in flags {
            let kind = f["kind"].as_str().unwrap_or("warning").to_string();
            if kind == "empty" && mode == VarMode::Csv {
                continue;
            }
            out.push(RowProblem { id: id.clone(), name: name.clone(), kind, message: f["message"].as_str().unwrap_or_default().to_string() });
        }
    }
    (out, more)
}

/// `var.contact_sheet`.
pub struct ContactSheet;

impl Command for ContactSheet {
    type Input = ContactSheetInput;
    const ID: &'static str = "var.contact_sheet";
    const TITLE: &'static str = "Check every row (contact sheet)";
    const DESCRIPTION: &'static str = "Render every serial number or CSV row as a run would burn it and check \
        the serial / CSV texts and barcodes: `overflow` (leaves the smallest closed shape holding its origin, else the \
        stock or the workspace), `missing_glyph`, `empty` (blank, an empty CSV cell, a row past the CSV), `shrunk` \
        (under half its height) and `error`. Rows run from `from` (default the start) to the end value by \
        advance_by, `count` at a time (60, at most 500); `next` starts the next page. `outlines: true` adds SVG paths \
        (mm, y down) for a grid preview. `summary.problems` counts each kind. Changes nothing: run it before a batch.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ContactSheetInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let found = targets(doc);
        if found.is_empty() {
            return Err(CommandError::Failed(format!(
                "{cmd}: the document has no serial-number or CSV text or barcode (Variable Text)"
            )));
        }
        let count = i.count.unwrap_or(DEFAULT_ROWS);
        if count == 0 || count > MAX_ROWS {
            return Err(invalid(cmd, format!("count must be within 1..={MAX_ROWS}")));
        }
        let v = &doc.variables;
        let step = v.advance_by.max(1);
        let (lo, hi) = (v.start.min(v.end), v.start.max(v.end));
        let from = i.from.unwrap_or(lo);
        if from < lo || from > hi {
            return Err(invalid(cmd, format!("from must be within the start and end values {lo}..={hi}")));
        }
        let total = (hi - lo) / step + 1;
        let skip: Vec<ObjectId> = found.iter().map(|(id, ..)| id.clone()).collect();
        let list: Vec<Target> = found
            .iter()
            .map(|(id, barcode, mode)| {
                let o = doc.object(id).expect("listed above");
                Target { id: id.clone(), name: o.name.clone(), barcode: *barcode, mode: *mode, container: container_of(doc, id, &skip) }
            })
            .collect();
        let ws = workspace(doc);
        let mut work = doc.clone();
        let mut rows = Vec::new();
        let mut problems = std::collections::BTreeMap::<String, u64>::new();
        let mut value = from;
        let mut shown_rows = 0u32;
        let mut view: Option<Rect> = None;
        while value <= hi && shown_rows < count {
            work.variables.current = value;
            let mut texts = Vec::new();
            let mut row_flags = 0;
            for t in &list {
                let (path, shown, flags) = check_row(&mut work, t, value, ws);
                for f in &flags {
                    if let Some(k) = f["kind"].as_str() {
                        *problems.entry(k.to_string()).or_default() += 1;
                    }
                }
                row_flags += flags.len();
                let mut e = json!({ "id": t.id, "shown": shown, "flags": flags });
                if i.outlines
                    && let Some(p) = &path
                {
                    e["d"] = json!(svg_d(p, 2));
                    if let Some(b) = geom::bounds(p) {
                        view = Some(view.map_or(b, |v| v.union(b)));
                    }
                }
                texts.push(e);
            }
            if !i.problems_only || row_flags > 0 {
                rows.push(json!({ "value": value, "texts": texts, "problems": row_flags }));
            }
            shown_rows += 1;
            value = value.saturating_add(step);
        }
        let next = (value <= hi).then_some(value);
        let mut r = json!({
            "from": from, "next": next, "total": total, "step": step, "start": lo, "end": hi,
            "targets": list.iter().map(|t| json!({
                "id": t.id, "name": t.name, "barcode": t.barcode,
                "mode": if t.mode == VarMode::Csv { "csv" } else { "serial" },
                "container": t.container.json(),
            })).collect::<Vec<_>>(),
            "rows": rows,
            "summary": { "rows": shown_rows, "problems": problems },
        });
        if i.outlines {
            let mut base = Vec::new();
            // Tool layers too: a jig's pocket outlines (T1) are what the texts must stay inside.
            for layer in doc.layers.iter().filter(|l| l.visible) {
                let mut p = BezPath::new();
                for o in doc.objects.iter().filter(|o| o.layer == layer.id && !skip.contains(&o.id)) {
                    let fp = match &o.geometry {
                        Geometry::Image { .. } => o.bounds().to_path(0.1),
                        _ => o.field_path(),
                    };
                    if let Some(b) = geom::bounds(&fp) {
                        view = Some(view.map_or(b, |v| v.union(b)));
                    }
                    p.extend(fp.iter());
                }
                if !p.elements().is_empty() {
                    let tool = layer.tool.is_some();
                    base.push(json!({ "layer": layer.id, "color": layer.color, "fill": !tool && layer.settings.mode != doc::FillMode::Line, "tool": tool, "d": svg_d(&p, 2) }));
                }
            }
            if let Some(s) = &doc.stock {
                let b = s.bounds();
                view = Some(view.map_or(b, |v| v.union(b)));
                r["stock"] = json!(svg_d(&s.outline(), 2));
            }
            r["base"] = json!(base);
            r["view_box"] = match view {
                Some(b) => {
                    let pad = (b.width().max(b.height()) * 0.04).max(0.5);
                    json!([util::r4(b.x0 - pad), util::r4(-b.y1 - pad), util::r4(b.width() + 2.0 * pad), util::r4(b.height() + 2.0 * pad)])
                }
                None => Value::Null,
            };
        }
        let n: u64 = problems.values().sum();
        Ok((r, format!("checked {shown_rows} rows: {n} problems")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(doc: &mut Document, id: &str, params: Value) -> Value {
        let mut s = crate::Session::new(doc.clone());
        let out = s.execute(crate::Invocation { id: id.into(), params }, crate::Origin::Ui).unwrap_or_else(|e| panic!("{id}: {e}"));
        *doc = s.document().clone();
        out.result
    }

    fn sheet(doc: &Document, params: Value) -> Value {
        let mut d = doc.clone();
        let (r, _) = ContactSheet::apply(&mut d, serde_json::from_value(params).unwrap()).unwrap();
        assert_eq!(&d, doc, "a query changes nothing");
        r
    }

    fn kinds(row: &Value) -> Vec<String> {
        row["texts"].as_array().unwrap().iter().flat_map(|t| t["flags"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string())).collect()
    }

    /// A 40 x 10 tag with a CSV name in it (Hershey Sans: bundled, so the test needs no installed font).
    fn tag_doc(csv: &str) -> (Document, String) {
        let mut d = Document { field_mm: Some(150.0), ..Document::default() };
        run(&mut d, "shape.rect", json!({"width_mm": 40, "height_mm": 10, "name": "Tag"}));
        let r = run(&mut d, "shape.text", json!({"text": "%0", "font": {"family": "Hershey Sans", "kind": "stroke"}, "height_mm": 5, "variable": {"mode": "csv"}}));
        let id = r["created"][0].as_str().unwrap().to_string();
        run(&mut d, "var.load_csv", json!({"content": csv}));
        (d, id)
    }

    #[test]
    fn flags_overflow_empty_and_missing_rows() {
        let (d, id) = tag_doc("Ann\nMaximilian Alexander von Habsburg\n\nZoë\n");
        let r = sheet(&d, json!({"outlines": true}));
        assert_eq!(r["total"], 4);
        assert_eq!(r["targets"][0]["container"]["name"], "Tag");
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert!(kinds(&rows[0]).is_empty(), "{:?}", rows[0]);
        assert!(kinds(&rows[1]).contains(&"overflow".to_string()), "a long name leaves the tag");
        assert!(kinds(&rows[2]).contains(&"empty".to_string()), "an empty cell");
        assert_eq!(rows[0]["texts"][0]["id"], id.as_str());
        assert_eq!(rows[0]["texts"][0]["shown"], "Ann");
        assert!(rows[0]["texts"][0]["d"].as_str().unwrap().starts_with('M'));
        assert!(r["base"][0]["d"].as_str().unwrap().contains('Z'));
        assert!(r["view_box"].is_array());
        // Hershey Sans has no ë: a missing glyph.
        assert!(kinds(&rows[3]).contains(&"missing_glyph".to_string()), "{:?}", rows[3]);
        assert!(r["summary"]["problems"]["overflow"].as_u64().unwrap() >= 1);
        // Only the problems, and paging.
        let p = sheet(&d, json!({"problems_only": true}));
        assert_eq!(p["rows"].as_array().unwrap().len(), 3);
        let page = sheet(&d, json!({"count": 2}));
        assert_eq!(page["rows"].as_array().unwrap().len(), 2);
        assert_eq!(page["next"], 2);
        let last = sheet(&d, json!({"from": 2, "count": 2}));
        assert_eq!(last["next"], Value::Null);
        assert_eq!(last["rows"][0]["value"], 2);
    }

    #[test]
    fn rows_past_the_csv_and_fit_texts() {
        let (mut d, id) = tag_doc("Ann\nBob\n");
        run(&mut d, "var.set_serial", json!({"end": 3}));
        let r = sheet(&d, json!({}));
        let rows = r["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows[3]["texts"][0]["flags"][0]["message"].as_str().unwrap().contains("not in the CSV"));
        // With auto-fit into the tag the long name fits (only a shrink note when it gets very small).
        run(&mut d, "var.load_csv", json!({"content": "Ann\nMaximilian Alexander von Habsburg-Lothringen\n"}));
        run(&mut d, "text.update", json!({"id": id, "fit": {"shape": "box", "width_mm": 36, "height_mm": 6, "mode": "shrink"}}));
        let r = sheet(&d, json!({}));
        let k = kinds(&r["rows"][1]);
        assert!(!k.contains(&"overflow".to_string()), "{k:?}");
        assert!(k.contains(&"shrunk".to_string()), "{k:?}");
        assert!(kinds(&r["rows"][0]).is_empty());
    }

    #[test]
    fn serial_texts_bad_inputs_and_no_variables() {
        let mut d = Document { field_mm: Some(100.0), ..Document::default() };
        assert!(ContactSheet::apply(&mut d.clone(), ContactSheetInput::default()).is_err(), "nothing to check");
        run(&mut d, "shape.text", json!({"text": "SN 000d", "font": {"family": "Hershey Sans", "kind": "stroke"}, "variable": {"mode": "serial"}}));
        run(&mut d, "var.set_serial", json!({"start": 5, "end": 9, "advance_by": 2}));
        let r = sheet(&d, json!({}));
        let values: Vec<i64> = r["rows"].as_array().unwrap().iter().map(|x| x["value"].as_i64().unwrap()).collect();
        assert_eq!(values, [5, 7, 9]);
        assert_eq!(r["rows"][1]["texts"][0]["shown"], "SN 007", "LightBurn: 0 pads, d is the last digit");
        assert!(ContactSheet::apply(&mut d.clone(), serde_json::from_value(json!({"from": 50})).unwrap()).is_err());
        assert!(ContactSheet::apply(&mut d.clone(), serde_json::from_value(json!({"count": 0})).unwrap()).is_err());
        assert_eq!(csv_columns("%0 %% %12 %x %0"), [0, 12]);
    }
}
