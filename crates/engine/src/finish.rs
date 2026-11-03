//! The MOPA finish library: what a material library entry gives on the material, kept beside the LightBurn `.clb`
//! in an OpenEtch sidecar (`<config-dir>/libraries/<name>.oefinish.json`) so the `.clb` stays LightBurn's own file
//! byte for byte. Per entry (keyed by its LinkPath, `material/title/description`; duplicated entries share a path and
//! so share their finish until one is renamed): a result photo (from the iPad or phone camera, or a file), the
//! expected result colour (by default the photo's average), notes, the lens, focus / defocus, the supplier, lot and
//! date, and the surface finish the setting was made for. Also the preset's OpenEtch-only settings a `.clb` cannot
//! hold: its layer's (and sub-layers') pause for the operator.
//!
//! Sidecar format: `{"format": "openetch-finish", "version": 1, "entries": {"<LinkPath>": Finish}}`, photos inline
//! as base64 (the web app sends them scaled to at most 1280 px, JPEG). Edits stay in memory with the library's other
//! edits and are written by Save Library (the library's unsaved mark covers both). The per-device hue step model
//! (`commands::finish::HueModel`) is kept in `libraries/hue-models.json`.

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::finish::{HueModel, SurfaceFinish};
use serde::{Deserialize, Serialize};

use crate::EngineError;

// The finish types (shared with the browser core).
pub use appcore::finish::{Finish, HUE_MODELS_FILE, MAX_PHOTO_BYTES, MAX_SIDECAR_BYTES, PHOTO_MIMES, Photo, SIDECAR_FORMAT, SIDECAR_SUFFIX};
/// Largest photo decoded to average its colour.
const MAX_PHOTO_PIXELS: u64 = 40_000_000;

pub fn now_unix_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A change to an entry's finish (`PUT .../finish`, MCP's `material_finish_update`): only the fields given change.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinishPatch {
    /// `#rrggbb` picks the result colour by hand; `null` goes back to the photo's average (or none).
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<String>")]
    pub color: Option<Option<String>>,
    /// mill | brushed | polished | mirror (what the setting was made for; `null` forgets it). Recording it changes no
    /// setting: adapting the setting to another surface is the hue nudge's `surface`.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<SurfaceFinish>")]
    pub surface: Option<Option<SurfaceFinish>>,
    pub notes: Option<String>,
    pub lens: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<f64>")]
    pub focus_mm: Option<Option<f64>>,
    pub supplier: Option<String>,
    pub lot: Option<String>,
    /// `YYYY-MM-DD` or "".
    pub date: Option<String>,
    /// A new photo (PNG, JPEG or WebP), base64. UI: the web app scales it first.
    #[schemars(skip)]
    pub photo_base64: Option<String>,
    #[schemars(skip)]
    pub photo_mime: Option<String>,
    /// Forget the photo.
    #[serde(default)]
    #[schemars(skip)]
    pub remove_photo: bool,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The average colour of the middle half (each way) of a photo, in linear light, as `#rrggbb`, and its size.
pub fn photo_average(bytes: &[u8], mime: Option<&str>) -> Result<(String, u32, u32), EngineError> {
    let format = fileio::bitmap::sniff(bytes, mime).ok_or_else(|| EngineError::BadRequest("the photo is not a PNG, JPEG or WebP image".into()))?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP) {
        return Err(EngineError::BadRequest("the photo must be a PNG, JPEG or WebP image".into()));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PHOTO_PIXELS * 4);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| EngineError::BadRequest(format!("cannot read the photo: {e}")))?.to_rgb8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(EngineError::BadRequest("the photo is empty".into()));
    }
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let srgb = |v: f64| {
        let c = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (c * 255.0).round().clamp(0.0, 255.0) as u8
    };
    let (x0, x1, y0, y1) = (w / 4, (w * 3 / 4).max(w / 4 + 1), h / 4, (h * 3 / 4).max(h / 4 + 1));
    let step = (((x1 - x0) as u64 * (y1 - y0) as u64 / 250_000) as f64).sqrt().max(1.0) as u32;
    let mut sum = [0.0f64; 3];
    let mut n = 0.0;
    for y in (y0..y1.min(h)).step_by(step as usize) {
        for x in (x0..x1.min(w)).step_by(step as usize) {
            let p = img.get_pixel(x, y);
            sum[0] += lin(p[0]);
            sum[1] += lin(p[1]);
            sum[2] += lin(p[2]);
            n += 1.0;
        }
    }
    let rgb = [srgb(sum[0] / n), srgb(sum[1] / n), srgb(sum[2] / n)];
    Ok((fileio::colorsep::hex(rgb), w, h))
}

