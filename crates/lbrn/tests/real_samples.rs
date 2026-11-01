//! Tests over real LightBurn files, which are never copied into the repository. Set `OPENETCH_LBRN_SAMPLES` to one or
//! more directories in the OS path-list form (`;`-separated on Windows, `:` elsewhere); every `*.lbrn2`, `*.lbt` and
//! `*.clb` in them or their subdirectories is checked (directories named `target`, `node_modules` or starting with a
//! dot, and source trees, which hold a `Cargo.toml` or `.git`, are skipped). Without the variable the tests print a note and pass. Run with `--nocapture` to see each file's
//! warnings and differences. With `OPENETCH_LBRN_EXPORT_DIR` set, each export is also written there as
//! `<name>.openetch.lbrn2`, to open in LightBurn by hand. For example
//!
//! ```text
//! OPENETCH_LBRN_SAMPLES="C:\samples;C:\samples\more" cargo test -p lbrn --test real_samples -- --nocapture
//! ```
//!
//! `real_projects_export_as_they_were_imported` checks the round trip LightBurn → OpenEtch document (saved and
//! loaded as `.openetch`) → LightBurn: the export must be structurally identical to the source, element for element
//! and attribute for attribute in order. The only differences allowed are listed in [`ALLOWED`] with their reason;
//! every difference found is printed, and the files that come back byte for byte are counted.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use doc::{Document, Object};
use kurbo::Rect;
use lbrn::{cut_setting_to_layer, from_document, parse, parse_library, to_document, write};

/// Field size the real files are imported with. Round-trip comparisons do not depend on it.
const FIELD_MM: f64 = 150.0;
const TOL_MM: f64 = 0.01;

/// Differences between a source file and its export that are expected, with why. Matched against the start of the
/// difference's description. (Empty: every sample comes back structurally identical.)
const ALLOWED: &[(&str, &str)] = &[];

fn collect(dir: &Path, exts: &[&str], out: &mut BTreeSet<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            let source_tree = p.join("Cargo.toml").exists() || p.join(".git").exists();
            if !(name.starts_with('.') || name == "target" || name == "node_modules" || source_tree) {
                collect(&p, exts, out);
            }
        } else if p.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e))) {
            out.insert(p.canonicalize().unwrap_or(p));
        }
    }
}

fn samples(exts: &[&str]) -> Option<Vec<PathBuf>> {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file tests");
        return None;
    };
    let mut files = BTreeSet::new();
    for dir in std::env::split_paths(&dirs) {
        collect(&dir, exts, &mut files);
    }
    Some(files.into_iter().collect())
}

fn display_name(f: &Path) -> String {
    let parent = f.parent().and_then(Path::file_name).unwrap_or_default().to_string_lossy();
    format!("{parent}/{}", f.file_name().unwrap_or_default().to_string_lossy())
}

fn union(objects: &[Object]) -> Option<Rect> {
    objects.iter().map(Object::bounds).reduce(|a, b| a.union(b))
}

fn near(a: Rect, b: Rect) -> bool {
    [(a.x0, b.x0), (a.y0, b.y0), (a.x1, b.x1), (a.y1, b.y1)].iter().all(|(x, y)| (x - y).abs() <= TOL_MM)
}

fn show(r: Option<Rect>, shift: f64) -> String {
    match r {
        Some(r) => format!("x {:.3}..{:.3} y {:.3}..{:.3}", r.x0 + shift, r.x1 + shift, r.y0 + shift, r.y1 + shift),
        None => "(none)".into(),
    }
}

fn short(s: &str) -> String {
    if s.chars().count() > 60 { format!("{}...", s.chars().take(60).collect::<String>()) } else { s.to_string() }
}

/// Structural differences between two XML documents: element names, attributes (names, order, values), trimmed
/// text and children in order. Each is `<what>: <where>: <detail>`.
fn xml_differences(a: &str, b: &str) -> Vec<String> {
    let opt = || roxmltree::ParsingOptions { nodes_limit: u32::MAX, ..Default::default() };
    let da = roxmltree::Document::parse_with_options(a.trim_start_matches('\u{feff}'), opt()).expect("source is XML");
    let db = match roxmltree::Document::parse_with_options(b, opt()) {
        Ok(d) => d,
        Err(e) => return vec![format!("export is not XML: {e}")],
    };
    let mut out = Vec::new();
    compare(da.root_element(), db.root_element(), "", &mut out);
    out
}

