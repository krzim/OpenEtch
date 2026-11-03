//! Example projects shipped with the app (File > New from example, the empty-canvas card, `doc.new {example}`,
//! `example.list`). Each one is built from commands at the moment it is opened, so it always matches the current
//! document format, lays itself out in the field centre of whatever device is active, and draws its text with the
//! bundled Hershey fonts (no font needs to be installed). Everything in them is made here: no third-party art.
//!
//! Each example sets the project notes (shown on open) to the walkthrough the user guide follows, and most are
//! templates with fill-in fields. Cut settings are illustrative starting points for a MOPA fiber laser on stainless
//! steel, not tested values: the notes say to run a material test and save your own presets.

use doc::{CutSettings, Document, FillMode};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::util::invalid;
use super::{
    array, fill_region, generate, image, layer, pages, place, shape, stock, template, text,
};
use crate::{Command, CommandError, Result, Risk};

/// One example project.
pub struct Example {
    pub id: &'static str,
    pub title: &'static str,
    /// One sentence for the picker.
    pub summary: &'static str,
    /// What it shows, as tags for the picker ("template", "fill region", ...).
    pub tags: &'static [&'static str],
    /// The walkthrough (the project notes and the user guide), one step per entry.
    pub steps: &'static [&'static str],
    build: fn(&mut Document) -> Result<()>,
}

/// The examples, in the order the picker shows them.
pub const EXAMPLES: &[Example] = &[
    Example {
        id: "two-tone-coin",
        title: "Two-tone MOPA coin",
        summary: "A 40 mm coin: a coloured ring made with Fill Region and fitted to the stock, a name that fits itself \
            into the centre, and a date. A template: fill in the name, date, font and ring colour.",
        tags: &["template", "fill region", "fit to stock", "auto-fit text", "MOPA colour"],
        steps: &[
            "The stock is a 40 mm disc (Laser Tools > Stock). Everything sits on it, centred in the field.",
            "The ring: two circles on the Rim layer, then Fill Region (Tools) clicked between them made the \"Colour \
             ring\" shape on its own layer. Place on Stock centred the ring on the coin and sized it to 39.5 mm, a 0.25 mm margin.",
            "The name is live text with Auto-fit: Box (Text properties > Fit), so any name grows or shrinks to fill \
             the 24 x 9 mm box. Try a long name and a short one.",
            "File > Fill in template (or the Fill in button on the canvas) asks for the name, the date, the font and \
             the ring colour. Everything else is locked, so a tap on the iPad cannot move the rim.",
            "The ring colours are starting points, not tested values: run Laser Tools > Material Test on your own \
             stock, then save the finishes you like as presets.",
            "Frame, then Start (or the foot switch). The ring burns first, then the name, then the rim.",
        ],
        build: two_tone_coin,
    },
    Example {
        id: "serial-tags",
        title: "Serial-numbered tags for a jig",
        summary: "Six tags in a 2 x 3 jig: each pocket's outline on the T1 frame layer, a serial number and a QR code \
            that count up per tag and per run.",
        tags: &["template", "serial numbers", "array", "barcode", "jig", "contact sheet"],
        steps: &[
            "Each pocket of the jig is a rounded rectangle on tool layer T1: it frames with the job but never burns.",
            "The tag text is Variable Text in serial mode (format SN 0dddd) and the QR code carries the same number. \
             The grid array (Edit > Grid Array) made five more tags with Auto-increment 1, so the six tags show six \
             numbers.",
            "Variable Text: start 1, end 60, advance by 6 after every finished run, so the next jig gets the next six \
             numbers.",
            "Check every row (Variable Text window, or command search) draws the ten jig loads (sixty tags) in a \
             grid and flags any number that runs off its pocket.",
            "Fill in template sets the first number; the range keeps its length.",
        ],
        build: serial_tags,
    },
    Example {
        id: "text-medallion",
        title: "Text-on-a-circle medallion",
        summary: "A 50 mm medallion: a line of text along the top of a circle, another along the bottom, both reading \
            upright, and a star in the middle.",
        tags: &["template", "text on a path", "star"],
        steps: &[
            "The two arcs on the \"Path guide\" layer (output off) are the paths the texts follow (select a text and \
             a shape, then Apply Path to Text).",
            "The top text sits on its arc by its baseline (Align Y bottom); the bottom text hangs from its arc by the \
             tops of its capitals (Align Y top), so both read left to right, upright.",
            "The star is a Polygon with 5 points and an inner ratio of 0.45, filled.",
            "Fill in template changes both lines of text; they stay centred on their arcs.",
        ],
        build: medallion,
    },
    Example {
        id: "dither-sample",
        title: "Image and dither sample",
        summary: "A generated test picture (grey steps, a shaded ball and rings) engraved twice, with Stucki \
            dithering and with a halftone, to compare image modes on your material.",
        tags: &["image", "dither", "halftone"],
        steps: &[
            "The picture is generated by OpenEtch (no photo needed): a ten-step grey wedge, a shaded ball, fine rings \
             and lines.",
            "The left copy is on a layer in Image mode with Stucki dithering at 0.05 mm; the right copy uses \
             Halftone. Select a layer and change its image mode to compare others.",
            "The preview (Laser workspace) shows the dots each mode makes before anything burns.",
        ],
        build: dither_sample,
    },
    Example {
        id: "palette-coin",
        title: "Frost, black and colour palette coin",
        summary: "A 40 mm coin in three sectors, one per finish (frost, black and a MOPA colour), each on its own \
            layer, with a label: a quick look at what each finish does.",
        tags: &["template", "MOPA colour", "layers"],
        steps: &[
            "Each sector is a closed shape on its own layer, so each finish has its own settings.",
            "Frost is a fast, light fill; Black anneals slowly with a fine cross-hatch; the colour sector's finish is \
             a template field (Gold, Blue or Purple).",
            "The settings are starting points only: run Laser Tools > Material Test, then keep what works as library \
             presets.",
        ],
        build: palette_coin,
    },
];

