//! EzCad `markcfg7` import: the `[LMC_CFG]` section of an INI file onto a [`MachineProfile`].
//!
//! Keys used (all others are listed in [`MarkcfgImport::ignored`]):
//!
//! | Key | Profile field | Note |
//! |---|---|---|
//! | `FIELDSIZE` | `field_mm` | |
//! | `GALVOX` | `swap_xy = (GALVOX == 1)` | Verified on the owner's B6 (GALVOX = 1, needs the swap; 2026-11-01). |
//! | `GALVONEGATE0` / `GALVONEGATE1` | `negate_x` / `negate_y` | **Unverified** that 0 is X. |
//! | `ENABLECORFILE` + `CORFILE` | `cor_file` (basename only) | `None` when `ENABLECORFILE=0` or empty. |
//! | `REDLIGHTSPEED` | `red_light_speed_mm_s` | |
//! | `MINSPEED` / `MAXSPEED` | `mark_speed.min_mm_s` / `max_mm_s` | |
//! | `MINPWMFREQ` / `MAXPWMFREQ` | `frequency_khz_min` / `max` | Hz in the file. |
//! | `STARTMARKPORT` | `start_mark_input_bit` (0..=15, else none) | **Unverified** what is wired to it (14 on the B6). |
//!
//! No meerk40t or galvoplotter markcfg importer was found to check these against; the key list was chosen from the
//! B6's own file. Written from scratch.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::profile::{MachineProfile, ProfileError};

/// Result of a markcfg import ([`MarkcfgImport::apply`]): the profile plus which keys were applied, ignored or are of unverified meaning.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkcfgImport {
    pub profile: MachineProfile,
    pub applied: Vec<String>,
    pub ignored: Vec<String>,
    /// Applied keys whose meaning has not been confirmed on hardware.
    pub unverified: Vec<String>,
}

impl MarkcfgImport {
    /// Parse `text` and apply it onto `base` (see the module docs for the keys).
    pub fn apply(text: &str, base: MachineProfile) -> Result<Self, ProfileError> {
        import(text, base)
    }
}

/// Decode bytes as latin-1 (every byte is one char), so non-UTF-8 comment values never fail the import.
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// The `[LMC_CFG]` key/value pairs (keys case-preserved, values trimmed).
fn parse(text: &str) -> Result<BTreeMap<String, String>, ProfileError> {
    let mut in_section = false;
    let mut found = false;
    let mut values = BTreeMap::new();
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line.eq_ignore_ascii_case("[LMC_CFG]");
            found |= in_section;
            continue;
        }
        if !in_section {
            continue;
        }
        // Lines without '=' are skipped rather than refused: only a handful of keys matter.
        if let Some((k, v)) = line.split_once('=') {
            values.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    if !found {
        return Err(ProfileError::Markcfg("no [LMC_CFG] section".into()));
    }
    Ok(values)
}

pub(crate) fn import(text: &str, base: MachineProfile) -> Result<MarkcfgImport, ProfileError> {
    let values = parse(text)?;
    let mut p = base;
    let mut applied = Vec::new();
    let mut unverified = Vec::new();
    let cor_enabled = values.get("ENABLECORFILE").map(|v| flag("ENABLECORFILE", v)).transpose()?.unwrap_or(true);
    for (key, raw) in &values {
        let used = match key.as_str() {
            "FIELDSIZE" => {
                p.field_mm = number(key, raw)?;
                true
            }
            "GALVOX" => {
                p.swap_xy = flag(key, raw)?;
                true
            }
            "GALVONEGATE0" => {
                p.negate_x = flag(key, raw)?;
                unverified.push(key.clone());
                true
            }
            "GALVONEGATE1" => {
                p.negate_y = flag(key, raw)?;
                unverified.push(key.clone());
                true
            }
            "ENABLECORFILE" => true,
            "CORFILE" => {
                p.cor_file = if cor_enabled { basename(raw).map(PathBuf::from) } else { None };
                true
            }
            "REDLIGHTSPEED" => {
                p.red_light_speed_mm_s = number(key, raw)?;
                true
            }
            "MINSPEED" => {
                p.mark_speed.min_mm_s = number(key, raw)?;
                true
            }
            "MAXSPEED" => {
                p.mark_speed.max_mm_s = number(key, raw)?;
                true
            }
            "MINPWMFREQ" => {
                p.frequency_khz_min = number(key, raw)? / 1000.0;
                true
            }
            "MAXPWMFREQ" => {
                p.frequency_khz_max = number(key, raw)? / 1000.0;
                true
            }
            "STARTMARKPORT" => {
                let bit = number(key, raw)?;
                p.start_mark_input_bit = (bit.fract() == 0.0 && (0.0..=15.0).contains(&bit)).then_some(bit as u8);
                unverified.push(key.clone());
                true
            }
            _ => false,
        };
        if used {
            applied.push(key.clone());
        }
    }
    if !cor_enabled {
        p.cor_file = None;
    }
    let ignored = values.keys().filter(|k| !applied.contains(k)).cloned().collect();
    p.validate()?;
    Ok(MarkcfgImport { profile: p, applied, ignored, unverified })
}

