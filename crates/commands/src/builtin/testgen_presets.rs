//! Material test presets (LightBurn's Material Test "Preset" list, LAY-070): a preset is a named set of
//! `generate.material_test` parameters. The engine stores them (module `test_presets`); this module checks them and
//! reads and writes LightBurn's preset file.
//!
//! **LightBurn's `material_test_presets.lbmt`** (in its prefs folder, `presets/`): documented only by name; the
//! layout below was read from the owner's own file (12 MOPA presets, LightBurn 2.x), not from a specification. It is
//! one JSON object of `name → preset`:
//!
//! | Key | OpenEtch | Notes |
//! |---|---|---|
//! | `XParam`, `YParam` | `columns.param`, `rows.param` | 1 power, 2 interval, 3 passes, 4 frequency, 5 Q-pulse (from the owner's presets: the ranges match these settings); **0 = speed is inferred** (it is the one setting left and no sample uses it) |
//! | `XMin`/`XMax`/`XCount`, `Y...` | `min`/`max`/`count` of the axis | frequency in kHz, interval in mm, Q-pulse in ns |
//! | `XSize`, `YSize` | `cell_width_mm`, `cell_height_mm` | |
//! | `XCenter`, `YCenter` | `center` | LightBurn coordinates (from the field's lower-left corner, 77 in a 153.8 mm field), so `center = XCenter - field / 2` |
//! | `MaterialCut` | `base` | a LightBurn cut setting as JSON (`type` Scan = fill, Cut = line; `maxPower`, `speed`, `frequency` in Hz, `QPulseWidth`, `interval`, `crossHatch`, `bidir`, `numPasses`, ...), converted like a `.clb` entry |
//! | `TextCut` | `label_settings`; `labels` = its `doOutput` (absent = on) | inferred: the file has no separate Enable Text key |
//! | `BorderCut` | `border_settings`; `border` = its `doOutput` | the owner's presets all have `doOutput: false` and no border |
//!
//! Not in the file, so left at OpenEtch's defaults on import and not written on export: the box spacing, label
//! height and title. Unknown keys are reported as warnings.

use doc::{CutSettings, Document, FillMode};
use serde_json::{Map, Value, json};

use super::testgen::MaterialTest;
use crate::{Command, CommandError, Result};

/// Most presets in one file or store.
pub const MAX_PRESETS: usize = 500;
/// Largest `.lbmt` file read, bytes.
pub const MAX_LBMT_BYTES: usize = 4 * 1024 * 1024;

/// A named material test preset: `params` is a `generate.material_test` input.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TestPreset {
    pub name: String,
    pub params: Value,
}

const ID: &str = "material_test_preset";

fn bad(msg: impl Into<String>) -> CommandError {
    super::util::invalid(ID, msg)
}

/// Check that `params` is a usable `generate.material_test` input: it is run on an empty document.
pub fn check_material_test_params(params: &Value) -> Result<()> {
    let input: <MaterialTest as Command>::Input =
        serde_json::from_value(params.clone()).map_err(|e| bad(format!("params: {e}")))?;
    let mut d = Document { field_mm: Some(150.0), ..Document::default() };
    MaterialTest::apply(&mut d, input).map(|_| ())
}

/// LightBurn's test parameter code ↔ OpenEtch's name.
const PARAMS: [(i64, &str); 6] =
    [(0, "speed"), (1, "power"), (2, "interval"), (3, "passes"), (4, "frequency"), (5, "pulse_width")];

fn param_name(code: &Value) -> Option<&'static str> {
    let c = code.as_f64().filter(|c| c.fract() == 0.0)? as i64;
    PARAMS.iter().find(|(k, _)| *k == c).map(|(_, n)| *n)
}

fn param_code(name: &str) -> Option<i64> {
    PARAMS.iter().find(|(_, n)| *n == name).map(|(k, _)| *k)
}