/// Run command `C` on `doc` with JSON `params`.
fn step<C: Command>(doc: &mut Document, params: Value) -> Result<Value> {
    let input: C::Input =
        serde_json::from_value(params).map_err(|e| CommandError::Failed(format!("example: {}: {e}", C::ID)))?;
    Ok(C::apply(doc, input)?.0)
}

fn created(r: &Value) -> Result<String> {
    r["created"][0].as_str().or_else(|| r["id"].as_str()).map(str::to_string).ok_or_else(|| CommandError::Failed("example: nothing was created".into()))
}

fn hershey(family: &str) -> Value {
    json!({ "family": family, "kind": "stroke" })
}

/// A layer (the first one renamed, or a new one) with these settings.
fn layer_with(doc: &mut Document, first: bool, name: &str, color: &str, settings: Value) -> Result<String> {
    let id = if first {
        doc.layers.first().map(|l| l.id.clone()).ok_or_else(|| CommandError::Failed("example: no layer".into()))?
    } else {
        created(&step::<layer::LayerAdd>(doc, json!({ "name": name, "color": color }))?)?
    };
    step::<layer::LayerUpdate>(doc, json!({ "id": id, "name": name, "color": color, "settings": settings }))?;
    Ok(id)
}

/// Illustrative fill settings (not tested values): power %, speed mm/s, frequency kHz, pulse width ns, hatch
/// interval mm, cross-hatch.
#[derive(Debug, Clone, Copy)]
struct Fill {
    power: f64,
    speed: f64,
    khz: f64,
    ns: f64,
    interval: f64,
    cross: bool,
}

const GOLD: Fill = Fill { power: 28.0, speed: 500.0, khz: 250.0, ns: 200.0, interval: 0.01, cross: false };
const BLUE: Fill = Fill { power: 22.0, speed: 300.0, khz: 450.0, ns: 200.0, interval: 0.01, cross: false };
const PURPLE: Fill = Fill { power: 24.0, speed: 350.0, khz: 400.0, ns: 200.0, interval: 0.01, cross: false };
const BLACK: Fill = Fill { power: 35.0, speed: 250.0, khz: 600.0, ns: 200.0, interval: 0.005, cross: true };
const FROST: Fill = Fill { power: 80.0, speed: 2500.0, khz: 40.0, ns: 200.0, interval: 0.03, cross: false };
const MARK: Fill = Fill { power: 55.0, speed: 900.0, khz: 60.0, ns: 200.0, interval: 0.03, cross: true };

