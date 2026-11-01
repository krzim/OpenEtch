//! LightBurn preferences import (File > Preferences and bundles > Import): a LightBurn `.lbprefs` backup or its
//! `prefs.ini` (both the same JSON object, 233 top-level keys in LightBurn 2.x) mapped onto OpenEtch's preferences.
//!
//! **Privacy.** LightBurn's preferences hold things OpenEtch must not keep (device lists, licence and account data,
//! recent files and folders). Only the keys of [`Whitelist`] are read, each through a typed field; serde skips every
//! other key without keeping it, and the import reports only how many there were, never their names or values. Nothing
//! here logs. Mapped values are re-typed (numbers clamped, booleans checked, the language a two-letter code), so no
//! LightBurn string reaches OpenEtch's files except a validated language code.
//!
//! What maps where:
//! - **Engine** (applied on import, the current preferences backed up first): `AutoSaveSeconds` → autosave interval
//!   (minutes, rounded up); `Optimize_*` → the active device's default Optimization Settings.
//! - **Browser** (returned for the web UI to apply to its own preferences): units, grid spacing, grid and object
//!   snapping, arrow-key move steps, mouse-wheel direction, crosshair cursor, filled view, language, system clipboard,
//!   import options, framing options, and the Grid Array, Radial Array, Nest and Offset dialogs' last settings.

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Value, json};

use doc::{OrderBy, Optimization};

/// Largest preferences file read, bytes (LightBurn's are a few tens of kB).
pub const MAX_LBPREFS_BYTES: usize = 4 * 1024 * 1024;

/// The LightBurn preference keys OpenEtch maps, and nothing else. Each is kept only as the JSON value of that one key.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
#[allow(non_snake_case)]
struct Whitelist {
    units: Option<Value>,
    GridSpacing: Option<Value>,
    GridSnap: Option<Value>,
    GridSnapEnabled: Option<Value>,
    ObjSnap: Option<Value>,
    ObjSnapEnabled: Option<Value>,
    NormalMoveIncrement: Option<Value>,
    ShiftMoveIncrement: Option<Value>,
    ControlMoveIncrement: Option<Value>,
    InvertMouseZoom: Option<Value>,
    ShowFullSpanCursor: Option<Value>,
    FilledView: Option<Value>,
    Language: Option<Value>,
    UseSystemClipboard: Option<Value>,
    GroupImportedShapes: Option<Value>,
    SelectImportedShapes: Option<Value>,
    CenterImportedShapes: Option<Value>,
    ImportHiddenAILayers: Option<Value>,
    AutoSaveSeconds: Option<Value>,
    Frame_Mode: Option<Value>,
    Frame_Individually: Option<Value>,
    Frame_OutsideOnly: Option<Value>,
    Frame_ToolsOnly: Option<Value>,
    Optimize_OptimizeCuts: Option<Value>,
    Optimize_ByLayer: Option<Value>,
    Optimize_ByGroup: Option<Value>,
    Optimize_ByPriority: Option<Value>,
    Optimize_InnerToOuter: Option<Value>,
    Optimize_ReduceTravel: Option<Value>,
    Optimize_OptimalEntryPoint: Option<Value>,
    Optimize_ChooseCorners: Option<Value>,
    Optimize_AllowReverse: Option<Value>,
    Optimize_RemoveOverlaps: Option<Value>,
    Optimize_OverlapDist: Option<Value>,
    GridArray_XSpace: Option<Value>,
    GridArray_YSpace: Option<Value>,
    GridArray_BetweenCentersX: Option<Value>,
    GridArray_BetweenCentersY: Option<Value>,
    GridArray_XReverse: Option<Value>,
    GridArray_YReverse: Option<Value>,
    GridArray_XShiftByHalf: Option<Value>,
    GridArray_YShiftByHalf: Option<Value>,
    GridArray_CreateVirtual: Option<Value>,
    GridArray_GroupResults: Option<Value>,
    GridArray_SelectResults: Option<Value>,
    GridArray_RandomOrientation: Option<Value>,
    GridArray_RandomSeed: Option<Value>,
    RadialArray_NumCopies: Option<Value>,
    RadialArray_RotateCopies: Option<Value>,
    RadialArray_GroupResults: Option<Value>,
    RadialArray_SelectResults: Option<Value>,
    NestMultisheet: Option<Value>,
    DockQuickNestLockRotation: Option<Value>,
    OffsetDistance: Option<Value>,
    OffsetDeleteOriginals: Option<Value>,
}

