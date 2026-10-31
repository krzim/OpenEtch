//! Layer commands: `layer.add`, `layer.update`, `layer.delete`, and the partial cut-settings patch they share.

use doc::{CutSettings, Document, FillMode, GalvoTiming, Layer, LayerId};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Default display colours for new layers, in order (LightBurn-like).
const PALETTE: &[&str] = &["#1f77b4", "#d62728", "#2ca02c", "#ff7f0e", "#9467bd", "#8c564b", "#e377c2", "#17becf"];

/// A partial [`CutSettings`]: only the fields given are changed, everything else keeps its current value.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    /// "line" traces outlines, "fill" hatches closed shapes, "fill_then_line" does both.
    pub mode: Option<FillMode>,
    /// Laser power, 0..=100 % of the source's rated power.
    pub power_pct: Option<f64>,
    /// Marking speed in mm/s.
    pub speed_mm_s: Option<f64>,
    /// Pulse repetition frequency in kHz.
    pub frequency_khz: Option<f64>,
    /// MOPA pulse width in ns; `null` leaves the source at its current setting.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<f64>")]
    pub pulse_width_ns: Option<Option<f64>>,
    /// Number of times the layer is marked, >= 1.
    pub passes: Option<u32>,
    /// Hatch fill settings (partial: only the fields given change).
    pub hatch: Option<HatchPatch>,
    /// Galvo timing in µs (all fields required), or `null` to use the machine profile's defaults.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<GalvoTiming>")]
    pub timing: Option<Option<GalvoTiming>>,
}

/// A partial [`doc::Hatch`].
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HatchPatch {
    /// Hatch line spacing in mm, > 0.
    pub interval_mm: Option<f64>,
    /// Hatch angle in degrees, counter-clockwise from +X.
    pub angle_deg: Option<f64>,
    /// Degrees added to the hatch angle on every pass.
    pub angle_increment_deg: Option<f64>,
    /// Add a second hatch at +90°.
    pub cross_hatch: Option<bool>,
    /// Mark alternate hatch lines in opposite directions.
    pub bidirectional: Option<bool>,
}

/// Distinguishes an absent field (`None`) from an explicit `null` (`Some(None)`).
fn double_option<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

impl SettingsPatch {
    /// Merge into `s`, then validate the result. `s` is unchanged on error.
    pub fn apply(self, cmd: &str, s: &mut CutSettings) -> Result<()> {
        let mut n = s.clone();
        if let Some(v) = self.mode {
            n.mode = v;
        }
        if let Some(v) = self.power_pct {
            n.power_pct = v;
        }
        if let Some(v) = self.speed_mm_s {
            n.speed_mm_s = v;
        }
        if let Some(v) = self.frequency_khz {
            n.frequency_khz = v;
        }
        if let Some(v) = self.pulse_width_ns {
            n.pulse_width_ns = v;
        }
        if let Some(v) = self.passes {
            n.passes = v;
        }
        if let Some(h) = self.hatch {
            if let Some(v) = h.interval_mm {
                n.hatch.interval_mm = v;
            }
            if let Some(v) = h.angle_deg {
                n.hatch.angle_deg = v;
            }
            if let Some(v) = h.angle_increment_deg {
                n.hatch.angle_increment_deg = v;
            }
            if let Some(v) = h.cross_hatch {
                n.hatch.cross_hatch = v;
            }
            if let Some(v) = h.bidirectional {
                n.hatch.bidirectional = v;
            }
        }
        if let Some(v) = self.timing {
            n.timing = v;
        }
        validate_settings(cmd, &n)?;
        *s = n;
        Ok(())
    }
}

/// Sanity limits only; the machine profile's hard limits are enforced by `plan`/`machine`, not here.
fn validate_settings(cmd: &str, s: &CutSettings) -> Result<()> {
    if !(0.0..=100.0).contains(&s.power_pct) {
        return Err(invalid(cmd, format!("power_pct must be within 0..=100 (got {})", s.power_pct)));
    }
    util::positive(cmd, "speed_mm_s", s.speed_mm_s)?;
    util::positive(cmd, "frequency_khz", s.frequency_khz)?;
    if let Some(pw) = s.pulse_width_ns {
        util::positive(cmd, "pulse_width_ns", pw)?;
    }
    if s.passes == 0 {
        return Err(invalid(cmd, "passes must be >= 1"));
    }
    util::positive(cmd, "hatch.interval_mm", s.hatch.interval_mm)?;
    util::finite(cmd, "hatch.angle_deg", s.hatch.angle_deg)?;
    util::finite(cmd, "hatch.angle_increment_deg", s.hatch.angle_increment_deg)?;
    if let Some(t) = &s.timing {
        let fields = [
            ("laser_on_delay_us", t.laser_on_delay_us),
            ("laser_off_delay_us", t.laser_off_delay_us),
            ("polygon_delay_us", t.polygon_delay_us),
            ("end_delay_us", t.end_delay_us),
            ("min_jump_delay_us", t.min_jump_delay_us),
            ("max_jump_delay_us", t.max_jump_delay_us),
            ("jump_distance_limit_mm", t.jump_distance_limit_mm),
        ];
        for (f, v) in fields {
            util::finite(cmd, &format!("timing.{f}"), v)?;
        }
        util::positive(cmd, "timing.jump_speed_mm_s", t.jump_speed_mm_s)?;
    }
    Ok(())
}

