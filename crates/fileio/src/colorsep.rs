//! Colour separation (xTool's Colour Separation, Silhouette's trace by colour): a bitmap reduced to a few solid
//! colours, small specks merged into their surroundings, and each colour's area traced into closed outlines, so every
//! colour can become its own fill layer (on a MOPA, its own colour preset).
//!
//! 1. **Colours**: the opaque pixels (alpha at least 128) are clustered in CIELAB (D65), where distances follow what
//!    the eye sees. An image with no more distinct colours than asked (a logo) keeps its own colours exactly;
//!    otherwise k-means++ (a fixed seed, so the same image always separates the same way) on at most 65,536 evenly
//!    spaced sample pixels, then every pixel takes its nearest centre. Each colour is the mean of its pixels, and the
//!    colours are listed by area, largest first.
//! 2. **Specks**: a connected area (4-neighbours) of one colour smaller than `min_speck_px` takes the colour that
//!    borders it most, so dust and anti-aliasing fringes do not become tiny shapes.
//! 3. **Background**: `Auto` marks the colour covering most of the image's border (at least 40 % of the opaque border
//!    pixels) as background, `Color` the colour nearest the one given; the background is listed but not traced.
//! 4. **Trace**: each colour's pixels traced as Trace Image traces a mask (pixel-edge boundaries, then
//!    [`trace::simplify_closed`] and [`trace::smooth_into`]). Neighbouring colours share their pixel edges, so their
//!    outlines meet within the simplification tolerance (0.5 + `optimize` px).

use std::collections::HashMap;

use image::RgbaImage;
use kurbo::BezPath;

use crate::trace::{self, Traced};
use crate::{Result, invalid};

/// Most colours a separation makes.
pub const MAX_COLORS: usize = 16;
/// Label of a transparent pixel.
pub const NONE: u16 = u16::MAX;
/// Sample size of the clustering.
const SAMPLE: usize = 65_536;
/// Lloyd iterations at most.
const ITERATIONS: usize = 30;

/// What counts as background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Background {
    /// Every colour is traced.
    None,
    /// The colour covering most of the border (at least 40 % of its opaque pixels).
    Auto,
    /// The colour nearest this sRGB colour.
    Color([u8; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// 1..=[`MAX_COLORS`].
    pub colors: usize,
    /// Areas of one colour smaller than this many pixels merge into their neighbours (0 or 1: none).
    pub min_speck_px: u32,
    pub background: Background,
}

/// One colour of a separation.
#[derive(Debug, Clone, PartialEq)]
pub struct Swatch {
    /// The mean sRGB colour of its pixels.
    pub rgb: [u8; 3],
    pub pixels: usize,
    pub background: bool,
}

/// A separated image: a colour index per pixel ([`NONE`] for transparent pixels) and the colours.
#[derive(Debug, Clone)]
pub struct Separation {
    pub width: usize,
    pub height: usize,
    pub labels: Vec<u16>,
    pub palette: Vec<Swatch>,
}

/// sRGB (0..=255) to CIELAB, D65.
pub fn lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 colour difference (Euclidean in CIELAB; about 2.3 is just noticeable).
pub fn delta_e(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (p, q) = (lab(a), lab(b));
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

/// `#rrggbb`.
pub fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// `#rgb` or `#rrggbb` (case ignored) as sRGB.
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        _ => return None,
    };
    if !full.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = |i: usize| u8::from_str_radix(&full[i..i + 2], 16).ok();
    Some([v(0)?, v(2)?, v(4)?])
}

