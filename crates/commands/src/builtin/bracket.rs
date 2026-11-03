//! Bracket test (`generate.bracket_test`): a small material test grid around one complete setting (a library
//! entry's), so a preset can be re-checked on a new lot of material or after a lens change. It is the material test
//! generator (`generate.material_test`: labels, LightBurn's safe burn order, one layer per box, all grouped) with
//! each box's layer holding the whole preset (sub-layers and kept LightBurn values too) and only the two bracketed
//! values changed, by default power and speed within ±10 % in 5 steps.
//!
//! The boxes say where they came from, so picking the best one can be written back to the library entry (the
//! engine's `materials` routes): the group's `extras` hold `bracket.library` and `bracket.path` (the entry's
//! LinkPath), `bracket.rows` and `bracket.columns` (the tested settings); each box layer's `bracket.cell` holds its
//! offsets from the preset in percent, `"<row %>,<column %>"` (`"0,0"` is the preset itself).

use std::collections::BTreeMap;

use doc::{CutSettings, Document, PresetLink};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::validate_settings;
use super::testgen::{MAX_COUNT, MaterialTest, MaterialTestInput, TestAxis, TestParam};
use super::util::{invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The group `extras` keys of a bracket test.
pub const LIBRARY_KEY: &str = "bracket.library";
pub const PATH_KEY: &str = "bracket.path";
pub const ROWS_KEY: &str = "bracket.rows";
pub const COLUMNS_KEY: &str = "bracket.columns";
/// The box layer `extras` key: `"<row %>,<column %>"`.
pub const CELL_KEY: &str = "bracket.cell";

/// One bracketed setting.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BracketAxis {
    pub param: TestParam,
    /// How far the values go each way, percent of the preset's value, 0.1..=50. Default 10.
    #[serde(default = "ten")]
    pub spread_pct: f64,
    /// Values, 1..=15 (odd counts include the preset's own value). Default 5.
    #[serde(default = "five")]
    pub count: u32,
}

fn ten() -> f64 {
    10.0
}
fn five() -> u32 {
    5
}
fn cell() -> f64 {
    5.0
}
fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BracketInput {
    /// The preset's main settings (complete CutSettings).
    pub settings: CutSettings,
    /// The preset's sub-layers, copied to every box. Default none.
    #[serde(default)]
    pub sub_layers: Vec<CutSettings>,
    /// The preset's kept LightBurn values (`lbrn2.*`), copied to every box. Default none.
    #[serde(default)]
    pub extras: BTreeMap<String, String>,
    /// Across the columns. Default power ±10 %, 5 values.
    pub columns: Option<BracketAxis>,
    /// Down the rows. Default speed ±10 %, 5 values.
    pub rows: Option<BracketAxis>,
    /// Box size, mm. Default 5.
    #[serde(default = "cell")]
    pub cell_mm: f64,
    /// Gap between boxes, mm. Default 1.
    #[serde(default = "one")]
    pub spacing_mm: f64,
    /// Centre of the grid, field mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Mark the tested values beside the grid. Default true.
    #[serde(default = "yes")]
    pub labels: bool,
    /// Title marked above the grid. Default none.
    pub title: Option<String>,
    /// The library entry tested (recorded so the best box can be written back to it).
    pub source: Option<PresetLink>,
}

/// The value of `p` in `s` (`None` for a pulse width the settings leave to the source).
pub fn value_of(p: TestParam, s: &CutSettings) -> Option<f64> {
    Some(match p {
        TestParam::Power => s.power_pct,
        TestParam::Speed => s.speed_mm_s,
        TestParam::Frequency => s.frequency_khz,
        TestParam::PulseWidth => s.pulse_width_ns?,
        TestParam::Interval => s.hatch.interval_mm,
        TestParam::Passes => f64::from(s.passes),
    })
}

/// The JSON name of `p`.
pub fn param_name(p: TestParam) -> &'static str {
    match p {
        TestParam::Power => "power",
        TestParam::Speed => "speed",
        TestParam::Frequency => "frequency",
        TestParam::PulseWidth => "pulse_width",
        TestParam::Interval => "interval",
        TestParam::Passes => "passes",
    }
}

fn pct(v: f64, base: f64) -> f64 {
    // `+ 0.0` turns a rounded -0 into 0.
    ((v / base - 1.0) * 1000.0).round() / 10.0 + 0.0
}

