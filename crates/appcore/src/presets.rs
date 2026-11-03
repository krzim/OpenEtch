//! Material test and Adjust Image presets (moved from the engine's `test_presets` module, which keeps the files):
//! the Adjust Image preset type and its checks, LightBurn `.lbimg` conversion, preset names.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::AppError;
use commands::testgen_presets::MAX_PRESETS;

/// Longest preset name, characters.
pub const MAX_NAME_CHARS: usize = 100;

/// One Adjust Image preset.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImagePreset {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjust: Option<doc::ImageAdjust>,
    /// A partial `settings.image` of `layer.update`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Value>,
}

/// Check a preset's values: the adjustments' ranges, and the image settings by applying them to a scratch layer.
pub fn check_image_preset(p: &ImagePreset) -> Result<(), AppError> {
    if p.adjust.is_none() && p.image.is_none() {
        return Err(AppError::BadRequest("a preset holds adjust, image or both".into()));
    }
    if let Some(why) = p.adjust.as_ref().and_then(doc::ImageAdjust::problem) {
        return Err(AppError::BadRequest(format!("adjust: {why}")));
    }
    if let Some(image) = &p.image {
        let mut s = commands::Session::new(doc::Document::default());
        let id = s.document().layers.first().map(|l| l.id.clone()).unwrap_or_default();
        let inv = commands::Invocation { id: "layer.update".into(), params: json!({ "id": id, "settings": { "image": image } }) };
        s.execute(inv, commands::Origin::Ui).map_err(|e| AppError::BadRequest(format!("image: {e}")))?;
    }
    Ok(())
}

/// A LightBurn number as written from a float32 (1.2000000476837158), back to the value typed (1.2).
fn f32_value(v: &Value) -> Option<f64> {
    let x = v.as_f64().filter(|x| x.is_finite())?;
    format!("{}", x as f32).parse().ok()
}

/// A dialog mode name (`Stucki`, `Dither`, `3D Slice`, ...) as OpenEtch's mode.
fn lbimg_mode(name: &str) -> Option<doc::DitherMode> {
    let n: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    doc::DitherMode::from_lightburn(&n)
}

/// OpenEtch's mode as LightBurn's dialog names it (`Stucki` is from the sample; the others follow LightBurn's docs).
fn lbimg_mode_name(m: doc::DitherMode) -> &'static str {
    use doc::DitherMode as D;
    match m {
        D::Threshold => "Threshold",
        D::Ordered => "Ordered",
        D::Atkinson => "Atkinson",
        D::FloydSteinberg => "Dither",
        D::Stucki => "Stucki",
        D::Jarvis => "Jarvis",
        D::Newsprint => "Newsprint",
        D::Halftone => "Halftone",
        D::Sketch => "Sketch",
        D::Grayscale => "Grayscale",
        D::Slice3d => "3D Slice",
    }
}

