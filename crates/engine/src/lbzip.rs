//! LightBurn user bundles (`.lbzip`, File > Bundles > Export / Import Bundle): read through `POST /api/bundle/import`
//! and written by `GET /api/bundle/export?format=lbzip` (see `userfiles`).
//!
//! # The format (LightBurn 2.2.00's own export, all items ticked)
//!
//! A plain ZIP (deflated entries with data descriptors, UTF-8 names, no folders) holding the parts as their usual
//! files plus a manifest, `LightBurnSoftware.json`, written last: Qt's indented JSON (4 spaces, keys sorted, a final
//! line break):
//!
//! ```text
//! {"LightBurnSoftware": {"guid": "<10 characters>", "manifest": [
//!     {"action": "import-settings", "filename": "settings.lbprefs", "group": "Application",
//!      "icon": "filetype:preferences", "name": "Settings", "size": <bytes>, "source": "settings.lbprefs"},
//!     {"action": "import-materialtestpresets", "filename": "material_test_presets.lbmt", "group": "Application",
//!      "icon": "filetype:materialtest-presets", "name": "Material Test Presets", ...},
//!     {"action": "import-device", "filename": "<random>.lbdev", "group": "Devices", "icon": "protocol:JCZFiber",
//!      "name": "<device display name>", ...},
//!     {"action": "import-artlib", "filename": "<library>.lbart", "group": "Art Libraries",
//!      "icon": "filetype:art-library", "name": "<library>", ...}],
//!   "version": {"Bundle": "1.0", "LightBurn": "2.2.00"}}}
//! ```
//!
//! (The sample's device `size` was smaller than its file, so `size` is informational; OpenEtch reads the file.)
//!
//! # Import
//!
//! Each manifest item whose `filename` is a plain file name present in the archive maps onto OpenEtch's own
//! importers, for the parts asked for: `import-settings` → `prefs` (the whitelist importer, `crate::lbprefs`: only
//! known keys are read, never logged; the browser-side values are returned as `browser` for the UI to apply),
//! `import-device` → `devices` (as `POST /api/devices/import`: a device matching an imported one is updated),
//! `import-materialtestpresets` → `presets` (renamed "<name> (2)" when taken), `import-artlib` → `art` (a new Art
//! Library; "<name> (bundle)" when the name is taken). Other actions (hotkeys, image presets, material libraries:
//! their action names are not known yet) are listed as skipped. The archive is read by `fileio::zip` (sizes capped,
//! overlapping entries refused); entries are matched by exact name, never written to disk by their names.
//!
//! # Export
//!
//! The same layout with the parts OpenEtch can give LightBurn: `devices` (every device as `.lbdev`,
//! `machine::to_lbdev`), `presets` (`material_test_presets.lbmt`), `art` (each Art Library as `.lbart`). OpenEtch
//! writes no `settings.lbprefs` (its preferences are a small subset of LightBurn's, and importing a partial file
//! could reset the rest of LightBurn's settings) and no material libraries (their action name is not known).

use serde_json::{Value, json};

use crate::{AppState, EngineError};

/// The manifest's file name.
pub const MANIFEST: &str = "LightBurnSoftware.json";
/// The LightBurn version written into exported manifests (the version the format was read from).
const LIGHTBURN_VERSION: &str = "2.2.00";

/// Whether a zip's entries are a LightBurn bundle.
pub fn is_lbzip(entries: &[fileio::zip::Entry]) -> bool {
    entries.iter().any(|e| e.name == MANIFEST)
}

/// One manifest item, as read (every field optional; only `action` and `filename` are needed).
#[derive(Debug, Clone, Default)]
struct Item {
    action: String,
    filename: String,
    name: String,
}

fn manifest(entries: &[fileio::zip::Entry]) -> Result<Vec<Item>, EngineError> {
    let bad = |why: &str| EngineError::BadRequest(format!("not a LightBurn bundle: {why}"));
    let e = entries.iter().find(|e| e.name == MANIFEST).ok_or_else(|| bad("no LightBurnSoftware.json"))?;
    let v: Value = serde_json::from_slice(e.data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&e.data))
        .map_err(|_| bad("LightBurnSoftware.json is not JSON"))?;
    let root = &v["LightBurnSoftware"];
    let list = root["manifest"].as_array().ok_or_else(|| bad("LightBurnSoftware.json has no manifest"))?;
    if let Some(b) = root["version"]["Bundle"].as_str()
        && !b.starts_with("1.")
    {
        return Err(bad(&format!("bundle version {} is newer than 1.x", b.chars().take(10).collect::<String>())));
    }
    let text = |v: &Value| v.as_str().unwrap_or_default().chars().take(200).collect::<String>();
    Ok(list
        .iter()
        .take(fileio::zip::MAX_ENTRIES)
        .map(|i| Item { action: text(&i["action"]), filename: text(&i["filename"]), name: text(&i["name"]) })
        .collect())
}

