//! `job.remark_check`: what re-marking a region would touch (Re-mark region, short list #8). The run itself is a
//! normal Start / `job_request_run` with the `region` run option: the engine clips the planned toolpath (not the
//! design) to the region and runs it as Cut Selected Graphics, through the normal approval. This query tells the
//! person (or an agent) beforehand which shapes and layers are in the region, and warns about layers linked to a
//! material library preset: on a MOPA a colour comes from heat, and marking the same area again shifts it.

use doc::{Document, FillMode, Geometry};
use kurbo::Point;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::util::{self, invalid};
use super::preflight::touches;
use crate::{Command, Result, Risk};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemarkInput {
    /// The region's outline (a lasso or a rectangle's corners), field mm, at least 3 points.
    pub region: Vec<[f64; 2]>,
    /// Only these object or group ids (Cut Selected Graphics); default every shape on an output layer.
    pub selection: Option<Vec<String>>,
}

/// `job.remark_check`.
pub struct RemarkCheck;

impl Command for RemarkCheck {
    type Input = RemarkInput;
    const ID: &'static str = "job.remark_check";
    const TITLE: &'static str = "Check a re-mark region";
    const DESCRIPTION: &'static str = "Before re-marking a region (run option region on Start or job_request_run, \
        which clips the planned toolpath to it): the shapes on output layers (within selection, if given) that \
        reach into the region, their layers, and warnings for layers linked to a material library preset, whose \
        MOPA colour shifts when the same area is marked again. Returns objects, layers [{id, name, preset}], \
        warnings [{check: \"remark_colour\", text, layers}]. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: RemarkInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        if i.region.len() < 3 || i.region.iter().flatten().any(|v| !v.is_finite()) {
            return Err(invalid(cmd, "region must be at least 3 finite points"));
        }
        let ring: Vec<Point> = i.region.iter().map(|p| Point::new(p[0], p[1])).collect();
        let scope: Option<Vec<String>> = match &i.selection {
            Some(ids) => Some(util::object_ids(cmd, doc, ids)?),
            None => None,
        };
        let mut objects = Vec::new();
        let mut layers: Vec<&doc::Layer> = Vec::new();
        for l in doc.layers.iter().filter(|l| l.emits()) {
            for o in doc.objects.iter().filter(|o| o.layer == l.id) {
                if scope.as_ref().is_some_and(|s| !s.contains(&o.id)) {
                    continue;
                }
                let filled = matches!(l.settings.mode, FillMode::Fill | FillMode::FillThenLine | FillMode::OffsetFill) || matches!(o.geometry, Geometry::Image { .. });
                let rings: Vec<(Vec<Point>, bool)> = geom::flatten(&o.field_path(), 0.02).map(|ls| ls.into_iter().map(|p| (p.points, p.closed)).collect()).unwrap_or_default();
                if touches(&rings, filled, &ring) {
                    objects.push(o.id.clone());
                    if !layers.iter().any(|x| x.id == l.id) {
                        layers.push(l);
                    }
                }
            }
        }
        let warnings: Vec<Value> = layers
            .iter()
            .filter_map(|l| {
                let link = l.link.as_ref()?;
                Some(json!({
                    "check": "remark_colour",
                    "text": format!("Layer \"{}\" uses library preset \"{}\": marking the same area again shifts a MOPA colour. Test on scrap first.", l.name, link.path),
                    "params": { "layer": l.name, "preset": link.path },
                    "layers": [l.id],
                }))
            })
            .collect();
        let layer_list: Vec<Value> = layers.iter().map(|l| json!({ "id": l.id, "name": l.name, "preset": l.link.as_ref().map(|k| k.path.clone()) })).collect();
        let n = objects.len();
        Ok((json!({ "objects": objects, "layers": layer_list, "warnings": warnings }), format!("{n} shapes reach into the region")))
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

    #[test]
    fn region_lists_shapes_and_warns_on_linked_layers() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let a = exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10}))["created"][0].as_str().unwrap().to_string();
        exec(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4, "center": [40, 40]}));
        let region = json!([[3, -1], [8, -1], [8, 1], [3, 1]]);
        let r = exec(&mut s, "job.remark_check", json!({"region": region}));
        assert_eq!(r["objects"], json!([a]));
        assert_eq!(r["warnings"], json!([]));
        // A layer linked to a library preset warns.
        let mut d = s.document().clone();
        d.layers[0].link = Some(doc::PresetLink { library: "mopa".into(), path: "Steel/Blue/fill".into() });
        let r = RemarkCheck::apply(&mut d, serde_json::from_value(json!({"region": region})).unwrap()).unwrap().0;
        assert_eq!(r["warnings"][0]["check"], json!("remark_colour"));
        assert_eq!(r["layers"][0]["preset"], json!("Steel/Blue/fill"));
        // Outside the region, or outside the selection: nothing.
        let r = exec(&mut s, "job.remark_check", json!({"region": [[20, 20], [21, 20], [21, 21]]}));
        assert_eq!(r["objects"], json!([]));
        assert!(s.execute(Invocation { id: "job.remark_check".into(), params: json!({"region": [[0, 0], [1, 1]]}) }, Origin::Ui).is_err());
    }
}
