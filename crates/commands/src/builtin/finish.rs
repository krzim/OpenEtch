//! MOPA finish tools: the hue step model ("one hue warmer / cooler" and the surface finish offset), applied to a
//! layer (`layer.hue_nudge`) or computed for any settings (`material.hue_step`, e.g. a library entry's), and a
//! layer's expected result colour (`layer.set_result_color`, the colour the canvas draws in Result colours view).
//!
//! # The hue step model
//!
//! Seeded from xTool's published guide for colour marking stainless steel with a MOPA source
//! (<https://support.xtool.com/article/2196>): one hue is about 0.5 % power, or 100 mm/s of speed, or 100 to 200 kHz
//! of frequency above 500 kHz; keep the pulse width above 20 ns; use power or speed for jumps of 4 hues or more; a
//! polished surface needs about +1 % power and a mirror finish +2 % over a mill or brushed one. Those numbers are for
//! xTool's 60 W source on 304 stainless, so the model is stored per device and refined by bracket tests (the engine's
//! `materials` routes keep it).
//!
//! Direction: the oxide colours on steel go straw, gold, brown, purple, blue as the energy per area grows, so **one
//! hue cooler is more energy** (power up, speed down, frequency down: fewer, stronger pulses heat more) and one hue
//! warmer is less. Positive `steps` are warmer. Unverified on the owner's B6: the bracket test exists to check it.

use doc::{CutSettings, Document};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::layer::validate_settings;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The layer `extras` key holding a layer's expected result colour (`#rrggbb`).
pub const RESULT_COLOR_KEY: &str = "finish.color";

/// The per-device hue step model. Every field has xTool's value as its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HueModel {
    /// Power change per hue, percentage points. Default 0.5.
    pub power_pct: f64,
    /// Speed change per hue, mm/s. Default 100.
    pub speed_mm_s: f64,
    /// Frequency change per hue, kHz (only at and above `frequency_from_khz`). Default 150.
    pub frequency_khz: f64,
    /// Frequency steps apply from this frequency up, kHz. Default 500.
    pub frequency_from_khz: f64,
    /// The model holds for pulse widths of at least this, ns. Default 20.
    pub min_pulse_ns: f64,
    /// Jumps of this many hues or more use power (auto never uses frequency). Default 4.
    pub big_jump_hues: u32,
    /// Power added for a brushed surface over a mill finish, percentage points. Default 0.
    pub brushed_power_pct: f64,
    /// Power added for a polished surface. Default 1.
    pub polished_power_pct: f64,
    /// Power added for a mirror surface. Default 2.
    pub mirror_power_pct: f64,
    /// Where the steps come from: "xtool" (the published guide) or "bracket" (refined from a bracket test).
    pub source: String,
}

impl Default for HueModel {
    fn default() -> Self {
        HueModel {
            power_pct: 0.5,
            speed_mm_s: 100.0,
            frequency_khz: 150.0,
            frequency_from_khz: 500.0,
            min_pulse_ns: 20.0,
            big_jump_hues: 4,
            brushed_power_pct: 0.0,
            polished_power_pct: 1.0,
            mirror_power_pct: 2.0,
            source: "xtool".into(),
        }
    }
}

impl HueModel {
    /// Why the model cannot be used, if it cannot.
    pub fn problem(&self) -> Option<String> {
        let steps = [("power_pct", self.power_pct, 20.0), ("speed_mm_s", self.speed_mm_s, 5000.0), ("frequency_khz", self.frequency_khz, 2000.0)];
        for (k, v, max) in steps {
            if !(v.is_finite() && v > 0.0 && v <= max) {
                return Some(format!("{k} must be above 0 and at most {max} (got {v})"));
            }
        }
        let others = [
            ("frequency_from_khz", self.frequency_from_khz),
            ("min_pulse_ns", self.min_pulse_ns),
            ("brushed_power_pct", self.brushed_power_pct),
            ("polished_power_pct", self.polished_power_pct),
            ("mirror_power_pct", self.mirror_power_pct),
        ];
        for (k, v) in others {
            if !(v.is_finite() && (-50.0..=10_000.0).contains(&v)) {
                return Some(format!("{k} must be a number within -50..=10000 (got {v})"));
            }
        }
        if !(1..=50).contains(&self.big_jump_hues) {
            return Some(format!("big_jump_hues must be within 1..=50 (got {})", self.big_jump_hues));
        }
        if self.source.chars().count() > 40 || self.source.chars().any(char::is_control) {
            return Some("source must be a short name".into());
        }
        None
    }