/// What a manifest action imports into, and the part that selects it.
fn part_of(action: &str) -> Option<&'static str> {
    match action {
        "import-settings" => Some("prefs"),
        "import-device" => Some("devices"),
        "import-materialtestpresets" => Some("presets"),
        "import-artlib" => Some("art"),
        _ => None,
    }
}

/// Import the `parts` of a LightBurn bundle. Returns `{ok, lightburn: true, imported: {prefs, devices, libraries,
/// presets, art}, browser?, skipped: [text]}`.
pub fn import(st: &AppState, entries: &[fileio::zip::Entry], parts: &[String]) -> Result<Value, EngineError> {
    let items = manifest(entries)?;
    let wanted = |p: &str| parts.iter().any(|x| x == p);
    let mut skipped: Vec<String> = Vec::new();
    let (mut prefs, mut browser) = (false, Value::Null);
    let (mut devices, mut presets, mut art) = (Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new());
    // A file listed again (LightBurn lists each once) is imported once: a hostile manifest repeating one entry
    // thousands of times must not import it thousands of times.
    let mut seen = std::collections::HashSet::new();
    for it in items {
        let label = if it.name.trim().is_empty() { it.filename.clone() } else { it.name.clone() };
        let Some(part) = part_of(&it.action) else {
            skipped.push(format!("{label}: LightBurn's \"{}\" is not imported by OpenEtch", it.action));
            continue;
        };
        if !wanted(part) {
            continue;
        }
        if !seen.insert((part, it.filename.clone())) {
            skipped.push(format!("{label}: {} is listed more than once", it.filename.chars().take(80).collect::<String>()));
            continue;
        }
        if !crate::userfiles::bundle_file_name(&it.filename) {
            skipped.push(format!("{label}: not a plain file name"));
            continue;
        }
        let Some(e) = entries.iter().find(|e| e.name == it.filename) else {
            skipped.push(format!("{label}: {} is missing from the bundle", it.filename));
            continue;
        };
        let text = || String::from_utf8(e.data.clone()).map_err(|_| format!("{} is not UTF-8 text", it.filename));
        let result: Result<(), String> = (|| match part {
            "prefs" => {
                let v = crate::userfiles::import_lightburn_prefs(st, &e.data, false).map_err(|e| e.to_string())?;
                browser = v["browser"].clone();
                prefs = true;
                Ok(())
            }
            "devices" => {
                let r = st.import_devices(&text()?, &crate::devices::ImportFilter::All).map_err(|e| e.to_string())?;
                devices.extend(r.added.iter().chain(&r.updated).cloned());
                skipped.extend(r.skipped.iter().map(|s| format!("device \"{}\": {}", s.name, s.reason)));
                Ok(())
            }
            "presets" => {
                let v = crate::test_presets::import_lbmt(st, &text()?, false).map_err(|e| e.to_string())?;
                presets.extend(v["imported"].as_array().into_iter().flatten().filter_map(|n| n.as_str().map(str::to_owned)));
                skipped.extend(v["warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str().map(|w| format!("{label}: {w}"))));
                Ok(())
            }
            "art" => {
                let (file, warnings) = crate::artlib::library_from_lbart(&e.data).map_err(|e| e.to_string())?;
                skipped.extend(warnings.into_iter().map(|w| format!("{label}: {w}")));
                let stem = it.filename.strip_suffix(".lbart").unwrap_or(&it.filename);
                let base = if crate::projects::validate_name(it.name.trim()).is_ok() { it.name.trim() } else { stem };
                let made = crate::artlib::ArtStore::create(st, base, Some(file.clone()), false).or_else(|err| match err {
                    EngineError::Conflict(_) => crate::artlib::ArtStore::create(st, &format!("{base} (bundle)"), Some(file), false),
                    other => Err(other),
                });
                let v = made.map_err(|e| e.to_string())?;
                art.extend(v["name"].as_str().map(str::to_owned));
                Ok(())
            }
            _ => Ok(()),
        })();
        if let Err(why) = result {
            skipped.push(format!("{label}: {why}"));
        }
    }
    Ok(json!({
        "ok": true,
        "lightburn": true,
        "imported": { "prefs": prefs, "devices": devices, "libraries": [], "presets": presets, "art": art },
        "browser": browser,
        "skipped": skipped,
    }))
}

/// A short random-looking id for the manifest's `guid` (LightBurn writes 10 letters and digits).
fn guid() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let mut x = (nanos as u64) ^ ((nanos >> 64) as u64) ^ 0x9E37_79B9_7F4A_7C15 ^ u64::from(std::process::id());
    (0..10)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ALPHABET[(x % ALPHABET.len() as u64) as usize] as char
        })
        .collect()
}

/// A file name LightBurn and OpenEtch both accept: letters, digits, `-`, `_`, `.`, space and parentheses.
fn file_stem(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || " -_()".contains(c) { c } else { '_' }).take(80).collect();
    let t = t.trim().trim_matches('.').to_owned();
    if t.is_empty() { "item".into() } else { t }
}