/// A complete copy of `layer`'s settings with fill `f`, for a finish choice.
fn finish(doc: &Document, layer: &str, f: Fill) -> Result<CutSettings> {
    let mut s = doc.layer(layer).ok_or_else(|| CommandError::Failed("example: layer".into()))?.settings.clone();
    s.mode = FillMode::Fill;
    s.power_pct = f.power;
    s.speed_mm_s = f.speed;
    s.frequency_khz = f.khz;
    s.pulse_width_ns = Some(f.ns);
    s.hatch.interval_mm = f.interval;
    s.hatch.cross_hatch = f.cross;
    s.hatch.bidirectional = true;
    Ok(s)
}

fn fill(f: Fill) -> Value {
    json!({ "mode": "fill", "power_pct": f.power, "speed_mm_s": f.speed, "frequency_khz": f.khz, "pulse_width_ns": f.ns,
        "hatch": { "interval_mm": f.interval, "cross_hatch": f.cross, "bidirectional": true } })
}

fn line(power: f64, speed: f64, khz: f64) -> Value {
    json!({ "mode": "line", "power_pct": power, "speed_mm_s": speed, "frequency_khz": khz, "pulse_width_ns": 200 })
}

fn notes(doc: &mut Document, ex: &Example) -> Result<()> {
    let mut text = format!("{}\n\n{}\n", ex.title, ex.summary.split_whitespace().collect::<Vec<_>>().join(" "));
    for (k, s) in ex.steps.iter().enumerate() {
        text.push_str(&format!("\n{}. {}", k + 1, s.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    step::<pages::SetNotes>(doc, json!({ "text": text, "show_on_load": true }))?;
    Ok(())
}

/// Cubic Bézier control distance for a circular arc of `deg` degrees and radius `r`.
fn kappa(deg: f64, r: f64) -> f64 {
    4.0 / 3.0 * (deg.to_radians() / 4.0).tan() * r
}

/// SVG data (y-up mm) of an arc of radius `r` about the origin from angle `a0` to `a1` (degrees, counter-clockwise
/// when `a1 > a0`), in segments of at most 90°. `start` begins it with a move (else a line) to its first point.
fn arc(r: f64, a0: f64, a1: f64, start: bool) -> String {
    let n = ((a1 - a0).abs() / 90.0).ceil().max(1.0) as usize;
    let d = (a1 - a0) / n as f64;
    let pt = |a: f64| (r * a.to_radians().cos(), r * a.to_radians().sin());
    let (x0, y0) = pt(a0);
    let mut s = format!("{} {x0:.4} {y0:.4}", if start { "M" } else { "L" });
    for k in 0..n {
        let (a, b) = (a0 + d * k as f64, a0 + d * (k + 1) as f64);
        let h = kappa(d.abs(), r) * d.signum();
        let (p, q) = (pt(a), pt(b));
        let (ta, tb) = ((-a.to_radians().sin(), a.to_radians().cos()), (-b.to_radians().sin(), b.to_radians().cos()));
        s.push_str(&format!(
            " C {:.4} {:.4} {:.4} {:.4} {:.4} {:.4}",
            p.0 + h * ta.0,
            p.1 + h * ta.1,
            q.0 - h * tb.0,
            q.1 - h * tb.1,
            q.0,
            q.1
        ));
    }
    s
}

fn two_tone_coin(doc: &mut Document) -> Result<()> {
    step::<stock::StockDefine>(doc, json!({ "name": "40 mm coin", "shape": { "kind": "circle", "diameter_mm": 40 }, "thickness_mm": 1.5, "material": "stainless steel" }))?;
    let ring = layer_with(doc, true, "Colour ring", "#c08a00", fill(GOLD))?;
    let name = layer_with(doc, false, "Name", "#0000ff", line(60.0, 800.0, 40.0))?;
    let rim = layer_with(doc, false, "Rim", "#000000", line(70.0, 1000.0, 40.0))?;
    let outer = created(&step::<shape::ShapeEllipse>(doc, json!({ "rx_mm": 19.5, "ry_mm": 19.5, "layer": rim, "name": "Rim" }))?)?;
    let inner = created(&step::<shape::ShapeEllipse>(doc, json!({ "rx_mm": 15, "ry_mm": 15, "layer": rim, "name": "Inner rim" }))?)?;
    let band = created(&step::<fill_region::FillRegion>(doc, json!({ "points": [[17.25, 0]], "layer": ring, "boundary_ids": [outer, inner], "name": "Colour ring" }))?)?;
    // Fit to stock: the ring and rims centred on the coin, as wide as it less a quarter millimetre each side (a
    // round stock: "contain" would fit their square bounding box inside the disc).
    step::<place::Place>(doc, json!({ "ids": [outer, inner, band], "anchor": "center", "fit": { "mode": "width", "width_mm": 39.5 } }))?;
    let title = created(&step::<text::ShapeText>(doc, json!({ "text": "ADA", "font": hershey("Hershey Sans Medium"), "height_mm": 7,
        "center": [0, 2.5], "layer": name, "name": "Name",
        "fit": { "shape": "box", "width_mm": 24, "height_mm": 9, "mode": "contain" } }))?)?;
    let date = created(&step::<text::ShapeText>(doc, json!({ "text": "03.11.2026", "font": hershey("Hershey Sans"), "height_mm": 2.8,
        "center": [0, -9], "layer": name, "name": "Date" }))?)?;
    let gold = finish(doc, &ring, GOLD)?;
    let blue = finish(doc, &ring, BLUE)?;
    let black = finish(doc, &ring, BLACK)?;
    step::<template::SetField>(doc, json!({ "field": { "id": "name", "label": "Name", "kind": "text", "targets": [title], "max_chars": 16,
        "hint": "Fits itself into the centre of the coin." } }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "date", "label": "Date", "kind": "date", "targets": [date], "format": "dd.MM.yyyy", "value": "2026-11-03" } }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "font", "label": "Font", "kind": "font", "targets": [title], "choices": [
        { "label": "Plain", "font": hershey("Hershey Sans Medium") },
        { "label": "Script", "font": hershey("Hershey Script Medium") },
        { "label": "Serif", "font": hershey("Hershey Serif Bold") }] } }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "ring", "label": "Ring colour", "kind": "preset", "targets": [ring], "choices": [
        { "label": "Gold", "settings": gold, "swatch": "#c08a00" },
        { "label": "Blue", "settings": blue, "swatch": "#2a5db0" },
        { "label": "Black", "settings": black, "swatch": "#222222" }] } }))?;
    step::<template::SetLock>(doc, json!({ "lock_others": true }))?;
    Ok(())
}

