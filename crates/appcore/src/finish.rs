//! The finish of a material library entry (moved from the engine's `finish` module, which keeps the sidecar files):
//! result photo, colour, notes, lens, focus, lot, surface, and the preset's pauses for the operator.

use commands::finish::SurfaceFinish;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The sidecar's file name suffix (`<library>.oefinish.json`).
pub const SIDECAR_SUFFIX: &str = ".oefinish.json";
pub const SIDECAR_FORMAT: &str = "openetch-finish";
/// The per-device hue step models.
pub const HUE_MODELS_FILE: &str = "hue-models.json";
/// Largest photo kept (encoded).
pub const MAX_PHOTO_BYTES: usize = 3 * 1024 * 1024;
/// The types a result photo may have.
pub const PHOTO_MIMES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];
/// Largest sidecar read.
pub const MAX_SIDECAR_BYTES: usize = 128 * 1024 * 1024;

/// A result photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Photo {
    pub mime: String,
    pub data_base64: String,
    pub width: u32,
    pub height: u32,
    /// The photo's average colour (`#rrggbb`, the middle half of it).
    pub average: String,
    #[serde(default)]
    pub taken_unix_s: u64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What one entry gives on the material. See the module docs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Finish {
    /// The expected result colour, `#rrggbb`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The colour follows the photo's average (true until a colour is picked by hand).
    #[serde(skip_serializing_if = "is_false")]
    pub color_from_photo: bool,
    /// The surface the setting was made for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<SurfaceFinish>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The lens (e.g. "F254 175 x 175").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lens: String,
    /// Focus offset in mm (defocus: + above the focal plane, - below).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_mm: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub supplier: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub lot: String,
    /// The lot's or the test's date, `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo: Option<Photo>,
    /// The preset's pause for the operator before its layer (`CutOptions::operator_pause`, OpenEtch only): a `.clb`
    /// has no place for it, so it is kept here and given back when the preset is applied.
    #[serde(skip_serializing_if = "is_false")]
    pub operator_pause: bool,
    /// The preset's sub-layers (by index) that pause for the operator.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sub_layer_pauses: Vec<usize>,
    /// When the entry's settings last changed through a hue step or a bracket pick (a photo older than this shows the
    /// old settings).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_unix_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_unix_s: Option<u64>,
}

impl Finish {
    pub fn is_empty(&self) -> bool {
        Finish { updated_unix_s: None, settings_unix_s: None, ..self.clone() } == Finish::default()
    }

    /// The finish as the API shows it: the photo as its size and time (`GET .../photo` serves it).
    pub fn summary(&self) -> Value {
        let mut v = serde_json::to_value(Finish { photo: None, ..self.clone() }).unwrap_or_else(|_| json!({}));
        if let Some(p) = &self.photo {
            v["photo"] = json!({ "mime": p.mime, "width": p.width, "height": p.height, "average": p.average, "taken_unix_s": p.taken_unix_s, "bytes": p.data_base64.len() / 4 * 3 });
            v["photo_stale"] = json!(self.settings_unix_s.is_some_and(|s| s > p.taken_unix_s));
        }
        v
    }

    /// Why the finish cannot be kept, if it cannot.
    pub fn problem(&self) -> Option<String> {
        if let Some(c) = &self.color
            && commands::finish::normal_hex(c).as_deref() != Some(c.as_str())
        {
            return Some(format!("color must be #rrggbb in lower case (got \"{c}\")"));
        }
        for (k, v, max) in [("notes", &self.notes, 4000), ("lens", &self.lens, 200), ("supplier", &self.supplier, 200), ("lot", &self.lot, 200)] {
            if v.chars().count() > max || v.chars().any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t') {
                return Some(format!("{k} must be at most {max} characters of text"));
            }
        }
        if !self.date.is_empty() && !valid_date(&self.date) {
            return Some(format!("date must be YYYY-MM-DD (got \"{}\")", self.date));
        }
        if let Some(f) = self.focus_mm
            && !(f.is_finite() && (-100.0..=100.0).contains(&f))
        {
            return Some(format!("focus_mm must be within -100..=100 (got {f})"));
        }
        if let Some(p) = &self.photo {
            if p.data_base64.len() > MAX_PHOTO_BYTES / 3 * 4 + 4 {
                return Some(format!("a photo may be at most {} MB", MAX_PHOTO_BYTES / (1024 * 1024)));
            }
            // The photo is served with its own type (`GET .../photo`): only an image type, never one a browser would
            // run (an imported sidecar's "text/html" photo would be a page on the engine's origin).
            if !PHOTO_MIMES.contains(&p.mime.as_str()) {
                return Some(format!("a photo must be image/png, image/jpeg or image/webp (got \"{}\")", p.mime));
            }
            if commands::finish::normal_hex(&p.average).as_deref() != Some(p.average.as_str()) {
                return Some(format!("a photo's average must be #rrggbb in lower case (got \"{}\")", p.average));
            }
        }
        None
    }
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && (1..=12).contains(&s[5..7].parse::<u32>().unwrap_or(0))
        && (1..=31).contains(&s[8..10].parse::<u32>().unwrap_or(0))
}