/// Apply `p` to `f` (checked whole: on error `f` is unchanged).
pub fn apply_patch(f: &mut Finish, p: FinishPatch) -> Result<(), EngineError> {
    let mut next = f.clone();
    if let Some(n) = p.notes {
        next.notes = n;
    }
    if let Some(v) = p.lens {
        next.lens = v.trim().to_string();
    }
    if let Some(v) = p.supplier {
        next.supplier = v.trim().to_string();
    }
    if let Some(v) = p.lot {
        next.lot = v.trim().to_string();
    }
    if let Some(v) = p.date {
        next.date = v.trim().to_string();
    }
    if let Some(v) = p.focus_mm {
        next.focus_mm = v;
    }
    if let Some(v) = p.surface {
        next.surface = v;
    }
    if p.remove_photo {
        next.photo = None;
        if next.color_from_photo {
            next.color = None;
            next.color_from_photo = false;
        }
    }
    if let Some(b64) = &p.photo_base64 {
        let body = b64.trim_start().strip_prefix("data:").and_then(|r| r.split_once(',').map(|x| x.1)).unwrap_or(b64);
        let clean: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        if clean.len() > MAX_PHOTO_BYTES / 3 * 4 + 4 {
            return Err(EngineError::TooLarge(format!("a photo may be at most {} MB", MAX_PHOTO_BYTES / (1024 * 1024))));
        }
        let bytes = STANDARD.decode(clean.as_bytes()).map_err(|_| EngineError::BadRequest("photo_base64 is not base64".into()))?;
        let (average, width, height) = photo_average(&bytes, p.photo_mime.as_deref())?;
        let mime = match fileio::bitmap::sniff(&bytes, p.photo_mime.as_deref()) {
            Some(image::ImageFormat::Png) => "image/png",
            Some(image::ImageFormat::WebP) => "image/webp",
            _ => "image/jpeg",
        };
        next.photo = Some(Photo { mime: mime.into(), data_base64: STANDARD.encode(&bytes), width, height, average: average.clone(), taken_unix_s: now_unix_s() });
        if next.color.is_none() || next.color_from_photo {
            next.color = Some(average);
            next.color_from_photo = true;
        }
    }
    if let Some(c) = p.color {
        match c {
            Some(c) => {
                next.color = Some(commands::finish::normal_hex(&c).ok_or_else(|| EngineError::BadRequest(format!("color must be #rrggbb (got \"{c}\")")))?);
                next.color_from_photo = false;
            }
            None => {
                next.color = next.photo.as_ref().map(|p| p.average.clone());
                next.color_from_photo = next.color.is_some();
            }
        }
    }
    if let Some(problem) = next.problem() {
        return Err(EngineError::BadRequest(problem));
    }
    next.updated_unix_s = Some(now_unix_s());
    *f = next;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    format: String,
    #[serde(default)]
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, Finish>,
}

/// The sidecar text of `finish`.
pub fn sidecar_text(finish: &BTreeMap<String, Finish>) -> String {
    let s = Sidecar { format: SIDECAR_FORMAT.into(), version: 1, entries: finish.iter().filter(|(_, f)| !f.is_empty()).map(|(k, f)| (k.clone(), f.clone())).collect() };
    serde_json::to_string_pretty(&s).unwrap_or_else(|_| "{}".into())
}

/// Parse a sidecar (each entry checked; a broken entry is refused with its path).
pub fn parse_sidecar(bytes: &[u8]) -> Result<BTreeMap<String, Finish>, EngineError> {
    if bytes.len() > MAX_SIDECAR_BYTES {
        return Err(EngineError::TooLarge(format!("a finish file may be at most {} MB", MAX_SIDECAR_BYTES / (1024 * 1024))));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| EngineError::BadRequest("the finish file is not UTF-8 text".into()))?;
    let s: Sidecar = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| EngineError::BadRequest(format!("not an OpenEtch finish file: {e}")))?;
    if s.format != SIDECAR_FORMAT {
        return Err(EngineError::BadRequest(format!("not an OpenEtch finish file (format \"{}\")", s.format)));
    }
    if s.version > 1 {
        return Err(EngineError::BadRequest(format!("the finish file is version {}; this engine reads version 1", s.version)));
    }
    for (k, f) in &s.entries {
        if let Some(p) = f.problem() {
            return Err(EngineError::BadRequest(format!("finish of \"{k}\": {p}")));
        }
    }
    Ok(s.entries)
}

/// Read `<dir>/<name>.oefinish.json` (absent: empty).
pub fn read_sidecar(dir: &Path, name: &str) -> Result<BTreeMap<String, Finish>, EngineError> {
    match std::fs::read(dir.join(format!("{name}{SIDECAR_SUFFIX}"))) {
        Ok(b) => parse_sidecar(&b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(EngineError::Internal(format!("cannot read the finish file of \"{name}\": {e}"))),
    }
}

/// Write `<dir>/<name>.oefinish.json`, or remove it when nothing is kept.
pub fn write_sidecar(dir: &Path, name: &str, finish: &BTreeMap<String, Finish>) -> Result<(), EngineError> {
    let path = dir.join(format!("{name}{SIDECAR_SUFFIX}"));
    if finish.values().all(Finish::is_empty) {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| EngineError::Internal(format!("cannot remove {}: {e}", path.display())))?;
        }
        return Ok(());
    }
    crate::prefs::write_atomic(&path, sidecar_text(finish).as_bytes())
}