fn serial_tags(doc: &mut Document) -> Result<()> {
    let text_layer = layer_with(doc, true, "Serial", "#0000ff", line(55.0, 900.0, 40.0))?;
    let code_layer = layer_with(doc, false, "QR code", "#ff0000", fill(Fill { cross: false, ..MARK }))?;
    let pocket_layer = created(&step::<layer::LayerTool>(doc, json!({ "tool": "T1" }))?)?;
    // One tag, centred on the first pocket at the lower left; the array fills the other five.
    let (w, h, gap) = (30.0, 12.0, 4.0);
    let x0 = -(w + gap) / 2.0;
    let y0 = -(h + gap);
    let pocket = created(&step::<shape::ShapeRect>(doc, json!({ "width_mm": w, "height_mm": h, "corner_radius_mm": 3, "center": [x0, y0], "layer": pocket_layer, "name": "Pocket" }))?)?;
    let serial = created(&step::<text::ShapeText>(doc, json!({ "text": "SN 0dddd", "font": hershey("Hershey Sans"), "height_mm": 4,
        "center": [x0 + 4.0, y0], "layer": text_layer, "name": "Serial", "variable": { "mode": "serial" } }))?)?;
    let qr = created(&step::<text::ShapeBarcode>(doc, json!({ "kind": "qr", "data": "SN 0dddd", "module_mm": 0.35, "variable": { "mode": "serial" },
        "center": [x0 - 9.0, y0], "layer": code_layer, "name": "QR code" }))?)?;
    step::<text::SetSerial>(doc, json!({ "start": 1, "end": 60, "current": 1, "advance_by": 6, "auto_advance": true }))?;
    step::<array::Array>(doc, json!({ "ids": [pocket, serial, qr], "rows": 3, "cols": 2, "pitch_x_mm": w + gap, "pitch_y_mm": h + gap, "auto_increment": 1 }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "first", "label": "First serial number", "kind": "serial",
        "hint": "The six tags show this number and the five after it." } }))?;
    Ok(())
}