fn compare(a: roxmltree::Node<'_, '_>, b: roxmltree::Node<'_, '_>, at: &str, out: &mut Vec<String>) {
    let here = format!("{at}/{}", a.tag_name().name());
    if a.tag_name().name() != b.tag_name().name() {
        out.push(format!("element name: {here}: export has <{}>", b.tag_name().name()));
        return;
    }
    let attrs = |n: roxmltree::Node<'_, '_>| -> Vec<(String, String)> {
        n.attributes().map(|x| (x.name().to_string(), x.value().to_string())).collect()
    };
    let (aa, ba) = (attrs(a), attrs(b));
    let names = |v: &[(String, String)]| v.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    if names(&aa) != names(&ba) {
        out.push(format!("attribute names: {here}: {:?} vs {:?}", names(&aa), names(&ba)));
    }
    for (k, v) in &aa {
        if let Some((_, w)) = ba.iter().find(|(l, _)| l == k)
            && v != w
        {
            out.push(format!("attribute value: {here}@{k}: {:?} vs {:?}", short(v), short(w)));
        }
    }
    let text = |n: roxmltree::Node<'_, '_>| -> String {
        n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect::<String>().trim().to_string()
    };
    let (ta, tb) = (text(a), text(b));
    if ta != tb {
        out.push(format!("text: {here}: {:?} vs {:?}", short(&ta), short(&tb)));
    }
    let ca: Vec<_> = a.children().filter(|c| c.is_element()).collect();
    let cb: Vec<_> = b.children().filter(|c| c.is_element()).collect();
    if ca.len() != cb.len() {
        let list = |v: &[roxmltree::Node<'_, '_>]| v.iter().map(|n| n.tag_name().name()).collect::<Vec<_>>().join(",");
        out.push(format!("child count: {here}: {} vs {} ([{}] vs [{}])", ca.len(), cb.len(), short(&list(&ca)), short(&list(&cb))));
    }
    for (k, (x, y)) in ca.iter().zip(&cb).enumerate() {
        compare(*x, *y, &format!("{here}[{k}]"), out);
    }
}

/// The document an import makes, as stored in and loaded from a `.openetch` file.
fn import_document(name: &str, text: &str) -> (Document, Vec<String>) {
    let p = parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let imported = to_document(&p, FIELD_MM).unwrap_or_else(|e| panic!("{name}: {e}"));
    let d = Document {
        layers: imported.layers,
        objects: imported.objects,
        groups: imported.groups,
        extras: imported.extras,
        field_mm: Some(FIELD_MM),
        next_id: 1_000_000,
        ..Document::default()
    };
    let saved = doc::file::try_to_openetch_json(&d).unwrap_or_else(|e| panic!("{name}: {e}"));
    let loaded = doc::file::from_openetch_json(&saved).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(loaded, d, "{name}: the .openetch round trip changed the document");
    (loaded, imported.warnings)
}

