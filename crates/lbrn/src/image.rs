//! LightBurn's Image mode settings ([`doc::ImageSettings`]) and a Bitmap's Adjust Image values
//! ([`doc::ImageAdjust`]) ⇄ `.lbrn2`.
//!
//! **Image settings** live in a `CutSetting_Img` (and its `SubLayer`s, which start from the parent's values):
//!
//! | LightBurn tag | `ImageSettings` |
//! |---|---|
//! | `ditherMode` (`threshold`, `ordered`, `atkinson`, `dithered`, `stucki`, `jarvis`, `newsprint`, `halftone`, `sketch`, `grayscale`) | `dither`; other modes (`3dslice`) stay in `extras` with a warning |
//! | `interval` (else 25.4 / `dpi`) | `interval_mm`; `dpi` is written back as read while the interval is unchanged, else as round(25.4 / interval) |
//! | `angle`, `anglePerPass`, `bidir` | `angle_deg`, `angle_increment_deg`, `bidirectional` (also the layer's hatch, as for any LightBurn layer) |
//! | `negative`, `passThrough` | `negative`, `pass_through` |
//! | `useDotCorrection 1` + `dotWidth` | `dot_width_mm` (with correction off both stay in `extras` and `dot_width_mm` is 0) |
//! | `minPower` | `min_power_pct` |
//! | `halftoneAngle` | `halftone_angle_deg` |
//! | (unknown tag) | `halftone_cells_per_inch` is not written (no LightBurn file seen with it) |
//!
//! Values LightBurn leaves out (at its defaults) stay out while they still equal what the import assumed for them,
//! and every value keeps its source spelling while unchanged, so an unchanged import writes the tags back byte for
//! byte. Evidence: the owner's `photo on namecard.lbrn2` (`dithered`, `negative 1`, `bidir 0`, `useDotCorrection 1`,
//! `dotWidth 0.025`, `interval 0.0651` with `dpi 390`, `halftoneAngle 22`, `minPower 10`), the 3D sliced sample
//! (`3dslice`, `interval 0.025` with `dpi 1016`) and a template with `stucki` on all 30 layers.
//!
//! **Adjust Image** values are attributes of the `Bitmap` shape: `Gamma` (as is), `Contrast` and `Brightness`
//! (fractions in the file, 0.2 = 20 %; OpenEtch shows %), `EnhanceRadius`, `EnhanceAmount`, `EnhanceDenoise` (as
//! is; every sample has 0, so their scale is unverified). The attributes also stay in the object's extras, so their
//! spelling and order are kept while unchanged.

use std::collections::BTreeMap;

use doc::{DitherMode, ImageAdjust, ImageSettings};

use crate::model::{CutParams, boolean, num_like};