/// A LightBurn cut setting in JSON (`MaterialCut`, `TextCut`, `BorderCut`) as a settings patch, plus its
/// `doOutput` and warnings.
fn cut_from_json(what: &str, v: &Value, warnings: &mut Vec<String>) -> (Value, Option<bool>) {
    let Some(obj) = v.as_object() else {
        warnings.push(format!("{what} is not an object; defaults used"));
        return (Value::Null, None);
    };
    let text = |v: &Value| match v {
        Value::Bool(b) => Some(if *b { "1".to_owned() } else { "0".to_owned() }),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    let pairs: Vec<(String, String)> =
        obj.iter().filter(|(k, _)| k.as_str() != "type" && k.as_str() != "index").filter_map(|(k, v)| Some((k.clone(), text(v)?))).collect();
    let params = lbrn::CutParams::from_values(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let do_output = params.do_output;
    let kind = obj.get("type").and_then(Value::as_str).unwrap_or("Cut").to_owned();
    let index = obj.get("index").and_then(Value::as_u64).unwrap_or(0).min(29) as u32;
    let cs = lbrn::CutSetting { kind, image: false, index, params, sub_layers: Vec::new() };
    let (layer, w) = lbrn::cut_setting_to_layer(&cs, "preset");
    warnings.extend(w.into_iter().map(|w| format!("{what}: {w}")));
    (patch_of(&layer.settings), do_output)
}

/// The fields of `s` a material test uses, as a settings patch.
fn patch_of(s: &CutSettings) -> Value {
    json!({
        "mode": s.mode,
        "power_pct": s.power_pct,
        "speed_mm_s": s.speed_mm_s,
        "frequency_khz": s.frequency_khz,
        "pulse_width_ns": s.pulse_width_ns,
        "passes": s.passes,
        "hatch": {
            "interval_mm": s.hatch.interval_mm,
            "angle_deg": s.hatch.angle_deg,
            "angle_increment_deg": s.hatch.angle_increment_deg,
            "cross_hatch": s.hatch.cross_hatch,
            "bidirectional": s.hatch.bidirectional,
        },
    })
}

/// Read LightBurn's `material_test_presets.lbmt` for a galvo field of `field_mm`: the presets (each checked as a
/// material test; one that is not usable is left out with a warning) and warnings.
pub fn lbmt_to_presets(text: &str, field_mm: f64) -> Result<(Vec<TestPreset>, Vec<String>)> {
    if text.len() > MAX_LBMT_BYTES {
        return Err(bad(format!("the preset file is larger than {} MB", MAX_LBMT_BYTES / (1024 * 1024))));
    }
    if !(field_mm.is_finite() && field_mm > 0.0) {
        return Err(bad(format!("field_mm must be a positive number (got {field_mm})")));
    }
    let root: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| bad(format!("not a LightBurn material test preset file (JSON): {e}")))?;
    let Value::Object(map) = root else { return Err(bad("not a LightBurn material test preset file: expected a JSON object")) };
    if map.len() > MAX_PRESETS {
        return Err(bad(format!("more than {MAX_PRESETS} presets")));
    }
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for (name, p) in &map {
        let mut w = Vec::new();
        match preset_from_lbmt(name, p, field_mm, &mut w) {
            Ok(params) => match check_material_test_params(&params) {
                Ok(()) => {
                    warnings.extend(w.into_iter().map(|x| format!("{name}: {x}")));
                    out.push(TestPreset { name: name.clone(), params });
                }
                Err(e) => warnings.push(format!("{name}: left out: {e}")),
            },
            Err(e) => warnings.push(format!("{name}: left out: {e}")),
        }
    }
    Ok((out, warnings))
}

fn preset_from_lbmt(name: &str, p: &Value, field_mm: f64, warnings: &mut Vec<String>) -> std::result::Result<Value, String> {
    let _ = name;
    let o = p.as_object().ok_or("not an object")?;
    let num = |k: &str| o.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let axis = |a: &str| -> std::result::Result<Value, String> {
        let code = o.get(&format!("{a}Param")).ok_or(format!("{a}Param is missing"))?;
        let param = param_name(code).ok_or(format!("{a}Param {code} is not a known test setting"))?;
        let min = num(&format!("{a}Min")).ok_or(format!("{a}Min is missing"))?;
        let max = num(&format!("{a}Max")).ok_or(format!("{a}Max is missing"))?;
        let count = num(&format!("{a}Count")).unwrap_or(10.0);
        // LightBurn saves floats through single precision (0.004999999888 for 0.005): round to 7 significant digits.
        let (min, max) = (tidy(min), tidy(max));
        Ok(json!({ "param": param, "min": min, "max": max, "count": count.round().max(1.0) as u64 }))
    };
    let mut params = Map::new();
    params.insert("columns".into(), axis("X")?);
    params.insert("rows".into(), axis("Y")?);
    if let Some(w) = num("XSize") {
        params.insert("cell_width_mm".into(), json!(tidy(w)));
    }
    if let Some(h) = num("YSize") {
        params.insert("cell_height_mm".into(), json!(tidy(h)));
    }
    let half = field_mm / 2.0;
    params.insert("center".into(), json!([tidy(num("XCenter").unwrap_or(half) - half), tidy(num("YCenter").unwrap_or(half) - half)]));
    if let Some(m) = o.get("MaterialCut") {
        let (base, _) = cut_from_json("MaterialCut", m, warnings);
        params.insert("base".into(), base);
    }
    if let Some(t) = o.get("TextCut") {
        let (s, out) = cut_from_json("TextCut", t, warnings);
        params.insert("label_settings".into(), s);
        params.insert("labels".into(), json!(out.unwrap_or(true)));
    }
    if let Some(b) = o.get("BorderCut") {
        let (s, out) = cut_from_json("BorderCut", b, warnings);
        params.insert("border_settings".into(), s);
        params.insert("border".into(), json!(out.unwrap_or(true)));
    }
    let known = [
        "XParam", "XMin", "XMax", "XCount", "XSize", "XCenter", "YParam", "YMin", "YMax", "YCount", "YSize", "YCenter",
        "MaterialCut", "TextCut", "BorderCut",
    ];
    for k in o.keys().filter(|k| !known.contains(&k.as_str())) {
        warnings.push(format!("{k} is not known to OpenEtch; ignored"));
    }
    Ok(Value::Object(params))
}

/// `v` rounded to 7 significant digits (LightBurn's single-precision floats).
fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let digits = 7 - (v.abs().log10().floor() as i32 + 1);
    let f = 10f64.powi(digits.clamp(-300, 300));
    (v * f).round() / f
}