#[test]
fn real_projects_export_as_they_were_imported() {
    let Some(files) = samples(&["lbrn2", "lbt"]) else { return };
    assert!(!files.is_empty(), "OPENETCH_LBRN_SAMPLES lists no .lbrn2 files");
    let (mut identical, mut unexpected) = (0, Vec::new());
    for f in &files {
        let name = display_name(f);
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let p = parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let rewritten = write(&p);
        assert_eq!(parse(&rewritten).unwrap(), p, "{name}: the model does not survive write + parse");
        let model_same = rewritten == text.trim_start_matches('\u{feff}');

        let (d, warnings) = import_document(&name, &text);
        let (exported, export_warnings) = from_document(&d, FIELD_MM).unwrap_or_else(|e| panic!("{name}: {e}"));
        let out = write(&exported);
        let bytes_same = out == text.trim_start_matches('\u{feff}');
        identical += usize::from(bytes_same);
        println!(
            "== {name}: LightBurn {}, {} bytes; parse+write {}; import+export {}",
            p.attribute("AppVersion").unwrap_or("?"),
            text.len(),
            if model_same { "byte-identical" } else { "differs" },
            if bytes_same { "byte-identical".to_string() } else { format!("{} bytes", out.len()) },
        );
        println!(
            "   {} layers, {} objects, {} groups; bounds (LightBurn mm) {}",
            d.layers.len(),
            d.objects.len(),
            d.groups.len(),
            show(union(&d.objects), FIELD_MM / 2.0)
        );
        for w in &warnings {
            println!("   import warning: {w}");
        }
        for w in &export_warnings {
            println!("   export warning: {w}");
        }
        if let Some(dir) = std::env::var_os("OPENETCH_LBRN_EXPORT_DIR") {
            let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let path = PathBuf::from(dir).join(format!("{stem}.openetch.lbrn2"));
            std::fs::write(&path, &out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
        let diffs = xml_differences(&text, &out);
        for d in &diffs {
            match ALLOWED.iter().find(|(what, _)| d.starts_with(what)) {
                Some((_, why)) => println!("   allowed difference: {d} ({why})"),
                None => {
                    println!("   DIFFERENCE: {d}");
                    unexpected.push(format!("{name}: {d}"));
                }
            }
        }
        if !bytes_same && diffs.is_empty() {
            let at = out.bytes().zip(text.bytes()).position(|(a, b)| a != b).unwrap_or(out.len().min(text.len()));
            let ctx = |s: &str| short(&s[at.saturating_sub(20).min(s.len())..(at + 40).min(s.len())]);
            println!("   formatting only, first at byte {at}: source {:?} export {:?}", ctx(&text), ctx(&out));
        }
    }
    println!("{identical} of {} files came back byte for byte", files.len());
    assert!(unexpected.is_empty(), "{} unexpected difference(s), first: {}", unexpected.len(), unexpected[0]);
}

#[test]
fn real_projects_survive_edits_and_reimport() {
    let Some(files) = samples(&["lbrn2", "lbt"]) else { return };
    for f in files {
        let name = display_name(&f);
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (mut d, _) = import_document(&name, &text);
        // Move every object 1 mm right: every transform is recomputed, every list stays.
        for o in &mut d.objects {
            o.transform = kurbo::Affine::translate((1.0, 0.0)) * o.transform;
        }
        let (exported, _) = from_document(&d, FIELD_MM).unwrap();
        let out = write(&exported);
        let back = to_document(&parse(&out).unwrap_or_else(|e| panic!("{name}: {e}")), FIELD_MM).unwrap();
        assert_eq!(back.objects.len(), d.objects.len(), "{name}: object count");
        assert_eq!(back.groups.len(), d.groups.len(), "{name}: group count");
        assert_eq!(back.layers, d.layers, "{name}: layers");
        for (a, b) in d.objects.iter().zip(&back.objects) {
            assert!(near(a.bounds(), b.bounds()), "{name}: {} moved: {:?} vs {:?}", a.name, a.bounds(), b.bounds());
            assert_eq!(a.extras.get("lbrn2._type"), b.extras.get("lbrn2._type"), "{name}: {}", a.name);
        }
        println!("== {name}: moved, exported and re-imported: {} objects in place", back.objects.len());
    }
}

#[test]
fn real_libraries_parse() {
    let Some(files) = samples(&["clb"]) else { return };
    for f in files {
        let name = display_name(&f);
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let entries = parse_library(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut subs = 0;
        let mut warnings = Vec::new();
        for e in &entries {
            let (layer, w) = cut_setting_to_layer(&e.setting, "layer-1");
            assert_eq!(layer.sub_layers.len(), e.setting.sub_layers.len(), "{name}: {}", e.description);
            subs += layer.sub_layers.len();
            // Exported as a project layer, the entry keeps its subnames and every other value.
            let d = Document { layers: vec![layer.clone()], ..Document::default() };
            let (p, _) = from_document(&d, FIELD_MM).unwrap();
            let back = &parse(&write(&p)).unwrap().cut_settings[0];
            assert_eq!(back.params.subname, e.setting.params.subname, "{name}: {}", e.description);
            for (a, b) in back.sub_layers.iter().zip(&e.setting.sub_layers) {
                assert_eq!(a.params.subname, b.params.subname, "{name}: {}", e.description);
            }
            let (again, _) = cut_setting_to_layer(back, "layer-1");
            assert_eq!(again.extras, layer.extras, "{name}: {}", e.description);
            assert_eq!((&again.settings, &again.sub_layers), (&layer.settings, &layer.sub_layers), "{name}");
            warnings.extend(w);
        }
        warnings.sort();
        warnings.dedup();
        println!("== {name}: {} entries, {subs} sub-layers", entries.len());
        for w in warnings {
            println!("   warning: {w}");
        }
    }
}
