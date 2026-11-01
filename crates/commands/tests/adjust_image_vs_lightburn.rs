//! OpenEtch's Adjust Image and dithering against LightBurn's own processed bitmap (OTH-022). Uses the owner's
//! LightBurn 2.2 sample (`OPENETCH_LBRN_SAMPLES`, see the samples' README): `adjust-image.lbrn2` (Stucki at 254 DPI,
//! Contrast 25, Brightness 10, Gamma 1.2, Enhance Radius 3, Enhance Amount 50) and `adjust-image-processed.png`
//! (File > Save Processed Bitmap). Without the variable the test prints a note and passes. Run with `--nocapture` to
//! see the agreement and the tone curves.
//!
//! Two dithers of the same grey image agree dot for dot only where the grey is near black or white (error diffusion
//! is chaotic), so the main metric is the agreement of local density: the mean absolute difference of the fired
//! fraction over 9 x 9 dot windows, and the tone curve (fired fraction per source grey level).

use std::path::PathBuf;

use doc::{DitherMode, Geometry, ImageAdjust};

fn sample(name: &str) -> Option<PathBuf> {
    let dirs = std::env::var_os("OPENETCH_LBRN_SAMPLES")?;
    std::env::split_paths(&dirs).map(|d| d.join(name)).find(|p| p.is_file())
}

/// Fired fraction over the `(2r+1)²` window around each dot (clamped at the edges).
fn density(fired: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut n) = (0.0, 0.0);
            for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                    s += fired[yy * w + xx];
                    n += 1.0;
                }
            }
            out[y * w + x] = s / n;
        }
    }
    out
}

struct Compared {
    /// Dots that agree (fired or not), 0..=1.
    exact: f64,
    /// Mean absolute difference of the 9 x 9 fired fraction, 0..=1.
    density_mae: f64,
    /// Overall fired fraction: OpenEtch, LightBurn.
    fired: (f64, f64),
}

fn compare(oe: &[f32], lb: &[f32], w: usize, h: usize) -> Compared {
    let exact = oe.iter().zip(lb).filter(|(a, b)| a == b).count() as f64 / oe.len() as f64;
    let (da, db) = (density(oe, w, h, 4), density(lb, w, h, 4));
    let density_mae = da.iter().zip(&db).map(|(a, b)| f64::from((a - b).abs())).sum::<f64>() / da.len() as f64;
    let mean = |v: &[f32]| v.iter().map(|x| f64::from(*x)).sum::<f64>() / v.len() as f64;
    Compared { exact, density_mae, fired: (mean(oe), mean(lb)) }
}

#[test]
fn adjust_image_matches_lightburns_processed_bitmap() {
    let (Some(project), Some(processed)) = (sample("adjust-image.lbrn2"), sample("adjust-image-processed.png")) else {
        println!("OPENETCH_LBRN_SAMPLES is not set or lacks adjust-image.lbrn2; skipping the real-file test");
        return;
    };
    let text = std::fs::read_to_string(project).unwrap();
    let imported = lbrn::to_document(&lbrn::parse(&text).unwrap(), 110.0).unwrap();
    let o = imported.objects.iter().find(|o| matches!(o.geometry, Geometry::Image { .. })).unwrap().clone();
    let Geometry::Image { adjust, .. } = &o.geometry else { unreachable!() };
    // LightBurn's Adjust Image values as OpenEtch reads them (UI numbers: 25, 10, 1.2, 3, 50; Denoise 0).
    assert_eq!(
        (adjust.contrast, adjust.brightness, adjust.gamma, adjust.enhance_radius, adjust.enhance_amount, adjust.denoise),
        (25.0, 10.0, 1.2, 3.0, 50.0, 0.0)
    );
    let layer = imported.layers.iter().find(|l| l.id == o.layer).unwrap();
    let s = layer.settings.image.clone();
    assert_eq!((s.dither, s.interval_mm), (DitherMode::Stucki, 0.1));

    let lb = fileio::image::load_from_memory(&std::fs::read(processed).unwrap()).unwrap().to_luma8();
    let (w, h) = (lb.width() as usize, lb.height() as usize);
    let ours = plan::processed_bitmap(&o, &s, 1).unwrap();
    println!("LightBurn {w} x {h}, OpenEtch {} x {} at {:.2} dpi", ours.width, ours.height, ours.dpi);
    assert_eq!((ours.width, ours.height), (1058, 928), "LightBurn's processed size at 254 DPI");
    let lb_fired: Vec<f32> = lb.pixels().map(|p| if p.0[0] < 128 { 1.0 } else { 0.0 }).collect();
    let oe_fired: Vec<f32> = ours.levels.iter().map(|&l| if l >= 128 { 1.0 } else { 0.0 }).collect();
    let c = compare(&oe_fired, &lb_fired, w, h);
    println!(
        "Stucki, adjusted: dots agreeing {:.1} %, 9x9 density difference {:.2} %, fired OpenEtch {:.1} % / LightBurn {:.1} %",
        c.exact * 100.0,
        c.density_mae * 100.0,
        c.fired.0 * 100.0,
        c.fired.1 * 100.0
    );

    // The tone curve: LightBurn's fired fraction per source darkness (OpenEtch's grey sampling of the unadjusted
    // image), next to OpenEtch's adjusted darkness for the same dots.
    let mut plain = o.clone();
    if let Geometry::Image { adjust, .. } = &mut plain.geometry {
        *adjust = ImageAdjust::default();
    }
    let grey = doc::ImageSettings { dither: DitherMode::Grayscale, ..s.clone() };
    let src = plan::processed_bitmap(&plain, &grey, 1).unwrap();
    let adj = plan::processed_bitmap(&o, &grey, 1).unwrap();
    let lb_density = density(&lb_fired, w, h, 4);
    let oe_density = density(&oe_fired, w, h, 4);
    let mut bins = [(0.0f64, 0.0f64, 0.0f64, 0.0f64); 16];
    for i in 0..w * h {
        let b = &mut bins[usize::from(src.levels[i]) * 16 / 256];
        b.0 += f64::from(lb_density[i]);
        b.1 += f64::from(adj.levels[i]) / 255.0;
        b.2 += f64::from(oe_density[i]);
        b.3 += 1.0;
    }
    println!("source darkness -> fired: LightBurn | OpenEtch adjusted grey | OpenEtch Stucki  (dots)");
    let mut curve_err = 0.0;
    let mut weight = 0.0;
    for (k, b) in bins.iter().enumerate() {
        if b.3 > 0.0 {
            println!("  {:3}..{:3}: {:.3} | {:.3} | {:.3}  ({})", k * 16, k * 16 + 15, b.0 / b.3, b.1 / b.3, b.2 / b.3, b.3);
            curve_err += (b.0 - b.2).abs();
            weight += b.3;
        }
    }
    println!("tone curve difference (dot-weighted): {:.2} %", curve_err / weight * 100.0);
    // Measured with the fitted tone scales (plan::tone_factors) and the enhance as implemented: 78 % of the dots
    // agree, the 9 x 9 density differs by 4.8 %, the tone curves by 1.7 %. (With OpenEtch's earlier scales: 70 %,
    // 18 %, 18 %.)
    assert!(c.exact > 0.75, "only {:.1} % of the dots agree with LightBurn's", c.exact * 100.0);
    assert!(c.density_mae < 0.06, "the local density differs by {:.1} % from LightBurn's", c.density_mae * 100.0);
    assert!(curve_err / weight < 0.03, "the tone curve differs by {:.1} % from LightBurn's", curve_err / weight * 100.0);
}
