//! LightBurn's cut options that OpenEtch executes with its own [`doc::CutOptions`] ⇄ `.lbrn2` tags, for a layer's
//! main settings and each sub-layer alike (LAY-034).
//!
//! | LightBurn tag | `CutOptions` |
//! |---|---|
//! | `scanOpt` (Fill Grouping: absent or `mergeAll` "Fill all shapes at once", `byGroup` "Fill groups together", `individual` "Fill shapes individually"; also the enum's numbers 2, 1, 0) | `fill_grouping` `all_at_once`, `by_group`, `individually` |
//! | `floodFill` (Flood Fill) | `flood_fill` |
//! | `autoRotate` (Auto-rotate) | `auto_rotate` |
//! | `wobbleEnable` + `wobbleStep`, `wobbleSize` mm (absent: 0.05, 0.1) | `wobble {step_mm, size_mm}` |
//! | `perforate` + `perfLen`, `perfSkip` mm (absent: 0.1, 0.1) | `perforation {cut_mm, skip_mm}` |
//! | `dotMode` + `dotTime` ms, `dotSpacing` mm (absent: 0, 0.1) | `dot_mode {dot_time_ms, dot_spacing_mm}` |
//!
//! Evidence: LightBurn's documentation (Fill Grouping's three choices with "Fill all shapes at once" the default;
//! Flood Fill fills regions to avoid travel across blank space; Auto-rotate advances the starting scan angle by the
//! angle increment on each run of the job; Wobble's step and size; Perforation's cut and skip lengths; Dot Mode's
//! time in ms) and the cut-parameter struct LightBurn's developer posted on its forum ("Looking for a definitive list
//! of parameters stored in a .clb file", 2026): `scanOpt` with `SO_Individual, SO_ByGroup, SO_MergeAll` (default
//! `SO_MergeAll`), `wobbleStep 0.05`, `wobbleSize 0.1`, `perfLen 0.1`, `perfSkip 0.1`, `dotTime 0`, `dotSpacing 0.1`
//! as defaults (LightBurn leaves values at their defaults out of files). How `scanOpt` is spelled in a file is not
//! confirmed: no sample has one (all of the owner's ~70 projects and ~170 libraries use the default); the names
//! `individual`, `byGroup`, `mergeAll` are a forum member's reading, and the enum numbers are accepted too. Any other
//! value fills all at once with a warning.
//!
//! The tags stay in the layer's extras, so an unchanged import writes them back byte for byte ([`options_values`]
//! only touches a tag whose option no longer reads as the kept tags say). A switch that is on with values OpenEtch
//! cannot use (e.g. Dot Mode with `dotTime` 0) is left off with a warning and stays listed by
//! [`crate::unexecuted_settings`]. Tabs (`tabsEnabled`) and Kerf Offset (`kerf`) are not mapped; see that module.

use std::collections::BTreeMap;

use doc::{CutOptions, DotMode, FillGrouping, Perforation, Wobble};

use crate::model::{boolean, num_like};

/// LightBurn's defaults for parameters it leaves out of files.
const WOBBLE_STEP: f64 = 0.05;
const WOBBLE_SIZE: f64 = 0.1;
const PERF_LEN: f64 = 0.1;
const PERF_SKIP: f64 = 0.1;
const DOT_TIME: f64 = 0.0;
const DOT_SPACING: f64 = 0.1;

/// A switch value: `1`/`true` (or any non-zero number) is on.
pub(crate) fn on(value: &str) -> bool {
    boolean(value).unwrap_or_else(|| value.trim().parse::<f64>().is_ok_and(|x| x != 0.0))
}

/// `scanOpt` → fill grouping, `None` for a value OpenEtch does not know.
pub(crate) fn fill_grouping_of(value: &str) -> Option<FillGrouping> {
    match value.trim() {
        "mergeAll" | "2" => Some(FillGrouping::AllAtOnce),
        "byGroup" | "1" => Some(FillGrouping::ByGroup),
        "individual" | "0" => Some(FillGrouping::Individually),
        _ => None,
    }
}

