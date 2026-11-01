//! Sub-layer commands (LightBurn's sub-layer tabs): `layer.sub_add`, `layer.sub_update`, `layer.sub_delete`,
//! `layer.sub_move`, `layer.sub_duplicate`. A sub-layer is one more set of [`CutSettings`] run over the layer's
//! objects after its main settings ([`doc::Layer::sub_layers`]); a cleanup sub-layer runs between the main passes
//! instead ([`doc::Cleanup`]). Indices are 0-based positions in `sub_layers` (the main settings are not counted).
//!
//! Data kept for other software per sub-layer (`extras["<format>.sub<N>.<tag>"]`, N the 1-based position, e.g.
//! LightBurn's `lbrn2.sub2.minPower`) moves with its sub-layer when sub-layers are deleted, moved or duplicated.

use std::collections::BTreeMap;

use doc::{Cleanup, CutSettings, Document, LayerId};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::layer::{SettingsPatch, check_cleanup, validate_settings};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Most sub-layers one layer may have. Each one replans the layer's objects, so the count is bounded like `passes`.
pub const MAX_SUB_LAYERS: usize = 64;

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// `sub3.speed` → `(3, "speed")` for an extras key `<format>.sub3.speed` (the tag after the format prefix).
fn sub_position(key: &str) -> Option<(&str, usize, &str)> {
    let (format, rest) = key.split_once('.')?;
    let rest = rest.strip_prefix("sub")?;
    let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|d| *d > 0)?;
    let (n, tag) = rest.split_at(digits);
    Some((format, n.parse().ok()?, tag.strip_prefix('.')?))
}

/// Renumber the per-sub-layer extras of a layer: `to(n)` gives the new 1-based position of sub-layer `n`, or `None`
/// to drop its extras. Other keys are untouched.
fn remap_sub_extras(extras: &mut BTreeMap<String, String>, to: impl Fn(usize) -> Option<usize>) {
    let old = std::mem::take(extras);
    for (k, v) in old {
        match sub_position(&k) {
            Some((format, n, tag)) => {
                if let Some(m) = to(n) {
                    extras.insert(format!("{format}.sub{m}.{tag}"), v);
                }
            }
            None => {
                extras.insert(k, v);
            }
        }
    }
}

/// Copies of the extras of sub-layer `from` (1-based) for position `to`.
fn copied_sub_extras(extras: &BTreeMap<String, String>, from: usize, to: usize) -> Vec<(String, String)> {
    extras
        .iter()
        .filter_map(|(k, v)| match sub_position(k) {
            Some((format, n, tag)) if n == from => Some((format!("{format}.sub{to}.{tag}"), v.clone())),
            _ => None,
        })
        .collect()
}

/// The sub-layer at `index` of `layer`, or an error naming the valid range.
fn check_index(cmd: &str, field: &str, layer: &doc::Layer, index: usize) -> Result<()> {
    match layer.sub_layers.len() {
        0 => Err(invalid(cmd, format!("layer {} has no sub-layers", layer.id))),
        n if index >= n => Err(invalid(
            cmd,
            format!("{field} {index} is out of range: layer {} has sub-layers 0..={}", layer.id, n - 1),
        )),
        _ => Ok(()),
    }
}

/// How a sub-layer is called in summaries: `1 "Cleanup"` or `1`.
fn label(index: usize, s: &CutSettings) -> String {
    match &s.name {
        Some(n) => format!("{index} \"{n}\""),
        None => index.to_string(),
    }
}

/// The result of a sub-layer edit: the layer, its objects and bounds, the sub-layer `index` (when one remains) and the
/// layer's sub-layers after the edit.
fn result(doc: &Document, layer: &LayerId, index: Option<usize>) -> Value {
    let objs: Vec<String> = doc.objects.iter().filter(|o| &o.layer == layer).map(|o| o.id.clone()).collect();
    let subs = doc.layer(layer).map(|l| l.sub_layers.clone()).unwrap_or_default();
    json!({
        "created": [],
        "affected": [layer],
        "objects": objs,
        "bounds": util::bounds_json(doc, &objs),
        "index": index,
        "sub_layer": index.and_then(|i| subs.get(i)).map(|s| serde_json::to_value(s).unwrap_or(Value::Null)),
        "sub_layers": serde_json::to_value(&subs).unwrap_or(Value::Null),
    })
}

