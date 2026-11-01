//! Material library presets on layers (LightBurn's Library window): `layer.apply_preset` (Assign, Link, and the
//! layer defaults' Reset to Default) and `layer.unlink_preset`. The engine owns the libraries and the defaults and
//! passes the preset's settings in the invocation, so the timeline replays without them.

use std::collections::BTreeMap;

use doc::{CutSettings, Document, PresetLink};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::{MAX_PASSES, validate_settings};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The `extras` key LightBurn's `LinkPath` is kept under (written back to `.lbrn2` exports).
pub const LINK_PATH_KEY: &str = "lbrn2.LinkPath";

/// Most sub-layers a layer may have (as `layer.sub_add`).
const MAX_SUB_LAYERS: usize = 64;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPresetInput {
    /// Exact layer id.
    pub layer: String,
    /// The preset's main settings (complete).
    pub settings: CutSettings,
    /// The preset's sub-layers (complete; replaces the layer's). Default none.
    #[serde(default)]
    pub sub_layers: Vec<CutSettings>,
    /// Whole-layer repeats (LightBurn's global passes). Default: unchanged.
    pub global_passes: Option<u32>,
    /// LightBurn values the preset keeps for export (`lbrn2.*` keys, e.g. `lbrn2.minPower`); the layer's own
    /// `lbrn2.*` cut values are replaced by these (its LightBurn index is kept). Default none.
    #[serde(default)]
    pub extras: BTreeMap<String, String>,
    /// Link the layer to this preset (LightBurn's Link: its settings follow the library and are not edited
    /// directly); `null` or absent assigns a copy (LightBurn's Assign) and removes any link.
    pub link: Option<PresetLink>,
    /// New layer name. Default: unchanged.
    pub name: Option<String>,
}

/// `layer.apply_preset`.
pub struct ApplyPreset;

