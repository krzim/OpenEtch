//! LightBurn settings an imported layer keeps but OpenEtch does not execute (LAY-034). LightBurn runs every setting
//! of a `.lbrn2` / `.clb` layer; the import keeps the tags OpenEtch does not model in `Layer.extras["lbrn2.<tag>"]`
//! (and `"lbrn2.subN.<tag>"` for sub-layer N) so they round-trip, but planning never reads extras. So a job can mark
//! differently from LightBurn without notice; [`unexecuted_settings`] lists, per layer, what that is:
//!
//! - **On in the file, off in OpenEtch**: LightBurn switches whose effect OpenEtch has its own cut option for.
//!   The import turns on those that mean the same (the `options` module: `floodFill` Flood Fill, `autoRotate`
//!   Auto-rotate, `wobbleEnable` Wobble, `perforate` Perforation, `dotMode` Dot Mode, and `scanOpt` Fill Grouping),
//!   so they are listed only when their values could not be used (e.g. Dot Mode with LightBurn's default `dotTime` of
//!   0) or the option was turned off since. Two are never turned on, because OpenEtch's option does not mean the same:
//!   `tabsEnabled` (LightBurn's tabs are placed by hand by default, can skip inner shapes and cut at a power; OpenEtch
//!   spaces gaps evenly on every closed outline) and `kerf` (Kerf Offset: LightBurn's documentation does not say how
//!   holes or fill layers are offset; OpenEtch grows parts and shrinks holes on line passes only, and no sample file
//!   has one to compare). Listed while the switch is on in the kept tags and OpenEtch's option is still off; setting
//!   the option (`layer.update settings.options`) clears the entry.
//! - **Unknown**: any other kept tag OpenEtch has no meaning for (LightBurn versions add settings), and a `scanOpt`
//!   value the import did not recognise. Listed with `active: null`, since whether it changes the output is not
//!   known.
//!
//! Tags that never change a galvo job are not listed: gantry-only values (`minPower` of a vector layer, `minPower2`,
//! `maxPower2`, `PPI`, `overscan`, `overscanPercent`), the per-layer `JumpSpeed` (galvo jumps use the device's), the
//! parameters of a switch (`perfLen`, `tabCount`, `dotTime`, ...), the library link (`LinkPath`), names
//! and bookkeeping (`subname`, `cleanupPass`, `isCleanup`, `dotWidth` and `useDotCorrection` of a vector layer).

use doc::{CutSettings, Layer};
use serde::Serialize;

use crate::convert::EXTRA_PREFIX;
use crate::options::{fill_grouping_of, on};

/// One setting a layer keeps that the job will not execute.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UnexecutedSetting {
    /// The LightBurn tag, e.g. `wobbleEnable`.
    pub tag: String,
    /// What LightBurn calls it, e.g. "Wobble" (the tag for unknown ones).
    pub setting: String,
    /// The value in the file.
    pub value: String,
    /// `None` for the main settings, else the 0-based sub-layer index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_layer: Option<usize>,
    /// `Some(true)`: the setting is on in the file and OpenEtch does not do it; `None`: unknown setting, its effect
    /// is not known.
    pub active: Option<bool>,
    /// What to do about it.
    pub note: String,
}

/// Switches OpenEtch has its own option for: (tag, LightBurn's name, whether OpenEtch's option is on, parameters,
/// why the import does not turn it on: `None` when it does, see the module docs).
type Switch = (&'static str, &'static str, fn(&CutSettings) -> bool, &'static [&'static str], Option<&'static str>);

const SWITCHES: &[Switch] = &[
    ("wobbleEnable", "Wobble", |s| s.options.wobble.is_some(), &["wobbleStep", "wobbleSize"], None),
    ("perforate", "Perforation", |s| s.options.perforation.is_some(), &["perfLen", "perfSkip"], None),
    (
        "tabsEnabled",
        "Tabs",
        |s| s.options.tabs.is_some(),
        &["tabCount", "tabCountMax", "tabSpacing", "tabSize", "tabsUseSpacing", "manualTabs", "skipInnerTabs", "tabCutPower"],
        Some("LightBurn's tabs, hand-placed by default and able to skip inner shapes or cut at a power, differ from OpenEtch's evenly spaced gaps"),
    ),
    ("floodFill", "Flood Fill", |s| s.options.flood_fill, &[], None),
    ("autoRotate", "Auto-rotate", |s| s.options.auto_rotate, &[], None),
    (
        "kerf",
        "Kerf Offset",
        |s| s.options.kerf_offset_mm != 0.0,
        &[],
        Some("how LightBurn offsets holes and fill layers is not known, so the offset is not copied"),
    ),
    ("dotMode", "Dot Mode", |s| s.options.dot_mode.is_some(), &["dotTime", "dotSpacing"], None),
];