/// Ids of the objects on `layer`.
fn objects_on(doc: &Document, layer: &str) -> Vec<String> {
    doc.objects.iter().filter(|o| o.layer == layer).map(|o| o.id.clone()).collect()
}

fn layer_result(doc: &Document, id: &LayerId) -> Value {
    let objs = objects_on(doc, id);
    json!({ "created": [], "affected": [id], "objects": objs, "bounds": util::bounds_json(doc, &objs) })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerAddInput {
    /// Display name, e.g. "Engrave" or "Cut".
    pub name: String,
    /// CSS display colour, e.g. "#d62728". Defaults to the next palette colour.
    pub color: Option<String>,
    /// Cut settings to change from the defaults (power 20 %, 1000 mm/s, 30 kHz, 1 pass, line mode).
    pub settings: Option<SettingsPatch>,
}

/// `layer.add`.
pub struct LayerAdd;

impl Command for LayerAdd {
    type Input = LayerAddInput;
    const ID: &'static str = "layer.add";
    const TITLE: &'static str = "Add layer";
    const DESCRIPTION: &'static str = "Add a layer (a set of laser settings that objects are assigned to). Optional \
        settings are a partial object merged into the defaults; speeds in mm/s, frequency in kHz, power in %. Returns \
        the new layer id.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LayerAddInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let name = util::name(Self::ID, "name", &input.name)?;
            let color = match &input.color {
                Some(c) => util::color(Self::ID, c)?,
                None => PALETTE[doc.layers.len() % PALETTE.len()].to_string(),
            };
            let id = doc.alloc_id("layer");
            let mut layer = Layer::new(id.clone(), &name, &color);
            if let Some(p) = input.settings {
                p.apply(Self::ID, &mut layer.settings)?;
            }
            doc.layers.push(layer);
            Ok((
                json!({ "created": [id], "affected": [], "id": id, "bounds": Value::Null }),
                format!("added layer {id} \"{name}\""),
            ))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerUpdateInput {
    /// Exact layer id, e.g. "layer-1".
    pub id: String,
    /// New display name.
    pub name: Option<String>,
    /// New CSS display colour.
    pub color: Option<String>,
    /// Show or hide the layer in the editor. Hidden layers still run unless `output` is false.
    pub visible: Option<bool>,
    /// Include the layer in jobs.
    pub output: Option<bool>,
    /// Partial cut settings merged into the current ones: {"power_pct": 40} changes only power.
    pub settings: Option<SettingsPatch>,
}

/// `layer.update`.
pub struct LayerUpdate;

impl Command for LayerUpdate {
    type Input = LayerUpdateInput;
    const ID: &'static str = "layer.update";
    const TITLE: &'static str = "Update layer";
    const DESCRIPTION: &'static str = "Change a layer's name, colour, visibility, output flag or cut settings. \
        `settings` is merged into the existing settings, so only the fields given change (power %, speed mm/s, \
        frequency kHz, pulse width ns, hatch interval mm, hatch angle deg).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LayerUpdateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.id)?.id.clone();
            let name = input.name.as_deref().map(|n| util::name(Self::ID, "name", n)).transpose()?;
            let color = input.color.as_deref().map(|c| util::color(Self::ID, c)).transpose()?;
            let layer = doc.layer_mut(&id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
            let mut changed = Vec::new();
            if let Some(n) = name {
                layer.name = n;
                changed.push("name");
            }
            if let Some(c) = color {
                layer.color = c;
                changed.push("color");
            }
            if let Some(v) = input.visible {
                layer.visible = v;
                changed.push("visible");
            }
            if let Some(v) = input.output {
                layer.output = v;
                changed.push("output");
            }
            if let Some(p) = input.settings {
                p.apply(Self::ID, &mut layer.settings)?;
                changed.push("settings");
            }
            if changed.is_empty() {
                return Err(invalid(
                    Self::ID,
                    "nothing to change: give at least one of name, color, visible, output, settings",
                ));
            }
            let mut result = layer_result(doc, &id);
            if let Some(l) = doc.layer(&id) {
                result["settings"] = serde_json::to_value(&l.settings).unwrap_or(Value::Null);
            }
            Ok((result, format!("updated layer {id} ({})", changed.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerDeleteInput {
    /// Exact id of the layer to delete.
    pub id: String,
    /// Layer to move this layer's objects to. Required when the layer has objects.
    pub reassign_to: Option<String>,
}

/// `layer.delete`.
pub struct LayerDelete;

impl Command for LayerDelete {
    type Input = LayerDeleteInput;
    const ID: &'static str = "layer.delete";
    const TITLE: &'static str = "Delete layer";
    const DESCRIPTION: &'static str = "Delete a layer. Refused if objects use it unless `reassign_to` names another \
        layer to move them to, and refused for the last remaining layer. Objects are never deleted by this command.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LayerDeleteInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.id)?.id.clone();
            if doc.layers.len() == 1 {
                return Err(CommandError::Failed(format!("{id} is the last layer; a document needs at least one")));
            }
            let objs = objects_on(doc, &id);
            let target = match input.reassign_to.as_deref() {
                Some(t) => {
                    let t = util::layer(doc, t)?.id.clone();
                    if t == id {
                        return Err(invalid(Self::ID, "reassign_to must be a different layer"));
                    }
                    Some(t)
                }
                None if !objs.is_empty() => {
                    return Err(CommandError::Failed(format!(
                        "layer {id} has {} object(s); pass reassign_to with another layer id, or delete the objects first",
                        objs.len()
                    )));
                }
                None => None,
            };
            if let Some(t) = &target {
                for o in doc.objects.iter_mut().filter(|o| o.layer == id) {
                    o.layer = t.clone();
                }
            }
            doc.layers.retain(|l| l.id != id);
            let summary = match &target {
                Some(t) => format!("deleted layer {id}, moved {} object(s) to {t}", objs.len()),
                None => format!("deleted layer {id}"),
            };
            Ok((
                json!({
                    "created": [],
                    "affected": objs,
                    "deleted": [id],
                    "reassigned_to": target,
                    "bounds": util::bounds_json(doc, &objs),
                }),
                summary,
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(v: Value) -> SettingsPatch {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn partial_patch_changes_only_given_fields() {
        let mut s = CutSettings::default();
        patch(json!({"power_pct": 40})).apply("t", &mut s).unwrap();
        assert_eq!(s, CutSettings { power_pct: 40.0, ..CutSettings::default() });
        patch(json!({"hatch": {"interval_mm": 0.02}})).apply("t", &mut s).unwrap();
        assert_eq!(s.hatch.interval_mm, 0.02);
        assert_eq!(s.hatch.angle_deg, 0.0);
        assert_eq!(s.power_pct, 40.0);
    }

    #[test]
    fn null_clears_optional_fields() {
        let mut s = CutSettings::default();
        patch(json!({"pulse_width_ns": 200})).apply("t", &mut s).unwrap();
        assert_eq!(s.pulse_width_ns, Some(200.0));
        patch(json!({"speed_mm_s": 500})).apply("t", &mut s).unwrap();
        assert_eq!(s.pulse_width_ns, Some(200.0), "absent field must not clear");
        patch(json!({"pulse_width_ns": null})).apply("t", &mut s).unwrap();
        assert_eq!(s.pulse_width_ns, None);
    }

    #[test]
    fn invalid_values_and_unknown_keys_are_refused() {
        let mut s = CutSettings::default();
        assert!(patch(json!({"power_pct": 140})).apply("t", &mut s).is_err());
        assert!(patch(json!({"passes": 0})).apply("t", &mut s).is_err());
        assert_eq!(s, CutSettings::default());
        assert!(serde_json::from_value::<SettingsPatch>(json!({"power": 40})).is_err());
    }

    /// The patch must cover every field of `CutSettings` and `Hatch`, so a field added to `doc` fails this test.
    #[test]
    fn patch_covers_every_settings_field() {
        fn keys(v: &Value) -> Vec<String> {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        }
        let settings = serde_json::to_value(CutSettings::default()).unwrap();
        let schema = serde_json::to_value(schemars::schema_for!(SettingsPatch)).unwrap();
        assert_eq!(keys(&settings), keys(&schema["properties"]));
        let hatch_schema = serde_json::to_value(schemars::schema_for!(HatchPatch)).unwrap();
        assert_eq!(keys(&settings["hatch"]), keys(&hatch_schema["properties"]));
    }
}