fn finite(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The line interval the import gives: `interval` when usable, else 25.4 / `dpi`, else `base`.
fn imported_interval(interval: Option<f64>, dpi: Option<&str>, base: f64) -> f64 {
    match (interval.filter(|v| *v > 0.0), dpi.and_then(finite).filter(|v| *v > 0.0)) {
        (Some(i), _) => i,
        (None, Some(d)) => 25.4 / d,
        (None, None) => base,
    }
}

/// Image settings from a `CutSetting_Img` (or one of its sub-layers, with `base` its parent's), taking the tags it
/// maps out of `rest` (the tags the caller keeps in `extras`). `interval`, `angle`, `anglePerPass` and `bidir` come
/// from the typed parameters (the caller maps them to the hatch too).
pub(crate) fn image_from(
    p: &CutParams,
    rest: &mut BTreeMap<String, String>,
    base: &ImageSettings,
    label: &str,
    warn: &mut dyn FnMut(String),
) -> ImageSettings {
    let mut s = base.clone();
    s.interval_mm = imported_interval(p.interval, rest.get("dpi").map(String::as_str), base.interval_mm);
    if rest.get("dpi").and_then(|d| finite(d)).is_some_and(|d| d > 0.0) {
        rest.remove("dpi");
    }
    s.angle_deg = p.angle.unwrap_or(base.angle_deg);
    s.angle_increment_deg = p.angle_per_pass.unwrap_or(base.angle_increment_deg);
    s.bidirectional = p.bidir.unwrap_or(base.bidirectional);
    if let Some(v) = p.min_power.filter(|v| (0.0..=100.0).contains(v)) {
        s.min_power_pct = v;
        rest.remove("minPower");
    }
    if let Some(m) = rest.get("ditherMode") {
        match DitherMode::from_lightburn(m) {
            Some(mode) => {
                s.dither = mode;
                rest.remove("ditherMode");
            }
            None => warn(format!(
                "{label}: image mode '{}' is not supported; it is kept for LightBurn, and OpenEtch engraves the \
                 layer's images with {}",
                m.trim(),
                base.dither.lightburn_name()
            )),
        }
    }
    for (tag, field) in [("negative", &mut s.negative), ("passThrough", &mut s.pass_through)] {
        if let Some(b) = rest.get(tag).and_then(|v| boolean(v)) {
            *field = b;
            rest.remove(tag);
        }
    }
    if let Some(a) = rest.get("halftoneAngle").and_then(|v| finite(v)) {
        s.halftone_angle_deg = a;
        rest.remove("halftoneAngle");
    }
    let on = rest.get("useDotCorrection").and_then(|v| boolean(v)) == Some(true);
    if let Some(w) = rest.get("dotWidth").and_then(|v| finite(v)).filter(|w| on && *w > 0.0) {
        s.dot_width_mm = w;
        rest.remove("dotWidth");
        rest.remove("useDotCorrection");
    }
    s
}

/// The export side of [`image_from`]: write `img` into a cut setting's `tag → value` pairs. `source` is the setting's
/// source tags (a setting read from LightBurn) and `base` what the import assumed for a missing tag (the defaults,
/// or the parent's settings for a sub-layer). Returns a warning when something could not be written.
pub(crate) fn image_values(
    img: &ImageSettings,
    v: &mut BTreeMap<String, String>,
    source: Option<&[(String, String)]>,
    base: &ImageSettings,
) -> Option<String> {
    let src = |t: &str| source.and_then(|s| s.iter().find(|(k, _)| k == t)).map(|(_, x)| x.as_str());
    let in_source = |t: &str| src(t).is_some();
    // Shared with vector settings: written like `settings_values` does (always for a setting not from LightBurn).
    let shared = |v: &mut BTreeMap<String, String>, tag: &str, text: String, as_if_absent: bool| {
        if source.is_none() || in_source(tag) || !as_if_absent {
            v.insert(tag.to_string(), text);
        } else {
            v.remove(tag);
        }
    };
    // Image-only tags: written when in the source or no longer what the import assumed.
    let own = |v: &mut BTreeMap<String, String>, tag: &str, text: String, as_if_absent: bool| {
        if in_source(tag) || !as_if_absent {
            v.insert(tag.to_string(), text);
        } else {
            v.remove(tag);
        }
    };
    let flag = |tag: &str, b: bool| match src(tag) {
        Some(raw) if boolean(raw) == Some(b) => raw.to_string(),
        _ => if b { "1" } else { "0" }.to_string(),
    };
    let source_interval = src("interval").and_then(finite);
    let assumed_interval = imported_interval(source_interval, src("dpi"), base.interval_mm);
    let interval_same = img.interval_mm == assumed_interval;
    shared(v, "interval", num_like(src("interval"), img.interval_mm), source_interval.is_none() && interval_same);
    if source.is_none() || in_source("dpi") || !interval_same {
        let dpi = match src("dpi") {
            Some(raw) if interval_same => raw.to_string(),
            _ => format!("{}", (25.4 / img.interval_mm).round()),
        };
        v.insert("dpi".into(), dpi);
    }
    shared(v, "angle", num_like(src("angle"), img.angle_deg), img.angle_deg == base.angle_deg);
    shared(
        v,
        "anglePerPass",
        num_like(src("anglePerPass"), img.angle_increment_deg),
        img.angle_increment_deg == base.angle_increment_deg,
    );
    if source.is_none() && img.angle_increment_deg == 0.0 {
        v.remove("anglePerPass");
    }
    shared(v, "bidir", flag("bidir", img.bidirectional), img.bidirectional == base.bidirectional);
    own(v, "minPower", num_like(src("minPower"), img.min_power_pct), img.min_power_pct == base.min_power_pct);
    let kept_unknown = v.get("ditherMode").is_some_and(|m| DitherMode::from_lightburn(m).is_none());
    if !(kept_unknown && img.dither == base.dither) {
        let text = match src("ditherMode") {
            Some(raw) if DitherMode::from_lightburn(raw) == Some(img.dither) => raw.to_string(),
            _ => img.dither.lightburn_name().to_string(),
        };
        own(v, "ditherMode", text, img.dither == base.dither && !kept_unknown);
    }
    own(v, "negative", flag("negative", img.negative), img.negative == base.negative);
    own(v, "passThrough", flag("passThrough", img.pass_through), img.pass_through == base.pass_through);
    own(
        v,
        "halftoneAngle",
        num_like(src("halftoneAngle"), img.halftone_angle_deg),
        img.halftone_angle_deg == base.halftone_angle_deg,
    );
    if img.dot_width_mm > 0.0 {
        v.insert("useDotCorrection".into(), flag("useDotCorrection", true));
        v.insert("dotWidth".into(), num_like(src("dotWidth"), img.dot_width_mm));
    } else if !v.contains_key("useDotCorrection") && src("useDotCorrection").and_then(boolean) == Some(true) {
        // Correction was on in the file and has been turned off; the width stays as it was.
        v.insert("useDotCorrection".into(), flag("useDotCorrection", false));
        if let Some(raw) = src("dotWidth") {
            v.insert("dotWidth".into(), raw.to_string());
        }
    }
    (img.halftone_cells_per_inch != base.halftone_cells_per_inch).then(|| {
        format!(
            "halftone cells per inch ({}) were not exported: LightBurn's tag for them is unknown",
            img.halftone_cells_per_inch
        )
    })
}

/// Adjust Image attributes: (attribute, read from the file into OpenEtch's value, OpenEtch's value into the file).
/// File value → OpenEtch value, or back.
type Convert = fn(f64) -> f64;

const ADJUST: [(&str, Convert, Convert); 6] = [
    ("Gamma", |x| x, |x| x),
    ("Contrast", percent, fraction),
    ("Brightness", percent, fraction),
    ("EnhanceRadius", |x| x, |x| x),
    ("EnhanceAmount", |x| x, |x| x),
    ("EnhanceDenoise", |x| x, |x| x),
];

/// A fraction as %, rounded to 1e-9 so 0.2 reads as 20 rather than 20.000000000000004.
fn percent(x: f64) -> f64 {
    (x * 100.0 * 1e9).round() / 1e9
}

fn fraction(x: f64) -> f64 {
    x / 100.0
}

fn field(a: &mut ImageAdjust, k: usize) -> &mut f64 {
    match k {
        0 => &mut a.gamma,
        1 => &mut a.contrast,
        2 => &mut a.brightness,
        3 => &mut a.enhance_radius,
        4 => &mut a.enhance_amount,
        _ => &mut a.denoise,
    }
}

/// The Adjust Image values of a Bitmap with attributes `attr`. A value that is missing, does not parse or is out of
/// OpenEtch's range keeps its default (and the second value is a warning naming it).
pub(crate) fn adjust_from(attr: &dyn Fn(&str) -> Option<String>) -> (ImageAdjust, Vec<String>) {
    let mut a = ImageAdjust::default();
    let mut warnings = Vec::new();
    for (k, (name, read, _)) in ADJUST.iter().enumerate() {
        let Some(raw) = attr(name) else { continue };
        let Some(v) = finite(&raw).map(read) else {
            warnings.push(format!("{name}=\"{raw}\" could not be read"));
            continue;
        };
        let mut t = a.clone();
        *field(&mut t, k) = v;
        if t.problem().is_none() {
            a = t;
        } else {
            warnings.push(format!("{name}={raw} is out of range and was not used"));
        }
    }
    (a, warnings)
}

/// Write `a` into a Bitmap's attributes, keeping each kept value's spelling while it still reads as the same value.
/// Attributes that are missing are added after the last Adjust one present (else at the end).
pub(crate) fn adjust_attributes(a: &ImageAdjust, attributes: &mut Vec<(String, String)>) {
    for (k, (name, read, write)) in ADJUST.iter().enumerate() {
        let mut a = a.clone();
        let value = *field(&mut a, k);
        match attributes.iter_mut().find(|(n, _)| n == name) {
            Some((_, raw)) => {
                if finite(raw).map(read) != Some(value) {
                    *raw = crate::model::num(write(value));
                }
            }
            None if value != *field(&mut ImageAdjust::default(), k) => {
                let at = attributes.iter().rposition(|(n, _)| ADJUST.iter().any(|(m, _, _)| m == n)).map_or(attributes.len(), |i| i + 1);
                attributes.insert(at, (name.to_string(), crate::model::num(write(value))));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> (CutParams, BTreeMap<String, String>) {
        let p = CutParams::from_values(pairs.iter().copied());
        let mapped = ["maxPower", "speed", "frequency", "QPulseWidth", "interval", "angle", "anglePerPass", "crossHatch", "bidir", "numPasses"];
        let rest = p.to_values().into_iter().filter(|(k, _)| !mapped.contains(&k.as_str())).collect();
        (p, rest)
    }

    #[test]
    fn namecard_image_settings_map_and_write_back() {
        let pairs = [
            ("minPower", "10"),
            ("maxPower", "60"),
            ("bidir", "0"),
            ("dotWidth", "0.025"),
            ("useDotCorrection", "1"),
            ("interval", "0.0651"),
            ("angle", "180"),
            ("halftoneAngle", "22"),
            ("negative", "1"),
            ("ditherMode", "dithered"),
            ("dpi", "390"),
        ];
        let (p, mut rest) = params(&pairs);
        let mut warnings = Vec::new();
        let s = image_from(&p, &mut rest, &ImageSettings::default(), "C00", &mut |w| warnings.push(w));
        assert!(warnings.is_empty());
        assert_eq!(s.dither, DitherMode::FloydSteinberg);
        assert_eq!((s.interval_mm, s.angle_deg, s.bidirectional, s.negative), (0.0651, 180.0, false, true));
        assert_eq!((s.dot_width_mm, s.min_power_pct, s.halftone_angle_deg), (0.025, 10.0, 22.0));
        assert!(rest.is_empty(), "every image tag is mapped: {rest:?}");
        // Unchanged: the same values with the same spelling.
        let source: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let mut v = BTreeMap::new();
        assert_eq!(image_values(&s, &mut v, Some(&source), &ImageSettings::default()), None);
        for (k, val) in pairs.iter().filter(|(k, _)| *k != "maxPower") {
            assert_eq!(v.get(*k).map(String::as_str), Some(*val), "{k}");
        }
        // A new interval rewrites the dpi; dot correction off writes 0 and keeps the width.
        let changed = ImageSettings { interval_mm: 0.1, dot_width_mm: 0.0, dither: DitherMode::Jarvis, ..s };
        let mut v = BTreeMap::new();
        image_values(&changed, &mut v, Some(&source), &ImageSettings::default());
        assert_eq!((v["interval"].as_str(), v["dpi"].as_str()), ("0.1", "254"));
        assert_eq!((v["useDotCorrection"].as_str(), v["dotWidth"].as_str()), ("0", "0.025"));
        assert_eq!(v["ditherMode"], "jarvis");
    }

    #[test]
    fn defaults_stay_out_and_unknown_modes_are_kept() {
        let (p, mut rest) = params(&[("ditherMode", "3dslice"), ("interval", "0.025"), ("dpi", "1016")]);
        let mut warnings = Vec::new();
        let s = image_from(&p, &mut rest, &ImageSettings::default(), "C29", &mut |w| warnings.push(w));
        assert_eq!(s.dither, DitherMode::Jarvis);
        assert!(warnings[0].contains("'3dslice' is not supported"), "{warnings:?}");
        assert_eq!(rest.get("ditherMode").map(String::as_str), Some("3dslice"), "kept for export");
        let source = vec![("ditherMode".to_string(), "3dslice".to_string()), ("interval".into(), "0.025".into()), ("dpi".into(), "1016".into())];
        let mut v: BTreeMap<String, String> = rest.clone();
        image_values(&s, &mut v, Some(&source), &ImageSettings::default());
        assert_eq!(v.get("ditherMode").map(String::as_str), Some("3dslice"));
        assert!(!v.contains_key("negative") && !v.contains_key("minPower") && !v.contains_key("halftoneAngle"));
        // Only dpi given: the interval follows it, and stays out on export.
        let (p, mut rest) = params(&[("dpi", "254")]);
        let s = image_from(&p, &mut rest, &ImageSettings::default(), "C00", &mut |_| {});
        assert!((s.interval_mm - 0.1).abs() < 1e-12);
        let mut v = BTreeMap::new();
        image_values(&s, &mut v, Some(&[("dpi".to_string(), "254".to_string())]), &ImageSettings::default());
        assert_eq!(v.get("dpi").map(String::as_str), Some("254"));
        assert!(!v.contains_key("interval"));
        // Settings not from LightBurn: the shared tags and dpi always, image-only tags when not default.
        let mut v = BTreeMap::new();
        let w = image_values(&ImageSettings { halftone_cells_per_inch: 30.0, ..ImageSettings::default() }, &mut v, None, &ImageSettings::default());
        let keys: Vec<&str> = v.keys().map(String::as_str).collect();
        assert_eq!(keys, ["angle", "bidir", "dpi", "interval"]);
        assert!(w.is_some_and(|w| w.contains("halftone cells")));
    }

    #[test]
    fn adjust_attributes_round_trip_with_their_spelling() {
        let attrs: Vec<(String, String)> = [("W", "4"), ("Gamma", "1"), ("Contrast", "0.2"), ("Brightness", "-0.11"), ("EnhanceAmount", "0"), ("EnhanceRadius", "0"), ("EnhanceDenoise", "0"), ("File", "x.png")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .to_vec();
        let get = |n: &str| attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
        let (a, w) = adjust_from(&get);
        assert!(w.is_empty());
        assert_eq!((a.gamma, a.contrast, a.brightness), (1.0, 20.0, -11.0));
        let mut out = attrs.clone();
        adjust_attributes(&a, &mut out);
        assert_eq!(out, attrs, "unchanged values keep their text");
        let mut out = attrs.clone();
        adjust_attributes(&ImageAdjust { contrast: 35.0, enhance_amount: 400.0, ..a }, &mut out);
        assert_eq!((out[2].1.as_str(), out[4].1.as_str()), ("0.35", "400"));
        // Missing attributes are added only when they matter; bad ones keep the default.
        let mut bare = vec![("W".to_string(), "4".to_string())];
        adjust_attributes(&ImageAdjust { gamma: 2.0, ..ImageAdjust::default() }, &mut bare);
        assert_eq!(bare[1], ("Gamma".to_string(), "2".to_string()));
        let (a, w) = adjust_from(&|n: &str| (n == "Contrast").then(|| "7".to_string()));
        assert_eq!(a.contrast, 0.0);
        assert!(w[0].contains("Contrast"), "{w:?}");
    }
}