/// A box value at 6 decimals (the generator's steps carry float noise: 40.000000000000004).
fn tidy(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// `generate.bracket_test`.
pub struct BracketTest;

impl Command for BracketTest {
    type Input = BracketInput;
    const ID: &'static str = "generate.bracket_test";
    const TITLE: &'static str = "Bracket test";
    const DESCRIPTION: &'static str = "Add a small material test grid around one complete setting (a library preset: \
        settings, sub_layers, extras), to re-check it on a new material lot: columns and rows each bracket one of \
        power, speed, frequency, pulse_width, interval or passes by spread_pct each way (default power x speed, \
        ±10 %, 5 x 5); every box's layer is the whole preset with those two values changed, in LightBurn's safe burn \
        order, labelled and grouped. source {library, path} records the preset so the best box can be written back \
        (the engine's material_bracket_pick). Returns {created, layers, group, boxes}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: BracketInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            validate_settings(cmd, &i.settings)?;
            if i.sub_layers.len() > 64 {
                return Err(invalid(cmd, "at most 64 sub-layers"));
            }
            for (n, s) in i.sub_layers.iter().enumerate() {
                validate_settings(cmd, s).map_err(|e| invalid(cmd, format!("sub_layers[{n}]: {e}")))?;
            }
            if let Some((k, _)) = i.extras.iter().find(|(k, _)| !k.starts_with(lbrn::EXTRA_PREFIX) || !doc::is_extras_key(k)) {
                return Err(invalid(cmd, format!("extras key '{k}' is not a LightBurn value (lbrn2.<tag>)")));
            }
            if let Some(l) = &i.source
                && (l.library.trim().is_empty() || l.path.trim().is_empty() || l.path.len() > 1000 || l.library.len() > 200)
            {
                return Err(invalid(cmd, "source needs a library name and a material/title/description path"));
            }
            let cols = i.columns.clone().unwrap_or(BracketAxis { param: TestParam::Power, spread_pct: 10.0, count: 5 });
            let rows = i.rows.clone().unwrap_or(BracketAxis { param: TestParam::Speed, spread_pct: 10.0, count: 5 });
            if cols.param == rows.param {
                return Err(invalid(cmd, "rows and columns must bracket different settings"));
            }
            let axis = |a: &BracketAxis| -> Result<(TestAxis, f64)> {
                if !(a.spread_pct.is_finite() && (0.1..=50.0).contains(&a.spread_pct)) {
                    return Err(invalid(cmd, format!("spread_pct must be within 0.1..=50 (got {})", a.spread_pct)));
                }
                if !(1..=15.min(MAX_COUNT)).contains(&a.count) {
                    return Err(invalid(cmd, format!("count must be within 1..=15 (got {})", a.count)));
                }
                let v = value_of(a.param, &i.settings)
                    .ok_or_else(|| invalid(cmd, "the settings leave the pulse width to the source; set one to bracket it"))?;
                let s = a.spread_pct / 100.0;
                let (min, max) = if a.count == 1 { (v, v) } else { (v * (1.0 - s), v * (1.0 + s)) };
                Ok((TestAxis { param: a.param, min, max, count: a.count }, v))
            };
            let (ra, rbase) = axis(&rows)?;
            let (ca, cbase) = axis(&cols)?;
            let input = MaterialTestInput {
                rows: Some(ra),
                columns: Some(ca),
                cell_width_mm: i.cell_mm,
                cell_height_mm: i.cell_mm,
                spacing_mm: i.spacing_mm,
                center: i.center,
                base: None,
                labels: i.labels,
                label_height_mm: (i.cell_mm * 0.3).clamp(0.8, 2.0),
                label_settings: None,
                border: false,
                border_settings: None,
                title: i.title.clone(),
            };
            let (mut r, _) = <MaterialTest as Command>::apply(doc, input)?;
            let boxes = r["boxes"].as_u64().unwrap_or(0) as usize;
            let layers: Vec<String> = r["layers"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
            let extras: Vec<(String, String)> = i.extras.iter().filter(|(k, _)| k.as_str() != "lbrn2.index" && k.as_str() != "lbrn2.LinkPath").map(|(k, v)| (k.clone(), v.clone())).collect();
            for id in layers.iter().take(boxes) {
                let l = doc.layer_mut(id).ok_or_else(|| CommandError::Failed(format!("layer {id} vanished")))?;
                let rv = tidy(value_of(rows.param, &l.settings).unwrap_or(rbase));
                let cv = tidy(value_of(cols.param, &l.settings).unwrap_or(cbase));
                let mut s = i.settings.clone();
                rows.param.set(&mut s, rv);
                cols.param.set(&mut s, cv);
                validate_settings(cmd, &s)?;
                l.settings = s;
                l.sub_layers = i.sub_layers.clone();
                l.extras.extend(extras.iter().cloned());
                l.extras.insert(CELL_KEY.into(), format!("{},{}", pct(rv, rbase), pct(cv, cbase)));
            }
            let gid = r["group"].as_str().unwrap_or_default().to_string();
            if let Some(g) = doc.groups.iter_mut().find(|g| g.id == gid) {
                g.name = "Bracket test".into();
                g.extras.insert(ROWS_KEY.into(), param_name(rows.param).into());
                g.extras.insert(COLUMNS_KEY.into(), param_name(cols.param).into());
                if let Some(src) = &i.source {
                    g.extras.insert(LIBRARY_KEY.into(), src.library.clone());
                    g.extras.insert(PATH_KEY.into(), src.path.clone());
                }
            }
            r["rows"] = json!(param_name(rows.param));
            r["columns"] = json!(param_name(cols.param));
            let of = i.source.as_ref().map(|s| format!(" of \"{}\"", s.path)).unwrap_or_default();
            Ok((r, format!("added a {} x {} bracket test{of}", rows.count, cols.count)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    #[test]
    fn boxes_hold_the_whole_preset_around_its_values() {
        let mut sess = Session::new(Document::default());
        let base = CutSettings { power_pct: 40.0, speed_mm_s: 1000.0, frequency_khz: 600.0, pulse_width_ns: Some(30.0), ..CutSettings::default() };
        let sub = CutSettings { power_pct: 5.0, ..CutSettings::default() };
        let r = sess
            .execute(
                Invocation {
                    id: "generate.bracket_test".into(),
                    params: json!({
                        "settings": base, "sub_layers": [sub], "extras": { "lbrn2.minPower": "7" },
                        "source": { "library": "steel", "path": "Steel/Colours/Gold" }, "title": "Gold",
                    }),
                },
                Origin::Ui,
            )
            .unwrap();
        assert_eq!(r.result["boxes"], 25);
        let doc = sess.document();
        let g = doc.groups.iter().find(|g| g.extras.contains_key(PATH_KEY)).unwrap();
        assert_eq!(g.extras[PATH_KEY], "Steel/Colours/Gold");
        assert_eq!((g.extras[ROWS_KEY].as_str(), g.extras[COLUMNS_KEY].as_str()), ("speed", "power"));
        let cells: Vec<&doc::Layer> = doc.layers.iter().filter(|l| l.extras.contains_key(CELL_KEY)).collect();
        assert_eq!(cells.len(), 25);
        let centre = cells.iter().find(|l| l.extras[CELL_KEY] == "0,0").unwrap();
        assert_eq!(centre.settings, base);
        assert_eq!(centre.sub_layers.len(), 1);
        assert_eq!(centre.extras.get("lbrn2.minPower").map(String::as_str), Some("7"));
        let corner = cells.iter().find(|l| l.extras[CELL_KEY] == "-10,10").unwrap();
        assert!((corner.settings.speed_mm_s - 900.0).abs() < 1e-9 && (corner.settings.power_pct - 44.0).abs() < 1e-9);
        assert_eq!(corner.settings.frequency_khz, 600.0);
        assert_eq!(corner.settings.pulse_width_ns, Some(30.0));
        // The safest box (fastest, lowest power) runs first.
        let first = doc.layers.iter().find(|l| l.extras.contains_key(CELL_KEY)).unwrap();
        assert_eq!(first.extras[CELL_KEY], "10,-10");
    }

    #[test]
    fn bad_brackets_are_refused() {
        let mut sess = Session::new(Document::default());
        let run = |sess: &mut Session, p: Value| sess.execute(Invocation { id: "generate.bracket_test".into(), params: p }, Origin::Ui);
        let s = CutSettings::default();
        assert!(run(&mut sess, json!({ "settings": s, "columns": { "param": "pulse_width" } })).is_err(), "no pulse width");
        assert!(run(&mut sess, json!({ "settings": s, "columns": { "param": "speed" } })).is_err(), "same axes");
        assert!(run(&mut sess, json!({ "settings": s, "columns": { "param": "power", "spread_pct": 80 } })).is_err());
        assert!(run(&mut sess, json!({ "settings": s, "extras": { "other.key": "1" } })).is_err());
        let r = run(&mut sess, json!({ "settings": s, "columns": { "param": "frequency", "count": 3 }, "rows": { "param": "power", "count": 1 } })).unwrap();
        assert_eq!(r.result["boxes"], 3);
    }
}