impl Command for ApplyPreset {
    type Input = ApplyPresetInput;
    const ID: &'static str = "layer.apply_preset";
    const TITLE: &'static str = "Apply preset to layer";
    const DESCRIPTION: &'static str = "Replace a layer's cut settings and sub-layers with a material library preset \
        (LightBurn's Assign) or link the layer to it (`link: {library, path}`, LightBurn's Link: the engine updates \
        linked layers when the preset changes). Also used for Reset to Default. The settings are complete \
        CutSettings objects (as GET /api/document shows them). Name, colour, visibility and output are kept. \
        Returns the layer and its new settings.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ApplyPresetInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let id = util::layer(doc, &i.layer)?.id.clone();
            validate_settings(cmd, &i.settings)?;
            if i.sub_layers.len() > MAX_SUB_LAYERS {
                return Err(invalid(cmd, format!("at most {MAX_SUB_LAYERS} sub-layers")));
            }
            for (n, s) in i.sub_layers.iter().enumerate() {
                validate_settings(cmd, s).map_err(|e| invalid(cmd, format!("sub_layers[{n}]: {e}")))?;
            }
            if let Some(g) = i.global_passes
                && !(1..=MAX_PASSES).contains(&g)
            {
                return Err(invalid(cmd, format!("global_passes must be within 1..={MAX_PASSES} (got {g})")));
            }
            if let Some((k, _)) = i.extras.iter().find(|(k, _)| !k.starts_with(lbrn::EXTRA_PREFIX) || !doc::is_extras_key(k)) {
                return Err(invalid(cmd, format!("extras key '{k}' is not a LightBurn value (lbrn2.<tag>)")));
            }
            if let Some(l) = &i.link
                && (l.library.trim().is_empty() || l.path.trim().is_empty() || l.path.len() > 1000 || l.library.len() > 200)
            {
                return Err(invalid(cmd, "link needs a library name and a material/title/description path"));
            }
            let name = i.name.as_deref().map(|n| util::name(cmd, "name", n)).transpose()?;
            let layer = doc.layer_mut(&id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
            let index_key = format!("{}index", lbrn::EXTRA_PREFIX);
            let index = layer.extras.get(&index_key).cloned();
            // The layer's LightBurn cut values belong to its old settings; the preset brings its own.
            layer.extras.retain(|k, _| !k.starts_with(lbrn::EXTRA_PREFIX) || k.starts_with("lbrn2._img"));
            layer.extras.extend(i.extras.into_iter().filter(|(k, _)| *k != index_key && k != LINK_PATH_KEY));
            if let Some(ix) = index {
                layer.extras.insert(index_key, ix);
            }
            layer.settings = i.settings;
            layer.sub_layers = i.sub_layers;
            if let Some(g) = i.global_passes {
                layer.global_passes = g;
            }
            if let Some(n) = name {
                layer.name = n;
            }
            match &i.link {
                Some(l) => {
                    layer.extras.insert(LINK_PATH_KEY.into(), l.path.clone());
                }
                None => {
                    layer.extras.remove(LINK_PATH_KEY);
                }
            }
            layer.link = i.link.clone();
            let summary = match &i.link {
                Some(l) => format!("linked layer {id} to preset \"{}\" ({})", l.path, l.library),
                None => format!("applied settings to layer {id}"),
            };
            let l = doc.layer(&id).expect("just edited");
            Ok((
                json!({
                    "created": [], "affected": [id], "settings": l.settings, "sub_layers": l.sub_layers,
                    "link": l.link, "bounds": Value::Null,
                }),
                summary,
            ))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnlinkPresetInput {
    /// Exact layer id.
    pub layer: String,
}

/// `layer.unlink_preset`.
pub struct UnlinkPreset;

impl Command for UnlinkPreset {
    type Input = UnlinkPresetInput;
    const ID: &'static str = "layer.unlink_preset";
    const TITLE: &'static str = "Unlink layer from preset";
    const DESCRIPTION: &'static str = "Remove a layer's link to its material library preset; its settings stay as \
        they are and can be edited again.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: UnlinkPresetInput) -> Result<(Value, String)> {
        let id = util::layer(doc, &i.layer)?.id.clone();
        let layer = doc.layer_mut(&id).expect("looked up");
        if layer.link.take().is_none() {
            return Err(invalid(Self::ID, format!("layer {id} is not linked to a preset")));
        }
        layer.extras.remove(LINK_PATH_KEY);
        Ok((json!({ "created": [], "affected": [id], "bounds": Value::Null }), format!("unlinked layer {id}")))
    }
}

/// Refuse a direct settings edit of a linked layer (LightBurn: a linked layer cannot be edited in the Cut Settings
/// Editor).
pub(crate) fn check_not_linked(cmd: &str, doc: &Document, layer: &str) -> Result<()> {
    match doc.layer(layer).and_then(|l| l.link.as_ref()) {
        Some(l) => Err(invalid(
            cmd,
            format!("layer {layer} is linked to preset \"{}\" ({}); edit the preset or unlink the layer first", l.path, l.library),
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: Value) -> std::result::Result<Value, String> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result).map_err(|e| e.to_string())
    }

    #[test]
    fn assign_link_and_unlink() {
        let mut s = Session::new(Document::default());
        let mut settings = CutSettings { power_pct: 55.0, ..CutSettings::default() };
        settings.frequency_khz = 200.0;
        let sub = CutSettings { power_pct: 5.0, ..CutSettings::default() };
        let r = run(
            &mut s,
            "layer.apply_preset",
            json!({ "layer": "layer-1", "settings": settings, "sub_layers": [sub], "extras": {"lbrn2.minPower": "7"} }),
        )
        .unwrap();
        assert_eq!(r["settings"]["power_pct"], 55.0);
        let l = &s.document().layers[0];
        assert_eq!((l.sub_layers.len(), l.link.is_none()), (1, true));
        assert_eq!(l.extras.get("lbrn2.minPower").map(String::as_str), Some("7"));

        let link = json!({"library": "steel", "path": "Steel/Finish/Engrave"});
        run(&mut s, "layer.apply_preset", json!({ "layer": "layer-1", "settings": settings, "link": link })).unwrap();
        let l = &s.document().layers[0];
        assert_eq!(l.link.as_ref().map(|l| l.path.as_str()), Some("Steel/Finish/Engrave"));
        assert_eq!(l.extras.get(LINK_PATH_KEY).map(String::as_str), Some("Steel/Finish/Engrave"));
        assert!(l.sub_layers.is_empty() && !l.extras.contains_key("lbrn2.minPower"));
        // Linked layers are not edited directly.
        let e = run(&mut s, "layer.update", json!({ "id": "layer-1", "settings": {"power_pct": 10} })).unwrap_err();
        assert!(e.contains("linked"), "{e}");
        run(&mut s, "layer.update", json!({ "id": "layer-1", "name": "Renamed" })).unwrap();
        // Sub-layers are part of the preset too (before: they could be added and edited on a linked layer).
        let e = run(&mut s, "layer.sub_add", json!({ "layer": "layer-1" })).unwrap_err();
        assert!(e.contains("linked"), "{e}");
        assert!(s.document().layers[0].sub_layers.is_empty());
        run(&mut s, "layer.unlink_preset", json!({ "layer": "layer-1" })).unwrap();
        run(&mut s, "layer.sub_add", json!({ "layer": "layer-1" })).unwrap();
        assert!(s.document().layers[0].link.is_none());
        run(&mut s, "layer.update", json!({ "id": "layer-1", "settings": {"power_pct": 10} })).unwrap();
        assert!(run(&mut s, "layer.unlink_preset", json!({ "layer": "layer-1" })).is_err());
    }

    #[test]
    fn refuses_bad_presets() {
        let mut s = Session::new(Document::default());
        let bad = CutSettings { power_pct: 150.0, ..CutSettings::default() };
        assert!(run(&mut s, "layer.apply_preset", json!({ "layer": "layer-1", "settings": bad })).is_err());
        let ok = CutSettings::default();
        assert!(run(&mut s, "layer.apply_preset", json!({ "layer": "layer-9", "settings": ok })).is_err());
        let e = run(&mut s, "layer.apply_preset", json!({ "layer": "layer-1", "settings": ok, "extras": {"x": "1"} }));
        assert!(e.is_err());
        let e = run(&mut s, "layer.apply_preset", json!({ "layer": "layer-1", "settings": ok, "link": {"library": "", "path": "a"} }));
        assert!(e.is_err());
    }
}