/// Kept tags that never change a galvo job (see the module docs).
const HARMLESS: &[&str] = &[
    "minPower",
    "minPower2",
    "maxPower2",
    "PPI",
    "JumpSpeed",
    "overscan",
    "overscanPercent",
    "LinkPath",
    "subname",
    "cleanupPass",
    "isCleanup",
    "dotWidth",
    "useDotCorrection",
    "frequency_absent",
];

/// Every kept LightBurn setting of `layer` the job will not execute (see the module docs), main settings first,
/// then each sub-layer's, each in tag order. Empty for a layer that did not come from LightBurn.
pub fn unexecuted_settings(layer: &Layer) -> Vec<UnexecutedSetting> {
    let mut out = Vec::new();
    // Tool layers (T1 / T2) never output: nothing of theirs runs, so nothing is "not executed".
    if layer.extras.get(&format!("{EXTRA_PREFIX}type")).is_some_and(|t| t == "Tool") {
        return out;
    }
    // (sub-layer, tag, value) of every kept tag.
    let mut kept: Vec<(Option<usize>, &str, &str)> = Vec::new();
    for (k, v) in &layer.extras {
        let Some(tag) = k.strip_prefix(EXTRA_PREFIX) else { continue };
        if tag.starts_with('_') || matches!(tag, "index" | "type" | "image") {
            continue;
        }
        match sub_tag(tag) {
            Some((n, t)) if n >= 1 => {
                if !t.starts_with('_') && !matches!(t, "type" | "image") {
                    kept.push((Some(n - 1), t, v));
                }
            }
            Some(_) => {}
            None => kept.push((None, tag, v)),
        }
    }
    let settings_of = |sub: Option<usize>| -> Option<&CutSettings> {
        match sub {
            None => Some(&layer.settings),
            Some(i) => layer.sub_layers.get(i),
        }
    };
    let params: Vec<&str> = SWITCHES.iter().flat_map(|s| s.3.iter().copied()).collect();
    for &(sub, tag, value) in &kept {
        if let Some((_, name, has, _, why)) = SWITCHES.iter().find(|s| s.0 == tag) {
            let Some(settings) = settings_of(sub) else { continue };
            if on(value) && !has(settings) {
                let note = match why {
                    Some(why) => format!(
                        "{name} is on in the LightBurn file but OpenEtch does not do it for this layer ({why}); set \
                         it in the layer's cut options to mark like LightBurn"
                    ),
                    None => format!(
                        "{name} is on in the LightBurn file but off for this layer in OpenEtch (its values could not \
                         be used, or it was turned off); set it in the layer's cut options to mark like LightBurn"
                    ),
                };
                out.push(UnexecutedSetting {
                    tag: tag.into(),
                    setting: (*name).into(),
                    value: value.into(),
                    sub_layer: sub,
                    active: Some(true),
                    note,
                });
            }
            continue;
        }
        // Fill Grouping is executed (the `options` module) when its value is known.
        if tag == "scanOpt" && fill_grouping_of(value).is_some() {
            continue;
        }
        if HARMLESS.contains(&tag) || params.contains(&tag) {
            continue;
        }
        out.push(UnexecutedSetting {
            tag: tag.into(),
            setting: tag.into(),
            value: value.into(),
            sub_layer: sub,
            active: None,
            note: "a LightBurn setting OpenEtch does not know; it is kept for export but its effect is not reproduced"
                .into(),
        });
    }
    out.sort_by(|a, b| (a.sub_layer.map_or(0, |i| i + 1), &a.tag).cmp(&(b.sub_layer.map_or(0, |i| i + 1), &b.tag)));
    out
}