fn medallion(doc: &mut Document) -> Result<()> {
    step::<stock::StockDefine>(doc, json!({ "name": "50 mm medallion", "shape": { "kind": "circle", "diameter_mm": 50 }, "thickness_mm": 2, "material": "brass" }))?;
    let text_layer = layer_with(doc, true, "Text", "#0000ff", line(55.0, 800.0, 40.0))?;
    let star_layer = layer_with(doc, false, "Star", "#ff0000", fill(MARK))?;
    let rim_layer = layer_with(doc, false, "Rings", "#000000", line(65.0, 1000.0, 40.0))?;
    let guide = layer_with(doc, false, "Path guide", "#00a000", line(10.0, 1000.0, 40.0))?;
    step::<layer::LayerUpdate>(doc, json!({ "id": guide, "output": false }))?;
    let r = 17.5;
    // Both arcs run left to right (clockwise over the top, counter-clockwise under the bottom), so text on either
    // reads upright.
    let top = created(&step::<shape::ShapePath>(doc, json!({ "svg_path_data": arc(r, 180.0, 0.0, true), "layer": guide, "name": "Top arc" }))?)?;
    let bottom = created(&step::<shape::ShapePath>(doc, json!({ "svg_path_data": arc(r, 180.0, 360.0, true), "layer": guide, "name": "Bottom arc" }))?)?;
    let t1 = created(&step::<text::ShapeText>(doc, json!({ "text": "OPENETCH WORKSHOP", "font": hershey("Hershey Serif Medium"), "height_mm": 4.2,
        "align_v": "bottom", "layer": text_layer, "name": "Top text" }))?)?;
    step::<text::ApplyPath>(doc, json!({ "id": t1, "path": top }))?;
    let t2 = created(&step::<text::ShapeText>(doc, json!({ "text": "MADE BY HAND", "font": hershey("Hershey Serif Medium"), "height_mm": 4.2,
        "align_v": "top", "layer": text_layer, "name": "Bottom text" }))?)?;
    step::<text::ApplyPath>(doc, json!({ "id": t2, "path": bottom }))?;
    step::<generate::ShapePolygon>(doc, json!({ "sides": 5, "radius_mm": 9, "star_inner_ratio": 0.45, "layer": star_layer, "name": "Star", "parametric": true }))?;
    step::<shape::ShapeEllipse>(doc, json!({ "rx_mm": 24, "ry_mm": 24, "layer": rim_layer, "name": "Outer ring" }))?;
    step::<shape::ShapeEllipse>(doc, json!({ "rx_mm": 12.5, "ry_mm": 12.5, "layer": rim_layer, "name": "Inner ring" }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "top", "label": "Top line", "kind": "text", "targets": [t1], "max_chars": 28 } }))?;
    step::<template::SetField>(doc, json!({ "field": { "id": "bottom", "label": "Bottom line", "kind": "text", "targets": [t2], "max_chars": 28, "required": false } }))?;
    Ok(())
}