/// The cut options LightBurn's tags `v` turn on (see the module docs); a switch whose values cannot be used is left
/// off and reported through `warn`, prefixed with `label`.
pub(crate) fn options_from(v: &BTreeMap<String, String>, label: &str, warn: &mut dyn FnMut(String)) -> CutOptions {
    let switch = |tag: &str| v.get(tag).is_some_and(|x| on(x));
    let param = |tag: &str, default: f64| v.get(tag).map_or(Some(default), |x| x.trim().parse::<f64>().ok());
    let mut o = CutOptions { fill_grouping: FillGrouping::AllAtOnce, ..CutOptions::default() };
    if let Some(raw) = v.get("scanOpt") {
        match fill_grouping_of(raw) {
            Some(g) => o.fill_grouping = g,
            None => warn(format!("{label}: fill grouping (scanOpt) '{raw}' is not known; all shapes are filled at once")),
        }
    }
    o.flood_fill = switch("floodFill");
    o.auto_rotate = switch("autoRotate");
    // A switch that is on: its parameters (LightBurn's defaults when absent) as options, kept when they are valid.
    let mut usable = |name: &str, t: Option<CutOptions>| -> Option<CutOptions> {
        let Some(t) = t else {
            warn(format!("{label}: {name} is on but its values could not be read; it is not done"));
            return None;
        };
        match t.problem() {
            Some(p) => {
                warn(format!("{label}: {name} is on but {p}; it is not done"));
                None
            }
            None => Some(t),
        }
    };
    if switch("wobbleEnable") {
        let t = param("wobbleStep", WOBBLE_STEP).zip(param("wobbleSize", WOBBLE_SIZE)).map(|(step_mm, size_mm)| {
            CutOptions { wobble: Some(Wobble { step_mm, size_mm }), ..CutOptions::absent() }
        });
        o.wobble = usable("Wobble", t).and_then(|t| t.wobble);
    }
    if switch("perforate") {
        let t = param("perfLen", PERF_LEN).zip(param("perfSkip", PERF_SKIP)).map(|(cut_mm, skip_mm)| {
            CutOptions { perforation: Some(Perforation { cut_mm, skip_mm }), ..CutOptions::absent() }
        });
        o.perforation = usable("Perforation", t).and_then(|t| t.perforation);
    }
    if switch("dotMode") {
        let t = param("dotTime", DOT_TIME).zip(param("dotSpacing", DOT_SPACING)).map(|(dot_time_ms, dot_spacing_mm)| {
            CutOptions { dot_mode: Some(DotMode { dot_time_ms, dot_spacing_mm }), ..CutOptions::absent() }
        });
        o.dot_mode = usable("Dot Mode", t).and_then(|t| t.dot_mode);
    }
    o
}