/// Every key name in [`Whitelist`] (to count the others).
const KEYS: &[&str] = &[
    "units", "GridSpacing", "GridSnap", "GridSnapEnabled", "ObjSnap", "ObjSnapEnabled", "NormalMoveIncrement",
    "ShiftMoveIncrement", "ControlMoveIncrement", "InvertMouseZoom", "ShowFullSpanCursor", "FilledView", "Language",
    "UseSystemClipboard", "GroupImportedShapes", "SelectImportedShapes", "CenterImportedShapes", "ImportHiddenAILayers",
    "AutoSaveSeconds", "Frame_Mode", "Frame_Individually", "Frame_OutsideOnly", "Frame_ToolsOnly",
    "Optimize_OptimizeCuts", "Optimize_ByLayer", "Optimize_ByGroup", "Optimize_ByPriority", "Optimize_InnerToOuter",
    "Optimize_ReduceTravel", "Optimize_OptimalEntryPoint", "Optimize_ChooseCorners", "Optimize_AllowReverse",
    "Optimize_RemoveOverlaps", "Optimize_OverlapDist", "GridArray_XSpace", "GridArray_YSpace",
    "GridArray_BetweenCentersX", "GridArray_BetweenCentersY", "GridArray_XReverse", "GridArray_YReverse",
    "GridArray_XShiftByHalf", "GridArray_YShiftByHalf", "GridArray_CreateVirtual", "GridArray_GroupResults",
    "GridArray_SelectResults", "GridArray_RandomOrientation", "GridArray_RandomSeed", "RadialArray_NumCopies",
    "RadialArray_RotateCopies", "RadialArray_GroupResults", "RadialArray_SelectResults", "NestMultisheet",
    "DockQuickNestLockRotation", "OffsetDistance", "OffsetDeleteOriginals",
];

/// What a LightBurn preferences file maps to.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    /// Autosave interval in minutes (0: off), from `AutoSaveSeconds`.
    pub autosave_minutes: Option<u32>,
    /// Optimization Settings defaults for the active device, from `Optimize_*` (over `base`).
    pub optimization: Option<Optimization>,
    /// The browser-side preferences, in the web UI's own shapes (`prefs`, `view`, `files`, `laser`, `dialogs`,
    /// `system_clipboard`); only what the file had.
    pub browser: Value,
    /// The LightBurn keys that were found and used.
    pub used: Vec<&'static str>,
    /// Whitelisted keys found with a value OpenEtch could not use (wrong type or out of range).
    pub rejected: Vec<&'static str>,
    /// How many other top-level keys the file had (not read, not kept).
    pub other_keys: usize,
}

/// Collects typed values and the names of the keys they came from.
struct Reader {
    used: Vec<&'static str>,
    rejected: Vec<&'static str>,
}

impl Reader {
    fn bool(&mut self, key: &'static str, v: &Option<Value>) -> Option<bool> {
        let v = v.as_ref()?;
        // LightBurn writes booleans as JSON booleans; 0 / 1 are accepted too.
        let b = v.as_bool().or_else(|| v.as_i64().filter(|n| *n == 0 || *n == 1).map(|n| n == 1));
        self.note(key, b)
    }

    fn num(&mut self, key: &'static str, v: &Option<Value>, lo: f64, hi: f64) -> Option<f64> {
        let v = v.as_ref()?;
        let n = v.as_f64().filter(|n| n.is_finite() && (lo..=hi).contains(n));
        self.note(key, n)
    }

    fn int(&mut self, key: &'static str, v: &Option<Value>, lo: i64, hi: i64) -> Option<i64> {
        let v = v.as_ref()?;
        let n = v.as_i64().filter(|n| (lo..=hi).contains(n));
        self.note(key, n)
    }

