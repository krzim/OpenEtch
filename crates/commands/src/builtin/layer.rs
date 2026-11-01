//! Layer commands: `layer.add`, `layer.update`, `layer.delete`, `layer.move`, and the partial cut-settings patch they
//! share.

use doc::{CutSettings, DitherMode, Document, FillMode, GalvoTiming, Layer, LayerId};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Default display colours for new layers, in order (LightBurn-like).
const PALETTE: &[&str] = &["#1f77b4", "#d62728", "#2ca02c", "#ff7f0e", "#9467bd", "#8c564b", "#e377c2", "#17becf"];

/// Most passes a layer may have. Each pass replans the whole layer, so an unbounded count would let one edit make
/// previews and job planning allocate without limit (`plan` refuses more too).
pub const MAX_PASSES: u32 = 1000;

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
    /// Number of times the layer is marked, 1..=1000.
    pub passes: Option<u32>,
    /// Hatch fill settings (partial: only the fields given change).
    pub hatch: Option<HatchPatch>,
    /// Galvo timing in µs (all fields required), or `null` to use the machine profile's defaults.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<GalvoTiming>")]
    pub timing: Option<Option<GalvoTiming>>,
    /// Image mode settings, used for the layer's images whatever its mode (partial: only the fields given change).
    pub image: Option<ImagePatch>,
    /// Cut options (partial, merged like a JSON merge patch; `null` resets one): output (sub-layer output toggle),
    /// pause_before_ms / pause_after_ms, dot_mode {dot_time_ms, dot_spacing_mm}, wobble {step_mm, size_mm},
    /// fill_dot_width_mm, fill_grouping (individually | all_at_once | by_group), auto_rotate, flood_fill,
    /// perforation {cut_mm, skip_mm}, tabs {count, spacing_mm, size_mm}, z_offset_mm, z_step_mm.
    #[schemars(with = "Option<doc::CutOptions>")]
    pub options: Option<Value>,
}

/// A partial [`doc::ImageSettings`] (LightBurn's Image mode).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImagePatch {
    /// threshold, ordered, atkinson, floyd_steinberg (LightBurn's "Dither"; "dither" is accepted), stucki, jarvis,
    /// newsprint, halftone, sketch, grayscale (power varies between min_power_pct and the layer's power) or slice3d
    /// (LightBurn's 3D Sliced, "3dslice": a depth map engraved one grey-level slice per pass, darkest first).
    pub dither: Option<DitherMode>,
    /// Scan line spacing and dot size in mm, > 0 (DPI = 25.4 / interval).
    pub interval_mm: Option<f64>,
    /// Dots per inch: sets interval_mm to 25.4 / dpi (give one of the two).
    pub dpi: Option<f64>,
    /// Scan angle in degrees (0 = along +X, rows from the bottom up).
    pub angle_deg: Option<f64>,
    /// Degrees added to the scan angle on every pass.
    pub angle_increment_deg: Option<f64>,
    /// Engrave rows in both directions.
    pub bidirectional: Option<bool>,
    /// Invert the image so light areas engrave.
    pub negative: Option<bool>,
    /// Engrave the image's own pixels as they are (no resampling; interval and scan angle are not used).
    pub pass_through: Option<bool>,
    /// Dot width correction in mm, 0..=interval_mm (0 is off).
    pub dot_width_mm: Option<f64>,
    /// Grayscale: power of the lightest dot, 0..=100 %.
    pub min_power_pct: Option<f64>,
    /// Halftone: screen cells per inch, > 0.
    pub halftone_cells_per_inch: Option<f64>,
    /// Halftone: screen angle in degrees.
    pub halftone_angle_deg: Option<f64>,
    /// 3D Sliced: cleanup sub-layers clean the whole image instead of the last slice (LightBurn's "Clean whole
    /// image").
    pub clean_whole_image: Option<bool>,
}