fn dist2(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn nearest(p: &[f64; 3], centres: &[[f64; 3]]) -> usize {
    let mut best = (0, f64::INFINITY);
    for (i, c) in centres.iter().enumerate() {
        let d = dist2(p, c);
        if d < best.1 {
            best = (i, d);
        }
    }
    best.0
}

/// A small deterministic generator (the same image always separates the same way).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// k-means++ centres of `pts` (k at most the number of points).
fn kmeans(pts: &[[f64; 3]], k: usize) -> Vec<[f64; 3]> {
    let mut rng = Lcg(0x5eed_c010_75ab_1e5e);
    let mut centres = vec![pts[(rng.next() * pts.len() as f64) as usize % pts.len()]];
    let mut d: Vec<f64> = pts.iter().map(|p| dist2(p, &centres[0])).collect();
    while centres.len() < k {
        let total: f64 = d.iter().sum();
        if total <= 0.0 {
            break;
        }
        let mut target = rng.next() * total;
        let mut pick = pts.len() - 1;
        for (i, v) in d.iter().enumerate() {
            target -= v;
            if target <= 0.0 {
                pick = i;
                break;
            }
        }
        centres.push(pts[pick]);
        for (i, p) in pts.iter().enumerate() {
            d[i] = d[i].min(dist2(p, &centres[centres.len() - 1]));
        }
    }
    let mut assign = vec![usize::MAX; pts.len()];
    for _ in 0..ITERATIONS {
        let mut changed = false;
        for (i, p) in pts.iter().enumerate() {
            let n = nearest(p, &centres);
            if assign[i] != n {
                assign[i] = n;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        let mut sum = vec![[0.0f64; 4]; centres.len()];
        for (p, &a) in pts.iter().zip(&assign) {
            let s = &mut sum[a];
            s[0] += p[0];
            s[1] += p[1];
            s[2] += p[2];
            s[3] += 1.0;
        }
        for (c, s) in centres.iter_mut().zip(&sum) {
            if s[3] > 0.0 {
                *c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            }
        }
    }
    centres
}

fn key(p: &image::Rgba<u8>) -> u32 {
    u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2])
}

fn rgb_of(k: u32) -> [u8; 3] {
    [(k >> 16) as u8, (k >> 8) as u8, k as u8]
}

/// Separate `img` into colours. See the module docs.
pub fn separate(img: &RgbaImage, o: &Options) -> Result<Separation> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return invalid("the image is empty");
    }
    if w * h > trace::MAX_PIXELS {
        return invalid(format!("the image has more than {} pixels; scale it down to separate it", trace::MAX_PIXELS));
    }
    if !(1..=MAX_COLORS).contains(&o.colors) {
        return invalid(format!("colors must be within 1..={MAX_COLORS} (got {})", o.colors));
    }
    let opaque: Vec<usize> = img.pixels().enumerate().filter(|(_, p)| p[3] >= 128).map(|(i, _)| i).collect();
    if opaque.is_empty() {
        return invalid("the image has no opaque pixels");
    }
    let px = |i: usize| img.get_pixel((i % w) as u32, (i / w) as u32);

    // Centres: the image's own colours when there are few enough, else k-means on a sample.
    let mut distinct: HashMap<u32, usize> = HashMap::new();
    for &i in &opaque {
        *distinct.entry(key(px(i))).or_default() += 1;
        if distinct.len() > o.colors {
            break;
        }
    }
    let centres: Vec<[f64; 3]> = if distinct.len() <= o.colors {
        let mut d: Vec<(u32, usize)> = distinct.into_iter().collect();
        d.sort();
        d.into_iter().map(|(k, _)| lab(rgb_of(k))).collect()
    } else {
        let step = (opaque.len() / SAMPLE).max(1);
        let sample: Vec<[f64; 3]> = opaque.iter().step_by(step).map(|&i| lab(rgb_of(key(px(i))))).collect();
        kmeans(&sample, o.colors.min(sample.len()))
    };

    // Every pixel to its nearest centre (cached per colour: logos and posterised art repeat colours).
    let mut labels = vec![NONE; w * h];
    let mut cache: HashMap<u32, u16> = HashMap::new();
    for &i in &opaque {
        let k = key(px(i));
        let l = *cache.entry(k).or_insert_with(|| nearest(&lab(rgb_of(k)), &centres) as u16);
        labels[i] = l;
    }
    merge_specks(&mut labels, w, h, o.min_speck_px);

    // Colours by area, largest first; each the mean of its pixels.
    let n = centres.len();
    let mut sums = vec![[0u64; 4]; n];
    for (i, &l) in labels.iter().enumerate() {
        if l != NONE {
            let p = px(i);
            let s = &mut sums[l as usize];
            s[0] += u64::from(p[0]);
            s[1] += u64::from(p[1]);
            s[2] += u64::from(p[2]);
            s[3] += 1;
        }
    }
    let mut order: Vec<usize> = (0..n).filter(|&c| sums[c][3] > 0).collect();
    order.sort_by(|&a, &b| sums[b][3].cmp(&sums[a][3]).then(a.cmp(&b)));
    let mut remap = vec![NONE; n];
    for (new, &old) in order.iter().enumerate() {
        remap[old] = new as u16;
    }
    for l in &mut labels {
        if *l != NONE {
            *l = remap[*l as usize];
        }
    }
    let mut palette: Vec<Swatch> = order
        .iter()
        .map(|&c| {
            let s = sums[c];
            let m = |v: u64| ((v as f64 / s[3] as f64).round()) as u8;
            Swatch { rgb: [m(s[0]), m(s[1]), m(s[2])], pixels: s[3] as usize, background: false }
        })
        .collect();

    match o.background {
        Background::None => {}
        Background::Color(c) => {
            if let Some((i, _)) = palette.iter().enumerate().min_by(|a, b| delta_e(a.1.rgb, c).total_cmp(&delta_e(b.1.rgb, c))) {
                palette[i].background = true;
            }
        }
        Background::Auto => {
            let mut counts = vec![0usize; palette.len()];
            let mut border = 0usize;
            let mut count = |i: usize| {
                if labels[i] != NONE {
                    counts[labels[i] as usize] += 1;
                    border += 1;
                }
            };
            for x in 0..w {
                count(x);
                if h > 1 {
                    count((h - 1) * w + x);
                }
            }
            for y in 1..h.saturating_sub(1) {
                count(y * w);
                if w > 1 {
                    count(y * w + w - 1);
                }
            }
            if let Some((i, &c)) = counts.iter().enumerate().max_by_key(|(_, c)| **c)
                && border > 0
                && c * 10 >= border * 4
                && palette.len() > 1
            {
                palette[i].background = true;
            }
        }
    }
    Ok(Separation { width: w, height: h, labels, palette })
}