/// The manifest text (Qt's indented JSON: 4 spaces, sorted keys, a final line break).
pub fn manifest_text(items: &[Value]) -> String {
    let v = json!({ "LightBurnSoftware": {
        "guid": guid(),
        "manifest": items,
        "version": { "Bundle": "1.0", "LightBurn": LIGHTBURN_VERSION },
    }});
    let mut out = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::with_indent(b"    "));
    serde::Serialize::serialize(&v, &mut ser).unwrap_or(());
    let mut s = String::from_utf8(out).unwrap_or_default();
    s.push('\n');
    s
}

/// A LightBurn bundle of `parts` (`devices`, `presets`, `art`; `prefs` and `libraries` are not written, see the
/// module docs). Returns the zip and how many parts were left out.
pub fn export(st: &AppState, parts: &[String]) -> Result<(Vec<u8>, usize), EngineError> {
    let wanted = |p: &str| parts.iter().any(|x| x == p);
    let mut entries: Vec<fileio::zip::Entry> = Vec::new();
    let mut items: Vec<Value> = Vec::new();
    let mut left_out = 0;
    let mut add = |entries: &mut Vec<fileio::zip::Entry>, file: String, data: Vec<u8>, action: &str, group: &str, icon: String, name: String| {
        // Two libraries or devices whose names clean up the same get "(2)", "(3)", ...
        let (stem, ext) = file.rsplit_once('.').map_or((file.clone(), String::new()), |(s, e)| (s.to_owned(), e.to_owned()));
        let file = (1..)
            .map(|n| if n == 1 { file.clone() } else { format!("{stem} ({n}).{ext}") })
            .find(|f| !entries.iter().any(|e| e.name.eq_ignore_ascii_case(f)))
            .unwrap_or(file);
        items.push(json!({
            "action": action, "filename": file, "group": group, "icon": icon, "name": name, "size": data.len(), "source": file,
        }));
        entries.push(fileio::zip::Entry { name: file, data });
    };
    if wanted("prefs") || wanted("libraries") {
        left_out += 1;
    }
    if wanted("presets")
        && let Some((text, _)) = crate::test_presets::export_lbmt(st)?
    {
        let name = "material_test_presets.lbmt".to_owned();
        add(&mut entries, name, text.into_bytes(), "import-materialtestpresets", "Application", "filetype:materialtest-presets".into(), "Material Test Presets".into());
    }
    if wanted("devices") {
        let list: Vec<(String, String, String, String)> = {
            let reg = st.devices();
            reg.list()
                .iter()
                .map(|d| {
                    let text = machine::to_lbdev(&d.display_name, d.lightburn.as_ref(), d.profile.as_ref());
                    (d.id.clone(), d.name.clone(), d.display_name.clone(), text)
                })
                .collect()
        };
        for (id, controller, display, text) in list {
            let controller = if controller.trim().is_empty() { "JCZFiber".to_owned() } else { controller };
            add(&mut entries, format!("{}.lbdev", file_stem(&id)), text.into_bytes(), "import-device", "Devices", format!("protocol:{controller}"), display);
        }
    }
    if wanted("art") {
        let names: Vec<String> = crate::artlib::ArtStore::list(st)?["libraries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["name"].as_str().map(str::to_owned))
            .collect();
        for n in names {
            let file = crate::artlib::ArtStore::file(st, &n)?;
            if file.items.is_empty() {
                continue;
            }
            let (bytes, _) = crate::artlib::library_to_lbart(&file)?;
            add(&mut entries, format!("{}.lbart", file_stem(&n)), bytes, "import-artlib", "Art Libraries", "filetype:art-library".into(), n);
        }
    }
    entries.push(fileio::zip::Entry { name: MANIFEST.into(), data: manifest_text(&items).into_bytes() });
    let zip = fileio::zip::write(&entries).map_err(|e| EngineError::Internal(e.to_string()))?;
    Ok((zip, left_out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_written_as_lightburn_writes_it() {
        let item = json!({"action": "import-artlib", "filename": "a.lbart", "group": "Art Libraries", "icon": "filetype:art-library", "name": "a", "size": 3, "source": "a.lbart"});
        let text = manifest_text(&[item]);
        assert!(text.starts_with("{\n    \"LightBurnSoftware\": {\n        \"guid\": \""), "{text}");
        assert!(text.ends_with("            \"LightBurn\": \"2.2.00\"\n        }\n    }\n}\n"), "{text}");
        assert!(text.contains("\"action\": \"import-artlib\",\n                \"filename\": \"a.lbart\","), "{text}");
        let g = guid();
        assert!(g.len() == 10 && g.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(file_stem("a/b:c"), "a_b_c");
        assert_eq!(file_stem("   "), "item");
    }
}