impl ImagePatch {
    fn apply(self, cmd: &str, s: &mut doc::ImageSettings) -> Result<()> {
        if self.interval_mm.is_some() && self.dpi.is_some() {
            return Err(invalid(cmd, "give image.interval_mm or image.dpi, not both"));
        }
        if let Some(v) = self.dither {
            s.dither = v;
        }
        if let Some(v) = self.interval_mm {
            s.interval_mm = v;
        }
        if let Some(dpi) = self.dpi {
            s.interval_mm = 25.4 / util::positive(cmd, "image.dpi", dpi)?;
        }
        if let Some(v) = self.angle_deg {
            s.angle_deg = v;
        }
        if let Some(v) = self.angle_increment_deg {
            s.angle_increment_deg = v;
        }
        if let Some(v) = self.bidirectional {
            s.bidirectional = v;
        }
        if let Some(v) = self.negative {
            s.negative = v;
        }
        if let Some(v) = self.pass_through {
            s.pass_through = v;
        }
        if let Some(v) = self.dot_width_mm {
            s.dot_width_mm = v;
        }
        if let Some(v) = self.min_power_pct {
            s.min_power_pct = v;
        }
        if let Some(v) = self.halftone_cells_per_inch {
            s.halftone_cells_per_inch = v;
        }
        if let Some(v) = self.halftone_angle_deg {
            s.halftone_angle_deg = v;
        }
        if let Some(v) = self.clean_whole_image {
            s.clean_whole_image = v;
        }
        Ok(())
    }
}

/// Sanity limits of image settings (LightBurn's ranges where it states them).
fn validate_image(cmd: &str, s: &doc::ImageSettings) -> Result<()> {
    util::positive(cmd, "image.interval_mm", s.interval_mm)?;
    util::finite(cmd, "image.angle_deg", s.angle_deg)?;
    util::finite(cmd, "image.angle_increment_deg", s.angle_increment_deg)?;
    util::non_negative(cmd, "image.dot_width_mm", s.dot_width_mm)?;
    if s.dot_width_mm > s.interval_mm {
        return Err(invalid(
            cmd,
            format!(
                "image.dot_width_mm must be within 0..=image.interval_mm ({}) (got {})",
                s.interval_mm, s.dot_width_mm
            ),
        ));
    }
    if !(0.0..=100.0).contains(&s.min_power_pct) {
        return Err(invalid(cmd, format!("image.min_power_pct must be within 0..=100 (got {})", s.min_power_pct)));
    }
    util::positive(cmd, "image.halftone_cells_per_inch", s.halftone_cells_per_inch)?;
    util::finite(cmd, "image.halftone_angle_deg", s.halftone_angle_deg)?;
    Ok(())
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
    /// Lines per inch (LightBurn's LPI entry): sets interval_mm to 25.4 / lines_per_inch (give one of the two).
    pub lines_per_inch: Option<f64>,
}

/// RFC 7386 JSON merge patch: objects merge key by key, `null` removes a key (back to its default), anything else
/// replaces.
pub(super) fn json_merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(changes) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let map = target.as_object_mut().expect("made an object above");
    for (k, v) in changes {
        if v.is_null() {
            map.remove(k);
        } else {
            json_merge_patch(map.entry(k.clone()).or_insert(Value::Null), v);
        }
    }
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
            if h.interval_mm.is_some() && h.lines_per_inch.is_some() {
                return Err(invalid(cmd, "give hatch.interval_mm or hatch.lines_per_inch, not both"));
            }
            if let Some(v) = h.interval_mm {
                n.hatch.interval_mm = v;
            }
            if let Some(lpi) = h.lines_per_inch {
                util::positive(cmd, "hatch.lines_per_inch", lpi)?;
                n.hatch.interval_mm = 25.4 / lpi;
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
        if let Some(p) = self.image {
            p.apply(cmd, &mut n.image)?;
            validate_image(cmd, &n.image)?;
        }
        if let Some(patch) = self.options {
            let mut v = serde_json::to_value(&n.options).map_err(|e| CommandError::Failed(e.to_string()))?;
            json_merge_patch(&mut v, &patch);
            n.options = serde_json::from_value(v).map_err(|e| invalid(cmd, format!("options: {e}")))?;
        }
        validate_settings(cmd, &n)?;
        *s = n;
        Ok(())
    }
}