fn layer_mut<'a>(doc: &'a mut Document, id: &str) -> Result<&'a mut doc::Layer> {
    doc.layer_mut(id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubAddInput {
    /// Exact layer id, e.g. "layer-1".
    pub layer: String,
    /// Settings to change from a copy of the layer's main settings (partial, as in `layer.update`).
    pub settings: Option<SettingsPatch>,
    /// Display name, e.g. "Cleanup" (LightBurn's sub-layer name).
    pub name: Option<String>,
    /// Make it a cleanup pass: {"every_n_passes": n} runs it after every n main passes and after the last one.
    pub cleanup: Option<Cleanup>,
    /// Position in `sub_layers` (0 = right after the main settings); defaults to the end.
    pub index: Option<usize>,
}

/// `layer.sub_add`.
pub struct SubAdd;

impl Command for SubAdd {
    type Input = SubAddInput;
    const ID: &'static str = "layer.sub_add";
    const TITLE: &'static str = "Add sub-layer";
    const DESCRIPTION: &'static str = "Add a sub-layer to a layer (LightBurn's sub-layers): one more pass over the \
        layer's objects with its own settings, run after the main settings. It starts as a copy of the main settings \
        with `settings` merged in (partial, as in layer.update). `cleanup` {every_n_passes} makes it a cleanup pass \
        that runs after every n main passes and after the last (LightBurn's 3D-slice cleanup); a typical cleanup is a \
        fast, low-power single pass with the hatch turned 45 degrees. Returns `index`, its 0-based position in \
        `sub_layers`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: SubAddInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.layer)?.id.clone();
            let name = input.name.as_deref().map(|n| util::name(Self::ID, "name", n)).transpose()?;
            if let Some(c) = &input.cleanup {
                check_cleanup(Self::ID, c)?;
            }
            let layer = layer_mut(doc, &id)?;
            let count = layer.sub_layers.len();
            if count >= MAX_SUB_LAYERS {
                return Err(invalid(
                    Self::ID,
                    format!("layer {id} already has the most sub-layers allowed ({MAX_SUB_LAYERS})"),
                ));
            }
            let index = input.index.unwrap_or(count);
            if index > count {
                return Err(invalid(Self::ID, format!("index {index} is out of range: use 0..={count}")));
            }
            let mut s = CutSettings { name: None, cleanup: None, ..layer.settings.clone() };
            match input.settings {
                Some(p) => p.apply(Self::ID, &mut s)?,
                None => validate_settings(Self::ID, &s)?,
            }
            s.name = name;
            s.cleanup = input.cleanup;
            let summary = format!("added sub-layer {} to layer {id}", label(index, &s));
            // Sub-layers from `index` on move one place down, and their kept data with them.
            remap_sub_extras(&mut layer.extras, |n| Some(if n > index { n + 1 } else { n }));
            layer.sub_layers.insert(index, s);
            Ok((result(doc, &id, Some(index)), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubUpdateInput {
    /// Exact layer id.
    pub layer: String,
    /// 0-based position of the sub-layer in `sub_layers`.
    pub index: usize,
    /// Partial settings merged into the sub-layer's: {"power_pct": 10} changes only power.
    pub settings: Option<SettingsPatch>,
    /// New display name; `null` clears it.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    pub name: Option<Option<String>>,
    /// {"every_n_passes": n} makes it a cleanup pass; `null` makes it an ordinary sub-layer again.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<Cleanup>")]
    pub cleanup: Option<Option<Cleanup>>,
}

/// `layer.sub_update`.
pub struct SubUpdate;

impl Command for SubUpdate {
    type Input = SubUpdateInput;
    const ID: &'static str = "layer.sub_update";
    const TITLE: &'static str = "Update sub-layer";
    const DESCRIPTION: &'static str = "Change a sub-layer's cut settings, name or cleanup. `settings` is merged into \
        the existing ones as in layer.update (power %, speed mm/s, frequency kHz, pulse width ns, passes, hatch). \
        `name: null` clears the name; `cleanup: null` makes a cleanup pass an ordinary sub-layer again.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: SubUpdateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.layer)?.id.clone();
            check_index(Self::ID, "index", util::layer(doc, &id)?, input.index)?;
            let name = match &input.name {
                Some(Some(n)) => Some(Some(util::name(Self::ID, "name", n)?)),
                Some(None) => Some(None),
                None => None,
            };
            if let Some(Some(c)) = &input.cleanup {
                check_cleanup(Self::ID, c)?;
            }
            let layer = layer_mut(doc, &id)?;
            let s = &mut layer.sub_layers[input.index];
            let mut changed = Vec::new();
            if let Some(p) = input.settings {
                let sets_frequency = p.frequency_khz.is_some();
                p.apply(Self::ID, s)?;
                if sets_frequency {
                    // An imported LightBurn sub-layer without a frequency now has one to export.
                    layer.extras.remove(&format!(
                        "{}sub{}.{}",
                        lbrn::EXTRA_PREFIX,
                        input.index + 1,
                        lbrn::FREQUENCY_ABSENT
                    ));
                }
                changed.push("settings");
            }
            let s = &mut layer.sub_layers[input.index];
            if let Some(n) = name {
                s.name = n;
                changed.push("name");
            }
            if let Some(c) = input.cleanup {
                s.cleanup = c;
                changed.push("cleanup");
            }
            if changed.is_empty() {
                return Err(invalid(Self::ID, "nothing to change: give at least one of settings, name, cleanup"));
            }
            let summary = format!("updated sub-layer {} of layer {id} ({})", label(input.index, s), changed.join(", "));
            Ok((result(doc, &id, Some(input.index)), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubDeleteInput {
    /// Exact layer id.
    pub layer: String,
    /// 0-based position of the sub-layer to delete.
    pub index: usize,
}

/// `layer.sub_delete`.
pub struct SubDelete;

impl Command for SubDelete {
    type Input = SubDeleteInput;
    const ID: &'static str = "layer.sub_delete";
    const TITLE: &'static str = "Delete sub-layer";
    const DESCRIPTION: &'static str = "Delete one sub-layer of a layer (the main settings cannot be deleted). Later \
        sub-layers move up one place.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: SubDeleteInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.layer)?.id.clone();
            check_index(Self::ID, "index", util::layer(doc, &id)?, input.index)?;
            let layer = layer_mut(doc, &id)?;
            let removed = layer.sub_layers.remove(input.index);
            let gone = input.index + 1;
            remap_sub_extras(&mut layer.extras, |n| match n.cmp(&gone) {
                std::cmp::Ordering::Less => Some(n),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(n - 1),
            });
            let summary = format!("deleted sub-layer {} of layer {id}", label(input.index, &removed));
            Ok((result(doc, &id, None), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubMoveInput {
    /// Exact layer id.
    pub layer: String,
    /// 0-based position of the sub-layer to move.
    pub from: usize,
    /// 0-based position it ends up at; the others keep their order.
    pub to: usize,
}

/// `layer.sub_move`.
pub struct SubMove;

impl Command for SubMove {
    type Input = SubMoveInput;
    const ID: &'static str = "layer.sub_move";
    const TITLE: &'static str = "Move sub-layer";
    const DESCRIPTION: &'static str = "Move a sub-layer to another position in the layer's sub-layer list, which is \
        their burn order (after the main settings). The others keep their order.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: SubMoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.layer)?.id.clone();
            let layer = util::layer(doc, &id)?;
            check_index(Self::ID, "from", layer, input.from)?;
            check_index(Self::ID, "to", layer, input.to)?;
            if input.from == input.to {
                return Err(invalid(Self::ID, format!("sub-layer {} is already at position {}", input.from, input.to)));
            }
            let layer = layer_mut(doc, &id)?;
            let s = layer.sub_layers.remove(input.from);
            let summary = format!("moved sub-layer {} of layer {id} to position {}", label(input.from, &s), input.to);
            layer.sub_layers.insert(input.to, s);
            let (from, to) = (input.from + 1, input.to + 1);
            remap_sub_extras(&mut layer.extras, |n| {
                Some(if n == from {
                    to
                } else if from < to && n > from && n <= to {
                    n - 1
                } else if to < from && n >= to && n < from {
                    n + 1
                } else {
                    n
                })
            });
            Ok((result(doc, &id, Some(input.to)), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubDuplicateInput {
    /// Exact layer id.
    pub layer: String,
    /// 0-based position of the sub-layer to copy.
    pub index: usize,
}

/// `layer.sub_duplicate`.
pub struct SubDuplicate;

impl Command for SubDuplicate {
    type Input = SubDuplicateInput;
    const ID: &'static str = "layer.sub_duplicate";
    const TITLE: &'static str = "Duplicate sub-layer";
    const DESCRIPTION: &'static str = "Copy a sub-layer (settings, name, cleanup and kept data) to the position right \
        after it. Returns the copy's `index`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: SubDuplicateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.layer)?.id.clone();
            check_index(Self::ID, "index", util::layer(doc, &id)?, input.index)?;
            let layer = layer_mut(doc, &id)?;
            if layer.sub_layers.len() >= MAX_SUB_LAYERS {
                return Err(invalid(
                    Self::ID,
                    format!("layer {id} already has the most sub-layers allowed ({MAX_SUB_LAYERS})"),
                ));
            }
            let copy = layer.sub_layers[input.index].clone();
            let at = input.index + 1;
            let summary = format!("duplicated sub-layer {} of layer {id} as {at}", label(input.index, &copy));
            // 1-based: the original is `at`, the copy `at + 1`, later ones move down.
            remap_sub_extras(&mut layer.extras, |n| Some(if n > at { n + 1 } else { n }));
            let kept = copied_sub_extras(&layer.extras, at, at + 1);
            layer.extras.extend(kept);
            layer.sub_layers.insert(at, copy);
            Ok((result(doc, &id, Some(at)), summary))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn exec(s: &mut Session, id: &str, params: Value) -> Result<Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Agent("test".into())).map(|o| o.result)
    }

    fn ok(s: &mut Session, id: &str, params: Value) -> Value {
        exec(s, id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn subs(s: &Session) -> Vec<CutSettings> {
        s.document().layers[0].sub_layers.clone()
    }

    fn names(s: &Session) -> Vec<Option<String>> {
        subs(s).into_iter().map(|c| c.name).collect()
    }

    /// A session whose layer-1 has power 50 % and sub-layers "a", "b", "c" with LightBurn extras for each.
    fn session() -> Session {
        session_with(|_| {})
    }

    fn session_with(f: impl FnOnce(&mut Document)) -> Session {
        let mut d = Document::default();
        d.layers[0].settings.power_pct = 50.0;
        for (n, name) in ["a", "b", "c"].into_iter().enumerate() {
            d.layers[0].sub_layers.push(CutSettings { name: Some(name.into()), ..CutSettings::default() });
            d.layers[0].extras.insert(format!("lbrn2.sub{}.minPower", n + 1), name.into());
        }
        d.layers[0].extras.insert("lbrn2.minPower".into(), "main".into());
        d.layers[0].extras.insert("lbrn2.subname".into(), "main".into());
        f(&mut d);
        Session::new(d)
    }

    fn extra(s: &Session, k: &str) -> Option<String> {
        s.document().layers[0].extras.get(k).cloned()
    }

    #[test]
    fn add_copies_the_main_settings_and_inserts_where_asked() {
        let mut s = session();
        let r = ok(
            &mut s,
            "layer.sub_add",
            json!({"layer": "layer-1", "settings": {"speed_mm_s": 4000}, "name": "Cleanup", "cleanup": {"every_n_passes": 2}, "index": 1}),
        );
        assert_eq!(r["index"], 1);
        assert_eq!(r["sub_layers"].as_array().unwrap().len(), 4);
        let new = &subs(&s)[1];
        assert_eq!((new.power_pct, new.speed_mm_s), (50.0, 4000.0), "copied from the main settings, patch applied");
        assert_eq!(new.cleanup, Some(Cleanup { every_n_passes: 2 }));
        assert_eq!(names(&s), [Some("a".into()), Some("Cleanup".into()), Some("b".into()), Some("c".into())]);
        // Kept data moved with b and c.
        assert_eq!(extra(&s, "lbrn2.sub1.minPower").as_deref(), Some("a"));
        assert_eq!(extra(&s, "lbrn2.sub2.minPower"), None);
        assert_eq!(extra(&s, "lbrn2.sub3.minPower").as_deref(), Some("b"));
        assert_eq!(extra(&s, "lbrn2.sub4.minPower").as_deref(), Some("c"));
        assert_eq!(extra(&s, "lbrn2.subname").as_deref(), Some("main"), "the main settings' tags stay");
        // Appends by default; a plain add copies the main settings without name or cleanup.
        let r = ok(&mut s, "layer.sub_add", json!({"layer": "layer-1"}));
        assert_eq!(r["index"], 4);
        assert_eq!(subs(&s)[4], CutSettings { name: None, cleanup: None, ..s.document().layers[0].settings.clone() });
        s.undo().unwrap();
        s.undo().unwrap();
        assert_eq!(names(&s), [Some("a".into()), Some("b".into()), Some("c".into())]);
        assert_eq!(extra(&s, "lbrn2.sub2.minPower").as_deref(), Some("b"));
    }

    #[test]
    fn add_validates_like_layer_update() {
        let mut s = session();
        for bad in [
            json!({"layer": "layer-1", "settings": {"power_pct": 140}}),
            json!({"layer": "layer-1", "settings": {"passes": 1001}}),
            json!({"layer": "layer-1", "cleanup": {"every_n_passes": 0}}),
            json!({"layer": "layer-1", "name": "  "}),
            json!({"layer": "layer-1", "index": 4}),
            json!({"layer": "layer-1", "settings": {"power": 3}}),
            json!({"layer": "layer-9"}),
        ] {
            assert!(exec(&mut s, "layer.sub_add", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(subs(&s).len(), 3);
        for _ in 3..MAX_SUB_LAYERS {
            ok(&mut s, "layer.sub_add", json!({"layer": "layer-1"}));
        }
        let e = exec(&mut s, "layer.sub_add", json!({"layer": "layer-1"})).unwrap_err().to_string();
        assert!(e.contains("most sub-layers"), "{e}");
        assert!(exec(&mut s, "layer.sub_duplicate", json!({"layer": "layer-1", "index": 0})).is_err());
    }

    #[test]
    fn update_merges_settings_and_sets_or_clears_name_and_cleanup() {
        let mut s = session_with(|d| {
            d.layers[0].extras.insert("lbrn2.sub2.frequency_absent".into(), "1".into());
        });
        ok(
            &mut s,
            "layer.sub_update",
            json!({"layer": "layer-1", "index": 1, "settings": {"power_pct": 10, "hatch": {"angle_deg": 45}}, "cleanup": {"every_n_passes": 3}}),
        );
        let b = &subs(&s)[1];
        assert_eq!((b.power_pct, b.hatch.angle_deg, b.speed_mm_s), (10.0, 45.0, CutSettings::default().speed_mm_s));
        assert_eq!(b.cleanup, Some(Cleanup { every_n_passes: 3 }));
        assert_eq!(extra(&s, "lbrn2.sub2.frequency_absent").as_deref(), Some("1"));
        ok(
            &mut s,
            "layer.sub_update",
            json!({"layer": "layer-1", "index": 1, "settings": {"frequency_khz": 60}, "name": null, "cleanup": null}),
        );
        let b = &subs(&s)[1];
        assert_eq!((b.name.as_ref(), b.cleanup.as_ref(), b.frequency_khz), (None, None, 60.0));
        assert_eq!(extra(&s, "lbrn2.sub2.frequency_absent"), None, "a set frequency is exported");
        for bad in [
            json!({"layer": "layer-1", "index": 1}),
            json!({"layer": "layer-1", "index": 3, "name": "x"}),
            json!({"layer": "layer-1", "index": 0, "settings": {"speed_mm_s": 0}}),
            json!({"layer": "layer-1", "index": 0, "cleanup": {"every_n_passes": 1001}}),
        ] {
            assert!(exec(&mut s, "layer.sub_update", bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(subs(&s)[0].name.as_deref(), Some("a"));
    }

    #[test]
    fn delete_move_and_duplicate_carry_their_kept_data() {
        let mut s = session();
        ok(&mut s, "layer.sub_delete", json!({"layer": "layer-1", "index": 0}));
        assert_eq!(names(&s), [Some("b".into()), Some("c".into())]);
        assert_eq!(extra(&s, "lbrn2.sub1.minPower").as_deref(), Some("b"));
        assert_eq!(extra(&s, "lbrn2.sub2.minPower").as_deref(), Some("c"));
        assert_eq!(extra(&s, "lbrn2.sub3.minPower"), None);
        s.undo().unwrap();

        let r = ok(&mut s, "layer.sub_move", json!({"layer": "layer-1", "from": 0, "to": 2}));
        assert_eq!(r["index"], 2);
        assert_eq!(names(&s), [Some("b".into()), Some("c".into()), Some("a".into())]);
        let kept: Vec<_> = (1..=3).map(|n| extra(&s, &format!("lbrn2.sub{n}.minPower")).unwrap()).collect();
        assert_eq!(kept, ["b", "c", "a"]);
        ok(&mut s, "layer.sub_move", json!({"layer": "layer-1", "from": 2, "to": 0}));
        let kept: Vec<_> = (1..=3).map(|n| extra(&s, &format!("lbrn2.sub{n}.minPower")).unwrap()).collect();
        assert_eq!(kept, ["a", "b", "c"]);
        assert!(exec(&mut s, "layer.sub_move", json!({"layer": "layer-1", "from": 1, "to": 1})).is_err());
        assert!(exec(&mut s, "layer.sub_move", json!({"layer": "layer-1", "from": 1, "to": 3})).is_err());

        let r = ok(&mut s, "layer.sub_duplicate", json!({"layer": "layer-1", "index": 0}));
        assert_eq!(r["index"], 1);
        assert_eq!(names(&s), [Some("a".into()), Some("a".into()), Some("b".into()), Some("c".into())]);
        let kept: Vec<_> = (1..=4).map(|n| extra(&s, &format!("lbrn2.sub{n}.minPower")).unwrap()).collect();
        assert_eq!(kept, ["a", "a", "b", "c"]);
        assert!(exec(&mut s, "layer.sub_delete", json!({"layer": "layer-1", "index": 4})).is_err());
        assert_eq!(s.history().len(), 3, "each edit is one undo step (the undone delete was discarded)");
    }

    #[test]
    fn a_loaded_cleanup_interval_of_zero_is_refused() {
        let mut d = Document::default();
        d.layers[0]
            .sub_layers
            .push(CutSettings { cleanup: Some(Cleanup { every_n_passes: 0 }), ..CutSettings::default() });
        let content = doc::file::try_to_openetch_json(&d).unwrap();
        let e = exec(&mut Session::new(Document::default()), "doc.load", json!({ "content": content })).unwrap_err();
        assert!(e.to_string().contains("every_n_passes"), "{e}");
    }

    #[test]
    fn sub_layer_keys_are_recognised() {
        assert_eq!(sub_position("lbrn2.sub12.minPower"), Some(("lbrn2", 12, "minPower")));
        assert_eq!(sub_position("lbrn2.sub1._src"), Some(("lbrn2", 1, "_src")));
        for not in ["lbrn2.subname", "lbrn2.sub.x", "lbrn2.sub1", "lbrn2.minPower", "sub1.x"] {
            assert_eq!(sub_position(not), None, "{not}");
        }
    }
}