fn number(key: &str, raw: &str) -> Result<f64, ProfileError> {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(ProfileError::Markcfg(format!("{key} is not a finite number: `{raw}`"))),
    }
}

fn flag(key: &str, raw: &str) -> Result<bool, ProfileError> {
    match raw {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(ProfileError::Markcfg(format!("{key} must be 0 or 1, got `{raw}`"))),
    }
}

/// The file name of a Windows or POSIX path (`F:\a\b\jcz150.cor` -> `jcz150.cor`), `None` when empty.
fn basename(raw: &str) -> Option<&str> {
    let name = raw.rsplit(['\\', '/']).next().unwrap_or("").trim();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKCFG: &[u8] = include_bytes!("../../../fixtures/markcfg7.ini");

    #[test]
    fn real_b6_markcfg() {
        let i = import(&latin1(MARKCFG), MachineProfile::generic_lmc_fiber()).unwrap();
        let p = &i.profile;
        assert_eq!(p.field_mm, 153.75);
        assert!(p.swap_xy && !p.negate_x && !p.negate_y);
        assert_eq!(p.cor_file, Some(PathBuf::from("jcz150.cor")));
        assert_eq!(p.red_light_speed_mm_s, 3000.0);
        assert_eq!((p.mark_speed.min_mm_s, p.mark_speed.max_mm_s), (1.0, 10000.0));
        assert_eq!((p.frequency_khz_min, p.frequency_khz_max), (1.0, 4000.0));
        assert_eq!(p.start_mark_input_bit, Some(14));
        assert_eq!(i.unverified, ["GALVONEGATE0", "GALVONEGATE1", "STARTMARKPORT"]);
        assert!(i.ignored.contains(&"FPK".to_owned()));
        assert!(i.applied.contains(&"CORFILE".to_owned()));
    }

    #[test]
    fn swap_negate_and_disabled_cor() {
        let text = "[LMC_CFG]\nFIELDSIZE=110\nGALVOX=0\nGALVONEGATE0=1\nGALVONEGATE1=0\nENABLECORFILE=0\nCORFILE=C:\\x\\a.cor\n";
        let p = MachineProfile::from_markcfg(text).unwrap();
        assert_eq!(p.field_mm, 110.0);
        assert!(!p.swap_xy && p.negate_x && !p.negate_y);
        assert_eq!(p.cor_file, None);
        assert_eq!(p.start_mark_input_bit, None);
        let p = MachineProfile::from_markcfg("[LMC_CFG]
STARTMARKPORT=-1
").unwrap();
        assert_eq!(p.start_mark_input_bit, None);
    }

    #[test]
    fn bad_input_refused() {
        assert!(MachineProfile::from_markcfg("FIELDSIZE=100\n").is_err());
        assert!(MachineProfile::from_markcfg("[LMC_CFG]\nFIELDSIZE=abc\n").is_err());
        assert!(MachineProfile::from_markcfg("[LMC_CFG]\nFIELDSIZE=-1\n").is_err());
        assert!(MachineProfile::from_markcfg("[LMC_CFG]\nGALVOX=2\n").is_err());
    }

    #[test]
    fn basenames() {
        assert_eq!(basename(r"C:\EzCad\jcz150.cor"), Some("jcz150.cor"));
        assert_eq!(basename("/a/b.cor"), Some("b.cor"));
        assert_eq!(basename("b.cor"), Some("b.cor"));
        assert_eq!(basename(""), None);
        assert_eq!(basename(r"C:\dir\"), None);
    }
}
