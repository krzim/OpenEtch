//! Job settings saved with the document: `job.set_optimization` (LightBurn's Optimization Settings),
//! `object.set_cut_properties` (Shape Properties: cut order priority and power scale) and `job.set_rotary`
//! (galvo rotary setup saved with the project). Each is one undo step; partial objects merge like a JSON merge
//! patch (`null` resets a field to its default).

use doc::{Document, ObjectId, ObjectOptions, Optimization, Rotary};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::layer::json_merge_patch;
use super::util::{self, invalid};
use crate::{Command, CommandError, Result, Risk};

fn merged<T: serde::Serialize + serde::de::DeserializeOwned>(cmd: &str, current: &T, patch: &Value) -> Result<T> {
    if !patch.is_object() {
        return Err(invalid(cmd, "expected an object of the fields to change"));
    }
    let mut v = serde_json::to_value(current).map_err(|e| CommandError::Failed(e.to_string()))?;
    json_merge_patch(&mut v, patch);
    serde_json::from_value(v).map_err(|e| invalid(cmd, e.to_string()))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetOptimizationInput {
    /// The fields to change (partial): enabled, order_by ["layer" | "group" | "priority", ...], inner_first,
    /// direction_order (null | top_to_bottom | bottom_to_top | left_to_right | right_to_left), reduce_travel,
    /// reduce_direction_changes, best_start, corners, best_direction, remove_overlaps, overlap_distance_mm, cuts_last.
    #[schemars(with = "Optimization")]
    pub optimization: Value,
}

/// `job.set_optimization`.
pub struct SetOptimization;

impl Command for SetOptimization {
    type Input = SetOptimizationInput;
    const ID: &'static str = "job.set_optimization";
    const TITLE: &'static str = "Optimization settings";
    const DESCRIPTION: &'static str = "Change the cut order settings saved with the document (LightBurn's \
        Optimization Settings). Partial: only the fields given change, null resets one. Layers always burn in their \
        list order (each has its own settings); inside a pass, order_by group/priority sorts the shapes first, then \
        inner_first, direction_order or nearest-first ordering applies. Returns the new `optimization`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetOptimizationInput) -> Result<(Value, String)> {
        let next: Optimization = merged(Self::ID, &doc.optimization, &i.optimization)?;
        if let Some(p) = next.problem() {
            return Err(invalid(Self::ID, p));
        }
        if next == doc.optimization {
            return Err(CommandError::Failed(format!("{}: nothing changed", Self::ID)));
        }
        doc.optimization = next;
        Ok((json!({ "optimization": doc.optimization }), "changed the optimization settings".into()))
    }
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCutPropertiesInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
    /// Cut order priority, 0 first (used when the optimization orders by priority); null removes it.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<u32>")]
    pub priority: Option<Option<u32>>,
    /// Power scale, 0..=100 % of the layer's power.
    pub power_scale_pct: Option<f64>,
}

/// `object.set_cut_properties`.
pub struct SetCutProperties;