    fn note<T>(&mut self, key: &'static str, v: Option<T>) -> Option<T> {
        if v.is_some() { self.used.push(key) } else { self.rejected.push(key) }
        v
    }
}

/// Round to 6 decimals (LightBurn stores floats as single precision: 0.05000000074505806).
fn tidy(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn put(m: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(key.into(), v);
    }
}

fn section(m: &mut Map<String, Value>, key: &str, inner: Map<String, Value>) {
    if !inner.is_empty() {
        m.insert(key.into(), Value::Object(inner));
    }
}

/// Read a LightBurn `.lbprefs` / `prefs.ini` and map the keys OpenEtch has. `base` is the Optimization the `Optimize_*`
/// keys are laid over (the active device's current defaults). Fails on anything that is not one JSON object.
pub fn map(bytes: &[u8], base: &Optimization) -> Result<Mapped, String> {
    if bytes.len() > MAX_LBPREFS_BYTES {
        return Err(format!("the file is larger than {} MB; not a LightBurn preferences file", MAX_LBPREFS_BYTES / (1024 * 1024)));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    // Key names only (values skipped), to count the keys that are not read.
    let names: std::collections::BTreeMap<String, IgnoredAny> =
        serde_json::from_slice(bytes).map_err(|_| "not a LightBurn preferences file (.lbprefs or prefs.ini: one JSON object)".to_string())?;
    let other_keys = names.keys().filter(|k| !KEYS.contains(&k.as_str())).count();
    drop(names);
    let w: Whitelist = serde_json::from_slice(bytes).map_err(|_| "not a LightBurn preferences file".to_string())?;
    let mut r = Reader { used: Vec::new(), rejected: Vec::new() };

    // Browser preferences (web/src/workspace/prefs.ts `Prefs`).
    let mut prefs = Map::new();
    put(&mut prefs, "units", r.int("units", &w.units, 0, 1).map(|u| json!(if u == 1 { "in" } else { "mm" })));
    put(&mut prefs, "gridMm", r.num("GridSpacing", &w.GridSpacing, 0.0, 1000.0).map(|v| json!(tidy(v))));
    put(&mut prefs, "snapTolPx", r.num("ObjSnap", &w.ObjSnap, 1.0, 40.0).map(|v| json!(v.round())));
    let mut nudge = Map::new();
    put(&mut nudge, "normal", r.num("NormalMoveIncrement", &w.NormalMoveIncrement, 0.001, 1000.0).map(|v| json!(tidy(v))));
    put(&mut nudge, "shift", r.num("ShiftMoveIncrement", &w.ShiftMoveIncrement, 0.001, 1000.0).map(|v| json!(tidy(v))));
    put(&mut nudge, "ctrl", r.num("ControlMoveIncrement", &w.ControlMoveIncrement, 0.001, 1000.0).map(|v| json!(tidy(v))));
    section(&mut prefs, "nudge", nudge);
    put(&mut prefs, "invertWheel", r.bool("InvertMouseZoom", &w.InvertMouseZoom).map(Value::Bool));
    put(&mut prefs, "crosshair", r.bool("ShowFullSpanCursor", &w.ShowFullSpanCursor).map(Value::Bool));
    if let Some(v) = &w.Language {
        let code = v.as_str().filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase())).map(str::to_string);
        put(&mut prefs, "language", r.note("Language", code).map(Value::String));
    }

    // Canvas snapping and view (App.tsx `ViewPrefs`).
    let mut view = Map::new();
    let mut snap = Map::new();
    put(&mut snap, "objects", r.bool("ObjSnapEnabled", &w.ObjSnapEnabled).map(Value::Bool));
    put(&mut snap, "grid", r.bool("GridSnapEnabled", &w.GridSnapEnabled).map(Value::Bool));
    put(&mut snap, "gridMm", r.num("GridSnap", &w.GridSnap, 0.001, 1000.0).map(|v| json!(tidy(v))));
    section(&mut view, "snap", snap);
    put(&mut view, "viewStyle", r.bool("FilledView", &w.FilledView).map(|f| json!(if f { "filled" } else { "wireframe" })));

    // Import options (files/fileFormats.ts `ImportPrefs`).
    let mut import = Map::new();
    put(&mut import, "group", r.bool("GroupImportedShapes", &w.GroupImportedShapes).map(Value::Bool));
    put(&mut import, "select", r.bool("SelectImportedShapes", &w.SelectImportedShapes).map(Value::Bool));
    put(&mut import, "center", r.bool("CenterImportedShapes", &w.CenterImportedShapes).map(Value::Bool));
    put(&mut import, "hiddenLayers", r.bool("ImportHiddenAILayers", &w.ImportHiddenAILayers).map(Value::Bool));
    let mut files = Map::new();
    section(&mut files, "importPrefs", import);

    // Framing (api/job.ts `FrameOptions`). Frame_Mode's numbers follow LightBurn's framing list order (rectangle,
    // rubber band, contour); unverified against LightBurn beyond the owner's file.
    let mut frame = Map::new();
    put(
        &mut frame,
        "style",
        r.int("Frame_Mode", &w.Frame_Mode, 0, 2).map(|m| json!(["bounds", "hull", "outline"][m as usize])),
    );
    put(&mut frame, "individually", r.bool("Frame_Individually", &w.Frame_Individually).map(Value::Bool));
    put(&mut frame, "outsideOnly", r.bool("Frame_OutsideOnly", &w.Frame_OutsideOnly).map(Value::Bool));
    put(&mut frame, "toolLayersOnly", r.bool("Frame_ToolsOnly", &w.Frame_ToolsOnly).map(Value::Bool));
    let mut laser = Map::new();
    section(&mut laser, "frame", frame);

    // Dialog defaults, as `art.array` / `art.circular_array` / `art.nest` / `path.offset` parameters.
    let mut grid = Map::new();
    put(&mut grid, "pitch_x_mm", r.num("GridArray_XSpace", &w.GridArray_XSpace, -10_000.0, 10_000.0).map(|v| json!(tidy(v))));
    put(&mut grid, "pitch_y_mm", r.num("GridArray_YSpace", &w.GridArray_YSpace, -10_000.0, 10_000.0).map(|v| json!(tidy(v))));
    let spacing = |c: bool| json!(if c { "center" } else { "gap" });
    put(&mut grid, "spacing_x", r.bool("GridArray_BetweenCentersX", &w.GridArray_BetweenCentersX).map(spacing));
    put(&mut grid, "spacing_y", r.bool("GridArray_BetweenCentersY", &w.GridArray_BetweenCentersY).map(spacing));
    put(&mut grid, "reverse_x", r.bool("GridArray_XReverse", &w.GridArray_XReverse).map(Value::Bool));
    put(&mut grid, "reverse_y", r.bool("GridArray_YReverse", &w.GridArray_YReverse).map(Value::Bool));
    if r.bool("GridArray_XShiftByHalf", &w.GridArray_XShiftByHalf) == Some(true) {
        grid.insert("row_shift".into(), json!("half"));
    }
    if r.bool("GridArray_YShiftByHalf", &w.GridArray_YShiftByHalf) == Some(true) {
        grid.insert("col_shift".into(), json!("half"));
    }
    put(&mut grid, "virtual", r.bool("GridArray_CreateVirtual", &w.GridArray_CreateVirtual).map(Value::Bool));
    put(&mut grid, "group_results", r.bool("GridArray_GroupResults", &w.GridArray_GroupResults).map(Value::Bool));
    put(&mut grid, "select_results", r.bool("GridArray_SelectResults", &w.GridArray_SelectResults).map(Value::Bool));
    let random = r.bool("GridArray_RandomOrientation", &w.GridArray_RandomOrientation);
    let seed = r.int("GridArray_RandomSeed", &w.GridArray_RandomSeed, 0, i64::from(u32::MAX));
    put(&mut grid, "random_orientation", random.map(Value::Bool));
    put(&mut grid, "random_seed", seed.map(|s| json!(s)));
    let mut radial = Map::new();
    put(&mut radial, "count", r.int("RadialArray_NumCopies", &w.RadialArray_NumCopies, 2, 10_000).map(|n| json!(n)));
    put(&mut radial, "rotate_copies", r.bool("RadialArray_RotateCopies", &w.RadialArray_RotateCopies).map(Value::Bool));
    put(&mut radial, "group_results", r.bool("RadialArray_GroupResults", &w.RadialArray_GroupResults).map(Value::Bool));
    put(&mut radial, "select_results", r.bool("RadialArray_SelectResults", &w.RadialArray_SelectResults).map(Value::Bool));
    let mut nest = Map::new();
    put(&mut nest, "multipage", r.bool("NestMultisheet", &w.NestMultisheet).map(Value::Bool));
    put(&mut nest, "lock_rotation", r.bool("DockQuickNestLockRotation", &w.DockQuickNestLockRotation).map(Value::Bool));
    let mut offset = Map::new();
    put(&mut offset, "distance_mm", r.num("OffsetDistance", &w.OffsetDistance, -1000.0, 1000.0).map(|v| json!(tidy(v))));
    put(&mut offset, "keep_original", r.bool("OffsetDeleteOriginals", &w.OffsetDeleteOriginals).map(|d| json!(!d)));
    let mut dialogs = Map::new();
    section(&mut dialogs, "grid_array", grid);
    section(&mut dialogs, "circular_array", radial);
    section(&mut dialogs, "nest", nest);
    section(&mut dialogs, "offset", offset);

    let mut browser = Map::new();
    section(&mut browser, "prefs", prefs);
    section(&mut browser, "view", view);
    section(&mut browser, "files", files);
    section(&mut browser, "laser", laser);
    section(&mut browser, "dialogs", dialogs);
    put(&mut browser, "system_clipboard", r.bool("UseSystemClipboard", &w.UseSystemClipboard).map(Value::Bool));

    // Engine: autosave (LightBurn seconds, OpenEtch minutes; any interval rounds up to at least a minute).
    let autosave_minutes = r.int("AutoSaveSeconds", &w.AutoSaveSeconds, 0, 24 * 3600).map(|s| ((s + 59) / 60) as u32);

    // Engine: Optimization Settings. Order By keys hold each criterion's place in the list, -1 when unticked.
    let mut o = base.clone();
    let mut any = false;
    if let Some(v) = r.bool("Optimize_OptimizeCuts", &w.Optimize_OptimizeCuts) {
        o.enabled = v;
        any = true;
    }
    let mut order: Vec<(i64, OrderBy)> = Vec::new();
    let mut have_order = false;
    for (key, v, by) in [
        ("Optimize_ByLayer", &w.Optimize_ByLayer, OrderBy::Layer),
        ("Optimize_ByGroup", &w.Optimize_ByGroup, OrderBy::Group),
        ("Optimize_ByPriority", &w.Optimize_ByPriority, OrderBy::Priority),
    ] {
        if let Some(n) = r.int(key, v, -1, 16) {
            have_order = true;
            if n >= 0 {
                order.push((n, by));
            }
        }
    }
    if have_order {
        order.sort_by_key(|(n, _)| *n);
        o.order_by = order.into_iter().map(|(_, b)| b).collect();
        any = true;
    }
    for (key, v, field) in [
        ("Optimize_InnerToOuter", &w.Optimize_InnerToOuter, &mut o.inner_first),
        ("Optimize_ReduceTravel", &w.Optimize_ReduceTravel, &mut o.reduce_travel),
        ("Optimize_OptimalEntryPoint", &w.Optimize_OptimalEntryPoint, &mut o.best_start),
        ("Optimize_ChooseCorners", &w.Optimize_ChooseCorners, &mut o.corners),
        ("Optimize_AllowReverse", &w.Optimize_AllowReverse, &mut o.best_direction),
        ("Optimize_RemoveOverlaps", &w.Optimize_RemoveOverlaps, &mut o.remove_overlaps),
    ] {
        if let Some(b) = r.bool(key, v) {
            *field = b;
            any = true;
        }
    }
    if let Some(d) = r.num("Optimize_OverlapDist", &w.Optimize_OverlapDist, 0.0, 10.0) {
        o.overlap_distance_mm = tidy(d);
        any = true;
    }
    Ok(Mapped {
        autosave_minutes,
        optimization: any.then_some(o),
        browser: Value::Object(browser),
        used: r.used,
        rejected: r.rejected,
        other_keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic LightBurn preferences file: the mapped keys plus stand-ins for the private ones.
    fn sample() -> Value {
        // Private keys (DeviceList, LicenseKey, LastOpenFolder, Email) are stand-ins: never read, never reported.
        serde_json::from_str(
            r#"{
            "units": 0, "GridSpacing": 10, "GridSnap": 1, "GridSnapEnabled": true, "ObjSnap": 10, "ObjSnapEnabled": true,
            "NormalMoveIncrement": 5, "ShiftMoveIncrement": 20, "ControlMoveIncrement": 1, "InvertMouseZoom": true,
            "ShowFullSpanCursor": false, "FilledView": true, "Language": "de", "UseSystemClipboard": true,
            "GroupImportedShapes": false, "SelectImportedShapes": true, "CenterImportedShapes": false, "ImportHiddenAILayers": false,
            "AutoSaveSeconds": 120, "Frame_Mode": 1, "Frame_Individually": false, "Frame_OutsideOnly": true, "Frame_ToolsOnly": false,
            "Optimize_OptimizeCuts": true, "Optimize_ByLayer": 0, "Optimize_ByGroup": -1, "Optimize_ByPriority": 1,
            "Optimize_InnerToOuter": true, "Optimize_ReduceTravel": true, "Optimize_OptimalEntryPoint": false,
            "Optimize_ChooseCorners": false, "Optimize_AllowReverse": true, "Optimize_RemoveOverlaps": false,
            "Optimize_OverlapDist": 0.02500000037252903,
            "GridArray_XSpace": 5, "GridArray_YSpace": 5, "GridArray_BetweenCentersX": false, "GridArray_BetweenCentersY": true,
            "GridArray_XReverse": false, "GridArray_YReverse": true, "GridArray_XShiftByHalf": true, "GridArray_YShiftByHalf": false,
            "GridArray_CreateVirtual": false, "GridArray_GroupResults": true, "GridArray_SelectResults": true,
            "GridArray_RandomOrientation": true, "GridArray_RandomSeed": 7,
            "RadialArray_NumCopies": 8, "RadialArray_RotateCopies": true, "RadialArray_GroupResults": false, "RadialArray_SelectResults": true,
            "NestMultisheet": true, "DockQuickNestLockRotation": true, "OffsetDistance": 0.5, "OffsetDeleteOriginals": false,
            "DeviceList": [{"name": "SECRET-DEVICE", "address": "10.0.0.9"}],
            "LicenseKey": "SECRET-LICENSE-1234",
            "LastOpenFolder": "C:/Users/someone/SECRET",
            "Email": "someone@example.com"
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn maps_the_whitelisted_keys_and_nothing_else() {
        let bytes = serde_json::to_vec(&sample()).unwrap();
        let m = map(&bytes, &Optimization::default()).unwrap();
        assert_eq!(m.other_keys, 4);
        assert!(m.rejected.is_empty(), "{:?}", m.rejected);
        assert_eq!(m.used.len(), KEYS.len());
        assert_eq!(m.autosave_minutes, Some(2));
        let o = m.optimization.clone().unwrap();
        assert_eq!(o.order_by, vec![OrderBy::Layer, OrderBy::Priority]);
        assert!(o.inner_first && o.reduce_travel && !o.best_start && o.best_direction && !o.remove_overlaps);
        assert_eq!(o.overlap_distance_mm, 0.025);
        let b = &m.browser;
        assert_eq!(b["prefs"]["units"], "mm");
        assert_eq!(b["prefs"]["gridMm"], 10.0);
        assert_eq!(b["prefs"]["nudge"], json!({"normal": 5.0, "shift": 20.0, "ctrl": 1.0}));
        assert_eq!(b["prefs"]["language"], "de");
        assert_eq!(b["view"], json!({"snap": {"objects": true, "grid": true, "gridMm": 1.0}, "viewStyle": "filled"}));
        assert_eq!(b["files"]["importPrefs"], json!({"group": false, "select": true, "center": false, "hiddenLayers": false}));
        assert_eq!(b["laser"]["frame"]["style"], "hull");
        let g = &b["dialogs"]["grid_array"];
        assert_eq!((g["spacing_x"].as_str(), g["spacing_y"].as_str(), g["row_shift"].as_str()), (Some("gap"), Some("center"), Some("half")));
        assert!(g.get("col_shift").is_none());
        assert_eq!(g["random_seed"], 7);
        assert_eq!(b["dialogs"]["nest"]["multipage"], true);
        assert_eq!(b["dialogs"]["offset"]["keep_original"], true);
        // No private value reaches the result.
        let text = format!("{m:?}");
        for secret in ["SECRET", "10.0.0.9", "someone", "DeviceList", "LicenseKey", "LastOpenFolder", "Email"] {
            assert!(!text.contains(secret), "{secret} leaked");
        }
    }

    #[test]
    fn bad_values_are_rejected_one_by_one() {
        let bytes = br#"{"units": 7, "GridSpacing": "ten", "Language": "../../etc", "AutoSaveSeconds": 1, "NestMultisheet": 1, "Optimize_ByGroup": -1}"#;
        let m = map(bytes, &Optimization::default()).unwrap();
        assert_eq!(m.rejected, vec!["units", "GridSpacing", "Language"]);
        assert_eq!(m.autosave_minutes, Some(1));
        assert_eq!(m.browser["dialogs"]["nest"]["multipage"], true);
        // Only the group criterion was given (unticked): nothing is ordered.
        assert!(m.optimization.unwrap().order_by.is_empty());
        assert!(map(b"[1, 2]", &Optimization::default()).is_err());
        assert!(map(b"not json", &Optimization::default()).is_err());
        assert!(map(&vec![b' '; MAX_LBPREFS_BYTES + 1], &Optimization::default()).is_err());
        // A UTF-8 byte order mark is fine; an empty object maps nothing.
        let m = map(b"\xEF\xBB\xBF{}", &Optimization::default()).unwrap();
        assert!(m.used.is_empty() && m.optimization.is_none() && m.browser == json!({}));
    }

    /// The owner's real LightBurn preferences: `OPENETCH_LIGHTBURN_PREFS` names a prefs.ini or .lbprefs file, or a
    /// folder of .lbprefs backups (e.g. %LOCALAPPDATA%\LightBurn\backup\prefs). Checks that every file maps and that
    /// no string of the file other than the language code reaches the result.
    #[test]
    fn real_lightburn_prefs() {
        let Some(root) = std::env::var_os("OPENETCH_LIGHTBURN_PREFS") else { return };
        let root = std::path::PathBuf::from(root);
        let files: Vec<std::path::PathBuf> = if root.is_dir() {
            std::fs::read_dir(&root).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect()
        } else {
            vec![root]
        };
        assert!(!files.is_empty());
        for f in files {
            let bytes = std::fs::read(&f).unwrap();
            let m = map(&bytes, &Optimization::default()).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            assert!(m.used.len() >= 20, "{}: only {} keys mapped", f.display(), m.used.len());
            assert!(m.other_keys > 0);
            // Every string in the result is one OpenEtch wrote (an enum word) or the validated language code.
            fn strings(v: &Value, out: &mut Vec<String>) {
                match v {
                    Value::String(s) => out.push(s.clone()),
                    Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
                    Value::Object(o) => o.values().for_each(|x| strings(x, out)),
                    _ => {}
                }
            }
            let mut found = Vec::new();
            strings(&m.browser, &mut found);
            let ours = ["mm", "in", "filled", "wireframe", "bounds", "hull", "outline", "center", "gap", "half"];
            for s in found {
                assert!(ours.contains(&s.as_str()) || (s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase())), "unexpected string in the result");
            }
        }
    }
}
