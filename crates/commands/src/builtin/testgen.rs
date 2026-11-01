//! Test pattern generators (LightBurn's Laser Tools): `generate.material_test` (a grid varying two settings, e.g.
//! power x speed or, for MOPA colour, frequency x Q-pulse width), `generate.interval_test` (fill swatches at a
//! range of line intervals) and `generate.focus_test` (lines at stepped Z heights). Each adds ordinary objects and
//! layers to the document (one layer per test setting, in LightBurn's safe execution order), labels in a bundled
//! single-line font on a labels layer, all in one group; marking them is an ordinary run.

use doc::{CutSettings, Document, FillMode, Geometry, Layer, Object, ObjectId};
use kurbo::{Affine, BezPath};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::layer::{MAX_PASSES, SettingsPatch, validate_settings};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Most values along one axis of a material test (LightBurn defaults to 10).
pub const MAX_COUNT: u32 = 20;
/// Most interval or focus steps.
pub const MAX_STEPS: u32 = 50;

/// A setting a test varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TestParam {
    /// Power, %.
    Power,
    /// Speed, mm/s.
    Speed,
    /// Pulse frequency, kHz.
    Frequency,
    /// MOPA Q-pulse width, ns.
    PulseWidth,
    /// Fill line interval, mm.
    Interval,
    /// Passes (rounded to whole passes).
    Passes,
}

impl TestParam {
    fn label(self) -> &'static str {
        match self {
            TestParam::Power => "Power (%)",
            TestParam::Speed => "Speed (mm/s)",
            TestParam::Frequency => "Frequency (kHz)",
            TestParam::PulseWidth => "Q-pulse (ns)",
            TestParam::Interval => "Interval (mm)",
            TestParam::Passes => "Passes",
        }
    }

    fn set(self, s: &mut CutSettings, v: f64) {
        match self {
            TestParam::Power => s.power_pct = v,
            TestParam::Speed => s.speed_mm_s = v,
            TestParam::Frequency => s.frequency_khz = v,
            TestParam::PulseWidth => s.pulse_width_ns = Some(v),
            TestParam::Interval => s.hatch.interval_mm = v,
            TestParam::Passes => s.passes = v.round().clamp(1.0, f64::from(MAX_PASSES)) as u32,
        }
    }

    /// Burn risk rank of a value (lower runs first): LightBurn runs the highest speed, lowest power, lowest interval
    /// and fewest passes first.
    fn risk(self, v: f64) -> f64 {
        match self {
            TestParam::Speed => -v,
            _ => v,
        }
    }

    /// LightBurn's order of the risk keys.
    fn rank(self) -> usize {
        match self {
            TestParam::Speed => 0,
            TestParam::Power => 1,
            TestParam::Interval => 2,
            TestParam::Passes => 3,
            TestParam::Frequency => 4,
            TestParam::PulseWidth => 5,
        }
    }
}

/// One axis of a material test.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestAxis {
    pub param: TestParam,
    pub min: f64,
    pub max: f64,
    /// Number of values, 1..=20 (LightBurn's Count; default 10).
    #[serde(default = "ten")]
    pub count: u32,
}

fn ten() -> u32 {
    10
}

impl TestAxis {
    fn values(&self, cmd: &str) -> Result<Vec<f64>> {
        util::finite(cmd, "min", self.min)?;
        util::finite(cmd, "max", self.max)?;
        if !(1..=MAX_COUNT).contains(&self.count) {
            return Err(invalid(cmd, format!("count must be within 1..={MAX_COUNT} (got {})", self.count)));
        }
        let n = self.count;
        let values: Vec<f64> = (0..n)
            .map(|i| if n == 1 { self.min } else { self.min + (self.max - self.min) * f64::from(i) / f64::from(n - 1) })
            .map(|v| if self.param == TestParam::Passes { v.round() } else { v })
            .collect();
        // Passes were clamped silently when set, so a box labelled "0" ran once: refuse instead.
        if self.param == TestParam::Passes
            && let Some(v) = values.iter().find(|v| !(1.0..=f64::from(MAX_PASSES)).contains(*v))
        {
            return Err(invalid(cmd, format!("passes must be within 1..={MAX_PASSES} (got {v})")));
        }
        Ok(values)
    }
}