/// Sanity limits only; the machine profile's hard limits are enforced by `plan`/`machine`, not here.
pub(super) fn validate_settings(cmd: &str, s: &CutSettings) -> Result<()> {
    if !(0.0..=100.0).contains(&s.power_pct) {
        return Err(invalid(cmd, format!("power_pct must be within 0..=100 (got {})", s.power_pct)));
    }
    util::positive(cmd, "speed_mm_s", s.speed_mm_s)?;
    util::positive(cmd, "frequency_khz", s.frequency_khz)?;
    if let Some(pw) = s.pulse_width_ns {
        util::positive(cmd, "pulse_width_ns", pw)?;
    }
    if !(1..=MAX_PASSES).contains(&s.passes) {
        return Err(invalid(cmd, format!("passes must be within 1..={MAX_PASSES} (got {})", s.passes)));
    }
    util::positive(cmd, "hatch.interval_mm", s.hatch.interval_mm)?;
    util::finite(cmd, "hatch.angle_deg", s.hatch.angle_deg)?;
    util::finite(cmd, "hatch.angle_increment_deg", s.hatch.angle_increment_deg)?;
    if let Some(c) = &s.cleanup {
        check_cleanup(cmd, c)?;
    }
    if let Some(p) = s.options.problem() {
        return Err(invalid(cmd, p));
    }
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

/// A cleanup interval within `1..=MAX_PASSES`.
pub(super) fn check_cleanup(cmd: &str, c: &doc::Cleanup) -> Result<()> {
    if !(1..=MAX_PASSES).contains(&c.every_n_passes) {
        return Err(invalid(
            cmd,
            format!("cleanup.every_n_passes must be within 1..={MAX_PASSES} (got {})", c.every_n_passes),
        ));
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
    /// Saved layer defaults by display colour, "*" for every colour (LightBurn's Make Default; the engine passes the
    /// active device's): the new layer starts from the one for its colour instead of the built-in defaults, then
    /// `settings` applies.
    pub defaults: Option<std::collections::BTreeMap<String, CutSettings>>,
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
            let id = doc.alloc_id("layer")?;
            let mut layer = Layer::new(id.clone(), &name, &color);
            if let Some(d) = input.defaults.as_ref().and_then(|d| d.get(&color.to_ascii_lowercase()).or_else(|| d.get("*"))) {
                validate_settings(Self::ID, d)?;
                layer.settings = d.clone();
            }
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
    /// Run the whole layer (main settings and sub-layers) this many times, 1..=1000 (LightBurn's global passes).
    pub global_passes: Option<u32>,
}

/// `layer.update`.
pub struct LayerUpdate;

impl Command for LayerUpdate {
    type Input = LayerUpdateInput;
    const ID: &'static str = "layer.update";
    const TITLE: &'static str = "Update layer";
    const DESCRIPTION: &'static str = "Change a layer's name, colour, visibility, output flag or cut settings. \
        `settings` is merged into the existing settings, so only the fields given change (power %, speed mm/s, \
        frequency kHz, pulse width ns, hatch interval mm, hatch angle deg). `settings.image` holds the Image mode \
        settings the layer's images are engraved with (as in LightBurn, images always use them while vectors keep \
        the mode): dither, interval_mm or dpi, angle_deg, angle_increment_deg, bidirectional, negative, pass_through, \
        dot_width_mm, min_power_pct (grayscale), halftone_cells_per_inch, halftone_angle_deg, clean_whole_image (3D \
        Sliced); also partial.";
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
            if let Some(g) = input.global_passes {
                if !(1..=MAX_PASSES).contains(&g) {
                    return Err(invalid(Self::ID, format!("global_passes must be within 1..={MAX_PASSES} (got {g})")));
                }
                layer.global_passes = g;
                changed.push("global_passes");
            }
            if let Some(p) = input.settings {
                super::preset::check_not_linked(Self::ID, doc, &id)?;
                let layer = doc.layer_mut(&id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
                let sets_frequency = p.frequency_khz.is_some();
                p.apply(Self::ID, &mut layer.settings)?;
                if sets_frequency {
                    // An imported LightBurn layer without a frequency now has one to export.
                    layer.extras.remove(lbrn::FREQUENCY_ABSENT_KEY);
                }
                changed.push("settings");
            }
            if changed.is_empty() {
                return Err(invalid(
                    Self::ID,
                    "nothing to change: give at least one of name, color, visible, output, global_passes, settings",
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerMoveInput {
    /// Exact id of the layer to move, e.g. "layer-2".
    pub id: String,
    /// 0-based position the layer ends up at in the layer list; the other layers keep their order.
    pub to_index: usize,
}

/// `layer.move`.
pub struct LayerMove;

impl Command for LayerMove {
    type Input = LayerMoveInput;
    const ID: &'static str = "layer.move";
    const TITLE: &'static str = "Move layer";
    const DESCRIPTION: &'static str = "Move a layer to another position in the layer list (LightBurn's Cuts / Layers \
        order). Jobs burn layer by layer in this order, so this changes the cut order. The other layers keep their \
        order; objects and settings are unchanged. Returns the new `order` of layer ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, input: LayerMoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let id = util::layer(doc, &input.id)?.id.clone();
            let n = doc.layers.len();
            if input.to_index >= n {
                return Err(invalid(
                    Self::ID,
                    format!("to_index must be within 0..={} (got {})", n - 1, input.to_index),
                ));
            }
            let from = doc
                .layers
                .iter()
                .position(|l| l.id == id)
                .ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
            if from == input.to_index {
                return Err(invalid(Self::ID, format!("layer {id} is already at position {from}")));
            }
            let layer = doc.layers.remove(from);
            doc.layers.insert(input.to_index, layer);
            let order: Vec<&str> = doc.layers.iter().map(|l| l.id.as_str()).collect();
            let mut result = layer_result(doc, &id);
            result["order"] = json!(order);
            result["index"] = json!(input.to_index);
            Ok((result, format!("moved layer {id} from position {from} to {}", input.to_index)))
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

    #[test]
    fn passes_are_capped() {
        let mut s = CutSettings::default();
        patch(json!({"passes": MAX_PASSES})).apply("t", &mut s).unwrap();
        assert_eq!(s.passes, MAX_PASSES);
        let e = patch(json!({"passes": MAX_PASSES + 1})).apply("t", &mut s).unwrap_err().to_string();
        assert!(e.contains("1..=1000"), "{e}");
        assert!(patch(json!({"passes": 4_000_000_000u32})).apply("t", &mut s).is_err());
        assert_eq!(s.passes, MAX_PASSES);
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
        let mut hatch_keys = keys(&settings["hatch"]);
        hatch_keys.push("lines_per_inch".into());
        hatch_keys.sort();
        assert_eq!(hatch_keys, keys(&hatch_schema["properties"]));
        let image_schema = serde_json::to_value(schemars::schema_for!(ImagePatch)).unwrap();
        let mut image_keys = keys(&settings["image"]);
        image_keys.push("dpi".into());
        image_keys.sort();
        assert_eq!(image_keys, keys(&image_schema["properties"]));
    }

    #[test]
    fn lpi_cut_options_and_global_passes() {
        let mut s = CutSettings::default();
        patch(json!({"hatch": {"lines_per_inch": 254}})).apply("t", &mut s).unwrap();
        assert!((s.hatch.interval_mm - 0.1).abs() < 1e-12);
        assert!(patch(json!({"hatch": {"lines_per_inch": 254, "interval_mm": 0.1}})).apply("t", &mut s).is_err());
        assert!(patch(json!({"hatch": {"lines_per_inch": 0}})).apply("t", &mut s).is_err());
        patch(json!({"options": {"pause_after_ms": 20, "wobble": {"step_mm": 0.2, "size_mm": 0.5}}})).apply("t", &mut s).unwrap();
        assert_eq!(s.options.pause_after_ms, 20.0);
        patch(json!({"options": {"output": false, "wobble": null}})).apply("t", &mut s).unwrap();
        assert!(!s.options.output && s.options.wobble.is_none() && s.options.pause_after_ms == 20.0);
        let before = s.clone();
        assert!(patch(json!({"options": {"pause_after_ms": -1}})).apply("t", &mut s).is_err());
        assert!(patch(json!({"options": {"wobel": 1}})).apply("t", &mut s).is_err());
        assert_eq!(s, before, "a refused patch changes nothing");
        let mut session = crate::Session::new(Document::default());
        let id = session.document().layers[0].id.clone();
        run(&mut session, "layer.update", json!({"id": id, "global_passes": 3})).unwrap();
        assert_eq!(session.document().layers[0].global_passes, 3);
        assert!(run(&mut session, "layer.update", json!({"id": id, "global_passes": 0})).is_err());
    }

    #[test]
    fn image_settings_patch_partially_and_are_checked() {
        let mut s = CutSettings::default();
        patch(json!({"image": {"dither": "dither", "negative": true, "dpi": 254}})).apply("t", &mut s).unwrap();
        assert_eq!(s.image.dither, DitherMode::FloydSteinberg);
        assert!(s.image.negative && s.image.bidirectional, "other fields keep their values");
        assert!((s.image.interval_mm - 0.1).abs() < 1e-12);
        patch(json!({"image": {"dither": "grayscale", "min_power_pct": 12, "dot_width_mm": 0.05}})).apply("t", &mut s).unwrap();
        assert_eq!((s.image.min_power_pct, s.image.dot_width_mm, s.image.negative), (12.0, 0.05, true));
        let before = s.clone();
        for bad in [
            json!({"image": {"interval_mm": 0}}),
            json!({"image": {"dot_width_mm": 0.2}}),
            json!({"image": {"min_power_pct": 101}}),
            json!({"image": {"halftone_cells_per_inch": -5}}),
            json!({"image": {"interval_mm": 0.1, "dpi": 254}}),
            json!({"image": {"dpi": 0}}),
        ] {
            assert!(patch(bad.clone()).apply("t", &mut s).is_err(), "{bad}");
        }
        assert_eq!(s, before, "a refused patch changes nothing");
        assert!(serde_json::from_value::<SettingsPatch>(json!({"image": {"dither": "3dslice"}})).is_ok(), "3D Sliced");
        assert!(serde_json::from_value::<SettingsPatch>(json!({"image": {"gamma": 2}})).is_err(), "adjust is per image");
    }

    fn run(s: &mut crate::Session, id: &str, params: Value) -> Result<Value> {
        s.execute(crate::Invocation { id: id.into(), params }, crate::Origin::Ui).map(|o| o.result)
    }

    fn order(s: &crate::Session) -> Vec<String> {
        s.document().layers.iter().map(|l| l.id.clone()).collect()
    }

    #[test]
    fn layer_move_reorders_in_one_undo_step() {
        let mut s = crate::Session::new(Document::default());
        let first = s.document().layers[0].id.clone();
        let b = run(&mut s, "layer.add", json!({"name": "B"})).unwrap()["id"].as_str().unwrap().to_string();
        let c = run(&mut s, "layer.add", json!({"name": "C"})).unwrap()["id"].as_str().unwrap().to_string();
        let obj = run(&mut s, "shape.rect", json!({"width_mm": 5, "height_mm": 5, "layer": c})).unwrap()["created"][0]
            .as_str()
            .unwrap()
            .to_string();
        let before = order(&s);
        let steps = s.history().len();

        // Last to first: C now burns before the others, which keep their order.
        let r = run(&mut s, "layer.move", json!({"id": c, "to_index": 0})).unwrap();
        assert_eq!(order(&s), [c.clone(), first.clone(), b.clone()]);
        assert_eq!(r["order"], json!([c, first, b]));
        assert_eq!(r["affected"], json!([c]));
        assert_eq!(r["objects"], json!([obj]));
        assert_eq!(s.history().len(), steps + 1, "one undo step");
        assert_eq!(s.document().object(&obj).unwrap().layer, c, "objects keep their layer");

        // First to last.
        run(&mut s, "layer.move", json!({"id": c, "to_index": 2})).unwrap();
        assert_eq!(order(&s), [first.clone(), b.clone(), c.clone()]);
        run(&mut s, "layer.move", json!({"id": first, "to_index": 1})).unwrap();
        assert_eq!(order(&s), [b.clone(), first.clone(), c.clone()]);

        s.undo().unwrap();
        s.undo().unwrap();
        s.undo().unwrap();
        assert_eq!(order(&s), before);
    }

    #[test]
    fn layer_move_refuses_bad_positions() {
        let mut s = crate::Session::new(Document::default());
        let first = s.document().layers[0].id.clone();
        run(&mut s, "layer.add", json!({"name": "B"})).unwrap();
        let before = order(&s);
        let rev = s.document().revision;
        let e = run(&mut s, "layer.move", json!({"id": first, "to_index": 2})).unwrap_err().to_string();
        assert!(e.contains("0..=1"), "{e}");
        let e = run(&mut s, "layer.move", json!({"id": first, "to_index": 0})).unwrap_err().to_string();
        assert!(e.contains("already at position 0"), "{e}");
        assert!(run(&mut s, "layer.move", json!({"id": "layer-99", "to_index": 0})).is_err());
        assert!(run(&mut s, "layer.move", json!({"id": first, "to_index": -1})).is_err());
        assert!(run(&mut s, "layer.move", json!({"id": first, "index": 1})).is_err());
        assert_eq!((order(&s), s.document().revision), (before, rev), "refusals change nothing");
    }
}
