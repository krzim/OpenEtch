//! Font favourites and hidden fonts (LightBurn 2.1's Font Manager; `GET/PUT /api/fonts/prefs`), moved from the
//! engine's `fontprefs` module, which keeps the file.

use doc::FontKind;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Most fonts in each list.
pub const MAX_FONTS: usize = 2000;

/// A font as `font.list` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontRef {
    pub family: String,
    #[serde(default)]
    pub kind: FontKind,
}

/// The stored lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontPrefs {
    pub favourites: Vec<FontRef>,
    pub hidden: Vec<FontRef>,
}

impl FontPrefs {
    /// Trimmed names, no duplicates (ignoring case), nothing in both lists (a favourite wins), within the limits.
    pub fn clean(mut self) -> Result<FontPrefs, AppError> {
        let fix = |list: &mut Vec<FontRef>, what: &str| -> Result<(), AppError> {
            if list.len() > MAX_FONTS {
                return Err(AppError::BadRequest(format!("at most {MAX_FONTS} {what}")));
            }
            let mut out: Vec<FontRef> = Vec::with_capacity(list.len());
            for f in list.drain(..) {
                let family = f.family.trim().to_string();
                if family.is_empty() || family.chars().count() > 200 || family.contains(char::is_control) {
                    return Err(AppError::BadRequest(format!("{what}: '{}' is not a font name", f.family)));
                }
                if !out.iter().any(|o| o.kind == f.kind && o.family.eq_ignore_ascii_case(&family)) {
                    out.push(FontRef { family, kind: f.kind });
                }
            }
            *list = out;
            Ok(())
        };
        fix(&mut self.favourites, "favourites")?;
        fix(&mut self.hidden, "hidden fonts")?;
        let fav = self.favourites.clone();
        self.hidden.retain(|h| !fav.iter().any(|f| f.kind == h.kind && f.family.eq_ignore_ascii_case(&h.family)));
        Ok(self)
    }
}

