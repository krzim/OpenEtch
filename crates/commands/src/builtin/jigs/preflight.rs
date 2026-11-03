//! Pre-flight checks shown next to Start (`job.preflight`, read-only) and their per-project switches
//! (`job.preflight_set`, saved in `Document::preflight`):
//!
//! - `cut_before_fill`: a line layer runs before a fill or image layer it overlaps (a cut that frees the part
//!   before it is engraved; LightBurn's most asked-for check);
//! - `empty_layer`: a layer that outputs but has nothing on it (the wrong layer was probably switched on);
//! - `unfilled_variable`: variable text with no CSV loaded, a CSV row past the file's end or an empty cell, or a
//!   serial number past End (it would wrap to Start);
//! - `duplicate_shapes`: exact duplicates on output layers, which burn twice (`path.delete_duplicates`' detector);
//! - `keep_out`: a shape on an output layer that reaches into a keep-out zone (a jig wall, a clamp, or a zone of the
//!   device). This one is a refusal, not a warning: the engine refuses to plan such a job, so it cannot be switched
//!   off here (switch the zone off instead).
//!
//! The checks look at the design; the keep-out refusal itself is enforced on the planned toolpath by the engine.

use std::collections::{BTreeMap, HashMap};

use doc::{Document, FillMode, Geometry, VarMode};
use kurbo::{BezPath, Point, Rect, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::path_tools::shape_key;
use super::super::util::{invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Every check, in the order they are listed.
pub const CHECKS: [&str; 5] = ["keep_out", "cut_before_fill", "empty_layer", "unfilled_variable", "duplicate_shapes"];

/// A keep-out zone handed in by the caller (the device's zones, which live in the engine's preferences).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ZoneInput {
    pub name: String,
    /// The zone's outline, field mm (at least 3 points).
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreflightInput {
    /// Extra keep-out zones to check against besides the jigs' (the app passes the active device's).
    #[serde(default)]
    pub keep_out: Vec<ZoneInput>,
}

/// One finding.
struct Finding {
    check: &'static str,
    text: String,
    params: Value,
    layers: Vec<String>,
    objects: Vec<String>,
}

fn layer_name(doc: &Document, id: &str) -> String {
    doc.layer(id).map_or_else(|| id.to_string(), |l| l.name.clone())
}

/// Objects on emitting layers, by layer id, in layer order.
fn output_objects(doc: &Document) -> Vec<(&doc::Layer, Vec<&doc::Object>)> {
    doc.layers.iter().filter(|l| l.emits()).map(|l| (l, doc.objects.iter().filter(|o| o.layer == l.id).collect())).collect()
}

fn fills(mode: &FillMode) -> bool {
    matches!(mode, FillMode::Fill | FillMode::FillThenLine | FillMode::OffsetFill)
}

fn cut_before_fill(doc: &Document, out: &mut Vec<Finding>) {
    if doc.optimization.cuts_last {
        return;
    }
    let layers = output_objects(doc);
    let bounds = |objs: &[&doc::Object], images: bool| -> Vec<Rect> {
        objs.iter().filter(|o| matches!(o.geometry, Geometry::Image { .. }) == images).map(|o| o.bounds()).collect()
    };
    for (i, (cut, cut_objs)) in layers.iter().enumerate() {
        if cut.settings.mode != FillMode::Line {
            continue;
        }
        let cut_boxes = bounds(cut_objs, false);
        if cut_boxes.is_empty() {
            continue;
        }
        for (fill, fill_objs) in &layers[i + 1..] {
            let mut boxes = bounds(fill_objs, true);
            if fills(&fill.settings.mode) {
                boxes.extend(bounds(fill_objs, false));
            }
            let overlap = boxes.iter().any(|b| cut_boxes.iter().any(|c| !c.intersect(*b).is_zero_area()));
            if overlap {
                out.push(Finding {
                    check: "cut_before_fill",
                    text: format!(
                        "Layer \"{}\" (lines) burns before layer \"{}\" (fill) where they overlap: if the lines cut through, the part can move before it is filled. Move \"{}\" below it, or turn on Cut layers last.",
                        cut.name, fill.name, cut.name
                    ),
                    params: json!({ "cut": cut.name, "fill": fill.name }),
                    layers: vec![cut.id.clone(), fill.id.clone()],
                    objects: vec![],
                });
            }
        }
    }
}

fn empty_layers(doc: &Document, out: &mut Vec<Finding>) {
    for (l, objs) in output_objects(doc) {
        if objs.is_empty() {
            out.push(Finding {
                check: "empty_layer",
                text: format!("Layer \"{}\" is set to output but has nothing on it.", l.name),
                params: json!({ "layer": l.name }),
                layers: vec![l.id.clone()],
                objects: vec![],
            });
        }
    }
}

/// The CSV column numbers (`%0`, `%1`, ...) a format uses outside single quotes.
fn csv_columns(format: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quoted = false;
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' => quoted = !quoted,
            '%' if !quoted => {
                let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    out.push(n);
                    i += digits.len();
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn unfilled_variables(doc: &Document, out: &mut Vec<Finding>) {
    let vars = &doc.variables;
    let emitting: Vec<&str> = doc.layers.iter().filter(|l| l.emits()).map(|l| l.id.as_str()).collect();
    let mut no_csv = Vec::new();
    let mut past_end: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let mut empty_cells: Vec<(String, i64, usize)> = Vec::new();
    let mut serial_wrap = Vec::new();
    for o in doc.objects.iter().filter(|o| emitting.contains(&o.layer.as_str())) {
        let Geometry::Text(t) = &o.geometry else { continue };
        let Some(v) = &t.variable else { continue };
        let index = vars.current.saturating_add(v.offset);
        match v.mode {
            VarMode::Csv => match &vars.csv {
                None => no_csv.push(o.id.clone()),
                Some(csv) => match text::vars::csv_row(Some(csv), index) {
                    None => past_end.entry(index).or_default().push(o.id.clone()),
                    Some(row) => {
                        for c in csv_columns(&t.text) {
                            if row.get(c).is_none_or(|s| s.trim().is_empty()) {
                                empty_cells.push((o.id.clone(), index, c));
                            }
                        }
                    }
                },
            },
            VarMode::Serial => {
                let (lo, hi) = if vars.start <= vars.end { (vars.start, vars.end) } else { (vars.end, vars.start) };
                if index > hi || index < lo {
                    serial_wrap.push(o.id.clone());
                }
            }
            _ => {}
        }
    }
    let names = |ids: &[String]| ids.iter().map(|id| doc.object(id).map_or(id.clone(), |o| format!("\"{}\"", o.name))).collect::<Vec<_>>().join(", ");
    if !no_csv.is_empty() {
        out.push(Finding {
            check: "unfilled_variable",
            text: format!("{} merges CSV columns, but no CSV file is loaded: it would burn empty.", names(&no_csv)),
            params: json!({ "kind": "no_csv", "names": names(&no_csv) }),
            layers: vec![],
            objects: no_csv,
        });
    }
    for (row, ids) in past_end {
        let rows = vars.csv.as_ref().map_or(0, |c| c.rows.len());
        out.push(Finding {
            check: "unfilled_variable",
            text: format!("{} would read CSV row {row}, past the file's last row ({}): it would burn empty.", names(&ids), rows.saturating_sub(1)),
            params: json!({ "kind": "past_end", "names": names(&ids), "row": row, "last": rows.saturating_sub(1) }),
            layers: vec![],
            objects: ids,
        });
    }
    if !empty_cells.is_empty() {
        let (id, row, col) = empty_cells[0].clone();
        let ids: Vec<String> = empty_cells.iter().map(|e| e.0.clone()).collect();
        let more = if empty_cells.len() > 1 { format!(" (and {} more)", empty_cells.len() - 1) } else { String::new() };
        out.push(Finding {
            check: "unfilled_variable",
            text: format!("{} uses column %{col} of CSV row {row}, which is empty{more}.", names(std::slice::from_ref(&id))),
            params: json!({ "kind": "empty_cell", "names": names(std::slice::from_ref(&id)), "row": row, "column": col, "more": empty_cells.len() - 1 }),
            layers: vec![],
            objects: ids,
        });
    }
    if !serial_wrap.is_empty() {
        out.push(Finding {
            check: "unfilled_variable",
            text: format!("{} would show a serial number past End ({}): it wraps back to Start ({}).", names(&serial_wrap), vars.end, vars.start),
            params: json!({ "kind": "serial_wrap", "names": names(&serial_wrap), "end": vars.end, "start": vars.start }),
            layers: vec![],
            objects: serial_wrap,
        });
    }
}

fn duplicates(doc: &Document, out: &mut Vec<Finding>) {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut dup: Vec<String> = Vec::new();
    for (_, objs) in output_objects(doc) {
        for o in objs {
            let Ok(key) = shape_key("job.preflight", o) else { continue };
            if seen.insert(key, o.id.clone()).is_some() {
                dup.push(o.id.clone());
            }
        }
    }
    if !dup.is_empty() {
        let names: Vec<String> = dup.iter().take(3).filter_map(|id| doc.object(id)).map(|o| format!("\"{}\"", o.name)).collect();
        let more = if dup.len() > 3 { format!(" and {} more", dup.len() - 3) } else { String::new() };
        out.push(Finding {
            check: "duplicate_shapes",
            text: format!("{} shape(s) are exact copies of another on the same layer and would burn twice: {}{more}. Edit > Delete duplicates removes them.", dup.len(), names.join(", ")),
            params: json!({ "count": dup.len(), "names": names.join(", "), "more": dup.len().saturating_sub(3) }),
            layers: vec![],
            objects: dup,
        });
    }
}

/// Points of every flattened subpath of `path`, and whether each is closed.
fn rings(path: &BezPath) -> Vec<(Vec<Point>, bool)> {
    geom::flatten(path, 0.02).map(|ls| ls.into_iter().map(|l| (l.points, l.closed)).collect()).unwrap_or_default()
}

fn segments_cross(a: Point, b: Point, c: Point, d: Point) -> bool {
    let o = |p: Point, q: Point, r: Point| (q - p).cross(r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
}

/// Whether object outline `obj` (filled when `filled`) reaches into zone ring `zone`.
pub(crate) fn touches(obj: &[(Vec<Point>, bool)], filled: bool, zone: &[Point]) -> bool {
    if zone.len() < 3 {
        return false;
    }
    for (pts, closed) in obj {
        if pts.iter().any(|p| geom::flatten::point_in_ring(*p, zone)) {
            return true;
        }
        if filled && *closed && zone.iter().any(|z| geom::flatten::point_in_ring(*z, pts)) {
            return true;
        }
        let n = pts.len();
        let edges = if *closed { n } else { n.saturating_sub(1) };
        for i in 0..edges {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            for j in 0..zone.len() {
                if segments_cross(a, b, zone[j], zone[(j + 1) % zone.len()]) {
                    return true;
                }
            }
        }
    }
    false
}

/// Every keep-out zone: the jigs' and the caller's, as flattened rings.
fn all_zones(doc: &Document, extra: &[ZoneInput]) -> Vec<(String, Vec<Point>)> {
    let mut z: Vec<(String, Vec<Point>)> = doc::jig::document_keep_outs(doc)
        .into_iter()
        .flat_map(|(name, path)| rings(&path).into_iter().filter(|(_, c)| *c).map(move |(r, _)| (name.clone(), r)))
        .collect();
    z.extend(extra.iter().map(|e| (e.name.clone(), e.points.iter().map(|p| Point::new(p[0], p[1])).collect())));
    z
}

fn keep_out(doc: &Document, extra: &[ZoneInput], out: &mut Vec<Finding>) {
    let zones = all_zones(doc, extra);
    if zones.is_empty() {
        return;
    }
    for (l, objs) in output_objects(doc) {
        for o in objs {
            let path = o.field_path();
            let ob = path.bounding_box();
            let filled = fills(&l.settings.mode) || matches!(o.geometry, Geometry::Image { .. });
            let r = rings(&path);
            for (name, ring) in &zones {
                let zb = ring.iter().fold(Rect::new(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY), |b, p| {
                    Rect::new(b.x0.min(p.x), b.y0.min(p.y), b.x1.max(p.x), b.y1.max(p.y))
                });
                if ob.intersect(zb).is_zero_area() && !(ob.width() == 0.0 || ob.height() == 0.0) {
                    continue;
                }
                if touches(&r, filled, ring) {
                    out.push(Finding {
                        check: "keep_out",
                        text: format!("\"{}\" on layer \"{}\" reaches into keep-out zone \"{name}\": Start will be refused. Move the design, or the jig, or switch the zone off.", o.name, layer_name(doc, &l.id)),
                        params: json!({ "name": o.name, "layer": l.name, "zone": name }),
                        layers: vec![l.id.clone()],
                        objects: vec![o.id.clone()],
                    });
                    break;
                }
            }
        }
    }
}

/// `job.preflight`.
pub struct Preflight;

impl Command for Preflight {
    type Input = PreflightInput;
    const ID: &'static str = "job.preflight";
    const TITLE: &'static str = "Pre-flight check";
    const DESCRIPTION: &'static str = "Check the job before Start: cut_before_fill (a line layer burns before a fill \
        or image layer it overlaps), empty_layer (outputs but has nothing), unfilled_variable (CSV text with no \
        file, a row past its end or an empty cell; a serial past End), duplicate_shapes (exact copies on output \
        layers burn twice) and keep_out (a shape reaching into a jig's or the given keep_out zone: the engine \
        refuses such a job; blocking). Checks switched off for the project (job.preflight_set) are skipped. \
        Returns warnings [{check, blocking, text, params, layers, objects}], disabled and checks. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: PreflightInput) -> Result<(Value, String)> {
        for z in &i.keep_out {
            if z.points.len() < 3 || z.points.iter().flatten().any(|v| !v.is_finite()) {
                return Err(invalid(Self::ID, format!("keep_out zone \"{}\" needs at least 3 finite points", z.name)));
            }
        }
        let mut found = Vec::new();
        keep_out(doc, &i.keep_out, &mut found);
        let on = |c: &str| doc.preflight.enabled(c);
        if on("cut_before_fill") {
            cut_before_fill(doc, &mut found);
        }
        if on("empty_layer") {
            empty_layers(doc, &mut found);
        }
        if on("unfilled_variable") {
            unfilled_variables(doc, &mut found);
        }
        if on("duplicate_shapes") {
            duplicates(doc, &mut found);
        }
        let n = found.len();
        let warnings: Vec<Value> = found
            .into_iter()
            .map(|f| json!({ "check": f.check, "blocking": f.check == "keep_out", "text": f.text, "params": f.params, "layers": f.layers, "objects": f.objects }))
            .collect();
        let disabled: Vec<&String> = doc.preflight.disabled.iter().collect();
        Ok((json!({ "warnings": warnings, "disabled": disabled, "checks": CHECKS }), format!("{n} pre-flight findings")))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreflightSetInput {
    /// The check: cut_before_fill, empty_layer, unfilled_variable or duplicate_shapes (keep_out cannot be switched
    /// off; switch the zone off instead).
    pub check: String,
    /// Warn about it in this project (true) or not (false).
    pub enabled: bool,
}

/// `job.preflight_set`.
pub struct PreflightSet;

impl Command for PreflightSet {
    type Input = PreflightSetInput;
    const ID: &'static str = "job.preflight_set";
    const TITLE: &'static str = "Switch a pre-flight check";
    const DESCRIPTION: &'static str = "Switch one pre-flight warning on or off for this project (saved with it): \
        cut_before_fill, empty_layer, unfilled_variable or duplicate_shapes. keep_out is a refusal and cannot be \
        switched off (switch the keep-out zone off instead). Returns disabled.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PreflightSetInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.check == "keep_out" {
                return Err(invalid(cmd, "keep_out is a refusal, not a warning: switch the keep-out zone off instead (jig.remove_keep_out with enabled false)"));
            }
            if !CHECKS.contains(&i.check.as_str()) {
                return Err(CommandError::NotFound {
                    message: format!("{cmd}: no pre-flight check {}", i.check),
                    candidates: CHECKS[1..].iter().map(|c| c.to_string()).collect(),
                });
            }
            let changed = if i.enabled { doc.preflight.disabled.remove(&i.check) } else { doc.preflight.disabled.insert(i.check.clone()) };
            if !changed {
                return Err(CommandError::Failed(format!("{cmd}: {} is already {}", i.check, if i.enabled { "on" } else { "off" })));
            }
            let disabled: Vec<&String> = doc.preflight.disabled.iter().collect();
            let r = json!({ "created": [], "affected": [], "disabled": disabled, "bounds": Value::Null });
            Ok((r, format!("pre-flight check {} {}", i.check, if i.enabled { "on" } else { "off" })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn exec(s: &mut Session, id: &str, params: Value) -> Value {
        match s.execute(Invocation { id: id.into(), params }, Origin::Agent("test".into())) {
            Ok(o) => o.result,
            Err(e) => panic!("{id} failed: {e}"),
        }
    }

    fn checks(v: &Value) -> Vec<String> {
        v["warnings"].as_array().unwrap().iter().map(|w| w["check"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn finds_each_problem_and_can_be_switched_off() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        assert!(checks(&exec(&mut s, "job.preflight", json!({}))).contains(&"empty_layer".to_string()));
        let fill = exec(&mut s, "layer.add", json!({"name": "Fill", "color": "#ff0000", "settings": {"mode": "fill"}}))["id"].as_str().unwrap().to_string();
        let a = exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}))["created"][0].as_str().unwrap().to_string();
        exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}));
        exec(&mut s, "shape.rect", json!({"width_mm": 6, "height_mm": 6, "layer": fill}));
        let v = exec(&mut s, "job.preflight", json!({}));
        let c = checks(&v);
        assert!(c.contains(&"cut_before_fill".to_string()), "{v}");
        assert!(c.contains(&"duplicate_shapes".to_string()), "{v}");
        assert!(!c.contains(&"empty_layer".to_string()), "{v}");
        assert!(v["warnings"].as_array().unwrap().iter().all(|w| w["blocking"] == json!(false)));
        exec(&mut s, "job.preflight_set", json!({"check": "cut_before_fill", "enabled": false}));
        let v = exec(&mut s, "job.preflight", json!({}));
        assert!(!checks(&v).contains(&"cut_before_fill".to_string()));
        assert_eq!(v["disabled"], json!(["cut_before_fill"]));
        assert!(s.execute(Invocation { id: "job.preflight_set".into(), params: json!({"check": "keep_out", "enabled": false}) }, Origin::Ui).is_err());
        // Keep-out: a zone passed in, and one on a jig.
        let v = exec(&mut s, "job.preflight", json!({"keep_out": [{"name": "Clamp", "points": [[4, -1], [8, -1], [8, 1], [4, 1]]}]}));
        let k: Vec<&Value> = v["warnings"].as_array().unwrap().iter().filter(|w| w["check"] == json!("keep_out")).collect();
        assert!(!k.is_empty() && k[0]["blocking"] == json!(true), "{v}");
        assert!(k.iter().any(|w| w["objects"] == json!([a])));
        let far = exec(&mut s, "job.preflight", json!({"keep_out": [{"name": "Far", "points": [[40, 40], [41, 40], [41, 41]]}]}));
        assert!(!checks(&far).contains(&"keep_out".to_string()));
        // A zone wholly inside a filled shape counts; inside a line-mode outline it does not.
        let inner = json!({"keep_out": [{"name": "In", "points": [[-1, -1], [1, -1], [1, 1], [-1, 1]]}]});
        let v = exec(&mut s, "job.preflight", inner);
        let hits: Vec<Value> = v["warnings"].as_array().unwrap().iter().filter(|w| w["check"] == json!("keep_out")).map(|w| w["objects"][0].clone()).collect();
        assert_eq!(hits.len(), 1, "only the filled square: {v}");
    }

    #[test]
    fn variable_text_checks() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
        exec(&mut s, "shape.text", json!({ "text": "%0 %1", "font": font, "variable": { "mode": "csv" } }));
        let v = exec(&mut s, "job.preflight", json!({}));
        assert!(v["warnings"].as_array().unwrap().iter().any(|w| w["params"]["kind"] == json!("no_csv")), "{v}");
        exec(&mut s, "var.load_csv", json!({ "name": "names.csv", "content": "Ann,\nBob,x\n" }));
        let v = exec(&mut s, "job.preflight", json!({}));
        assert!(v["warnings"].as_array().unwrap().iter().any(|w| w["params"]["kind"] == json!("empty_cell") && w["params"]["column"] == json!(1)), "{v}");
        exec(&mut s, "var.set_serial", json!({ "start": 0, "end": 1, "current": 5 }));
        let v = exec(&mut s, "job.preflight", json!({}));
        assert!(v["warnings"].as_array().unwrap().iter().any(|w| w["params"]["kind"] == json!("past_end")), "{v}");
        assert_eq!(csv_columns("a %0 '%1' %12"), vec![0, 12]);
    }
}
