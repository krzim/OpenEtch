//! OpenEtch's text layout against LightBurn's own: every Text shape in the real LightBurn files (read in place from
//! `OPENETCH_LBRN_SAMPLES`, `;`-separated on Windows; never copied into the repository) is rendered from its
//! attributes (`Str`, `Font`, `H`, `LS`, `LnS`, `Ah`, `Av`) and compared with the `BackupPath` LightBurn saved. Shapes
//! whose font is not installed are skipped. Without the variable the test prints a note and passes.
//!
//! ```text
//! OPENETCH_LBRN_SAMPLES="C:\samples" cargo test -p text --features system --test lightburn -- --nocapture
//! ```

use std::path::{Path, PathBuf};

use doc::{AlignH, AlignV, FontSpec, Text};
use kurbo::{BezPath, PathEl, Point};
use lbrn::ShapeKind;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            if !(name.starts_with('.') || name == "target" || p.join("Cargo.toml").exists()) {
                collect(&p, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lbrn2")) {
            out.push(p);
        }
    }
}

/// The outline flattened to polylines.
fn polylines(p: &BezPath) -> Vec<Vec<Point>> {
    let mut out: Vec<Vec<Point>> = Vec::new();
    let mut start = Point::ORIGIN;
    kurbo::flatten(p.elements().iter().copied(), 0.0005, |el| match el {
        PathEl::MoveTo(q) => {
            start = q;
            out.push(vec![q]);
        }
        PathEl::LineTo(q) => out.last_mut().expect("starts with a move").push(q),
        PathEl::ClosePath => out.last_mut().expect("starts with a move").push(start),
        _ => {}
    });
    out
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let ab = b - a;
    let t = if ab.hypot2() > 0.0 { ((p - a).dot(ab) / ab.hypot2()).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).hypot()
}

/// The largest distance from a point of `a` to the outline `b` (one side of the Hausdorff distance).
fn farthest(a: &[Vec<Point>], b: &[Vec<Point>]) -> f64 {
    a.iter()
        .flatten()
        .map(|p| {
            b.iter()
                .flat_map(|l| l.windows(2))
                .map(|w| segment_distance(*p, w[0], w[1]))
                .fold(f64::MAX, f64::min)
        })
        .fold(0.0, f64::max)
}

fn texts<'a>(shapes: &'a [lbrn::Shape], out: &mut Vec<&'a lbrn::Shape>) {
    for s in shapes {
        match &s.kind {
            ShapeKind::Text { backup: Some(_) } => out.push(s),
            ShapeKind::Group(children) => texts(children, out),
            _ => {}
        }
    }
}

#[test]
fn layout_matches_lightburns_saved_outlines() {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping");
        return;
    };
    let mut files = Vec::new();
    for d in std::env::split_paths(&dirs) {
        collect(&d, &mut files);
    }
    files.sort();
    files.dedup();
    let (mut compared, mut skipped) = (0, 0);
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        let Ok(p) = lbrn::parse(&src) else { continue };
        let mut found = Vec::new();
        texts(&p.shapes, &mut found);
        for s in found {
            let ShapeKind::Text { backup: Some(b) } = &s.kind else { continue };
            let a = |k: &str| s.attribute(k).unwrap_or_default();
            let family = a("Font").split(',').next().unwrap_or_default();
            let num = |k: &str| a(k).trim().parse::<f64>().unwrap_or(0.0);
            let mut t = Text::new(a("Str"), FontSpec::system(family), num("H"));
            t.h_spacing = num("LS");
            t.v_spacing = num("LnS");
            t.align_h = [AlignH::Left, AlignH::Middle, AlignH::Right][(num("Ah") as usize).min(2)];
            t.align_v = [AlignV::Top, AlignV::Middle, AlignV::Bottom][(num("Av") as usize).min(2)];
            t.welded = a("Weld") == "1";
            let r = match text::render_text(&t, &t.text, None) {
                Ok(r) => r,
                Err(e) => {
                    println!("{}: \"{}\" skipped: {e}", f.display(), t.text);
                    skipped += 1;
                    continue;
                }
            };
            let theirs = polylines(&b.path.to_bez());
            let tb = geom::bounds(&b.path.to_bez()).unwrap();
            let mut outline = r.outline.clone();
            // LightBurn 1.x centred lines by their advance, 2.x (and OpenEtch) by their ink: for 1.x files compare
            // the shapes with the ink centres matched, and check the shift is that small difference.
            if p.attribute("AppVersion").is_some_and(|v| v.starts_with('1')) && t.align_h == AlignH::Middle {
                let ob = geom::bounds(&outline).unwrap();
                let dx = (tb.x0 + tb.x1) / 2.0 - (ob.x0 + ob.x1) / 2.0;
                assert!(dx.abs() < 0.1 * t.height_mm, "1.x advance centring moved \"{}\" by {dx} mm", t.text);
                outline = kurbo::Affine::translate((dx, 0.0)) * outline;
            }
            let ours = polylines(&outline);
            let worst = farthest(&theirs, &ours).max(farthest(&ours, &theirs));
            let ob = geom::bounds(&outline).unwrap();
            println!(
                "{}: \"{}\" H {} mm: outlines at most {worst:.4} mm apart; bounds {tb:?} vs {ob:?}",
                f.display(),
                t.text.escape_debug(),
                t.height_mm,
            );
            // LightBurn (Qt) rounds each advance to whole pixels of the font's pixel size (the third field of `Font`:
            // 100 in 1.x files, 4096 in 2.x), which moves glyphs by up to half a pixel per character; OpenEtch keeps
            // exact advances.
            let px = a("Font").split(',').nth(2).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v > 0.0);
            let chars = t.text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
            // Half a pixel per character from the anchor, plus 0.7 % of the height (curve approximations, spacing).
            let allowed = 0.002 + 0.5 * t.height_mm / px.unwrap_or(100.0) * chars + 0.007 * t.height_mm;
            assert!(worst < allowed, "\"{}\" differs from LightBurn's outline by {worst} mm", t.text);
            compared += 1;
        }
    }
    println!("{compared} LightBurn text outlines matched, {skipped} skipped (font not installed)");
}