/// The generated test picture: a grey wedge, a shaded ball, rings and fine lines (8-bit grey, `w` x `h`).
pub fn test_picture(w: u32, h: u32) -> fileio::image::GrayImage {
    let wedge = h / 4;
    fileio::image::GrayImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (f64::from(x), f64::from(y));
        let v = if y < wedge {
            // Ten grey steps, black to white.
            let k = (x * 10 / w).min(9);
            f64::from(k) / 9.0
        } else {
            let (cx, cy, r) = (f64::from(w) * 0.27, f64::from(wedge) + f64::from(h - wedge) * 0.5, f64::from(h - wedge) * 0.42);
            let (dx, dy) = ((fx - cx) / r, (fy - cy) / r);
            let d2 = dx * dx + dy * dy;
            if d2 <= 1.0 {
                // A ball lit from the upper left.
                let z = (1.0 - d2).sqrt();
                let lit = (-0.5 * dx - 0.5 * dy + 0.7 * z).max(0.0) / 0.995;
                0.1 + 0.9 * lit.min(1.0)
            } else if fx > f64::from(w) * 0.55 {
                let (rx, ry) = (fx - f64::from(w) * 0.78, fy - cy);
                let rr = (rx * rx + ry * ry).sqrt();
                if fx > f64::from(w) * 0.94 {
                    // Fine lines, 1, 2 and 3 px wide.
                    let band = ((fy - f64::from(wedge)) / f64::from(h - wedge) * 3.0) as u32 + 1;
                    if (x / band).is_multiple_of(2) { 0.0 } else { 1.0 }
                } else if ((rr / 6.0) as u32).is_multiple_of(2) {
                    0.15
                } else {
                    0.95
                }
            } else {
                1.0
            }
        };
        fileio::image::Luma([(v.clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

fn dither_sample(doc: &mut Document) -> Result<()> {
    use base64::Engine;
    let img = fileio::image::DynamicImage::ImageLuma8(test_picture(360, 240));
    let png = fileio::bitmap::png(&img, Some(300.0)).map_err(|e| CommandError::Failed(format!("example: {e}")))?;
    let data = base64::engine::general_purpose::STANDARD.encode(png);
    let stucki = layer_with(doc, true, "Image: Stucki", "#000000", json!({ "power_pct": 35, "speed_mm_s": 1500, "frequency_khz": 60, "pulse_width_ns": 200,
        "image": { "dither": "stucki", "interval_mm": 0.05, "bidirectional": true } }))?;
    let halftone = layer_with(doc, false, "Image: Halftone", "#0000ff", json!({ "power_pct": 35, "speed_mm_s": 1500, "frequency_khz": 60, "pulse_width_ns": 200,
        "image": { "dither": "halftone", "interval_mm": 0.05, "halftone_cells_per_inch": 60, "halftone_angle_deg": 45, "bidirectional": true } }))?;
    let labels = layer_with(doc, false, "Labels", "#ff0000", line(50.0, 900.0, 40.0))?;
    let a = created(&step::<image::ImportImage>(doc, json!({ "data_base64": data, "mime": "image/png", "width_mm": 30, "layer": stucki, "name": "Test picture (Stucki)" }))?)?;
    let b = created(&step::<image::ImportImage>(doc, json!({ "data_base64": data, "mime": "image/png", "width_mm": 30, "layer": halftone, "name": "Test picture (Halftone)" }))?)?;
    let move_by = |doc: &mut Document, id: &str, dx: f64| step::<super::object::Transform>(doc, json!({ "ids": [id], "translate": [dx, 0] }));
    move_by(doc, &a, -17.0)?;
    move_by(doc, &b, 17.0)?;
    for (x, label) in [(-17.0, "STUCKI 0.05"), (17.0, "HALFTONE 60 LPI")] {
        step::<text::ShapeText>(doc, json!({ "text": label, "font": hershey("Hershey Sans"), "height_mm": 2.5, "center": [x, -13], "layer": labels, "name": label }))?;
    }
    Ok(())
}

fn palette_coin(doc: &mut Document) -> Result<()> {
    step::<stock::StockDefine>(doc, json!({ "name": "40 mm coin", "shape": { "kind": "circle", "diameter_mm": 40 }, "thickness_mm": 1.5, "material": "stainless steel" }))?;
    let frost = layer_with(doc, true, "Frost", "#7f7f7f", fill(FROST))?;
    let black = layer_with(doc, false, "Black", "#000000", fill(BLACK))?;
    let colour = layer_with(doc, false, "Colour", "#c08a00", fill(GOLD))?;
    let labels = layer_with(doc, false, "Labels", "#0000ff", line(55.0, 900.0, 40.0))?;
    let r = 19.0;
    // Three 120° sectors, a little apart so the finishes do not overlap.
    let sectors: [(f64, &String, &str); 3] = [(90.0, &frost, "Frost"), (210.0, &black, "Black"), (330.0, &colour, "Colour")];
    for (mid, layer_id, name) in sectors {
        let (a0, a1) = (mid - 59.0, mid + 59.0);
        let gap = 0.6;
        let off = (gap * mid.to_radians().cos(), gap * mid.to_radians().sin());
        let d = format!("M {:.4} {:.4} {} Z", off.0, off.1, arc(r, a0, a1, false));
        step::<shape::ShapePath>(doc, json!({ "svg_path_data": d, "layer": layer_id, "name": name }))?;
    }
    step::<shape::ShapeEllipse>(doc, json!({ "rx_mm": 19.6, "ry_mm": 19.6, "layer": labels, "name": "Rim" }))?;
    for (mid, label) in [(90.0, "FROST"), (210.0, "BLACK"), (330.0, "COLOUR")] {
        let (x, y) = (11.0 * f64::cos(f64::to_radians(mid)), 11.0 * f64::sin(f64::to_radians(mid)));
        step::<text::ShapeText>(doc, json!({ "text": label, "font": hershey("Hershey Sans"), "height_mm": 2.6, "center": [x, y], "layer": labels, "name": label }))?;
    }
    let mut list = Vec::new();
    for (label, f, swatch) in [("Gold", GOLD, "#c08a00"), ("Blue", BLUE, "#2a5db0"), ("Purple", PURPLE, "#6a3d9a")] {
        list.push(json!({ "label": label, "settings": finish(doc, &colour, f)?, "swatch": swatch }));
    }
    step::<template::SetField>(doc, json!({ "field": { "id": "colour", "label": "Colour sector", "kind": "preset", "targets": [colour], "choices": list } }))?;
    Ok(())
}

/// Build example `id` into `doc` (a fresh document from `doc.new`): its name, objects, notes and fields.
pub fn build(id: &str, doc: &mut Document) -> Result<()> {
    let ex = EXAMPLES.iter().find(|e| e.id == id).ok_or_else(|| {
        let ids: Vec<&str> = EXAMPLES.iter().map(|e| e.id).collect();
        invalid("doc.new", format!("no example '{id}'; examples: {}", ids.join(", ")))
    })?;
    (ex.build)(doc)?;
    notes(doc, ex)?;
    doc.name = ex.title.to_string();
    Ok(())
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExampleListInput {}

/// `example.list`.
pub struct ExampleList;

impl Command for ExampleList {
    type Input = ExampleListInput;
    const ID: &'static str = "example.list";
    const TITLE: &'static str = "Example projects";
    const DESCRIPTION: &'static str = "The example projects that ship with OpenEtch: `[{id, title, summary, tags, \
        steps}]` (steps: the walkthrough, also shown as the project notes). Open one with doc.new {example: id} (it \
        replaces the document like doc.new; undoable). Several are templates: template.fields / template.fill. \
        Changes nothing.";
    const RISK: Risk = Risk::Query;

    fn apply(_doc: &mut Document, _: ExampleListInput) -> Result<(Value, String)> {
        let list: Vec<Value> = EXAMPLES
            .iter()
            .map(|e| {
                let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
                json!({ "id": e.id, "title": e.title, "summary": clean(e.summary), "tags": e.tags,
                    "steps": e.steps.iter().map(|s| clean(s)).collect::<Vec<_>>() })
            })
            .collect();
        Ok((json!(list), format!("{} examples", EXAMPLES.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doc::Geometry;

    fn open(id: &str) -> Document {
        let mut s = crate::Session::new(Document { field_mm: Some(110.0), ..Document::default() });
        s.execute(crate::Invocation { id: "doc.new".into(), params: json!({ "example": id }) }, crate::Origin::Ui)
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        s.document().clone()
    }

    #[test]
    fn every_example_builds_inside_a_small_field_and_round_trips() {
        for ex in EXAMPLES {
            let d = open(ex.id);
            assert_eq!(d.name, ex.title);
            assert!(!d.objects.is_empty(), "{}", ex.id);
            assert!(d.notes.text.contains(ex.steps[0].split_whitespace().next().unwrap()));
            assert!(d.notes.show_on_load);
            // Everything inside 70 mm round the field centre, so it fits any galvo lens and bed.
            let b = d.bounds_of(&d.objects.iter().map(|o| o.id.clone()).collect::<Vec<_>>()).unwrap();
            assert!(b.x0 > -35.0 && b.x1 < 35.0 && b.y0 > -35.0 && b.y1 < 35.0, "{}: {b:?}", ex.id);
            // Texts all render with the bundled fonts and are not empty.
            for o in &d.objects {
                if let Geometry::Text(t) = &o.geometry {
                    assert!(!t.outline.elements().is_empty(), "{} {}", ex.id, o.name);
                }
            }
            // Saved and loaded again unchanged.
            let text = doc::file::try_to_openetch_json(&d).unwrap();
            let back = doc::file::from_openetch_json(&text).unwrap();
            assert_eq!(back.objects.len(), d.objects.len());
            assert_eq!(back.extras, d.extras);
        }
    }

    #[test]
    fn examples_have_what_their_notes_promise() {
        let coin = open("two-tone-coin");
        let spec = template::spec(&coin);
        assert_eq!(spec.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["name", "date", "font", "ring"]);
        assert!(spec.lock_others);
        assert!(coin.stock.is_some());
        let ring = coin.objects.iter().find(|o| o.name == "Colour ring").expect("fill region made the ring");
        let rb = ring.bounds();
        assert!((rb.width() - 39.5).abs() < 0.05, "fitted to the stock less 0.25 mm: {rb:?}");
        // Filling it in works and nothing overflows.
        let mut s = crate::Session::new(coin);
        let r = s
            .execute(crate::Invocation { id: "template.fill".into(), params: json!({ "values": { "name": "Grace Hopper", "ring": "Blue", "font": "Script" } }) }, crate::Origin::Agent("t".into()))
            .unwrap();
        assert_eq!(r.result["overflow"], json!([]), "{}", r.result);

        let tags = open("serial-tags");
        assert_eq!(tags.objects.iter().filter(|o| matches!(o.geometry, Geometry::Text(_))).count(), 6);
        let shown: Vec<String> = tags
            .objects
            .iter()
            .filter_map(|o| match &o.geometry {
                Geometry::Text(t) => Some(t.shown.clone().unwrap_or_default()),
                _ => None,
            })
            .collect();
        for n in 1..=6 {
            assert!(shown.contains(&format!("SN {n:04}")), "{shown:?}");
        }
        let mut s = crate::Session::new(tags);
        let r = s.execute(crate::Invocation { id: "var.contact_sheet".into(), params: json!({ "count": 500 }) }, crate::Origin::Ui).unwrap();
        assert_eq!(r.result["summary"]["problems"], json!({}), "every tag number fits its pocket");

        let medal = open("text-medallion");
        for name in ["Top text", "Bottom text"] {
            let o = medal.objects.iter().find(|o| o.name == name).unwrap();
            let b = o.bounds();
            assert!(b.width() > 15.0, "{name} follows its arc: {b:?}");
            if name == "Top text" { assert!(b.y0 > 5.0, "{b:?}") } else { assert!(b.y1 < -5.0, "{b:?}") }
        }

        let dither = open("dither-sample");
        assert_eq!(dither.objects.iter().filter(|o| matches!(o.geometry, Geometry::Image { .. })).count(), 2);
        let palette = open("palette-coin");
        assert_eq!(palette.layers.iter().filter(|l| l.settings.mode == FillMode::Fill).count(), 3);

        let mut d = Document::default();
        let (list, _) = ExampleList::apply(&mut d, ExampleListInput {}).unwrap();
        assert_eq!(list.as_array().unwrap().len(), EXAMPLES.len());
        assert!(!list[0]["steps"][0].as_str().unwrap().contains("  "));
    }

    #[test]
    fn unknown_examples_are_refused_and_the_picture_is_stable() {
        let mut s = crate::Session::new(Document::default());
        let e = s.execute(crate::Invocation { id: "doc.new".into(), params: json!({ "example": "nope" }) }, crate::Origin::Ui).unwrap_err();
        assert!(e.to_string().contains("two-tone-coin"), "{e}");
        let a = test_picture(60, 40);
        assert_eq!(a, test_picture(60, 40));
        assert_eq!(a.get_pixel(0, 0).0[0], 0, "the wedge starts black");
        assert_eq!(a.get_pixel(59, 0).0[0], 255, "and ends white");
    }
}