/// The export side of [`options_from`]: write `o` into the LightBurn tags `v` (the kept ones of an imported layer,
/// else none), touching only the tags of options that no longer read as `v` says. A switch turned off is left out
/// (LightBurn's default), its parameters kept; a parameter at LightBurn's default is left out unless `v` has it.
pub(crate) fn options_values(o: &CutOptions, v: &mut BTreeMap<String, String>) {
    let read = options_from(v, "", &mut |_| {});
    if read.fill_grouping != o.fill_grouping {
        match o.fill_grouping {
            FillGrouping::AllAtOnce => v.remove("scanOpt"),
            FillGrouping::ByGroup => v.insert("scanOpt".into(), "byGroup".into()),
            FillGrouping::Individually => v.insert("scanOpt".into(), "individual".into()),
        };
    }
    let switch = |v: &mut BTreeMap<String, String>, tag: &str, was: bool, is: bool| {
        if was != is {
            if is {
                v.insert(tag.into(), "1".into());
            } else {
                v.remove(tag);
            }
        }
    };
    switch(v, "floodFill", read.flood_fill, o.flood_fill);
    switch(v, "autoRotate", read.auto_rotate, o.auto_rotate);
    switch(v, "wobbleEnable", read.wobble.is_some(), o.wobble.is_some());
    switch(v, "perforate", read.perforation.is_some(), o.perforation.is_some());
    switch(v, "dotMode", read.dot_mode.is_some(), o.dot_mode.is_some());
    let param = |v: &mut BTreeMap<String, String>, tag: &str, x: f64, default: f64| {
        let text = num_like(v.get(tag).map(String::as_str), x);
        if v.contains_key(tag) || x != default {
            v.insert(tag.into(), text);
        }
    };
    if let Some(w) = o.wobble.as_ref().filter(|w| read.wobble.as_ref() != Some(*w)) {
        param(v, "wobbleStep", w.step_mm, WOBBLE_STEP);
        param(v, "wobbleSize", w.size_mm, WOBBLE_SIZE);
    }
    if let Some(p) = o.perforation.as_ref().filter(|p| read.perforation.as_ref() != Some(*p)) {
        param(v, "perfLen", p.cut_mm, PERF_LEN);
        param(v, "perfSkip", p.skip_mm, PERF_SKIP);
    }
    if let Some(d) = o.dot_mode.as_ref().filter(|d| read.dot_mode.as_ref() != Some(*d)) {
        param(v, "dotTime", d.dot_time_ms, DOT_TIME);
        param(v, "dotSpacing", d.dot_spacing_mm, DOT_SPACING);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn read(pairs: &[(&str, &str)]) -> (CutOptions, Vec<String>) {
        let mut w = Vec::new();
        let o = options_from(&tags(pairs), "C00", &mut |m| w.push(m));
        (o, w)
    }

    #[test]
    fn absent_tags_fill_all_at_once_and_switch_nothing_on() {
        let (o, w) = read(&[("dotTime", "1"), ("perfLen", "0.01"), ("tabCount", "1")]);
        assert_eq!(o, CutOptions { fill_grouping: FillGrouping::AllAtOnce, ..CutOptions::absent() });
        assert!(w.is_empty());
    }

    #[test]
    fn switches_and_fill_grouping_are_read() {
        let (o, w) = read(&[
            ("scanOpt", "individual"),
            ("floodFill", "1"),
            ("autoRotate", "1"),
            ("wobbleEnable", "1"),
            ("perforate", "1"),
            ("perfLen", "0.5"),
            ("dotMode", "1"),
            ("dotTime", "2"),
        ]);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(o.fill_grouping, FillGrouping::Individually);
        assert!(o.flood_fill && o.auto_rotate);
        assert_eq!(o.wobble, Some(Wobble { step_mm: WOBBLE_STEP, size_mm: WOBBLE_SIZE }), "LightBurn's defaults");
        assert_eq!(o.perforation, Some(Perforation { cut_mm: 0.5, skip_mm: PERF_SKIP }));
        assert_eq!(o.dot_mode, Some(DotMode { dot_time_ms: 2.0, dot_spacing_mm: DOT_SPACING }));
        assert_eq!(read(&[("scanOpt", "byGroup")]).0.fill_grouping, FillGrouping::ByGroup);
        assert_eq!(read(&[("scanOpt", "2")]).0.fill_grouping, FillGrouping::AllAtOnce);
        assert!(!read(&[("floodFill", "0")]).0.flood_fill);
    }

    #[test]
    fn unusable_values_are_left_off_with_a_warning() {
        // Dot Mode at LightBurn's default time of 0 ms, a wobble step that is not a number, an unknown grouping.
        let (o, w) = read(&[("dotMode", "1"), ("wobbleEnable", "1"), ("wobbleStep", "x"), ("scanOpt", "spiral")]);
        assert_eq!((o.dot_mode, o.wobble, o.fill_grouping), (None, None, FillGrouping::AllAtOnce));
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].starts_with("C00: fill grouping (scanOpt) 'spiral'"), "{w:?}");
        assert!(w.iter().any(|m| m.contains("Dot Mode is on but options.dot_mode.dot_time_ms")), "{w:?}");
    }

    #[test]
    fn export_touches_only_changed_options() {
        let kept = tags(&[("floodFill", "1"), ("wobbleEnable", "1"), ("perfLen", "0.0100"), ("scanOpt", "individual")]);
        let o = options_from(&kept, "", &mut |_| {});
        let mut v = kept.clone();
        options_values(&o, &mut v);
        assert_eq!(v, kept, "unchanged options keep the tags as they were");
        let changed = CutOptions {
            flood_fill: false,
            fill_grouping: FillGrouping::AllAtOnce,
            wobble: Some(Wobble { step_mm: 0.2, size_mm: WOBBLE_SIZE }),
            perforation: Some(Perforation { cut_mm: 0.01, skip_mm: 0.3 }),
            ..o.clone()
        };
        options_values(&changed, &mut v);
        let expected =
            tags(&[("wobbleEnable", "1"), ("wobbleStep", "0.2"), ("perforate", "1"), ("perfLen", "0.0100"), ("perfSkip", "0.3")]);
        assert_eq!(v, expected);
        assert_eq!(options_from(&v, "", &mut |_| {}), changed, "the export reads back as the options");
        // Settings made in OpenEtch: only what differs from LightBurn's defaults is written.
        let mut v = BTreeMap::new();
        options_values(&CutOptions::default(), &mut v);
        assert!(v.is_empty(), "{v:?}");
        options_values(&CutOptions::absent(), &mut v);
        assert_eq!(v, tags(&[("scanOpt", "individual")]));
    }
}