/// The hue step models by device id.
pub fn read_hue_models(dir: &Path) -> BTreeMap<String, HueModel> {
    std::fs::read_to_string(dir.join(HUE_MODELS_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_hue_models(dir: &Path, m: &BTreeMap<String, HueModel>) -> Result<(), EngineError> {
    let text = serde_json::to_string_pretty(m).map_err(|e| EngineError::Internal(e.to_string()))?;
    crate::prefs::write_atomic(&dir.join(HUE_MODELS_FILE), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(rgb: [u8; 3], border: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_fn(40, 40, |x, y| if (10..30).contains(&x) && (10..30).contains(&y) { image::Rgb(rgb) } else { image::Rgb(border) });
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn photo_sets_the_colour_until_one_is_picked() {
        let mut f = Finish::default();
        let p = FinishPatch { photo_base64: Some(STANDARD.encode(png([200, 150, 40], [0, 0, 0]))), photo_mime: Some("image/png".into()), ..FinishPatch::default() };
        apply_patch(&mut f, p).unwrap();
        // Only the middle half counts: the black border is left out.
        assert_eq!(f.color.as_deref(), Some("#c89628"));
        assert!(f.color_from_photo);
        assert_eq!(f.photo.as_ref().map(|p| (p.width, p.mime.as_str())), Some((40, "image/png")));
        // A picked colour stays when a new photo comes.
        apply_patch(&mut f, FinishPatch { color: Some(Some("#0A0B0C".into())), ..FinishPatch::default() }).unwrap();
        assert_eq!(f.color.as_deref(), Some("#0a0b0c"));
        let p = FinishPatch { photo_base64: Some(STANDARD.encode(png([10, 20, 200], [0, 0, 0]))), ..FinishPatch::default() };
        apply_patch(&mut f, p).unwrap();
        assert_eq!(f.color.as_deref(), Some("#0a0b0c"));
        // null goes back to the photo's average; removing the photo then forgets it.
        apply_patch(&mut f, FinishPatch { color: Some(None), ..FinishPatch::default() }).unwrap();
        assert_eq!(f.color.as_deref(), Some("#0a14c8"));
        apply_patch(&mut f, FinishPatch { remove_photo: true, ..FinishPatch::default() }).unwrap();
        assert_eq!((f.color.as_deref(), f.photo.is_none()), (None, true));
    }

    #[test]
    fn fields_are_checked_and_the_sidecar_round_trips() {
        let mut f = Finish::default();
        assert!(apply_patch(&mut f, FinishPatch { date: Some("2026-13-01".into()), ..FinishPatch::default() }).is_err());
        assert!(apply_patch(&mut f, FinishPatch { color: Some(Some("blue".into())), ..FinishPatch::default() }).is_err());
        assert!(apply_patch(&mut f, FinishPatch { focus_mm: Some(Some(500.0)), ..FinishPatch::default() }).is_err());
        assert!(apply_patch(&mut f, FinishPatch { photo_base64: Some("bm90IGFuIGltYWdl".into()), ..FinishPatch::default() }).is_err());
        assert_eq!(f, Finish::default(), "nothing changed");
        let p = FinishPatch {
            notes: Some("Lot 7 came out bluer".into()),
            lens: Some(" F254 ".into()),
            focus_mm: Some(Some(-1.5)),
            lot: Some("A-7".into()),
            date: Some("2026-11-02".into()),
            surface: Some(Some(SurfaceFinish::Brushed)),
            color: Some(Some("#c08040".into())),
            ..FinishPatch::default()
        };
        apply_patch(&mut f, p).unwrap();
        assert_eq!(f.lens, "F254");
        let mut map = BTreeMap::new();
        map.insert("Steel/Colours/Gold".to_string(), f.clone());
        map.insert("Steel/Colours/Empty".to_string(), Finish::default());
        let text = sidecar_text(&map);
        assert!(!text.contains("Empty"), "empty finishes are left out");
        let back = parse_sidecar(text.as_bytes()).unwrap();
        assert_eq!(back.get("Steel/Colours/Gold"), Some(&f));
        assert!(parse_sidecar(b"{\"format\": \"other\"}").is_err());
        assert!(parse_sidecar(b"{\"format\": \"openetch-finish\", \"version\": 2}").is_err());
        let s = f.summary();
        assert_eq!(s["surface"], "brushed");
        assert!(s.get("photo").is_none());
    }
}
