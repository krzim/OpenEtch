//! MCP tools for MOPA materials (listed after `crate::mcp`'s own): the material library with its finishes and the
//! hue step model, the palette, finish notes, hue steps, the bracket test and colour separation. Design and library
//! edits only, through the same functions as the HTTP routes (`crate::api_materials`). None of them can frame, run,
//! approve or press anything; library edits stay unsaved until a person saves the library, and the hue step model
//! (a device setting) is changed only in the UI.

use std::sync::Arc;

use commands::Origin;
use commands::finish::SurfaceFinish;
use rmcp::model::{CallToolResult, JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::api_materials::{self as am, BracketArgs, NudgeArgs, SeparateArgs};
use crate::finish::FinishPatch;
use crate::mcp::{args, ok_json, tool};
use crate::{AppState, EngineError};

/// The tools, in listing order.
pub const NAMES: [&str; 7] = [
    "material_library",
    "material_palette",
    "material_finish_update",
    "material_hue_nudge",
    "material_bracket_test",
    "material_bracket_pick",
    "material_color_separate",
];

/// What the server instructions add.
pub const INSTRUCTIONS: &str = "\n\nMOPA materials: material_library shows the loaded material libraries (LightBurn \
.clb) with each preset's finish (expected result colour, surface, notes, lens, focus, supplier lot and date, whether \
it has a result photo) and the device's hue step model; material_palette lists the presets that have a result colour \
(the colours you can make). material_finish_update edits a preset's finish notes, colour and lot (no photos; those \
come from the camera in the UI). material_hue_nudge computes (apply: false, the default) or applies the settings one \
or more hues warmer (steps > 0) or cooler, or adapted to another surface (mill, brushed, polished, mirror), for a \
preset or a layer. material_bracket_test makes a small test grid around a preset in a new document; after the person \
burns it and picks the best box, material_bracket_pick writes that box's settings back to the preset (only when the \
person tells you which box). material_color_separate reduces an image object to a few colours, traces each into a \
fill layer and links it to the nearest preset by colour (preview: true first to show the colours). Library edits stay \
unsaved until the person saves the library in the UI; burning a test is a normal run that a person must approve.";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LibraryArgs {
    /// Library name (default the selected one).
    library: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PaletteArgs {
    /// Library name (default the selected one).
    library: Option<String>,
    /// Only presets made for this surface.
    surface: Option<SurfaceFinish>,
    /// Only this material (as the library names it).
    material: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishArgs {
    /// Library name (default the selected one).
    library: Option<String>,
    /// The preset: id (m<i>.e<j>) or LinkPath (material/title/description).
    entry: String,
    /// The changes (fields left out stay).
    finish: FinishPatch,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PickArgs {
    /// The layer of the bracket test box the person chose (each box has its own layer).
    layer: String,
}

/// The tool definitions.
pub fn tools() -> Vec<Tool> {
    vec![
        tool::<LibraryArgs>(
            "material_library",
            "Material library",
            "The loaded material libraries and one library's tree (default the selected one): materials > thickness or \
             title > presets {id, path, summary (mode, power_pct, speed_mm_s, frequency_khz, pulse_width_ns, passes, \
             interval_mm), finish {color, surface, notes, lens, focus_mm, supplier, lot, date, photo}}, plus hue_model \
             (the active device's hue step model). Read-only.",
            true,
        ),
        tool::<PaletteArgs>(
            "material_palette",
            "Material palette",
            "The presets of a library that have an expected result colour (the colours the machine can make on that \
             material), optionally only one surface or material: {library, swatches: [{id, path, material, \
             description, color, surface, lot, date, summary, photo}]}. Read-only.",
            true,
        ),
        tool::<FinishArgs>(
            "material_finish_update",
            "Edit a preset's finish",
            "Change a library preset's finish data: color (#rrggbb, or null for the photo's average), surface (mill, \
             brushed, polished, mirror: what the setting was made for; recording it changes no setting), notes, lens, \
             focus_mm, supplier, lot, date (YYYY-MM-DD). Unsaved until the person saves the library. Returns the \
             finish.",
            false,
        ),
        tool::<NudgeArgs>(
            "material_hue_nudge",
            "One hue warmer / cooler",
            "MOPA colour on steel: the settings of a library preset (library?, entry) or a document layer (layer) \
             `steps` hues warmer (> 0, less energy) or cooler (< 0), via auto|power|speed|frequency, and/or adapted to \
             another surface (surface: mill|brushed|polished|mirror), by the active device's hue step model (xTool's \
             published one until bracket tests refine it: 0.5 % power, 100 mm/s or about 150 kHz above 500 kHz per \
             hue; polished +1 %, mirror +2 % power). apply: false (default) only computes; true writes the preset \
             (unsaved; linked layers follow) or the layer (one undo step). Returns {settings, param, delta, warnings, \
             applied}.",
            false,
        ),
        tool::<BracketArgs>(
            "material_bracket_test",
            "Bracket test around a preset",
            "Make a small material test grid around a library preset (library?, entry): columns and rows each bracket \
             one setting by spread_pct each way (default power x speed, ±10 %, 5 x 5 boxes of cell_mm 5), every box \
             the whole preset with those two values changed, labelled, in LightBurn's safe order. By default it goes \
             into a new document that becomes the active one (the person sees it); new_document: false adds it to the \
             active document. Burning it is a normal run a person approves. Returns {document, result}.",
            false,
        ),
        tool::<PickArgs>(
            "material_bracket_pick",
            "Write a bracket box back",
            "Write the settings of the bracket test box on `layer` (the one the person chose as best) back to the \
             library preset the test was made from (unsaved until the person saves the library; linked layers \
             follow). Returns {library, id, path, cell (row %, column % from the preset)}.",
            false,
        ),
        tool::<SeparateArgs>(
            "material_color_separate",
            "Colour separation",
            "Reduce an image object (image) to `colors` solid colours (default 4; background auto|none|#rrggbb is left \
             out; specks under min_speck_px merge), trace each colour into a path on a new fill layer, and give each \
             layer the settings of the nearest preset with a result colour in the library (default the selected one, \
             \"\" for none; within max_distance ΔE, default 30; link: true makes linked layers). preview: true only \
             lists the colours and their matches (and a PNG) without changing anything. skip lists colour indices \
             not to trace. One undo step. Returns the layers made and their matches.",
            false,
        ),
    ]
}

/// Run material tool `name`, or `None` when it is not one of these.
pub async fn dispatch(st: &Arc<AppState>, name: &str, a: JsonObject, origin: Origin) -> Option<Result<CallToolResult, EngineError>> {
    let r = match name {
        "material_library" => (|| {
            let x: LibraryArgs = args(name, a)?;
            Ok(ok_json(am::library_json(st, x.library.as_deref())?))
        })(),
        "material_palette" => (|| {
            let x: PaletteArgs = args(name, a)?;
            Ok(ok_json(am::palette_json(st, x.library.as_deref(), x.surface, x.material.as_deref())?))
        })(),
        "material_finish_update" => (|| {
            let x: FinishArgs = args(name, a)?;
            if x.finish.photo_base64.is_some() || x.finish.remove_photo {
                return Err(EngineError::Forbidden("result photos are taken or removed by a person in the OpenEtch UI".into()));
            }
            let lib = am::library_name(st, x.library.as_deref())?;
            Ok(ok_json(am::finish_update(st, &lib, &x.entry, x.finish)?))
        })(),
        "material_hue_nudge" => match args::<NudgeArgs>(name, a) {
            Ok(x) => am::hue_nudge(st, x, origin).await.map(ok_json),
            Err(e) => Err(e),
        },
        "material_bracket_test" => match args::<BracketArgs>(name, a) {
            Ok(x) => am::bracket_test(st, x, origin).await.map(ok_json),
            Err(e) => Err(e),
        },
        "material_bracket_pick" => match args::<PickArgs>(name, a) {
            Ok(x) => am::bracket_pick(st, &x.layer).await.map(ok_json),
            Err(e) => Err(e),
        },
        "material_color_separate" => match args::<SeparateArgs>(name, a) {
            Ok(x) => am::color_separate(st, x, origin).await.map(ok_json),
            Err(e) => Err(e),
        },
        _ => return None,
    };
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::GalvoMcp;
    use rmcp::model::ContentBlock;
    use serde_json::{Value, json};

    fn obj(v: Value) -> JsonObject {
        v.as_object().cloned().unwrap_or_default()
    }

    fn text_of(r: &CallToolResult) -> String {
        r.content
            .iter()
            .find_map(|c| match c {
                ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    async fn call(mcp: &GalvoMcp, name: &str, v: Value) -> Value {
        let r = mcp.call(name, obj(v)).await;
        assert_eq!(r.is_error, Some(false), "{name}: {}", text_of(&r));
        serde_json::from_str(&text_of(&r)).unwrap()
    }

    const CLB: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<LightBurnLibrary>\r\n  <Material name=\"Steel\">\r\n    <Entry Thickness=\"-1.0000\" Desc=\"Gold\" NoThickTitle=\"Colours\">\r\n      <CutSetting type=\"Scan\">\r\n        <index Value=\"0\" />\r\n        <name Value=\"\" />\r\n        <LinkPath Value=\"Steel/Colours/Gold\" />\r\n        <maxPower Value=\"40\" />\r\n        <speed Value=\"1000\" />\r\n        <frequency Value=\"600000\" />\r\n        <QPulseWidth Value=\"30\" />\r\n        <interval Value=\"0.01\" />\r\n        <priority Value=\"0\" />\r\n      </CutSetting>\r\n    </Entry>\r\n  </Material>\r\n</LightBurnLibrary>\r\n";

    #[test]
    fn names_match_and_none_can_approve() {
        let names: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(names, NAMES.map(String::from).to_vec());
        for n in NAMES {
            for word in ["approve", "pin", "arm", "input", "fire", "frame", "run", "start"] {
                assert!(!n.contains(word), "MCP tool {n}");
            }
        }
        let src = include_str!("mcp_materials.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or_default();
        for forbidden in ["Armed", "approved_in_ui", "approve::", ".run(", "Controller", "pin::", "PinStore", "request_run", "frame_start"] {
            assert!(!body.contains(forbidden), "mcp_materials.rs must not reference {forbidden}");
        }
        let am = include_str!("api_materials.rs");
        for forbidden in ["approve::", "request_run", "frame_start", "Armed"] {
            assert!(!am.contains(forbidden), "api_materials.rs must not reference {forbidden}");
        }
    }

    #[tokio::test]
    async fn agents_read_nudge_bracket_and_pick() {
        let st = crate::tests::test_state().await;
        lock_lib(&st).load("steel", CLB.as_bytes()).unwrap();
        let mcp = GalvoMcp::new(st.clone());
        let v = call(&mcp, "material_library", json!({})).await;
        assert_eq!(v["library"], "steel");
        assert_eq!(v["hue_model"]["model"]["power_pct"], 0.5);
        // No colour yet: an empty palette; a colour makes it a swatch.
        assert_eq!(call(&mcp, "material_palette", json!({})).await["swatches"], json!([]));
        let f = call(&mcp, "material_finish_update", json!({ "entry": "Steel/Colours/Gold", "finish": { "color": "#C8A040", "surface": "brushed", "lot": "A7" } })).await;
        assert_eq!(f["finish"]["color"], "#c8a040");
        let p = call(&mcp, "material_palette", json!({ "surface": "brushed" })).await;
        assert_eq!(p["swatches"][0]["path"], "Steel/Colours/Gold");
        assert_eq!(call(&mcp, "material_palette", json!({ "surface": "mirror" })).await["swatches"], json!([]));
        let r = mcp.call("material_finish_update", obj(json!({ "entry": "m0.e0", "finish": { "remove_photo": true } }))).await;
        assert_eq!(r.is_error, Some(true), "photos are UI only");
        // Compute, then apply, one hue warmer: 600 kHz is above 500, so a frequency step.
        let n = call(&mcp, "material_hue_nudge", json!({ "entry": "m0.e0", "steps": 1 })).await;
        assert_eq!((n["param"].as_str(), n["settings"]["frequency_khz"].as_f64(), n["applied"].as_bool()), (Some("frequency"), Some(750.0), Some(false)));
        let n = call(&mcp, "material_hue_nudge", json!({ "entry": "Steel/Colours/Gold", "surface": "mirror", "apply": true })).await;
        assert_eq!(n["settings"]["power_pct"], 42.0, "brushed to mirror is +2 %");
        {
            let s = lock_lib(&st);
            let e = s.entry("steel", (0, 0)).unwrap().1;
            assert_eq!(crate::library::preset_of(e).unwrap().settings.power_pct, 42.0);
            assert_eq!(s.finish_of("steel", "Steel/Colours/Gold").unwrap().surface, Some(SurfaceFinish::Mirror));
            assert!(s.get("steel").unwrap().dirty);
        }
        // A bracket test in a new document; picking a box writes it back.
        let b = call(&mcp, "material_bracket_test", json!({ "entry": "m0.e0", "rows": { "param": "frequency", "count": 3 }, "columns": { "param": "power", "count": 3 } })).await;
        let doc_id = b["document"].as_str().unwrap().to_string();
        assert_eq!(st.documents_view()["active"], json!(doc_id));
        assert_eq!(b["result"]["boxes"], 9);
        let doc = st.document();
        let hot = doc.layers.iter().find(|l| l.extras.get(commands::bracket::CELL_KEY).map(String::as_str) == Some("-10,10")).unwrap();
        let pick = call(&mcp, "material_bracket_pick", json!({ "layer": hot.id })).await;
        assert_eq!((pick["path"].as_str(), pick["cell"].as_str()), (Some("Steel/Colours/Gold"), Some("-10,10")));
        let s = lock_lib(&st);
        let p = crate::library::preset_of(s.entry("steel", (0, 0)).unwrap().1).unwrap();
        assert!((p.settings.power_pct - 46.2).abs() < 1e-6 && (p.settings.frequency_khz - 540.0).abs() < 1e-6, "{:?}", p.settings);
        assert_eq!(p.settings.pulse_width_ns, Some(30.0));
    }

    fn lock_lib(st: &AppState) -> std::sync::MutexGuard<'_, crate::library::LibraryStore> {
        crate::libs::lock(&st.libs.library)
    }
}