    /// Power offset of a surface finish over mill, percentage points.
    pub fn finish_power(&self, f: SurfaceFinish) -> f64 {
        match f {
            SurfaceFinish::Mill => 0.0,
            SurfaceFinish::Brushed => self.brushed_power_pct,
            SurfaceFinish::Polished => self.polished_power_pct,
            SurfaceFinish::Mirror => self.mirror_power_pct,
        }
    }
}

/// The surface a setting was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceFinish {
    Mill,
    Brushed,
    Polished,
    Mirror,
}

/// Which setting a hue step changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HueParam {
    /// Frequency at or above `frequency_from_khz` for jumps under `big_jump_hues`, else power.
    #[default]
    Auto,
    Power,
    Speed,
    Frequency,
}

/// The result of a hue step or a finish change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Nudged {
    pub settings: CutSettings,
    /// The setting changed (`None` when nothing changed).
    pub param: Option<HueParam>,
    /// The change of each setting that changed: `{power_pct?, speed_mm_s?, frequency_khz?}`.
    pub delta: Value,
    pub warnings: Vec<String>,
}

fn round(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

/// `steps` hues warmer (positive) or cooler (negative) by `via`, then the change of surface `finish` (from, to).
pub fn nudge(
    s: &CutSettings,
    steps: i32,
    via: HueParam,
    finish: Option<(SurfaceFinish, SurfaceFinish)>,
    m: &HueModel,
) -> std::result::Result<Nudged, String> {
    if let Some(p) = m.problem() {
        return Err(format!("hue model: {p}"));
    }
    if steps.unsigned_abs() > 50 {
        return Err(format!("steps must be within -50..=50 (got {steps})"));
    }
    let mut out = s.clone();
    let mut warnings = Vec::new();
    let mut delta = serde_json::Map::new();
    let mut param = None;
    if steps != 0 {
        let n = f64::from(steps);
        let big = steps.unsigned_abs() >= m.big_jump_hues;
        let p = match via {
            HueParam::Auto if !big && s.frequency_khz >= m.frequency_from_khz => HueParam::Frequency,
            HueParam::Auto => HueParam::Power,
            p => p,
        };
        match p {
            HueParam::Frequency => {
                if s.frequency_khz < m.frequency_from_khz {
                    return Err(format!(
                        "below {} kHz a hue is not a frequency step (this setting runs at {} kHz); use power or speed",
                        m.frequency_from_khz, s.frequency_khz
                    ));
                }
                if big {
                    warnings.push(format!("jumps of {} hues or more are steadier with power or speed", m.big_jump_hues));
                }
                let f = round(s.frequency_khz + n * m.frequency_khz, 0.1);
                if f < m.frequency_from_khz {
                    warnings.push(format!("the frequency leaves the model's range (below {} kHz)", m.frequency_from_khz));
                }
                out.frequency_khz = f;
            }
            HueParam::Speed => out.speed_mm_s = round(s.speed_mm_s + n * m.speed_mm_s, 0.1),
            HueParam::Power | HueParam::Auto => out.power_pct = round(s.power_pct - n * m.power_pct, 0.01),
        }
        param = Some(p);
    }
    if let Some((from, to)) = finish {
        let d = m.finish_power(to) - m.finish_power(from);
        if d != 0.0 {
            out.power_pct = round(out.power_pct + d, 0.01);
            param.get_or_insert(HueParam::Power);
        }
    }
    if out.power_pct <= 0.0 || out.power_pct > 100.0 {
        return Err(format!("the power would be {} %, outside 0..=100", out.power_pct));
    }
    if out.speed_mm_s <= 0.0 {
        return Err(format!("the speed would be {} mm/s", out.speed_mm_s));
    }
    if out.frequency_khz <= 0.0 {
        return Err(format!("the frequency would be {} kHz", out.frequency_khz));
    }
    if let Some(pw) = s.pulse_width_ns
        && pw < m.min_pulse_ns
    {
        warnings.push(format!("the pulse width is {pw} ns; the hue steps hold from {} ns up", m.min_pulse_ns));
    }
    for (k, a, b) in [
        ("power_pct", s.power_pct, out.power_pct),
        ("speed_mm_s", s.speed_mm_s, out.speed_mm_s),
        ("frequency_khz", s.frequency_khz, out.frequency_khz),
    ] {
        if a != b {
            delta.insert(k.into(), json!(round(b - a, 0.01)));
        }
    }
    Ok(Nudged { settings: out, param, delta: Value::Object(delta), warnings })
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishChange {
    /// The surface the settings were made for.
    pub from: SurfaceFinish,
    /// The surface they are for now.
    pub to: SurfaceFinish,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HueStepInput {
    /// Complete settings (as a layer's or a library entry's).
    pub settings: CutSettings,
    /// Hues warmer (positive, less energy) or cooler (negative). Default 0.
    #[serde(default)]
    pub steps: i32,
    /// The setting to change. Default auto.
    #[serde(default)]
    pub via: HueParam,
    /// A change of surface finish (adds the model's power offset).
    pub finish: Option<FinishChange>,
    /// The step model. Default xTool's published one; the engine's material tools use the device's.
    pub model: Option<HueModel>,
}

fn run_nudge(cmd: &str, s: &CutSettings, steps: i32, via: HueParam, finish: Option<FinishChange>, model: Option<&HueModel>) -> Result<Nudged> {
    let m = model.cloned().unwrap_or_default();
    let n = nudge(s, steps, via, finish.map(|f| (f.from, f.to)), &m).map_err(|e| invalid(cmd, e))?;
    validate_settings(cmd, &n.settings)?;
    Ok(n)
}

/// `material.hue_step`.
pub struct HueStep;

impl Command for HueStep {
    type Input = HueStepInput;
    const ID: &'static str = "material.hue_step";
    const TITLE: &'static str = "Hue step (compute)";
    const DESCRIPTION: &'static str = "MOPA colour on steel: the settings one or more hues warmer (steps > 0: less \
        energy) or cooler (steps < 0: more) by the hue step model (default xTool's published one: 0.5 % power, \
        100 mm/s or 150 kHz above 500 kHz per hue; polished +1 % power, mirror +2 % over mill / brushed), and/or for \
        another surface finish (finish {from, to}). via: auto (frequency at or above 500 kHz for jumps under 4 hues, \
        else power), power, speed or frequency. Changes nothing. Returns {settings, param, delta, warnings}.";
    const RISK: Risk = Risk::Query;

    fn apply(_doc: &mut Document, i: HueStepInput) -> Result<(Value, String)> {
        let n = run_nudge(Self::ID, &i.settings, i.steps, i.via, i.finish, i.model.as_ref())?;
        Ok((json!(n), String::new()))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HueNudgeInput {
    /// Exact layer id.
    pub layer: String,
    /// Hues warmer (positive) or cooler (negative). Default 0.
    #[serde(default)]
    pub steps: i32,
    #[serde(default)]
    pub via: HueParam,
    pub finish: Option<FinishChange>,
    pub model: Option<HueModel>,
}

/// `layer.hue_nudge`.
pub struct HueNudge;

impl Command for HueNudge {
    type Input = HueNudgeInput;
    const ID: &'static str = "layer.hue_nudge";
    const TITLE: &'static str = "One hue warmer / cooler";
    const DESCRIPTION: &'static str = "Change a layer's main settings by MOPA hue steps (see material.hue_step: \
        steps > 0 warmer, < 0 cooler; via; finish {from, to}; model, default xTool's). A layer linked to a library \
        preset is refused (nudge the preset, or unlink first). Returns {layer, settings, param, delta, warnings}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: HueNudgeInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let l = util::layer(doc, &i.layer)?;
            if let Some(k) = &l.link {
                return Err(invalid(cmd, format!("layer {} is linked to preset \"{}\"; nudge the preset or unlink the layer", l.id, k.path)));
            }
            let id = l.id.clone();
            let n = run_nudge(cmd, &l.settings, i.steps, i.via, i.finish, i.model.as_ref())?;
            if n.param.is_none() {
                return Err(invalid(cmd, "give steps or a finish change"));
            }
            let layer = doc.layer_mut(&id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
            layer.settings = n.settings.clone();
            let what = match i.steps {
                0 => "changed the finish of".to_string(),
                s if s > 0 => format!("{s} hue(s) warmer:"),
                s => format!("{} hue(s) cooler:", -s),
            };
            Ok((
                json!({ "affected": [id], "created": [], "layer": id, "settings": n.settings, "param": n.param, "delta": n.delta, "warnings": n.warnings, "bounds": Value::Null }),
                format!("{what} layer {id}"),
            ))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResultColorInput {
    /// Exact layer id.
    pub layer: String,
    /// The colour the layer is expected to come out, `#rrggbb`; `null` forgets it (the layer's own colour is shown).
    pub color: Option<String>,
}

/// `layer.set_result_color`.
pub struct SetResultColor;

impl Command for SetResultColor {
    type Input = ResultColorInput;
    const ID: &'static str = "layer.set_result_color";
    const TITLE: &'static str = "Set expected result colour";
    const DESCRIPTION: &'static str = "Set (or with null clear) the colour a layer is expected to come out on the \
        material (e.g. a MOPA colour on steel), kept in the layer's extras finish.color. The canvas and preview draw \
        layers in these colours when Result colours is on; burning is unchanged. Returns {layer, color}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ResultColorInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let id = util::layer(doc, &i.layer)?.id.clone();
            let color = match &i.color {
                Some(c) => Some(normal_hex(c).ok_or_else(|| invalid(cmd, format!("color must be #rrggbb (got \"{c}\")")))?),
                None => None,
            };
            let layer = doc.layer_mut(&id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
            match &color {
                Some(c) => layer.extras.insert(RESULT_COLOR_KEY.into(), c.clone()),
                None => layer.extras.remove(RESULT_COLOR_KEY),
            };
            let summary = match &color {
                Some(c) => format!("layer {id} is expected to come out {c}"),
                None => format!("cleared the result colour of layer {id}"),
            };
            Ok((json!({ "affected": [id], "created": [], "layer": id, "color": color, "bounds": Value::Null }), summary))
        })
    }
}

/// `#rgb` / `#rrggbb` as lower-case `#rrggbb`.
pub fn normal_hex(s: &str) -> Option<String> {
    fileio::colorsep::parse_hex(s).map(fileio::colorsep::hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn s(power: f64, speed: f64, freq: f64, pw: Option<f64>) -> CutSettings {
        CutSettings { power_pct: power, speed_mm_s: speed, frequency_khz: freq, pulse_width_ns: pw, ..CutSettings::default() }
    }

    #[test]
    fn steps_follow_the_xtool_model() {
        let m = HueModel::default();
        // Above 500 kHz a small step is a frequency step; warmer is faster pulses.
        let n = nudge(&s(40.0, 800.0, 600.0, Some(30.0)), 1, HueParam::Auto, None, &m).unwrap();
        assert_eq!((n.param, n.settings.frequency_khz), (Some(HueParam::Frequency), 750.0));
        assert_eq!(n.delta, json!({ "frequency_khz": 150.0 }));
        assert!(n.warnings.is_empty());
        // Below 500 kHz, or a jump of 4, is a power step; cooler is more power.
        let n = nudge(&s(40.0, 800.0, 200.0, Some(30.0)), -2, HueParam::Auto, None, &m).unwrap();
        assert_eq!((n.param, n.settings.power_pct), (Some(HueParam::Power), 41.0));
        let n = nudge(&s(40.0, 800.0, 600.0, Some(30.0)), 4, HueParam::Auto, None, &m).unwrap();
        assert_eq!((n.param, n.settings.power_pct, n.settings.frequency_khz), (Some(HueParam::Power), 38.0, 600.0));
        let n = nudge(&s(40.0, 800.0, 200.0, None), 3, HueParam::Speed, None, &m).unwrap();
        assert_eq!(n.settings.speed_mm_s, 1100.0);
        // Frequency below the model's range is refused; a short pulse is warned.
        assert!(nudge(&s(40.0, 800.0, 200.0, None), 1, HueParam::Frequency, None, &m).is_err());
        let n = nudge(&s(40.0, 800.0, 600.0, Some(8.0)), -1, HueParam::Auto, None, &m).unwrap();
        assert_eq!(n.settings.frequency_khz, 450.0);
        assert_eq!(n.warnings.len(), 2, "{:?}", n.warnings);
        // Finish offsets: brushed to mirror is +2 % power.
        let n = nudge(&s(40.0, 800.0, 600.0, None), 0, HueParam::Auto, Some((SurfaceFinish::Brushed, SurfaceFinish::Mirror)), &m).unwrap();
        assert_eq!((n.param, n.settings.power_pct), (Some(HueParam::Power), 42.0));
        let n = nudge(&s(40.0, 800.0, 600.0, None), 0, HueParam::Auto, Some((SurfaceFinish::Polished, SurfaceFinish::Mill)), &m).unwrap();
        assert_eq!(n.settings.power_pct, 39.0);
        // Out of range results are refused.
        assert!(nudge(&s(0.4, 800.0, 200.0, None), 1, HueParam::Power, None, &m).is_err());
        assert!(nudge(&s(40.0, 800.0, 200.0, None), 99, HueParam::Power, None, &m).is_err());
        let bad = HueModel { power_pct: 0.0, ..HueModel::default() };
        assert!(nudge(&s(40.0, 800.0, 200.0, None), 1, HueParam::Power, None, &bad).is_err());
    }

    #[test]
    fn layer_commands_nudge_and_colour() {
        let mut sess = Session::new(Document::default());
        let layer = sess.document().layers[0].id.clone();
        let r = sess
            .execute(Invocation { id: "layer.hue_nudge".into(), params: json!({ "layer": layer, "steps": -1, "via": "power" }) }, Origin::Ui)
            .unwrap();
        assert_eq!(r.result["param"], "power");
        let before = CutSettings::default().power_pct;
        assert_eq!(sess.document().layers[0].settings.power_pct, before + 0.5);
        assert!(sess.execute(Invocation { id: "layer.hue_nudge".into(), params: json!({ "layer": layer }) }, Origin::Ui).is_err());
        sess.execute(Invocation { id: "layer.set_result_color".into(), params: json!({ "layer": layer, "color": "#C80" }) }, Origin::Ui).unwrap();
        assert_eq!(sess.document().layers[0].extras.get(RESULT_COLOR_KEY).map(String::as_str), Some("#cc8800"));
        assert!(sess.execute(Invocation { id: "layer.set_result_color".into(), params: json!({ "layer": layer, "color": "red" }) }, Origin::Ui).is_err());
        sess.execute(Invocation { id: "layer.set_result_color".into(), params: json!({ "layer": layer, "color": null }) }, Origin::Ui).unwrap();
        assert!(!sess.document().layers[0].extras.contains_key(RESULT_COLOR_KEY));
        let q = sess
            .execute(
                Invocation { id: "material.hue_step".into(), params: json!({ "settings": CutSettings::default(), "steps": 2, "via": "speed" }) },
                Origin::Ui,
            )
            .unwrap();
        assert_eq!(q.result["settings"]["speed_mm_s"], 1200.0);
        // A linked layer is refused.
        let mut doc = sess.document().clone();
        doc.layers[0].link = Some(doc::PresetLink { library: "lib".into(), path: "a/b/c".into() });
        let mut linked = Session::new(doc);
        assert!(linked.execute(Invocation { id: "layer.hue_nudge".into(), params: json!({ "layer": layer, "steps": 1 }) }, Origin::Ui).is_err());
    }
}