impl Command for SetCutProperties {
    type Input = SetCutPropertiesInput;
    const ID: &'static str = "object.set_cut_properties";
    const TITLE: &'static str = "Cut properties";
    const DESCRIPTION: &'static str = "Set objects' cut properties (LightBurn's Shape Properties): `priority` (cut \
        order priority, 0 first, used by Order by Priority; null removes it) and `power_scale_pct` (the shape marks at \
        this percentage of its layer's power, 0..=100). Stored in the document's `object_options`. Returns the \
        affected ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetCutPropertiesInput) -> Result<(Value, String)> {
        let ids = util::object_ids(Self::ID, doc, &i.ids)?;
        if i.priority.is_none() && i.power_scale_pct.is_none() {
            return Err(invalid(Self::ID, "give priority and/or power_scale_pct"));
        }
        if let Some(p) = i.power_scale_pct
            && !(p.is_finite() && (0.0..=100.0).contains(&p))
        {
            return Err(invalid(Self::ID, format!("power_scale_pct must be within 0..=100 (got {p})")));
        }
        let before = doc.object_options.clone();
        for id in &ids {
            let mut o = doc.object_options.get(id).cloned().unwrap_or_default();
            if let Some(p) = i.priority {
                o.priority = p;
            }
            if let Some(s) = i.power_scale_pct {
                o.power_scale_pct = s;
            }
            if o == ObjectOptions::default() {
                doc.object_options.remove(id);
            } else {
                doc.object_options.insert(id.clone(), o);
            }
        }
        doc.object_options.retain(|id, _| doc.objects.iter().any(|o| &o.id == id));
        if doc.object_options == before {
            return Err(CommandError::Failed(format!("{}: nothing changed", Self::ID)));
        }
        Ok((util::edit_result(doc, &[], &ids), format!("set cut properties of {}", ids.join(", "))))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetRotaryInput {
    /// The fields to change (partial: enabled, kind chuck|roller, steps_per_rotation, object_diameter_mm,
    /// roller_diameter_mm, split_width_mm, cylinder_correction, taper {end_diameter_mm, length_mm} or null), or
    /// null to remove the rotary setup from the document.
    #[schemars(with = "Option<Rotary>")]
    pub rotary: Value,
}

/// `job.set_rotary`.
pub struct SetRotary;

impl Command for SetRotary {
    type Input = SetRotaryInput;
    const ID: &'static str = "job.set_rotary";
    const TITLE: &'static str = "Rotary setup";
    const DESCRIPTION: &'static str = "Set the galvo rotary setup saved with the document (LightBurn's galvo Rotary \
        Setup): the work's axis lies along field X and the design's Y is its unwrapped surface; with `enabled` the job \
        is marked in strips of split_width_mm, turning the work between strips with the board's extension axis. \
        Partial (null resets a field); `rotary: null` removes the setup. Returns the new `rotary`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetRotaryInput) -> Result<(Value, String)> {
        let next = if i.rotary.is_null() {
            None
        } else {
            let r: Rotary = merged(Self::ID, &doc.rotary.clone().unwrap_or_default(), &i.rotary)?;
            if let Some(p) = r.problem() {
                return Err(invalid(Self::ID, p));
            }
            Some(r)
        };
        if next == doc.rotary {
            return Err(CommandError::Failed(format!("{}: nothing changed", Self::ID)));
        }
        doc.rotary = next;
        Ok((json!({ "rotary": doc.rotary }), "changed the rotary setup".into()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: serde_json::Value) -> crate::Result<serde_json::Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn session() -> Session {
        let mut s = Session::new(doc::Document::default());
        run(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})).unwrap();
        s
    }

    #[test]
    fn optimization_is_partial_and_validated() {
        let mut s = session();
        run(&mut s, "job.set_optimization", json!({"optimization": {"inner_first": true, "order_by": ["priority", "layer"]}})).unwrap();
        let o = &s.document().optimization;
        assert!(o.inner_first && o.reduce_travel);
        assert_eq!(o.order_by, [doc::OrderBy::Priority, doc::OrderBy::Layer]);
        run(&mut s, "job.set_optimization", json!({"optimization": {"order_by": null}})).unwrap();
        assert_eq!(s.document().optimization.order_by, [doc::OrderBy::Layer], "null resets");
        assert!(run(&mut s, "job.set_optimization", json!({"optimization": {"overlap_distance_mm": -1}})).is_err());
        assert!(run(&mut s, "job.set_optimization", json!({"optimization": {"bogus": 1}})).is_err());
    }

    #[test]
    fn cut_properties_are_stored_per_object() {
        let mut s = session();
        let id = s.document().objects[0].id.clone();
        run(&mut s, "object.set_cut_properties", json!({"ids": [id], "priority": 2, "power_scale_pct": 50})).unwrap();
        let o = &s.document().object_options[&id];
        assert_eq!((o.priority, o.power_scale_pct), (Some(2), 50.0));
        run(&mut s, "object.set_cut_properties", json!({"ids": [id], "priority": null, "power_scale_pct": 100})).unwrap();
        assert!(s.document().object_options.is_empty(), "defaults are not kept");
        assert!(run(&mut s, "object.set_cut_properties", json!({"ids": [id], "power_scale_pct": 150})).is_err());
        assert!(run(&mut s, "object.set_cut_properties", json!({"ids": ["obj-404"], "priority": 1})).is_err());
    }

    #[test]
    fn rotary_is_set_and_removed() {
        let mut s = session();
        run(&mut s, "job.set_rotary", json!({"rotary": {"enabled": true, "object_diameter_mm": 70}})).unwrap();
        let r = s.document().rotary.clone().unwrap();
        assert!(r.enabled && r.object_diameter_mm == 70.0 && r.cylinder_correction);
        assert!(run(&mut s, "job.set_rotary", json!({"rotary": {"split_width_mm": 500}})).is_err());
        run(&mut s, "job.set_rotary", json!({"rotary": null})).unwrap();
        assert!(s.document().rotary.is_none());
    }
}