/// `subN.<tag>` → (N, tag).
fn sub_tag(tag: &str) -> Option<(usize, &str)> {
    let rest = tag.strip_prefix("sub")?;
    let digits = rest.find(|c: char| !c.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let (n, rest) = rest.split_at(digits);
    Some((n.parse().ok()?, rest.strip_prefix('.')?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(extras: &[(&str, &str)]) -> Layer {
        let mut l = Layer::new("layer-1".into(), "C00", "#000000");
        for (k, v) in extras {
            l.extras.insert(format!("{EXTRA_PREFIX}{k}"), v.to_string());
        }
        l
    }

    #[test]
    fn active_switches_and_unknown_tags_are_listed() {
        let mut l = layer(&[
            ("index", "0"),
            ("_src", "<s/>"),
            ("wobbleEnable", "1"),
            ("perforate", "0"),
            ("perfLen", "0.1"),
            ("tabsEnabled", "1"),
            ("tabCount", "4"),
            ("JumpSpeed", "4000"),
            ("minPower", "10"),
            ("futureThing", "7"),
            ("sub1.floodFill", "1"),
            ("sub1.subname", "pass 2"),
        ]);
        l.sub_layers.push(l.settings.clone());
        let u = unexecuted_settings(&l);
        let tags: Vec<(&str, Option<usize>, Option<bool>)> = u.iter().map(|s| (s.tag.as_str(), s.sub_layer, s.active)).collect();
        assert_eq!(
            tags,
            [("futureThing", None, None), ("tabsEnabled", None, Some(true)), ("wobbleEnable", None, Some(true)), ("floodFill", Some(0), Some(true))],
            "{u:?}"
        );
        assert_eq!(u[2].setting, "Wobble");
        // Turning OpenEtch's own option on clears the entry.
        l.settings.options.wobble = Some(doc::Wobble { step_mm: 0.1, size_mm: 0.5 });
        l.sub_layers[0].options.flood_fill = true;
        let u = unexecuted_settings(&l);
        assert_eq!(u.iter().map(|s| s.tag.as_str()).collect::<Vec<_>>(), ["futureThing", "tabsEnabled"]);
    }

    #[test]
    fn imported_switches_that_mean_the_same_are_executed_not_listed() {
        let cs = crate::parse(
            r#"<LightBurnProject AppVersion="2.1.04" FormatVersion="1">
    <CutSetting type="Scan">
        <index Value="0"/>
        <name Value="C00"/>
        <maxPower Value="50"/>
        <speed Value="1000"/>
        <floodFill Value="1"/>
        <autoRotate Value="1"/>
        <scanOpt Value="byGroup"/>
        <tabsEnabled Value="1"/>
        <SubLayer type="Cut" index="1">
            <maxPower Value="20"/>
            <speed Value="800"/>
            <dotMode Value="1"/>
            <wobbleEnable Value="1"/>
            <scanOpt Value="sideways"/>
        </SubLayer>
    </CutSetting>
</LightBurnProject>"#,
        )
        .unwrap()
        .cut_settings
        .remove(0);
        let (l, warnings) = crate::cut_setting_to_layer(&cs, "layer-1");
        assert!(l.settings.options.flood_fill && l.settings.options.auto_rotate);
        assert!(l.sub_layers[0].options.wobble.is_some(), "the sub-layer's wobble at LightBurn's defaults");
        let u = unexecuted_settings(&l);
        let tags: Vec<(&str, Option<usize>, Option<bool>)> = u.iter().map(|s| (s.tag.as_str(), s.sub_layer, s.active)).collect();
        // Tabs differ from OpenEtch's; Dot Mode's default time of 0 ms cannot be used; the grouping is not known.
        assert_eq!(tags, [("tabsEnabled", None, Some(true)), ("dotMode", Some(0), Some(true)), ("scanOpt", Some(0), None)], "{u:?}");
        assert!(u[0].note.contains("hand-placed"), "{}", u[0].note);
        assert!(warnings.iter().any(|w| w.contains("Dot Mode is on")), "{warnings:?}");
    }

    #[test]
    fn native_and_tool_layers_list_nothing() {
        assert!(unexecuted_settings(&Layer::new("l".into(), "L", "#000")).is_empty());
        assert!(unexecuted_settings(&layer(&[("type", "Tool"), ("wobbleEnable", "1")])).is_empty());
        assert!(unexecuted_settings(&layer(&[("kerf", "0"), ("autoRotate", "0")])).is_empty());
        assert_eq!(unexecuted_settings(&layer(&[("kerf", "0.05")]))[0].setting, "Kerf Offset");
    }
}