/// LightBurn `.lbimg` presets (see the module docs) as OpenEtch's, with warnings for values left out.
pub fn lbimg_to_presets(v: &Value) -> Result<(Vec<ImagePreset>, Vec<String>), AppError> {
    let map = v.as_object().ok_or_else(|| AppError::BadRequest("not a LightBurn .lbimg preset file (expected a JSON object)".into()))?;
    if map.len() > MAX_PRESETS {
        return Err(AppError::BadRequest(format!("at most {MAX_PRESETS} presets")));
    }
    let (mut out, mut warnings) = (Vec::new(), Vec::new());
    for (name, p) in map {
        let Some(p) = p.as_object() else {
            warnings.push(format!("\"{}\" is not a preset and was left out", name.chars().take(60).collect::<String>()));
            continue;
        };
        let num = |k: &str| p.get(k).and_then(f32_value);
        let pct = |k: &str| num(k).map(|x| (x * 100.0 * 1e6).round() / 1e6);
        let d = doc::ImageAdjust::default();
        let adjust = doc::ImageAdjust {
            gamma: num("Gamma").unwrap_or(d.gamma),
            contrast: pct("Contrast").unwrap_or(d.contrast),
            brightness: pct("Brightness").unwrap_or(d.brightness),
            enhance_radius: num("EnhanceRadius").unwrap_or(d.enhance_radius),
            enhance_amount: pct("EnhanceAmount").unwrap_or(d.enhance_amount),
            denoise: num("EnhanceDenoise").unwrap_or(d.denoise),
        };
        let mut image = serde_json::Map::new();
        if let Some(m) = p.get("ImgMode").and_then(Value::as_str) {
            match lbimg_mode(m) {
                Some(mode) => {
                    image.insert("dither".into(), json!(mode));
                }
                None => warnings.push(format!("{name}: image mode \"{}\" is not supported; the preset keeps the layer's", m.chars().take(40).collect::<String>())),
            }
        }
        if let Some(i) = num("ImgInterval").filter(|i| *i > 0.0) {
            image.insert("interval_mm".into(), json!(i));
        }
        if let Some(b) = p.get("Negative").and_then(Value::as_bool) {
            image.insert("negative".into(), json!(b));
        }
        if let Some(a) = num("HalftoneAngle") {
            image.insert("halftone_angle_deg".into(), json!(a));
        }
        if let Some(c) = num("HalftoneCells").filter(|c| *c > 0.0) {
            image.insert("halftone_cells_per_inch".into(), json!(c));
        }
        let preset = ImagePreset { name: name.trim().to_owned(), adjust: Some(adjust), image: (!image.is_empty()).then_some(Value::Object(image)) };
        match check_name(name).and_then(|_| check_image_preset(&preset)) {
            Ok(()) => out.push(preset),
            Err(e) => warnings.push(format!("{}: {e}; left out", name.chars().take(60).collect::<String>())),
        }
    }
    Ok((out, warnings))
}

/// Presets as LightBurn's `.lbimg` text (every key LightBurn writes; what a preset lacks takes OpenEtch's defaults
/// with LightBurn's 0.1 mm interval).
pub fn presets_to_lbimg(presets: &[ImagePreset]) -> String {
    let mut root = serde_json::Map::new();
    for p in presets {
        let a = p.adjust.clone().unwrap_or_default();
        let mut s = doc::ImageSettings { interval_mm: 0.1, ..doc::ImageSettings::default() };
        if let Some(img) = &p.image
            && let Ok(merged) = serde_json::from_value::<doc::ImageSettings>({
                let mut base = serde_json::to_value(&s).unwrap_or(Value::Null);
                if let (Some(b), Some(o)) = (base.as_object_mut(), img.as_object()) {
                    for (k, v) in o {
                        b.insert(k.clone(), v.clone());
                    }
                }
                base
            })
        {
            s = merged;
        }
        let mut o = json!({
            "Brightness": a.brightness / 100.0,
            "Contrast": a.contrast / 100.0,
            "EnhanceAmount": a.enhance_amount / 100.0,
            "EnhanceRadius": a.enhance_radius,
            "Gamma": a.gamma,
            "HalftoneAngle": s.halftone_angle_deg,
            "HalftoneCells": s.halftone_cells_per_inch,
            "ImgInterval": s.interval_mm,
            "ImgMode": lbimg_mode_name(s.dither),
            "Negative": s.negative,
        });
        if a.denoise != 0.0 {
            o["EnhanceDenoise"] = json!(a.denoise);
        }
        root.insert(p.name.clone(), o);
    }
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b"    "));
    serde::Serialize::serialize(&Value::Object(root), &mut ser).unwrap_or(());
    let mut text = String::from_utf8(out).unwrap_or_default();
    text.push('\n');
    text
}

/// "<base> (n)" for a preset name that is taken, the base shortened so the result keeps to [`MAX_NAME_CHARS`] (a longer
/// name could not be saved, renamed or read back from an export).
pub fn numbered(base: &str, n: usize) -> String {
    let suffix = format!(" ({n})");
    let keep = MAX_NAME_CHARS.saturating_sub(suffix.chars().count());
    format!("{}{suffix}", base.chars().take(keep).collect::<String>().trim_end())
}

pub fn check_name(name: &str) -> Result<String, AppError> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > MAX_NAME_CHARS || n.chars().any(char::is_control) {
        return Err(AppError::BadRequest(format!(
            "a preset name is 1-{MAX_NAME_CHARS} characters without control characters (got {name:?})"
        )));
    }
    Ok(n.to_owned())
}