/// A settings patch (as a material test takes it) as a LightBurn cut setting in JSON, over `default`.
fn cut_to_json(patch: Option<&Value>, default: CutSettings, index: u32, do_output: Option<bool>) -> Value {
    let mut s = default;
    if let Some(p) = patch.filter(|p| !p.is_null()) {
        // Reuse the command's own patch rules through a throwaway layer.
        if let Ok(sp) = serde_json::from_value::<super::layer::SettingsPatch>(p.clone()) {
            let mut next = s.clone();
            if sp.apply(ID, &mut next).is_ok() {
                s = next;
            }
        }
    }
    let mut o = Map::new();
    o.insert("index".into(), json!(index));
    o.insert("name".into(), json!(format!("C{index:02}")));
    o.insert("type".into(), json!(if s.mode == FillMode::Line { "Cut" } else { "Scan" }));
    o.insert("maxPower".into(), json!(s.power_pct));
    o.insert("minPower".into(), json!(s.power_pct));
    o.insert("speed".into(), json!(s.speed_mm_s));
    o.insert("frequency".into(), json!(s.frequency_khz * 1000.0));
    if let Some(pw) = s.pulse_width_ns {
        o.insert("QPulseWidth".into(), json!(pw));
    }
    if s.mode != FillMode::Line {
        o.insert("interval".into(), json!(s.hatch.interval_mm));
        if s.hatch.angle_deg != 0.0 {
            o.insert("angle".into(), json!(s.hatch.angle_deg));
        }
        if s.hatch.angle_increment_deg != 0.0 {
            o.insert("anglePerPass".into(), json!(s.hatch.angle_increment_deg));
        }
        o.insert("crossHatch".into(), json!(s.hatch.cross_hatch));
        o.insert("bidir".into(), json!(s.hatch.bidirectional));
    }
    if s.passes != 1 {
        o.insert("numPasses".into(), json!(s.passes));
    }
    o.insert("priority".into(), json!(0));
    if let Some(d) = do_output {
        o.insert("doOutput".into(), json!(d));
    }
    Value::Object(o)
}