/// A number for a label: at most 3 decimals, trailing zeros dropped.
fn show(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A display colour for test cell `i` of `n` (hues around the wheel).
fn cell_color(i: usize, n: usize) -> String {
    let h = (i as f64 / n.max(1) as f64) * 300.0;
    let (r, g, b) = hsv(h, 0.75, 0.85);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn hsv(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    let q = |t: f64| ((t + m) * 255.0).round() as u8;
    (q(r), q(g), q(b))
}

/// Settings from `base` (merged into `start`) and validated.
fn settings_from(cmd: &str, start: CutSettings, patch: Option<SettingsPatch>) -> Result<CutSettings> {
    let mut s = start;
    if let Some(p) = patch {
        p.apply(cmd, &mut s)?;
    }
    validate_settings(cmd, &s)?;
    Ok(s)
}

fn add_layer(doc: &mut Document, name: &str, color: &str, settings: CutSettings) -> Result<String> {
    let id = doc.alloc_id("layer")?;
    let mut l = Layer::new(id.clone(), name, color);
    l.settings = settings;
    doc.layers.push(l);
    Ok(id)
}

fn add_shape(doc: &mut Document, name: &str, layer: &str, geometry: Geometry, at: (f64, f64)) -> Result<ObjectId> {
    let id = doc.alloc_id("obj")?;
    doc.objects.push(Object::new(id.clone(), name, layer.to_string(), geometry, Affine::translate(at)));
    Ok(id)
}

fn add_rect(doc: &mut Document, name: &str, layer: &str, w: f64, h: f64, at: (f64, f64)) -> Result<ObjectId> {
    add_shape(doc, name, layer, Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: 0.0 }, at)
}

/// A label in the bundled Hershey Sans single-line font (fast to mark, needs no installed font).
fn add_label(doc: &mut Document, text: &str, layer: &str, height: f64, at: (f64, f64), align_h: &str, rotation_deg: f64) -> Result<ObjectId> {
    let input = json!({
        "text": text, "font": { "family": text::hershey::DEFAULT, "kind": "stroke" }, "height_mm": height,
        "center": [at.0, at.1], "rotation_deg": rotation_deg, "layer": layer, "align_h": align_h, "align_v": "middle",
        "name": format!("Label {text}"),
    });
    let input = serde_json::from_value(input).map_err(|e| CommandError::Failed(e.to_string()))?;
    let (r, _) = <super::text::ShapeText as Command>::apply(doc, input)?;
    r["created"][0].as_str().map(str::to_string).ok_or_else(|| CommandError::Failed("the label was not created".into()))
}

fn group(doc: &mut Document, ids: &[ObjectId], name: &str) -> Result<String> {
    let gid = doc.alloc_id("group")?;
    doc.groups.push(doc::Group::new(gid.clone(), name, None));
    for id in ids {
        if let Some(o) = doc.object_mut(id) {
            o.group = Some(gid.clone());
        }
    }
    Ok(gid)
}

fn label_layer(cmd: &str, doc: &mut Document, enabled: bool, patch: Option<SettingsPatch>, name: &str) -> Result<Option<String>> {
    if !enabled {
        return Ok(None);
    }
    let base = CutSettings { mode: FillMode::Line, power_pct: 30.0, speed_mm_s: 1000.0, ..CutSettings::default() };
    let s = settings_from(cmd, base, patch)?;
    Ok(Some(add_layer(doc, name, "#000000", s)?))
}

fn result(doc: &Document, created: &[ObjectId], layers: &[String], group: &str, extra: Value) -> Value {
    let mut r = util::edit_result(doc, created, &[]);
    r["layers"] = json!(layers);
    r["group"] = json!(group);
    if let Value::Object(m) = extra {
        for (k, v) in m {
            r[k] = v;
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------------------------
// Material test
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaterialTestInput {
    /// What varies down the rows. Default speed 500..5000 mm/s, 10 values.
    pub rows: Option<TestAxis>,
    /// What varies across the columns. Default power 10..100 %, 10 values.
    pub columns: Option<TestAxis>,
    /// Width of each box, mm. Default 5.
    #[serde(default = "five")]
    pub cell_width_mm: f64,
    /// Height of each box, mm. Default 5.
    #[serde(default = "five")]
    pub cell_height_mm: f64,
    /// Gap between boxes, mm. Default 1.
    #[serde(default = "one")]
    pub spacing_mm: f64,
    /// Centre of the grid, field mm (LightBurn's X/Y Center). Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Base settings of every box (LightBurn's Edit Material Setting), partial over fill mode at the defaults; the
    /// two tested values replace theirs.
    pub base: Option<SettingsPatch>,
    /// Mark labels with the tested values (LightBurn's Enable Text). Default true.
    #[serde(default = "yes")]
    pub labels: bool,
    /// Label height, mm. Default 1.5.
    #[serde(default = "label_height")]
    pub label_height_mm: f64,
    /// Label settings (LightBurn's Edit Text Setting), partial over line mode, 30 %, 1000 mm/s.
    pub label_settings: Option<SettingsPatch>,
    /// A rectangle around the grid (LightBurn's Enable Border). Default false.
    #[serde(default)]
    pub border: bool,
    /// Border settings (LightBurn's Edit Border Setting), partial over line mode, 30 %, 1000 mm/s.
    pub border_settings: Option<SettingsPatch>,
    /// Title marked above the grid. Default none.
    pub title: Option<String>,
}

fn five() -> f64 {
    5.0
}
fn one() -> f64 {
    1.0
}
fn yes() -> bool {
    true
}
fn label_height() -> f64 {
    1.5
}

/// `generate.material_test`.
pub struct MaterialTest;

impl Command for MaterialTest {
    type Input = MaterialTestInput;
    const ID: &'static str = "generate.material_test";
    const TITLE: &'static str = "Material test";
    const DESCRIPTION: &'static str = "Add a material test grid (LightBurn's Material Test): boxes varying two of power, \
        speed, frequency, pulse_width (MOPA Q-pulse), interval and passes between min and max (count 1..=20 each, \
        default 10 x 10 power x speed); every other setting comes from `base` (fill mode by default). Each box gets \
        its own layer, ordered so the safest boxes run first (highest speed, lowest power, lowest interval, fewest \
        passes, as LightBurn does). Optional labels (bundled single-line font, own layer), border and title. \
        Everything is grouped; mark it with an ordinary run. Returns the created ids, the layers in burn order and \
        the group.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MaterialTestInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let rows = i.rows.unwrap_or(TestAxis { param: TestParam::Speed, min: 500.0, max: 5000.0, count: 10 });
            let cols = i.columns.unwrap_or(TestAxis { param: TestParam::Power, min: 10.0, max: 100.0, count: 10 });
            if rows.param == cols.param {
                return Err(invalid(cmd, "rows and columns must test different settings"));
            }
            let (rv, cv) = (rows.values(cmd)?, cols.values(cmd)?);
            let w = util::positive(cmd, "cell_width_mm", i.cell_width_mm)?;
            let h = util::positive(cmd, "cell_height_mm", i.cell_height_mm)?;
            let gap = util::non_negative(cmd, "spacing_mm", i.spacing_mm)?;
            let lh = util::positive(cmd, "label_height_mm", i.label_height_mm)?;
            let [cx, cy] = util::finite2(cmd, "center", i.center)?;
            let base = settings_from(cmd, CutSettings { mode: FillMode::Fill, ..CutSettings::default() }, i.base)?;

            // Cells and their settings, checked before anything is added.
            let mut cells: Vec<(usize, usize, CutSettings)> = Vec::new();
            for (r, &a) in rv.iter().enumerate() {
                for (c, &b) in cv.iter().enumerate() {
                    let mut s = base.clone();
                    rows.param.set(&mut s, a);
                    cols.param.set(&mut s, b);
                    validate_settings(cmd, &s).map_err(|e| invalid(cmd, format!("box {} x {}: {e}", show(a), show(b))))?;
                    cells.push((r, c, s));
                }
            }
            // LightBurn's safe execution order.
            let key = |r: usize, c: usize| {
                let mut k = [(rows.param.rank(), rows.param.risk(rv[r])), (cols.param.rank(), cols.param.risk(cv[c]))];
                k.sort_by_key(|x| x.0);
                [k[0].1, k[1].1]
            };
            cells.sort_by(|a, b| {
                let (ka, kb) = (key(a.0, a.1), key(b.0, b.1));
                ka[0].total_cmp(&kb[0]).then(ka[1].total_cmp(&kb[1]))
            });

            let (nr, nc) = (rv.len() as f64, cv.len() as f64);
            let (gw, gh) = (nc * w + (nc - 1.0) * gap, nr * h + (nr - 1.0) * gap);
            let (left, top) = (cx - gw / 2.0, cy + gh / 2.0);
            let mut created = Vec::new();
            let mut layers = Vec::new();
            let n = cells.len();
            for (k, (r, c, s)) in cells.into_iter().enumerate() {
                let name = format!("{} {} / {} {}", rows.param.label(), show(rv[r]), cols.param.label(), show(cv[c]));
                let lid = add_layer(doc, &name, &cell_color(k, n), s)?;
                let x = left + c as f64 * (w + gap) + w / 2.0;
                let y = top - r as f64 * (h + gap) - h / 2.0;
                created.push(add_rect(doc, &format!("Test {} x {}", show(rv[r]), show(cv[c])), &lid, w, h, (x, y))?);
                layers.push(lid);
            }
            if let Some(ll) = label_layer(cmd, doc, i.labels, i.label_settings, "Test labels")? {
                for (c, v) in cv.iter().enumerate() {
                    let x = left + c as f64 * (w + gap) + w / 2.0;
                    created.push(add_label(doc, &show(*v), &ll, lh, (x, top + gap + lh), "middle", 0.0)?);
                }
                created.push(add_label(doc, cols.param.label(), &ll, lh, (cx, top + gap + 2.6 * lh), "middle", 0.0)?);
                for (r, v) in rv.iter().enumerate() {
                    let y = top - r as f64 * (h + gap) - h / 2.0;
                    created.push(add_label(doc, &show(*v), &ll, lh, (left - gap - 0.2 * lh, y), "right", 0.0)?);
                }
                let widest = rv.iter().map(|v| show(*v).len()).max().unwrap_or(1) as f64;
                created.push(add_label(doc, rows.param.label(), &ll, lh, (left - gap - (widest * 0.7 + 1.2) * lh, cy), "middle", 90.0)?);
                if let Some(t) = &i.title {
                    let t = util::name(cmd, "title", t)?;
                    created.push(add_label(doc, &t, &ll, lh * 1.4, (cx, top + gap + 4.4 * lh), "middle", 0.0)?);
                }
                layers.push(ll);
            }
            if i.border {
                let base = CutSettings { mode: FillMode::Line, power_pct: 30.0, speed_mm_s: 1000.0, ..CutSettings::default() };
                let bl = add_layer(doc, "Test border", "#555555", settings_from(cmd, base, i.border_settings)?)?;
                let m = gap.max(1.0);
                created.push(add_rect(doc, "Test border", &bl, gw + 2.0 * m, gh + 2.0 * m, (cx, cy))?);
                layers.push(bl);
            }
            let g = group(doc, &created, "Material test")?;
            let summary = format!("added a {} x {} material test ({} x {})", rv.len(), cv.len(), rows.param.label(), cols.param.label());
            Ok((result(doc, &created, &layers, &g, json!({ "boxes": n })), summary))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Interval test
// ---------------------------------------------------------------------------------------------------------------

/// How interval test swatches are filled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntervalFill {
    /// Solid fill (LightBurn's Simple Fill).
    #[default]
    Simple,
    /// A black-to-white gradient image engraved with the layer's image settings at the interval (LightBurn's
    /// Dithered Image).
    Dithered,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntervalTestInput {
    /// Marking speed, mm/s. Default 1000.
    pub speed_mm_s: Option<f64>,
    /// Power, %. Default 30.
    pub power_pct: Option<f64>,
    /// Number of swatches, 2..=50. Default 5.
    #[serde(default = "five_u")]
    pub steps: u32,
    /// Smallest line interval, mm. Default 0.08 (LightBurn's suggestion).
    #[serde(default = "min_interval")]
    pub min_interval_mm: f64,
    /// Largest line interval, mm. Default 0.16.
    #[serde(default = "max_interval")]
    pub max_interval_mm: f64,
    /// Side of each swatch, mm. Default 10.
    #[serde(default = "ten_f")]
    pub size_mm: f64,
    #[serde(default)]
    pub fill: IntervalFill,
    /// Centre of the pattern, field mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Other settings of the swatches (frequency, pulse width, ...), partial.
    pub base: Option<SettingsPatch>,
    /// Mark the interval beside each swatch. Default true.
    #[serde(default = "yes")]
    pub labels: bool,
    /// Label settings, partial over line mode, 30 %, 1000 mm/s.
    pub label_settings: Option<SettingsPatch>,
}

fn five_u() -> u32 {
    5
}
fn ten_f() -> f64 {
    10.0
}
fn min_interval() -> f64 {
    0.08
}
fn max_interval() -> f64 {
    0.16
}

/// A `w` x `h` 24-bit BMP of a horizontal black (left) to white (right) gradient.
pub fn gradient_bmp(w: u32, h: u32) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&[0, 0, 0, 0]);
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&(row * h).to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&2835u32.to_le_bytes());
    b.extend_from_slice(&[0; 8]);
    for _ in 0..h {
        for x in 0..w {
            let v = ((f64::from(x) / f64::from((w - 1).max(1))) * 255.0).round() as u8;
            b.extend_from_slice(&[v, v, v]);
        }
        b.resize(b.len() + (row - w * 3) as usize, 0);
    }
    b
}

/// `generate.interval_test`.
pub struct IntervalTest;

impl Command for IntervalTest {
    type Input = IntervalTestInput;
    const ID: &'static str = "generate.interval_test";
    const TITLE: &'static str = "Interval test";
    const DESCRIPTION: &'static str = "Add an interval test (LightBurn's Interval Test): `steps` square swatches of \
        `size_mm`, each filled at a line interval from min_interval_mm to max_interval_mm (defaults 5 swatches, \
        0.08..0.16 mm, 10 mm, 1000 mm/s, 30 %), either solid (`simple`) or as a black-to-white gradient image \
        engraved at that interval (`dithered`), with the interval marked beside each. Each swatch has its own \
        layer, smallest interval first. Returns the created ids, layers and group.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: IntervalTestInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if !(2..=MAX_STEPS).contains(&i.steps) {
                return Err(invalid(cmd, format!("steps must be within 2..={MAX_STEPS} (got {})", i.steps)));
            }
            let lo = util::positive(cmd, "min_interval_mm", i.min_interval_mm)?;
            let hi = util::positive(cmd, "max_interval_mm", i.max_interval_mm)?;
            if hi < lo {
                return Err(invalid(cmd, "max_interval_mm must be at least min_interval_mm"));
            }
            let size = util::positive(cmd, "size_mm", i.size_mm)?;
            let [cx, cy] = util::finite2(cmd, "center", i.center)?;
            let mut base = CutSettings { mode: FillMode::Fill, ..CutSettings::default() };
            base.speed_mm_s = i.speed_mm_s.unwrap_or(1000.0);
            base.power_pct = i.power_pct.unwrap_or(30.0);
            let base = settings_from(cmd, base, i.base)?;
            let gap = (size * 0.3).max(1.0);
            let lh = (size * 0.25).clamp(1.0, 4.0);
            let total = f64::from(i.steps) * size + f64::from(i.steps - 1) * gap;
            let top = cy + total / 2.0;
            let x = cx - (if i.labels { 2.0 * lh } else { 0.0 });
            let image = (i.fill == IntervalFill::Dithered).then(|| {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(gradient_bmp(256, 64))
            });
            let mut created = Vec::new();
            let mut layers = Vec::new();
            let mut values = Vec::new();
            for k in 0..i.steps {
                let v = ((lo + (hi - lo) * f64::from(k) / f64::from(i.steps - 1)) * 1e6).round() / 1e6;
                let mut s = base.clone();
                s.hatch.interval_mm = v;
                s.image.interval_mm = v;
                validate_settings(cmd, &s)?;
                let lid = add_layer(doc, &format!("Interval {}", show(v)), &cell_color(k as usize, i.steps as usize), s)?;
                let y = top - f64::from(k) * (size + gap) - size / 2.0;
                let id = match &image {
                    Some(data) => add_shape(
                        doc,
                        &format!("Interval {}", show(v)),
                        &lid,
                        Geometry::Image {
                            width_mm: size,
                            height_mm: size,
                            mime: "image/bmp".into(),
                            data_base64: data.as_str().into(),
                            adjust: doc::ImageAdjust::default(),
                        },
                        (x, y),
                    )?,
                    None => add_rect(doc, &format!("Interval {}", show(v)), &lid, size, size, (x, y))?,
                };
                created.push(id);
                layers.push(lid);
                values.push(v);
            }
            if let Some(ll) = label_layer(cmd, doc, i.labels, i.label_settings, "Test labels")? {
                for (k, v) in values.iter().enumerate() {
                    let y = top - k as f64 * (size + gap) - size / 2.0;
                    created.push(add_label(doc, &show(*v), &ll, lh, (x + size / 2.0 + gap / 2.0, y), "left", 0.0)?);
                }
                layers.push(ll);
            }
            let g = group(doc, &created, "Interval test")?;
            Ok((result(doc, &created, &layers, &g, json!({ "intervals_mm": values })), format!("added an interval test of {} swatches", i.steps)))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Focus test
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FocusTestInput {
    /// Z of the first line, mm (relative to the current Z, LightBurn's "Relative Z moves only"). Default -2.
    #[serde(default = "start_z")]
    pub start_z_mm: f64,
    /// Z of the last line, mm. Default 2.
    #[serde(default = "end_z")]
    pub end_z_mm: f64,
    /// Number of steps between them, 1..=50: Steps + 1 lines. Default 8.
    #[serde(default = "eight")]
    pub steps: u32,
    /// Marking speed, mm/s. Default 500.
    pub speed_mm_s: Option<f64>,
    /// Power, % (LightBurn's Max Power). Default 30.
    pub power_pct: Option<f64>,
    /// Length of each line, mm. Default 15.
    #[serde(default = "fifteen")]
    pub line_length_mm: f64,
    /// Distance between lines, mm. Default 2.5.
    #[serde(default = "spacing")]
    pub spacing_mm: f64,
    /// Centre of the pattern, field mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Other settings of the lines (frequency, pulse width, ...), partial.
    pub base: Option<SettingsPatch>,
    /// Mark the Z height under each line. Default true.
    #[serde(default = "yes")]
    pub labels: bool,
    /// Label settings, partial over line mode, 30 %, 1000 mm/s. For a high-power source LightBurn offers
    /// perforation here ("High Power Laser Mode").
    pub label_settings: Option<SettingsPatch>,
}

fn start_z() -> f64 {
    -2.0
}
fn end_z() -> f64 {
    2.0
}
fn eight() -> u32 {
    8
}
fn fifteen() -> f64 {
    15.0
}
fn spacing() -> f64 {
    2.5
}

/// `generate.focus_test`.
pub struct FocusTest;

impl Command for FocusTest {
    type Input = FocusTestInput;
    const ID: &'static str = "generate.focus_test";
    const TITLE: &'static str = "Focus test";
    const DESCRIPTION: &'static str = "Add a focus test (LightBurn's Focus Test): Steps + 1 lines, each on its own \
        layer whose Z offset steps from start_z_mm to end_z_mm ((end - start) / steps apart), with its Z marked \
        under it; the thinnest line is nearest focus. The Z moves need a board-driven Z axis (profile `axis`, role \
        z); without one the plan warns and ignores them, so set the height by hand between lines (each line is its \
        own layer: mark them one at a time with Cut Selected). Returns the created ids, layers, group and heights.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FocusTestInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if !(1..=MAX_STEPS).contains(&i.steps) {
                return Err(invalid(cmd, format!("steps must be within 1..={MAX_STEPS} (got {})", i.steps)));
            }
            let z0 = util::finite(cmd, "start_z_mm", i.start_z_mm)?;
            let z1 = util::finite(cmd, "end_z_mm", i.end_z_mm)?;
            let len = util::positive(cmd, "line_length_mm", i.line_length_mm)?;
            let pitch = util::positive(cmd, "spacing_mm", i.spacing_mm)?;
            let [cx, cy] = util::finite2(cmd, "center", i.center)?;
            let mut base = CutSettings { mode: FillMode::Line, ..CutSettings::default() };
            base.speed_mm_s = i.speed_mm_s.unwrap_or(500.0);
            base.power_pct = i.power_pct.unwrap_or(30.0);
            let base = settings_from(cmd, base, i.base)?;
            let n = i.steps + 1;
            let lh = (pitch * 0.5).clamp(0.8, 3.0);
            let width = f64::from(n - 1) * pitch;
            let left = cx - width / 2.0;
            let mut created = Vec::new();
            let mut layers = Vec::new();
            let mut heights = Vec::new();
            for k in 0..n {
                let z = z0 + (z1 - z0) * f64::from(k) / f64::from(i.steps);
                let mut s = base.clone();
                s.options.z_offset_mm = z;
                validate_settings(cmd, &s)?;
                let lid = add_layer(doc, &format!("Focus Z {}", show(z)), &cell_color(k as usize, n as usize), s)?;
                let x = left + f64::from(k) * pitch;
                let mut p = BezPath::new();
                p.move_to((0.0, -len / 2.0));
                p.line_to((0.0, len / 2.0));
                created.push(add_shape(doc, &format!("Focus Z {}", show(z)), &lid, Geometry::Path { path: p }, (x, cy + lh))?);
                layers.push(lid);
                heights.push(z);
            }
            if let Some(ll) = label_layer(cmd, doc, i.labels, i.label_settings, "Test labels")? {
                for (k, z) in heights.iter().enumerate() {
                    let x = left + k as f64 * pitch;
                    created.push(add_label(doc, &show(*z), &ll, lh, (x, cy - len / 2.0 + lh / 2.0 - 0.5 * lh), "right", 90.0)?);
                }
                layers.push(ll);
            }
            let g = group(doc, &created, "Focus test")?;
            Ok((result(doc, &created, &layers, &g, json!({ "heights_mm": heights })), format!("added a focus test of {n} lines")))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Invocation, Origin, Session};
    use doc::{Document, FillMode, Geometry};
    use serde_json::{Value, json};

    fn run(s: &mut Session, id: &str, params: Value) -> std::result::Result<Value, String> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result).map_err(|e| e.to_string())
    }

    #[test]
    fn material_test_grid_in_safe_order() {
        let mut s = Session::new(Document::default());
        let r = run(
            &mut s,
            "generate.material_test",
            json!({
                "rows": {"param": "frequency", "min": 20, "max": 200, "count": 4},
                "columns": {"param": "pulse_width", "min": 2, "max": 200, "count": 3},
                "base": {"power_pct": 50, "speed_mm_s": 800},
                "border": true, "title": "MOPA colours",
            }),
        )
        .unwrap();
        assert_eq!(r["boxes"], 12);
        let d = s.document();
        let cells: Vec<&doc::Layer> = d.layers.iter().filter(|l| l.name.starts_with("Frequency")).collect();
        assert_eq!(cells.len(), 12);
        for l in &cells {
            assert_eq!((l.settings.mode.clone(), l.settings.power_pct, l.settings.speed_mm_s), (FillMode::Fill, 50.0, 800.0));
        }
        // Frequency then Q-pulse ascending (neither has a LightBurn risk rule: lowest first).
        assert_eq!(cells[0].settings.frequency_khz, 20.0);
        assert_eq!(cells[0].settings.pulse_width_ns, Some(2.0));
        assert_eq!(cells[11].settings.frequency_khz, 200.0);
        assert!(d.layers.iter().any(|l| l.name == "Test labels") && d.layers.iter().any(|l| l.name == "Test border"));
        let texts = d.objects.iter().filter(|o| matches!(o.geometry, Geometry::Text(_))).count();
        assert_eq!(texts, 4 + 3 + 2 + 1, "row and column values, two axis titles, the title");
        let g = r["group"].as_str().unwrap();
        assert!(d.objects.iter().all(|o| o.group.as_deref() == Some(g)));
        // The default grid: speed x power, safest first (highest speed, lowest power).
        let mut t = Session::new(Document::default());
        run(&mut t, "generate.material_test", json!({ "labels": false })).unwrap();
        let first = &t.document().layers[1].settings;
        assert_eq!((first.speed_mm_s, first.power_pct), (5000.0, 10.0));
        let last = &t.document().layers[100].settings;
        assert_eq!((last.speed_mm_s, last.power_pct), (500.0, 100.0));
    }

    #[test]
    fn material_test_refusals() {
        let mut s = Session::new(Document::default());
        let same = json!({ "rows": {"param": "power", "min": 1, "max": 2}, "columns": {"param": "power", "min": 1, "max": 2} });
        assert!(run(&mut s, "generate.material_test", same).is_err());
        let over = json!({ "columns": {"param": "power", "min": 10, "max": 150, "count": 3} });
        assert!(run(&mut s, "generate.material_test", over).unwrap_err().contains("power_pct"));
        let many = json!({ "columns": {"param": "power", "min": 10, "max": 50, "count": 21} });
        assert!(run(&mut s, "generate.material_test", many).is_err());
        let zero_passes = json!({ "columns": {"param": "passes", "min": 0, "max": 3, "count": 4} });
        assert!(run(&mut s, "generate.material_test", zero_passes).unwrap_err().contains("passes"), "a box labelled 0 would run once");
        assert_eq!(s.document().layers.len(), 1, "nothing was added");
    }

    #[test]
    fn interval_test_simple_and_dithered() {
        let mut s = Session::new(Document::default());
        let r = run(&mut s, "generate.interval_test", json!({ "steps": 3, "min_interval_mm": 0.02, "max_interval_mm": 0.06 })).unwrap();
        assert_eq!(r["intervals_mm"], json!([0.02, 0.04, 0.06]));
        let d = s.document();
        let swatches: Vec<_> = d.layers.iter().filter(|l| l.name.starts_with("Interval")).collect();
        assert_eq!(swatches.len(), 3);
        assert_eq!(swatches[1].settings.hatch.interval_mm, 0.04);
        let mut t = Session::new(Document::default());
        run(&mut t, "generate.interval_test", json!({ "steps": 2, "fill": "dithered", "labels": false })).unwrap();
        let imgs = t.document().objects.iter().filter(|o| matches!(o.geometry, Geometry::Image { .. })).count();
        assert_eq!(imgs, 2);
        assert!(run(&mut t, "generate.interval_test", json!({ "steps": 1 })).is_err());
    }

    #[test]
    fn gradient_bmp_decodes() {
        let b = super::gradient_bmp(5, 2);
        assert_eq!(&b[..2], b"BM");
        assert_eq!(b.len(), 54 + 16 * 2, "rows padded to 4 bytes");
        assert_eq!(&b[54..57], &[0, 0, 0]);
        assert_eq!(&b[66..69], &[255, 255, 255]);
    }

    #[test]
    fn focus_test_steps_z() {
        let mut s = Session::new(Document::default());
        let r = run(&mut s, "generate.focus_test", json!({ "start_z_mm": -1, "end_z_mm": 1, "steps": 4 })).unwrap();
        assert_eq!(r["heights_mm"], json!([-1.0, -0.5, 0.0, 0.5, 1.0]));
        let d = s.document();
        let lines: Vec<_> = d.layers.iter().filter(|l| l.name.starts_with("Focus")).collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[1].settings.options.z_offset_mm, -0.5);
        assert!(run(&mut s, "generate.focus_test", json!({ "steps": 0 })).is_err());
    }
}
