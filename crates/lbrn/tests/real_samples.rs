//! Tests over real LightBurn files, which are never copied into the repository. Set `OPENETCH_LBRN_SAMPLES` to one or
//! more directories in the OS path-list form (`;`-separated on Windows, `:` elsewhere); every `*.lbrn2` and `*.clb`
//! directly inside them is checked. Without the variable the tests print a note and pass. Run with `--nocapture` to
//! see each file's warnings. With `OPENETCH_LBRN_EXPORT_DIR` set, each export is also written there as
//! `<name>.openetch.lbrn2`, to open in LightBurn by hand. For example
//!
//! ```text
//! OPENETCH_LBRN_SAMPLES="C:\samples;C:\samples\more" cargo test -p lbrn --test real_samples -- --nocapture
//! ```

use std::path::PathBuf;

use doc::{Document, Object};
use kurbo::Rect;
use lbrn::{cut_setting_to_layer, from_document, parse, parse_library, to_document, write};

/// Field size the real files are imported with. Round-trip comparisons do not depend on it.
const FIELD_MM: f64 = 150.0;
const TOL_MM: f64 = 0.01;

fn samples(ext: &str) -> Option<Vec<PathBuf>> {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file tests");
        return None;
    };
    let mut files = Vec::new();
    for dir in std::env::split_paths(&dirs) {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
                files.push(p);
            }
        }
    }
    files.sort();
    Some(files)
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

#[test]
fn real_projects_import_export_reimport() {
    let Some(files) = samples("lbrn2") else { return };
    assert!(!files.is_empty(), "OPENETCH_LBRN_SAMPLES lists no .lbrn2 files");
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let p = parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(parse(&write(&p)).unwrap(), p, "{name}: the model does not survive write + parse");

        let (layers, objects, warnings) = to_document(&p, FIELD_MM).unwrap();
        println!("== {name}: LightBurn {}", p.attribute("AppVersion").unwrap_or("?"));
        println!(
            "   {} cut settings, {} top-level shapes -> {} layers, {} objects",
            p.cut_settings.len(),
            p.shapes.len(),
            layers.len(),
            objects.len()
        );
        println!("   bounds (LightBurn mm): {}", show(union(&objects), FIELD_MM / 2.0));
        println!("   bounds (field frame):  {}", show(union(&objects), 0.0));
        for w in &warnings {
            println!("   import warning: {w}");
        }

        let d =
            Document { layers: layers.clone(), objects: objects.clone(), field_mm: Some(FIELD_MM), ..Document::default() };
        let (exported, export_warnings) = from_document(&d, FIELD_MM).unwrap();
        for w in &export_warnings {
            println!("   export warning: {w}");
        }
        let out = write(&exported);
        println!("   exported {} bytes (source {} bytes)", out.len(), text.len());
        if let Some(dir) = std::env::var_os("OPENETCH_LBRN_EXPORT_DIR") {
            let stem = f.file_stem().unwrap_or_default().to_string_lossy().to_string();
            let path = PathBuf::from(dir).join(format!("{stem}.openetch.lbrn2"));
            std::fs::write(&path, &out).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            println!("   wrote {}", path.display());
        }
        let (layers2, objects2, _) = to_document(&parse(&out).unwrap(), FIELD_MM).unwrap();

        assert_eq!(objects2.len(), objects.len(), "{name}: object count");
        assert_eq!(layers2, layers, "{name}: layer settings");
        for (a, b) in objects.iter().zip(&objects2) {
            assert!(near(a.bounds(), b.bounds()), "{name}: {} moved: {:?} vs {:?}", a.name, a.bounds(), b.bounds());
        }
        match (union(&objects), union(&objects2)) {
            (Some(a), Some(b)) => assert!(near(a, b), "{name}: bounds"),
            (a, b) => assert_eq!(a.is_none(), b.is_none(), "{name}: bounds"),
        }
        println!("   re-import: same layers, {} objects, bounds within {TOL_MM} mm", objects2.len());
    }
}

#[test]
fn real_libraries_parse() {
    let Some(files) = samples("clb") else { return };
    for f in files {
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        let entries = parse_library(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut subs = 0;
        let mut warnings = Vec::new();
        for e in &entries {
            let (layer, w) = cut_setting_to_layer(&e.setting, "layer-1");
            assert_eq!(layer.sub_layers.len(), e.setting.sub_layers.len(), "{name}: {}", e.description);
            subs += layer.sub_layers.len();
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