/// Write presets as LightBurn's `material_test_presets.lbmt` for a galvo field of `field_mm`. Presets whose
/// parameters do not read as a material test are left out with a warning; settings LightBurn's file has no place for
/// (spacing, label height, title) are reported.
pub fn presets_to_lbmt(presets: &[TestPreset], field_mm: f64) -> (String, Vec<String>) {
    let mut root = Map::new();
    let mut warnings = Vec::new();
    for p in presets {
        let input = match serde_json::from_value::<<MaterialTest as Command>::Input>(p.params.clone()) {
            Ok(i) => i,
            Err(e) => {
                warnings.push(format!("{}: left out: {e}", p.name));
                continue;
            }
        };
        let rows = p.params.get("rows").cloned().unwrap_or(json!({"param": "speed", "min": 500, "max": 5000, "count": 10}));
        let cols = p.params.get("columns").cloned().unwrap_or(json!({"param": "power", "min": 10, "max": 100, "count": 10}));
        let mut o = Map::new();
        for (a, ax) in [("X", &cols), ("Y", &rows)] {
            let code = ax.get("param").and_then(Value::as_str).and_then(param_code).unwrap_or(1);
            o.insert(format!("{a}Param"), json!(code));
            o.insert(format!("{a}Min"), ax.get("min").cloned().unwrap_or(json!(0)));
            o.insert(format!("{a}Max"), ax.get("max").cloned().unwrap_or(json!(0)));
            o.insert(format!("{a}Count"), ax.get("count").cloned().unwrap_or(json!(10)));
        }
        o.insert("XSize".into(), json!(input.cell_width_mm));
        o.insert("YSize".into(), json!(input.cell_height_mm));
        o.insert("XCenter".into(), json!(input.center[0] + field_mm / 2.0));
        o.insert("YCenter".into(), json!(input.center[1] + field_mm / 2.0));
        let fill = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
        let line = CutSettings { mode: FillMode::Line, power_pct: 30.0, speed_mm_s: 1000.0, ..CutSettings::default() };
        o.insert("MaterialCut".into(), cut_to_json(p.params.get("base"), fill, 0, None));
        o.insert("TextCut".into(), cut_to_json(p.params.get("label_settings"), line.clone(), 1, (!input.labels).then_some(false)));
        o.insert("BorderCut".into(), cut_to_json(p.params.get("border_settings"), line, 2, Some(input.border)));
        let mut lost = Vec::new();
        if (input.spacing_mm - 1.0).abs() > 1e-9 {
            lost.push("spacing");
        }
        if (input.label_height_mm - 1.5).abs() > 1e-9 {
            lost.push("label height");
        }
        if input.title.is_some() {
            lost.push("title");
        }
        if !lost.is_empty() {
            warnings.push(format!("{}: LightBurn's preset file has no place for the {}; left out", p.name, lost.join(", ")));
        }
        root.insert(p.name.clone(), Value::Object(o));
    }
    (serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_else(|_| "{}".into()), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LBMT: &str = r#"{
    "Steel Q x F": {
        "BorderCut": {"doOutput": false, "index": 2, "maxPower": 80, "name": "C02", "speed": 10, "type": "Cut"},
        "MaterialCut": {"QPulseWidth": 75, "bidir": false, "frequency": 300000, "index": 0, "interval": 0.00800000037997961,
            "crossHatch": true, "maxPower": 15, "minPower": 15, "name": "C00", "speed": 500, "type": "Scan"},
        "TextCut": {"dotTime": 1, "index": 1, "maxPower": 30, "name": "C01", "speed": 30, "type": "Cut"},
        "XCenter": 77, "XCount": 10, "XMax": 150, "XMin": 2, "XParam": 5, "XSize": 2,
        "YCenter": 77, "YCount": 5, "YMax": 0.004999999888241291, "YMin": 0.0020000000949949026, "YParam": 2, "YSize": 2.5
    },
    "Broken": {"XParam": 9, "YParam": 1}
}"#;

    #[test]
    fn lightburn_presets_read_as_material_tests_and_write_back() {
        let (presets, warnings) = lbmt_to_presets(LBMT, 154.0).unwrap();
        assert_eq!(presets.len(), 1, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.starts_with("Broken: left out")), "{warnings:?}");
        let p = &presets[0].params;
        assert_eq!(p["columns"], json!({"param": "pulse_width", "min": 2.0, "max": 150.0, "count": 10}));
        assert_eq!(p["rows"], json!({"param": "interval", "min": 0.002, "max": 0.005, "count": 5}));
        assert_eq!((p["cell_width_mm"].clone(), p["cell_height_mm"].clone(), p["center"].clone()), (json!(2.0), json!(2.5), json!([0.0, 0.0])));
        let base = &p["base"];
        assert_eq!((base["mode"].clone(), base["power_pct"].clone(), base["speed_mm_s"].clone()), (json!("fill"), json!(15.0), json!(500.0)));
        assert_eq!((base["frequency_khz"].clone(), base["pulse_width_ns"].clone()), (json!(300.0), json!(75.0)));
        assert_eq!((base["hatch"]["cross_hatch"].clone(), base["hatch"]["interval_mm"].clone()), (json!(true), json!(0.00800000037997961)));
        assert_eq!((p["labels"].clone(), p["border"].clone()), (json!(true), json!(false)));
        assert_eq!(p["label_settings"]["mode"], json!("line"));
        check_material_test_params(p).unwrap();

        let (text, warnings) = presets_to_lbmt(&presets, 154.0);
        assert!(warnings.is_empty(), "{warnings:?}");
        let back: Value = serde_json::from_str(&text).unwrap();
        let b = &back["Steel Q x F"];
        assert_eq!((b["XParam"].clone(), b["YParam"].clone(), b["XCenter"].clone(), b["YCount"].clone()), (json!(5), json!(2), json!(77.0), json!(5)));
        assert_eq!((b["MaterialCut"]["type"].clone(), b["MaterialCut"]["frequency"].clone(), b["MaterialCut"]["QPulseWidth"].clone()), (json!("Scan"), json!(300000.0), json!(75.0)));
        assert_eq!(b["BorderCut"]["doOutput"], json!(false));
        // And reads again to the same preset.
        let (again, _) = lbmt_to_presets(&text, 154.0).unwrap();
        assert_eq!(again[0].params["columns"], presets[0].params["columns"]);
        assert_eq!(again[0].params["base"]["power_pct"], json!(15.0));

        for bad in ["[]", "not json", "{\"x\": 1}"] {
            let r = lbmt_to_presets(bad, 154.0);
            assert!(r.is_err() || r.unwrap().0.is_empty(), "{bad}");
        }
        assert!(lbmt_to_presets(LBMT, f64::NAN).is_err());
    }

    /// The owner's own LightBurn preset file, read in place (never copied): every preset reads as a usable material
    /// test. Set OPENETCH_LBMT_SAMPLE to its path (LightBurn's prefs folder, presets/material_test_presets.lbmt).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_owners_lightburn_presets_read() {
        let Some(path) = std::env::var_os("OPENETCH_LBMT_SAMPLE") else {
            println!("note: OPENETCH_LBMT_SAMPLE not set; skipping");
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap();
        let count = serde_json::from_str::<Map<String, Value>>(&text).unwrap().len();
        let (presets, warnings) = lbmt_to_presets(&text, 153.8).unwrap();
        assert_eq!(presets.len(), count, "{warnings:?}");
        assert!(!warnings.iter().any(|w| w.contains("left out") || w.contains("not known")), "{warnings:?}");
        for p in &presets {
            let c = &p.params["center"];
            assert!(c[0].as_f64().unwrap().abs() < 1.0 && c[1].as_f64().unwrap().abs() < 1.0, "{}: {c}", p.name);
        }
    }
}