/// Merge connected areas (4-neighbours) of one colour smaller than `min` pixels into the colour bordering them most.
pub fn merge_specks(labels: &mut [u16], w: usize, h: usize, min: u32) {
    if min <= 1 {
        return;
    }
    let mut comp_of = vec![u32::MAX; labels.len()];
    let mut stack = Vec::new();
    let mut comp = Vec::new();
    let mut next = 0u32;
    for start in 0..labels.len() {
        if comp_of[start] != u32::MAX || labels[start] == NONE {
            continue;
        }
        let l = labels[start];
        comp.clear();
        stack.push(start);
        comp_of[start] = next;
        while let Some(i) = stack.pop() {
            comp.push(i);
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if comp_of[j] == u32::MAX && labels[j] == l {
                    comp_of[j] = next;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        next += 1;
        if comp.len() >= min as usize {
            continue;
        }
        // The colour along its border (transparent neighbours do not count).
        let mut votes: HashMap<u16, usize> = HashMap::new();
        for &i in &comp {
            let (x, y) = (i % w, i / w);
            let mut vote = |j: usize| {
                let n = labels[j];
                if n != l && n != NONE {
                    *votes.entry(n).or_default() += 1;
                }
            };
            if x > 0 {
                vote(i - 1);
            }
            if x + 1 < w {
                vote(i + 1);
            }
            if y > 0 {
                vote(i - w);
            }
            if y + 1 < h {
                vote(i + w);
            }
        }
        if let Some((&n, _)) = votes.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) {
            for &i in &comp {
                labels[i] = n;
            }
        }
    }
}

/// The outlines of colour `index` (pixel-edge coordinates, y down), as Trace Image would trace its mask.
pub fn trace_color(sep: &Separation, index: usize, smoothness: f64, optimize: f64) -> Result<Traced> {
    if !(0.0..=1.334).contains(&smoothness) || !(optimize.is_finite() && optimize >= 0.0) {
        return invalid("smoothness must be 0..1.33 and optimize 0 or more");
    }
    let m: Vec<bool> = sep.labels.iter().map(|&l| l as usize == index).collect();
    let tol = 0.5 + optimize;
    let mut path = BezPath::new();
    let (mut nodes, mut contours) = (0, 0);
    for c in trace::contours(&m, sep.width, sep.height) {
        let simple = trace::simplify_closed(&c, tol);
        nodes += trace::smooth_into(&mut path, &simple, true, smoothness);
        contours += 1;
    }
    Ok(Traced { path, contours, nodes })
}

/// The separation drawn in its colours (background and transparent pixels transparent).
pub fn preview(sep: &Separation) -> RgbaImage {
    let mut img = RgbaImage::new(sep.width as u32, sep.height as u32);
    for (i, &l) in sep.labels.iter().enumerate() {
        if l == NONE {
            continue;
        }
        let s = &sep.palette[l as usize];
        if s.background {
            continue;
        }
        img.put_pixel((i % sep.width) as u32, (i / sep.width) as u32, image::Rgba([s.rgb[0], s.rgb[1], s.rgb[2], 255]));
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Shape;

    /// White background, a red square and a blue disc, with a few stray pixels.
    fn logo() -> RgbaImage {
        RgbaImage::from_fn(60, 40, |x, y| {
            let (fx, fy) = (f64::from(x) - 42.0, f64::from(y) - 20.0);
            if (5..25).contains(&x) && (10..30).contains(&y) {
                image::Rgba([220, 20, 30, 255])
            } else if fx * fx + fy * fy < 100.0 {
                image::Rgba([20, 40, 200, 255])
            } else if (x, y) == (2, 2) || (x, y) == (55, 35) {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        })
    }

    #[test]
    fn a_logo_keeps_its_colours_and_drops_specks_and_background() {
        let sep = separate(&logo(), &Options { colors: 4, min_speck_px: 4, background: Background::Auto }).unwrap();
        // The two black specks merged into white, so three colours remain, largest first.
        assert_eq!(sep.palette.len(), 3, "{:?}", sep.palette);
        assert_eq!(sep.palette[0].rgb, [255, 255, 255]);
        assert!(sep.palette[0].background);
        let red = sep.palette.iter().position(|s| s.rgb == [220, 20, 30]).unwrap();
        let blue = sep.palette.iter().position(|s| s.rgb == [20, 40, 200]).unwrap();
        assert_eq!(sep.palette[red].pixels, 400);
        let t = trace_color(&sep, red, 0.0, 0.2).unwrap();
        assert_eq!(t.contours, 1);
        let b = t.path.bounding_box();
        assert_eq!((b.x0, b.y0, b.x1, b.y1), (5.0, 10.0, 25.0, 30.0));
        let disc = trace_color(&sep, blue, 1.0, 0.2).unwrap();
        assert_eq!(disc.contours, 1);
        assert!((disc.path.area().abs() - std::f64::consts::PI * 100.0).abs() < 25.0, "{}", disc.path.area());
        // Without speck removal the black specks are their own colour.
        let raw = separate(&logo(), &Options { colors: 4, min_speck_px: 0, background: Background::None }).unwrap();
        assert_eq!(raw.palette.len(), 4);
        assert!(raw.palette.iter().all(|s| !s.background));
    }

    #[test]
    fn a_gradient_is_clustered_deterministically() {
        let img = RgbaImage::from_fn(64, 16, |x, _| {
            let v = (x * 4) as u8;
            image::Rgba([v, 255 - v, 128, 255])
        });
        let o = Options { colors: 3, min_speck_px: 0, background: Background::None };
        let a = separate(&img, &o).unwrap();
        let b = separate(&img, &o).unwrap();
        assert_eq!(a.palette, b.palette);
        assert_eq!(a.labels, b.labels);
        assert_eq!(a.palette.len(), 3);
        assert_eq!(a.palette.iter().map(|s| s.pixels).sum::<usize>(), 64 * 16);
        // Each cluster is a run of columns (a band of the gradient).
        for y in 0..16 {
            let row = &a.labels[y * 64..(y + 1) * 64];
            let changes = row.windows(2).filter(|p| p[0] != p[1]).count();
            assert_eq!(changes, 2);
        }
    }

    #[test]
    fn transparent_pixels_are_left_out_and_background_by_colour() {
        let img = RgbaImage::from_fn(10, 10, |x, _| if x < 5 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([10, 200, 10, 255]) });
        let sep = separate(&img, &Options { colors: 2, min_speck_px: 3, background: Background::Auto }).unwrap();
        assert_eq!(sep.palette.len(), 1);
        assert!(!sep.palette[0].background, "one colour is never background");
        assert_eq!(sep.labels.iter().filter(|&&l| l == NONE).count(), 50);
        let p = preview(&sep);
        assert_eq!(p.get_pixel(0, 0)[3], 0);
        assert_eq!(p.get_pixel(9, 9).0, [10, 200, 10, 255]);
        let two = RgbaImage::from_fn(4, 1, |x, _| if x < 2 { image::Rgba([250, 250, 250, 255]) } else { image::Rgba([5, 5, 5, 255]) });
        let s = separate(&two, &Options { colors: 2, min_speck_px: 0, background: Background::Color([255, 255, 255]) }).unwrap();
        assert!(s.palette.iter().find(|q| q.rgb == [250, 250, 250]).unwrap().background);
        assert!(separate(&RgbaImage::new(3, 3), &Options { colors: 2, min_speck_px: 0, background: Background::None }).is_err());
        assert!(separate(&two, &Options { colors: 0, min_speck_px: 0, background: Background::None }).is_err());
    }

    #[test]
    fn colour_helpers() {
        assert_eq!(parse_hex("#FfA"), Some([255, 255, 170]));
        assert_eq!(parse_hex("#102030"), Some([16, 32, 48]));
        assert_eq!(parse_hex("102030"), None);
        assert_eq!(parse_hex("#12345g"), None);
        assert_eq!(hex([16, 32, 48]), "#102030");
        assert!(delta_e([255, 255, 255], [255, 255, 255]) < 1e-9);
        let l = lab([255, 255, 255]);
        assert!((l[0] - 100.0).abs() < 0.01 && l[1].abs() < 0.01 && l[2].abs() < 0.01, "{l:?}");
        assert!(delta_e([255, 0, 0], [0, 0, 255]) > 100.0);
    }
}
