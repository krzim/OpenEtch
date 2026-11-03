//! Colour separation (xTool's Colour Separation; trace by colour): `image.color_separate` reduces an image object to a
//! few solid colours (`fileio::colorsep`: k-means in CIELAB, speck removal, background), traces each colour into a
//! path object on its own fill layer, and links each layer to the nearest of the given swatches (library presets
//! with a result colour) by colour distance, so a logo becomes a coloured MOPA coin in one step.
//! `image.color_separate_preview` is the dialog's live preview (the colours, their areas and matches, and the
//! separated image as a PNG) without changing the document.
//!
//! The engine passes the swatches of a library (`materials` routes and MCP's `material_color_separate`); an agent may
//! pass its own. Each new layer keeps the colour it should come out in (`finish.color`: the matched swatch's, else
//! the image's colour) and the image's colour (`finish.target`).

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{CutSettings, Document, FillMode, Geometry, Layer, ObjectId, PresetLink};
use fileio::colorsep::{self, Background, Separation};
use kurbo::{Affine, BezPath, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::file_err;
use super::finish::RESULT_COLOR_KEY;
use super::image_tools::{decoded, image_of};
use super::layer::{MAX_PASSES, validate_settings};
use super::preset::LINK_PATH_KEY;
use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The layer `extras` key holding the image colour a separated layer stands for.
pub const TARGET_COLOR_KEY: &str = "finish.target";
/// Most swatches one separation may be given.
const MAX_SWATCHES: usize = 500;

/// A colour a separated layer can take its settings from (a library preset with its result colour).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeparateSwatch {
    /// The colour the preset comes out, `#rrggbb`.
    pub color: String,
    /// Layer name for it (e.g. the preset's description).
    pub name: Option<String>,
    /// The preset's main settings (complete). Default: fill at the default settings.
    pub settings: Option<CutSettings>,
    #[serde(default)]
    pub sub_layers: Vec<CutSettings>,
    pub global_passes: Option<u32>,
    /// The preset's kept LightBurn values (`lbrn2.*`).
    #[serde(default)]
    pub extras: BTreeMap<String, String>,
    /// Link the layer to this library preset (LightBurn's Link); absent: the settings are copied.
    pub link: Option<PresetLink>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeparateInput {
    /// The image object id.
    pub id: ObjectId,
    /// Colours to reduce the image to, 1..=16. Default 4.
    #[serde(default = "four")]
    pub colors: u32,
    /// "auto" (default: the colour covering most of the border is not traced), "none", or "#rrggbb" (the colour
    /// nearest it is the background).
    pub background: Option<String>,
    /// Areas of one colour smaller than this many pixels merge into their surroundings. Default: the image's pixels
    /// / 25000, at least 4.
    pub min_speck_px: Option<u32>,
    /// 0 (only lines) ..= 1.33 (only curves); default 1.
    pub smoothness: Option<f64>,
    /// Node reduction tolerance in pixels; default 0.2.
    pub optimize: Option<f64>,
    /// Images larger than this many pixels on their long side are scaled down first, 64..=4096. Default 1200.
    pub max_side_px: Option<u32>,
    /// Presets to link the colours to (each colour takes the nearest within max_distance). Default none.
    #[serde(default)]
    pub swatches: Vec<SeparateSwatch>,
    /// Largest colour difference (CIE76 ΔE; about 2 is just visible, 10 clearly different) for a match. Default 30.
    pub max_distance: Option<f64>,
    /// Colour indices (as the preview lists them) not to trace. Default none.
    #[serde(default)]
    pub skip: Vec<usize>,
    /// Delete the image afterwards. Default false.
    #[serde(default)]
    pub delete_image: bool,
}

fn four() -> u32 {
    4
}

struct Prepared {
    sep: Separation,
    /// Per colour: the nearest swatch within reach and its distance.
    matches: Vec<Option<(usize, f64)>>,
    swatch_rgb: Vec<[u8; 3]>,
    width_mm: f64,
    height_mm: f64,
    transform: Affine,
    image_name: String,
}

fn prepare(cmd: &str, doc: &Document, i: &SeparateInput) -> Result<Prepared> {
    let o = image_of(cmd, doc, &i.id)?;
    let Geometry::Image { width_mm, height_mm, .. } = o.geometry else { unreachable!("an image") };
    if !(1..=colorsep::MAX_COLORS as u32).contains(&i.colors) {
        return Err(invalid(cmd, format!("colors must be within 1..={} (got {})", colorsep::MAX_COLORS, i.colors)));
    }
    let max_side = i.max_side_px.unwrap_or(1200);
    if !(64..=4096).contains(&max_side) {
        return Err(invalid(cmd, format!("max_side_px must be within 64..=4096 (got {max_side})")));
    }
    if i.swatches.len() > MAX_SWATCHES {
        return Err(invalid(cmd, format!("at most {MAX_SWATCHES} swatches")));
    }
    let max_distance = i.max_distance.unwrap_or(30.0);
    if !(max_distance.is_finite() && max_distance >= 0.0) {
        return Err(invalid(cmd, "max_distance must be 0 or more"));
    }
    let background = match i.background.as_deref().map(str::trim) {
        None | Some("auto") => Background::Auto,
        Some("none") => Background::None,
        Some(c) => Background::Color(colorsep::parse_hex(c).ok_or_else(|| invalid(cmd, format!("background must be auto, none or #rrggbb (got \"{c}\")")))?),
    };
    let mut swatch_rgb = Vec::with_capacity(i.swatches.len());
    for (n, s) in i.swatches.iter().enumerate() {
        swatch_rgb.push(colorsep::parse_hex(&s.color).ok_or_else(|| invalid(cmd, format!("swatches[{n}].color must be #rrggbb (got \"{}\")", s.color)))?);
    }
    let (img, _) = decoded(cmd, o)?;
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    if w.max(h) > max_side {
        let f = f64::from(max_side) / f64::from(w.max(h));
        let (nw, nh) = (((f64::from(w) * f).round() as u32).max(1), ((f64::from(h) * f).round() as u32).max(1));
        rgba = fileio::image::imageops::resize(&rgba, nw, nh, fileio::image::imageops::FilterType::Triangle);
    }
    let px = rgba.width() as usize * rgba.height() as usize;
    let min_speck = i.min_speck_px.unwrap_or_else(|| ((px / 25_000) as u32).max(4));
    let opts = colorsep::Options { colors: i.colors as usize, min_speck_px: min_speck, background };
    let sep = colorsep::separate(&rgba, &opts).map_err(file_err(cmd))?;
    let matches = sep
        .palette
        .iter()
        .map(|c| {
            swatch_rgb
                .iter()
                .enumerate()
                .map(|(k, s)| (k, colorsep::delta_e(c.rgb, *s)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .filter(|(_, d)| *d <= max_distance)
        })
        .collect();
    Ok(Prepared {
        sep,
        matches,
        swatch_rgb,
        width_mm,
        height_mm,
        transform: o.transform,
        image_name: o.name.clone(),
    })
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn colors_json(p: &Prepared, i: &SeparateInput) -> Vec<Value> {
    let total = p.sep.palette.iter().map(|c| c.pixels).sum::<usize>().max(1);
    p.sep
        .palette
        .iter()
        .enumerate()
        .map(|(k, c)| {
            let swatch = p.matches[k].map(|(s, d)| {
                let sw = &i.swatches[s];
                json!({
                    "index": s, "name": sw.name, "color": colorsep::hex(p.swatch_rgb[s]), "distance": round1(d),
                    "link": sw.link,
                })
            });
            json!({
                "index": k, "color": colorsep::hex(c.rgb), "pixels": c.pixels,
                "share": round1(c.pixels as f64 * 100.0 / total as f64), "background": c.background,
                "traced": !c.background && !i.skip.contains(&k), "swatch": swatch,
            })
        })
        .collect()
}

/// `image.color_separate_preview`.
pub struct SeparatePreview;

impl Command for SeparatePreview {
    type Input = SeparateInput;
    const ID: &'static str = "image.color_separate_preview";
    const TITLE: &'static str = "Colour separation preview";
    const DESCRIPTION: &'static str = "What image.color_separate would make with the same settings, without changing \
        the document: {colors: [{index, color, pixels, share (%), background, traced, swatch: {index, name, color, \
        distance, link} | null}], width, height (pixels separated), png_base64 (the separated image in its colours, \
        background transparent, at most 360 px)}. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: SeparateInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let p = prepare(cmd, doc, &i)?;
        let mut img = colorsep::preview(&p.sep);
        let side = img.width().max(img.height());
        if side > 360 {
            let f = 360.0 / f64::from(side);
            let (w, h) = (((f64::from(img.width()) * f).round() as u32).max(1), ((f64::from(img.height()) * f).round() as u32).max(1));
            img = fileio::image::imageops::resize(&img, w, h, fileio::image::imageops::FilterType::Nearest);
        }
        let png = fileio::bitmap::png(&fileio::image::DynamicImage::ImageRgba8(img), None).map_err(file_err(cmd))?;
        Ok((
            json!({ "colors": colors_json(&p, &i), "width": p.sep.width, "height": p.sep.height, "png_base64": STANDARD.encode(png) }),
            String::new(),
        ))
    }
}

/// `image.color_separate`.
pub struct ColorSeparate;

impl Command for ColorSeparate {
    type Input = SeparateInput;
    const ID: &'static str = "image.color_separate";
    const TITLE: &'static str = "Colour separation";
    const DESCRIPTION: &'static str = "Reduce an image object to `colors` solid colours (k-means in CIELAB; specks \
        under min_speck_px merged; background auto|none|#rrggbb left out), trace each into a path on a new fill \
        layer, all grouped. Each layer takes the settings of the nearest swatch within max_distance (ΔE, default 30): \
        swatches [{color, name?, settings?, sub_layers?, global_passes?, extras?, link?}] are presets with the colour \
        they come out (link: a linked layer). Layers keep extras finish.color (expected result) and finish.target \
        (image colour). skip: colour indices not traced; delete_image. Returns {created, layers, group, deleted}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SeparateInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let p = prepare(cmd, doc, &i)?;
        for (n, s) in i.swatches.iter().enumerate() {
            if let Some(st) = &s.settings {
                validate_settings(cmd, st).map_err(|e| invalid(cmd, format!("swatches[{n}].settings: {e}")))?;
            }
            for (k, sub) in s.sub_layers.iter().enumerate() {
                validate_settings(cmd, sub).map_err(|e| invalid(cmd, format!("swatches[{n}].sub_layers[{k}]: {e}")))?;
            }
            if s.sub_layers.len() > 64 {
                return Err(invalid(cmd, format!("swatches[{n}]: at most 64 sub-layers")));
            }
            if let Some(g) = s.global_passes
                && !(1..=MAX_PASSES).contains(&g)
            {
                return Err(invalid(cmd, format!("swatches[{n}].global_passes must be within 1..={MAX_PASSES}")));
            }
            if let Some((k, _)) = s.extras.iter().find(|(k, _)| !k.starts_with(lbrn::EXTRA_PREFIX) || !doc::is_extras_key(k)) {
                return Err(invalid(cmd, format!("swatches[{n}].extras key '{k}' is not a LightBurn value (lbrn2.<tag>)")));
            }
            if let Some(l) = &s.link
                && (l.library.trim().is_empty() || l.path.trim().is_empty() || l.path.len() > 1000 || l.library.len() > 200)
            {
                return Err(invalid(cmd, format!("swatches[{n}].link needs a library name and a material/title/description path")));
            }
        }
        // Trace first: nothing is added when a colour cannot be traced.
        let (smooth, optimize) = (i.smoothness.unwrap_or(1.0), i.optimize.unwrap_or(0.2));
        let to_field = p.transform * fileio::trace::pixel_to_local(p.sep.width, p.sep.height, p.width_mm, p.height_mm);
        let mut traced: Vec<(usize, BezPath)> = Vec::new();
        for (k, c) in p.sep.palette.iter().enumerate() {
            if c.background || i.skip.contains(&k) {
                continue;
            }
            let t = colorsep::trace_color(&p.sep, k, smooth, optimize).map_err(file_err(cmd))?;
            if t.contours == 0 {
                continue;
            }
            let path = to_field * t.path;
            validate_path(cmd, &path)?;
            traced.push((k, path));
        }
        if traced.is_empty() {
            return Err(CommandError::Failed(format!("{cmd}: no colour left to trace (all background or skipped)")));
        }
        if i.delete_image {
            util::check_unlocked(cmd, doc, std::slice::from_ref(&i.id))?;
        }
        let colors_json = colors_json(&p, &i);
        transact(doc, |doc| {
            let used: Vec<String> = doc.layers.iter().map(|l| l.color.to_ascii_lowercase()).collect();
            let mut free = lbrn::palette::PALETTE.iter().filter(|c| !used.contains(&c.to_ascii_lowercase())).copied().collect::<Vec<_>>().into_iter();
            let mut created = Vec::new();
            let mut layers = Vec::new();
            for (n, (k, path)) in traced.iter().enumerate() {
                let c = &p.sep.palette[*k];
                let target = colorsep::hex(c.rgb);
                let sw = p.matches[*k].map(|(s, d)| (&i.swatches[s], p.swatch_rgb[s], d));
                let lid = doc.alloc_id("layer")?;
                let color = free.next().unwrap_or(lbrn::palette::PALETTE[n % lbrn::palette::PALETTE.len()]);
                let name = match sw.and_then(|(s, _, _)| s.name.as_deref()) {
                    Some(nm) => util::name(cmd, "swatch name", nm)?,
                    None => format!("Colour {} {target}", n + 1),
                };
                let mut l = Layer::new(lid.clone(), &name, color);
                l.settings = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
                let result = match sw {
                    Some((s, rgb, _)) => {
                        if let Some(st) = &s.settings {
                            l.settings = st.clone();
                        }
                        l.sub_layers = s.sub_layers.clone();
                        if let Some(g) = s.global_passes {
                            l.global_passes = g;
                        }
                        l.extras.extend(s.extras.iter().filter(|(k, _)| k.as_str() != "lbrn2.index" && k.as_str() != LINK_PATH_KEY).map(|(k, v)| (k.clone(), v.clone())));
                        if let Some(link) = &s.link {
                            l.extras.insert(LINK_PATH_KEY.into(), link.path.clone());
                            l.link = Some(link.clone());
                        }
                        colorsep::hex(rgb)
                    }
                    None => target.clone(),
                };
                l.extras.insert(RESULT_COLOR_KEY.into(), result.clone());
                l.extras.insert(TARGET_COLOR_KEY.into(), target.clone());
                doc.layers.push(l);
                let centre = path.bounding_box().center();
                let local = Affine::translate(-centre.to_vec2()) * path.clone();
                let obj = add_object(doc, format!("{} colour {}", p.image_name, n + 1), lid.clone(), Geometry::Path { path: local }, Affine::translate(centre.to_vec2()))?;
                created.push(obj.clone());
                layers.push(json!({
                    "layer": lid, "object": obj, "index": k, "color": target, "result_color": result, "pixels": c.pixels,
                    "swatch": colors_json[*k]["swatch"],
                }));
            }
            let gid = doc.alloc_id("group")?;
            doc.groups.push(doc::Group::new(gid.clone(), format!("Colours of {}", p.image_name), None));
            for id in &created {
                if let Some(o) = doc.object_mut(id) {
                    o.group = Some(gid.clone());
                }
            }
            let mut deleted: Vec<ObjectId> = Vec::new();
            if i.delete_image {
                super::pathutil::delete_objects(doc, std::slice::from_ref(&i.id));
                doc.image_masks.remove(&i.id);
                deleted.push(i.id.clone());
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["layers"] = json!(layers);
            r["group"] = json!(gid);
            r["deleted"] = json!(deleted);
            let linked = layers.iter().filter(|l| !l["swatch"].is_null()).count();
            Ok((r, format!("separated image {} into {} colour layer(s), {linked} matched to presets", i.id, layers.len())))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    /// A white image with a red square and a blue square, imported as an image object.
    fn session_with_logo() -> (Session, String) {
        let img = fileio::image::RgbaImage::from_fn(40, 20, |x, y| {
            if (2..12).contains(&x) && (5..15).contains(&y) {
                fileio::image::Rgba([210, 30, 30, 255])
            } else if (25..35).contains(&x) && (5..15).contains(&y) {
                fileio::image::Rgba([30, 40, 200, 255])
            } else {
                fileio::image::Rgba([255, 255, 255, 255])
            }
        });
        let png = fileio::bitmap::png(&fileio::image::DynamicImage::ImageRgba8(img), None).unwrap();
        let mut s = Session::new(Document::default());
        let r = s
            .execute(
                Invocation { id: "art.import_image".into(), params: json!({ "data_base64": STANDARD.encode(png), "mime": "image/png", "width_mm": 40.0 }) },
                Origin::Ui,
            )
            .unwrap();
        let id = r.result["created"][0].as_str().unwrap().to_string();
        (s, id)
    }

    #[test]
    fn separates_traces_and_links_to_the_nearest_swatch() {
        let (mut s, id) = session_with_logo();
        let gold = CutSettings { mode: FillMode::Fill, power_pct: 33.0, frequency_khz: 600.0, ..CutSettings::default() };
        let swatches = json!([
            { "color": "#d02020", "name": "Red", "settings": gold, "link": { "library": "steel", "path": "Steel/Colours/Red" } },
            { "color": "#00ff00", "name": "Green" },
        ]);
        let pv = s
            .execute(Invocation { id: "image.color_separate_preview".into(), params: json!({ "id": id, "colors": 3, "swatches": swatches }) }, Origin::Ui)
            .unwrap();
        let colors = pv.result["colors"].as_array().unwrap();
        assert_eq!(colors.len(), 3);
        assert_eq!(colors[0]["color"], "#ffffff");
        assert_eq!(colors[0]["background"], true);
        assert!(pv.result["png_base64"].as_str().unwrap().len() > 20);
        let layers_before = s.document().layers.len();
        let r = s
            .execute(Invocation { id: "image.color_separate".into(), params: json!({ "id": id, "colors": 3, "swatches": swatches, "smoothness": 0 }) }, Origin::Ui)
            .unwrap();
        let made = r.result["layers"].as_array().unwrap();
        assert_eq!(made.len(), 2, "white is background");
        let doc = s.document();
        assert_eq!(doc.layers.len(), layers_before + 2);
        let red = made.iter().find(|l| l["color"] == "#d21e1e").unwrap();
        assert_eq!(red["swatch"]["name"], "Red");
        let rl = doc.layer(red["layer"].as_str().unwrap()).unwrap();
        assert_eq!((rl.name.as_str(), rl.settings.power_pct), ("Red", 33.0));
        assert_eq!(rl.link.as_ref().unwrap().path, "Steel/Colours/Red");
        assert_eq!(rl.extras[RESULT_COLOR_KEY], "#d02020");
        assert_eq!(rl.extras[TARGET_COLOR_KEY], "#d21e1e");
        let blue = made.iter().find(|l| l["color"] == "#1e28c8").unwrap();
        assert!(blue["swatch"].is_null(), "green is too far from blue");
        let bl = doc.layer(blue["layer"].as_str().unwrap()).unwrap();
        assert_eq!(bl.settings.mode, FillMode::Fill);
        assert_eq!(bl.extras[RESULT_COLOR_KEY], "#1e28c8");
        // The red square is 10 x 10 px of a 40 x 20 px image 40 mm wide: 10 mm square, left of centre.
        let o = doc.object(red["object"].as_str().unwrap()).unwrap();
        let Geometry::Path { path } = &o.geometry else { panic!("a path") };
        let b = (o.transform * path.clone()).bounding_box();
        assert!((b.width() - 10.0).abs() < 1e-6 && (b.height() - 10.0).abs() < 1e-6, "{b:?}");
        assert!((b.center().x - -13.0).abs() < 1e-6 && b.center().y.abs() < 1e-6, "{b:?}");
        // Skipping everything traceable is refused; one undo takes it all back.
        assert!(s.execute(Invocation { id: "image.color_separate".into(), params: json!({ "id": id, "colors": 3, "skip": [1, 2] }) }, Origin::Ui).is_err());
        s.undo().unwrap();
        assert_eq!(s.document().layers.len(), layers_before);
    }
}
