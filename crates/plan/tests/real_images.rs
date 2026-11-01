//! Image engraving over real LightBurn projects and images, which are never copied into the repository. Set
//! `OPENETCH_LBRN_SAMPLES` to one or more directories (the OS path-list form, `;`-separated on Windows); every
//! `*.lbrn2` with a bitmap in them or their subdirectories is imported and planned, and every PNG/JPEG is planned
//! as an image at 0.1 mm in each dither mode (the two smallest). Without the variable the tests print a note and pass. Run with
//! `--nocapture` to see each file's numbers. `photo on namecard.lbrn2`, when present, must plan.

use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine;
use doc::{CutSettings, DitherMode, Document, GalvoTiming, Geometry, Object};
use kurbo::Affine;
use plan::{Target, encode, plan_job};

fn collect(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            let source = p.join("Cargo.toml").exists() || p.join(".git").exists();
            if !(name.starts_with('.') || name == "target" || name == "node_modules" || source) {
                collect(&p, exts, out);
            }
        } else if p.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e))) && !out.contains(&p) {
            out.push(p);
        }
    }
}

fn samples(exts: &[&str]) -> Option<Vec<PathBuf>> {
    let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else {
        println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file tests");
        return None;
    };
    let mut files = Vec::new();
    for d in std::env::split_paths(&dirs) {
        collect(&d, exts, &mut files);
    }
    files.sort();
    files.dedup();
    Some(files)
}

/// The B6's 150 mm-class lens with the profile's limits (a stand-in; the engine fills the real one).
fn target(field_mm: f64) -> Target {
    Target {
        field_mm,
        field_height_mm: None,
        curve_tolerance_mm: None,
        swap_xy: false,
        negate_x: false,
        negate_y: false,
        max_power_pct: 100.0,
        pulse_widths_ns: lmc::JPT_M7_PULSE_WIDTHS_NS.to_vec(),
        timing: GalvoTiming {
            laser_on_delay_us: 100.0,
            laser_off_delay_us: 100.0,
            polygon_delay_us: 10.0,
            end_delay_us: 0.0,
            jump_speed_mm_s: 4000.0,
            min_jump_delay_us: 100.0,
            max_jump_delay_us: 400.0,
            jump_distance_limit_mm: 10.0,
        },
        open_mo_delay_us: 8000.0,
        calibration: lmc::Calibration::default(),
        pulse_width_enabled: true,
        z_axis: false,
        rotary_axis: false,
        port_outputs: 0,
        gantry: false,
    }
}

/// With `OPENETCH_PLAN_PNG_DIR` set, draw the plan's marks (black on white, 0.05 mm per pixel, darker for more
/// power) into `<dir>/<name>.png` to look at.
fn dump(plan: &plan::JobPlan, name: &str) {
    let Some(dir) = std::env::var_os("OPENETCH_PLAN_PNG_DIR") else { return };
    let pts = plan.passes.iter().flat_map(|p| p.polylines.iter().flatten());
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
    }
    let px = 0.05;
    let (w, h) = (((x1 - x0) / px) as u32 + 2, ((y1 - y0) / px) as u32 + 2);
    let mut img = image::GrayImage::from_pixel(w, h, image::Luma([255]));
    for pass in &plan.passes {
        for (i, poly) in pass.polylines.iter().enumerate() {
            let shade = (255.0 * (1.0 - pass.power_of(i) / 100.0)) as u8;
            for s in poly.windows(2) {
                let n = ((s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1]) / px * 2.0).ceil().max(1.0) as usize;
                for k in 0..=n {
                    let t = k as f64 / n as f64;
                    let x = ((s[0][0] + (s[1][0] - s[0][0]) * t - x0) / px) as u32;
                    let y = h - 1 - ((s[0][1] + (s[1][1] - s[0][1]) * t - y0) / px) as u32;
                    img.put_pixel(x.min(w - 1), y.min(h - 1), image::Luma([shade]));
                }
            }
        }
    }
    let out = Path::new(&dir).join(format!("{name}.png"));
    img.save(&out).unwrap_or_else(|e| panic!("{}: {e}", out.display()));
}

#[test]
fn lightburn_projects_with_bitmaps_plan() {
    let Some(files) = samples(&["lbrn2"]) else { return };
    let mut namecard = false;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        if !text.contains("Type=\"Bitmap\"") {
            continue;
        }
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        let project = lbrn::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        // LightBurn laid these out for a 110 mm galvo field (the bitmap XForms are centred on 55 mm).
        let field = 110.0;
        let i = lbrn::to_document(&project, field).unwrap_or_else(|e| panic!("{name}: {e}"));
        let d = Document { layers: i.layers, objects: i.objects, groups: i.groups, field_mm: Some(field), ..Document::default() };
        let t0 = Instant::now();
        let result = plan_job(&d, &target(field));
        let took = t0.elapsed();
        match &result {
            Ok(plan) => println!(
                "== {name}: {} passes, {} segments, {:.0} mm marked, about {:.0} s, planned in {:.2?}",
                plan.passes.len(),
                plan.stats.segment_count,
                plan.stats.mark_length_mm,
                plan.stats.estimated_time_s,
                took
            ),
            Err(e) => println!("== {name}: refused in {took:.2?}: {e}"),
        }
        if name == "photo on namecard.lbrn2" {
            namecard = true;
            let plan = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(plan.stats.segment_count > 1000, "the photo is engraved");
            let s = &d.layers[0].settings.image;
            assert_eq!((s.dither, s.negative, s.bidirectional, s.angle_deg), (DitherMode::FloydSteinberg, true, false, 180.0));
            dump(&plan, &name);
            let packets = encode(&plan, &target(field)).unwrap_or_else(|e| panic!("{name}: {e}"));
            println!("   {} packets", packets.len());
        }
    }
    if !namecard {
        println!("photo on namecard.lbrn2 was not among the samples");
    }
}

#[test]
fn real_images_plan_in_every_dither_mode() {
    let Some(files) = samples(&["png", "jpg", "jpeg"]) else { return };
    // The two smallest files keep a debug test run short.
    let mut files = files;
    files.sort_by_key(|f| std::fs::metadata(f).map_or(u64::MAX, |m| m.len()));
    for f in files.iter().take(2) {
        let bytes = std::fs::read(f).unwrap_or_default();
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let g = Geometry::Image { width_mm: 40.0, height_mm: 30.0, mime: "image/png".into(), data_base64: data.into(), adjust: Default::default() };
        let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
        for m in DitherMode::ALL {
            let mut d = Document::default();
            d.objects.push(Object::new("obj-1".into(), "img", "layer-1".into(), g.clone(), Affine::IDENTITY));
            d.layers[0].settings = CutSettings::default();
            d.layers[0].settings.image.dither = m;
            d.layers[0].settings.image.interval_mm = 0.1;
            let t0 = Instant::now();
            match plan_job(&d, &target(110.0)) {
                Ok(p) => println!("== {name} {m:?}: {} segments in {:.2?}", p.stats.segment_count, t0.elapsed()),
                Err(e) => panic!("{name} {m:?}: {e}"),
            }
        }
    }
}
