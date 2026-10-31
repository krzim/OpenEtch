//! [`LbrnProject`] ⇄ OpenEtch document. See the crate docs for the frame and the full mapping table.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use doc::{CutSettings, Document, FillMode, Geometry, Layer, Object, StockShape};
use kurbo::{Affine, Rect};

use crate::LbrnError;
use crate::model::{CutParams, CutSetting, Element, LbrnProject, PathData, Shape, ShapeKind, SubLayer, TYPED_TAGS};
use crate::palette::{self, COLOR_LAYERS, T1};

/// Prefix of the [`Layer::extras`] keys this crate reads and writes.
pub const EXTRA_PREFIX: &str = "lbrn2.";
/// Most passes a layer may have (the `commands` and `plan` limit).
pub const MAX_PASSES: u32 = 1000;
/// Largest field size accepted, in mm.
const MAX_FIELD_MM: f64 = 2000.0;

/// Layers, objects and warnings from [`to_document`]. Ids are those of a fresh document (`layer-1`.. then `obj-n`..),
/// so callers merging into an existing document re-id them.
pub type Imported = (Vec<Layer>, Vec<Object>, Vec<String>);

/// Warnings with repeats counted, in first-seen order.
#[derive(Default)]
struct Warnings(Vec<(String, usize)>);

impl Warnings {
    fn push(&mut self, m: impl Into<String>) {
        let m = m.into();
        match self.0.iter_mut().find(|(s, _)| *s == m) {
            Some((_, n)) => *n += 1,
            None => self.0.push((m, 1)),
        }
    }

    fn into_vec(self) -> Vec<String> {
        self.0.into_iter().map(|(m, n)| if n == 1 { m } else { format!("{m} (x{n})") }).collect()
    }
}

fn check_field(field_mm: f64) -> Result<f64, LbrnError> {
    if field_mm.is_finite() && field_mm > 0.0 && field_mm <= MAX_FIELD_MM {
        Ok(field_mm)
    } else {
        Err(LbrnError::Invalid(format!("field_mm must be within 0..={MAX_FIELD_MM} mm (got {field_mm})")))
    }
}

/// OpenEtch fill mode for a LightBurn cut type, if it has a direct equivalent.
fn mode_of(kind: &str) -> Option<FillMode> {
    match kind {
        "Cut" => Some(FillMode::Line),
        "Scan" => Some(FillMode::Fill),
        "Scan+Cut" => Some(FillMode::FillThenLine),
        _ => None,
    }
}

/// The mode an imported layer of `kind` gets (also used to decide whether an exported layer keeps its kind).
fn imported_mode(kind: &str) -> FillMode {
    mode_of(kind).unwrap_or(if kind == "Image" { FillMode::Fill } else { FillMode::Line })
}

fn kind_of(mode: &FillMode) -> &'static str {
    match mode {
        FillMode::Line => "Cut",
        FillMode::Fill => "Scan",
        FillMode::FillThenLine => "Scan+Cut",
    }
}

fn is_tool(index: u32, kind: &str) -> bool {
    index >= COLOR_LAYERS || kind == "Tool"
}

/// LightBurn parameters → OpenEtch settings. Returns the settings and the `tag → value` pairs they do not use.
fn to_settings(kind: &str, p: &CutParams, label: &str, w: &mut Warnings) -> (CutSettings, BTreeMap<String, String>) {
    let d = CutSettings::default();
    let mut rest: BTreeMap<String, String> = p.to_values().into_iter().collect();
    let mapped = ["maxPower", "speed", "frequency", "QPulseWidth", "interval", "angle", "anglePerPass", "crossHatch"];
    for tag in mapped.into_iter().chain(["bidir", "numPasses"]) {
        rest.remove(tag);
    }
    for tag in TYPED_TAGS.iter().filter(|t| p.other.contains_key(**t)) {
        w.push(format!("{label}: {tag} value '{}' could not be read and was ignored", p.other[*tag]));
        rest.remove(*tag);
    }
    let mode = match mode_of(kind) {
        Some(m) => m,
        None if kind == "Image" => {
            w.push(format!("{label}: image (bitmap) settings were imported as a fill layer"));
            FillMode::Fill
        }
        None => {
            w.push(format!("{label}: cut type '{kind}' is not supported; imported as a line layer"));
            FillMode::Line
        }
    };
    let power_pct = match p.max_power {
        Some(v) if (0.0..=100.0).contains(&v) => v,
        Some(v) => {
            w.push(format!("{label}: power {v} % clamped to 0..=100"));
            v.clamp(0.0, 100.0)
        }
        None => {
            w.push(format!("{label}: no power (maxPower) in the file; using {} %", d.power_pct));
            d.power_pct
        }
    };
    let speed_mm_s = match p.speed {
        Some(v) if v > 0.0 => v,
        other => {
            w.push(format!("{label}: speed {} is not usable; using {} mm/s", show(other), d.speed_mm_s));
            d.speed_mm_s
        }
    };
    let frequency_khz = match p.frequency {
        Some(v) if v > 0.0 => v / 1000.0,
        None => {
            w.push(format!("{label}: no frequency in the file; using {} kHz", d.frequency_khz));
            d.frequency_khz
        }
        Some(v) => {
            w.push(format!("{label}: frequency {v} Hz is not usable; using {} kHz", d.frequency_khz));
            d.frequency_khz
        }
    };
    let pulse_width_ns = match p.q_pulse_width {
        Some(v) if v > 0.0 => Some(v),
        Some(v) => {
            w.push(format!("{label}: pulse width {v} ns ignored"));
            None
        }
        None => None,
    };
    let passes = match p.num_passes {
        Some(n) if (1..=MAX_PASSES).contains(&n) => n,
        Some(n) => {
            let c = n.clamp(1, MAX_PASSES);
            w.push(format!("{label}: {n} passes clamped to {c}"));
            c
        }
        None => 1,
    };
    let interval_mm = match p.interval {
        Some(v) if v > 0.0 => v,
        Some(v) => {
            w.push(format!("{label}: hatch interval {v} mm is not usable; using {} mm", d.hatch.interval_mm));
            d.hatch.interval_mm
        }
        None => d.hatch.interval_mm,
    };
    let s = CutSettings {
        mode,
        power_pct,
        speed_mm_s,
        frequency_khz,
        pulse_width_ns,
        passes,
        hatch: doc::Hatch {
            interval_mm,
            angle_deg: p.angle.unwrap_or(0.0),
            angle_increment_deg: p.angle_per_pass.unwrap_or(0.0),
            cross_hatch: p.cross_hatch.unwrap_or(false),
            bidirectional: p.bidir.unwrap_or(true),
        },
        timing: None,
    };
    (s, rest)
}

fn show(v: Option<f64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "(missing)".into())
}

/// One LightBurn layer (its vector settings, or image settings when that is all there is) → an OpenEtch layer.
fn layer_from(index: u32, cs: Option<&CutSetting>, id: String, w: &mut Warnings) -> Layer {
    let label = palette::default_name(index);
    let mut layer = Layer::new(id, &label, palette::color(index));
    layer.extras.insert(format!("{EXTRA_PREFIX}index"), index.to_string());
    let Some(cs) = cs else {
        if index < COLOR_LAYERS {
            w.push(format!("{label}: used by shapes but has no cut settings in the file; default settings used"));
        } else {
            layer.output = false;
        }
        return layer;
    };
    let p = &cs.params;
    let named = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(n) = named(&p.name).or_else(|| named(&p.subname)) {
        layer.name = n.chars().take(200).collect();
    }
    layer.visible = !p.hide.unwrap_or(false);
    layer.output = p.do_output.unwrap_or(true);
    if mode_of(&cs.kind).is_none() {
        layer.extras.insert(format!("{EXTRA_PREFIX}type"), cs.kind.clone());
    }
    if cs.image {
        layer.extras.insert(format!("{EXTRA_PREFIX}image"), "1".into());
    }
    if is_tool(index, &cs.kind) {
        // Tool layers (T1/T2) carry no laser settings and never output.
        layer.output = false;
        for (tag, value) in p.to_values() {
            if !matches!(tag.as_str(), "name" | "priority" | "hide") {
                layer.extras.insert(format!("{EXTRA_PREFIX}{tag}"), value);
            }
        }
        return layer;
    }
    let (settings, rest) = to_settings(&cs.kind, p, &label, w);
    layer.settings = settings;
    for (tag, value) in rest {
        if !matches!(tag.as_str(), "name" | "priority" | "doOutput" | "hide") {
            layer.extras.insert(format!("{EXTRA_PREFIX}{tag}"), value);
        }
    }
    for (n, sub) in cs.sub_layers.iter().enumerate() {
        let key = format!("{EXTRA_PREFIX}sub{}.", n + 1);
        let (s, rest) = to_settings(&sub.kind, &sub.params, &format!("{label} sub-layer {}", n + 1), w);
        layer.sub_layers.push(s);
        if mode_of(&sub.kind).is_none() {
            layer.extras.insert(format!("{key}type"), sub.kind.clone());
        }
        for (tag, value) in rest {
            layer.extras.insert(format!("{key}{tag}"), value);
        }
    }
    layer
}

/// Convert one LightBurn `CutSetting` (e.g. a `.clb` library entry) to an OpenEtch layer with id `id`.
pub fn cut_setting_to_layer(cs: &CutSetting, id: &str) -> (Layer, Vec<String>) {
    let mut w = Warnings::default();
    let layer = layer_from(cs.index, Some(cs), id.to_string(), &mut w);
    (layer, w.into_vec())
}

fn leaf_indices(s: &Shape, out: &mut BTreeSet<u32>) {
    match &s.kind {
        ShapeKind::Group(children) => children.iter().for_each(|c| leaf_indices(c, out)),
        ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Path(_) => {
            out.insert(s.cut_index.unwrap_or(0));
        }
        ShapeKind::Text { backup: Some(_) } => {
            out.insert(s.cut_index.unwrap_or(0));
        }
        ShapeKind::Text { backup: None } | ShapeKind::Bitmap { .. } | ShapeKind::Other { .. } => {}
    }
}

/// Convert a parsed project to OpenEtch layers and objects in the field frame. `field_mm` is the galvo field size the
/// project was laid out for: LightBurn coordinates run 0..field_mm from the field's lower-left corner, so every
/// position moves by `-field_mm / 2`.
pub fn to_document(p: &LbrnProject, field_mm: f64) -> Result<Imported, LbrnError> {
    let field = check_field(field_mm)?;
    let mut w = Warnings::default();
    for key in ["MirrorX", "MirrorY"] {
        if let Some(v) = p.attribute(key).filter(|v| !matches!(*v, "False" | "false" | "0")) {
            w.push(format!("project {key}={v} was not applied"));
        }
    }

    // Layers: every index with settings or used by an imported shape, in LightBurn's run order (priority).
    let mut indices: BTreeSet<u32> = p.cut_settings.iter().map(|c| c.index).collect();
    for s in &p.shapes {
        leaf_indices(s, &mut indices);
    }
    let setting_for = |i: u32| -> Option<&CutSetting> {
        let mut of_index = p.cut_settings.iter().filter(move |c| c.index == i);
        of_index.clone().find(|c| !c.image).or_else(|| of_index.next())
    };
    let mut order: Vec<u32> = indices.into_iter().collect();
    order.sort_by_key(|i| (setting_for(*i).and_then(|c| c.params.priority).unwrap_or(i64::MAX), *i));
    let mut next = 1u64;
    let mut layers = Vec::new();
    let mut layer_ids = HashMap::new();
    for i in order {
        if p.cut_settings.iter().any(|c| c.index == i && c.image)
            && p.cut_settings.iter().any(|c| c.index == i && !c.image)
        {
            w.push(format!("{}: its image (bitmap) settings were ignored", palette::default_name(i)));
        }
        let id = format!("layer-{next}");
        next += 1;
        layer_ids.insert(i, id.clone());
        layers.push(layer_from(i, setting_for(i), id, &mut w));
    }
    if layers.is_empty() {
        let id = format!("layer-{next}");
        next += 1;
        layers.push(layer_from(0, None, id, &mut Warnings::default()));
        layer_ids.insert(0, layers[0].id.clone());
    }

    let mut cx = ToDoc {
        shift: Affine::translate((-field / 2.0, -field / 2.0)),
        layer_ids,
        fallback: layers[0].id.clone(),
        next,
        objects: Vec::new(),
        w,
    };
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for s in &p.shapes {
        let kind = match s.kind {
            ShapeKind::Rect { .. } => "Rectangle",
            ShapeKind::Ellipse { .. } => "Ellipse",
            ShapeKind::Group(_) => "Group",
            ShapeKind::Text { .. } => "Text",
            _ => "Path",
        };
        let n = counts.entry(kind).or_default();
        *n += 1;
        let text = s.attribute("Str").and_then(|t| t.lines().next()).map(str::trim).unwrap_or_default();
        let name = match s.kind {
            ShapeKind::Text { .. } if !text.is_empty() => text.chars().take(100).collect(),
            _ => format!("{kind} {n}"),
        };
        cx.shape(s, Affine::IDENTITY, name);
    }
    let ToDoc { objects, mut w, .. } = cx;
    let outside = objects.iter().filter(|o| !inside(o.bounds(), field)).count();
    if outside > 0 {
        w.push(format!(
            "{outside} object(s) are partly or wholly outside the {field} mm field; LightBurn files do not record \
             the field size, so check that field_mm matches the one the project was laid out for"
        ));
    }
    Ok((layers, objects, w.into_vec()))
}

fn inside(b: Rect, field: f64) -> bool {
    let h = field / 2.0;
    b.x0 >= -h && b.y0 >= -h && b.x1 <= h && b.y1 <= h
}

struct ToDoc {
    shift: Affine,
    layer_ids: HashMap<u32, String>,
    fallback: String,
    next: u64,
    objects: Vec<Object>,
    w: Warnings,
}

impl ToDoc {
    fn push(&mut self, s: &Shape, name: String, geometry: Geometry, local: Affine) {
        let index = s.cut_index.unwrap_or(0);
        let layer = self.layer_ids.get(&index).cloned().unwrap_or_else(|| self.fallback.clone());
        let id = format!("obj-{}", self.next);
        self.next += 1;
        self.objects.push(Object { id, name, layer, geometry, transform: self.shift * local });
    }

    fn shape(&mut self, s: &Shape, parent: Affine, name: String) {
        let xf = parent * s.xform;
        match &s.kind {
            ShapeKind::Rect { w, h, cr } => {
                if !(*w > 0.0 && *h > 0.0) {
                    self.w.push("a zero-size rectangle was skipped");
                    return;
                }
                let max = w.min(*h) / 2.0;
                let r = cr.clamp(0.0, max);
                if r != *cr {
                    self.w.push(format!("a rectangle's corner radius {cr} mm was clamped to {r} mm"));
                }
                self.push(s, name, Geometry::Rect { width_mm: *w, height_mm: *h, corner_radius_mm: r }, xf);
            }
            ShapeKind::Ellipse { rx, ry } => {
                if !(*rx > 0.0 && *ry > 0.0) {
                    self.w.push("a zero-size ellipse was skipped");
                    return;
                }
                self.push(s, name, Geometry::Ellipse { rx_mm: *rx, ry_mm: *ry }, xf);
            }
            ShapeKind::Path(d) => self.path(s, name, d, xf),
            ShapeKind::Group(children) => {
                for (k, c) in children.iter().enumerate() {
                    self.shape(c, xf, format!("{name}.{}", k + 1));
                }
            }
            ShapeKind::Text { backup } => {
                match backup {
                    // The Text's own XForm is the live one; the backup's copy can be stale (see crate docs).
                    Some(b) => {
                        self.w.push("text was imported as its outline (BackupPath) and is no longer editable text");
                        self.path(s, name, &b.path, xf);
                    }
                    None => self.w.push(format!(
                        "text \"{name}\" was skipped: it has no BackupPath outline (convert it to a path in LightBurn)"
                    )),
                }
            }
            ShapeKind::Bitmap { .. } => self.w.push("a bitmap was skipped: OpenEtch does not engrave images yet"),
            ShapeKind::Other { type_name } => self.w.push(format!("a '{type_name}' shape was skipped (unsupported)")),
        }
    }

    fn path(&mut self, s: &Shape, name: String, d: &PathData, xf: Affine) {
        let path = d.to_bez();
        if path.segments().next().is_none() {
            self.w.push("an empty path was skipped");
            return;
        }
        self.push(s, name, Geometry::Path { path }, xf);
    }
}

fn extra<'a>(l: &'a Layer, key: &str) -> Option<&'a str> {
    l.extras.get(&format!("{EXTRA_PREFIX}{key}")).map(String::as_str)
}

/// OpenEtch settings → LightBurn `tag → value` pairs, starting from the kept extras.
fn settings_values(s: &CutSettings, base: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut v = base;
    let mut set = |k: &str, x: String| {
        v.insert(k.to_string(), x);
    };
    set("maxPower", s.power_pct.to_string());
    set("speed", s.speed_mm_s.to_string());
    let hz = s.frequency_khz * 1000.0;
    set("frequency", if (hz - hz.round()).abs() < 1e-6 { hz.round() } else { hz }.to_string());
    set("interval", s.hatch.interval_mm.to_string());
    set("angle", s.hatch.angle_deg.to_string());
    set("crossHatch", if s.hatch.cross_hatch { "1" } else { "0" }.into());
    set("bidir", if s.hatch.bidirectional { "1" } else { "0" }.into());
    set("numPasses", s.passes.to_string());
    match s.pulse_width_ns {
        Some(pw) => set("QPulseWidth", pw.to_string()),
        None => {
            v.remove("QPulseWidth");
        }
    }
    if s.hatch.angle_increment_deg != 0.0 {
        v.insert("anglePerPass".into(), s.hatch.angle_increment_deg.to_string());
    } else {
        v.remove("anglePerPass");
    }
    v
}

/// The kind an exported layer is written with: its original LightBurn kind when the mode still matches it.
fn export_kind(original: Option<&str>, mode: &FillMode) -> String {
    match original {
        Some(k) if imported_mode(k) == *mode && k != "Tool" => k.to_string(),
        _ => kind_of(mode).to_string(),
    }
}

/// Convert a document to a LightBurn project laid out for a `field_mm` field (positions move by `+field_mm / 2`).
/// Layers keep their LightBurn index when they have one (`lbrn2.index`), else take the palette index of their colour,
/// else the lowest free one; more than 30 output layers is an error. The stock outline goes on tool layer T1 (T2 if a
/// layer already uses T1).
pub fn from_document(d: &Document, field_mm: f64) -> Result<(LbrnProject, Vec<String>), LbrnError> {
    let field = check_field(field_mm)?;
    let mut w = Warnings::default();

    // Assign indices: kept index, then palette colour, then the lowest free colour index.
    let mut taken = BTreeSet::new();
    let mut index_of: HashMap<&str, u32> = HashMap::new();
    for l in &d.layers {
        let kept = extra(l, "index").and_then(|i| i.parse::<u32>().ok());
        if let Some(i) = kept.filter(|i| *i < COLOR_LAYERS || ((*i == T1 || *i == T1 + 1) && !l.output))
            && taken.insert(i)
        {
            index_of.insert(&l.id, i);
        }
    }
    for l in &d.layers {
        if index_of.contains_key(l.id.as_str()) {
            continue;
        }
        let i = palette::index_of_color(&l.color)
            .filter(|i| !taken.contains(i))
            .or_else(|| (0..COLOR_LAYERS).find(|i| !taken.contains(i)))
            .ok_or_else(|| {
                LbrnError::Invalid(format!("the document has more layers than LightBurn's {COLOR_LAYERS}"))
            })?;
        taken.insert(i);
        index_of.insert(&l.id, i);
    }

    let mut cut_settings = Vec::new();
    for (priority, l) in d.layers.iter().enumerate() {
        let index = index_of[l.id.as_str()];
        let original = extra(l, "type");
        let tool = index >= COLOR_LAYERS || original == Some("Tool");
        // Kept LightBurn values, minus the keys this function owns.
        let mut base = BTreeMap::new();
        for (k, v) in &l.extras {
            if let Some(tag) = k.strip_prefix(EXTRA_PREFIX)
                && !tag.starts_with("sub")
                && !matches!(tag, "index" | "type" | "image")
            {
                base.insert(tag.to_string(), v.clone());
            }
        }
        let mut values = if tool { base } else { settings_values(&l.settings, base) };
        values.insert("name".into(), l.name.clone());
        values.insert("priority".into(), priority.to_string());
        if !l.output && !tool {
            values.insert("doOutput".into(), "0".into());
        }
        if !l.visible {
            values.insert("hide".into(), "1".into());
        }
        let kind = if tool { "Tool".to_string() } else { export_kind(original, &l.settings.mode) };
        let mut sub_layers = Vec::new();
        if !tool {
            for (n, s) in l.sub_layers.iter().enumerate() {
                let key = format!("{EXTRA_PREFIX}sub{}.", n + 1);
                let mut base = BTreeMap::new();
                let mut sub_kind = None;
                for (k, v) in &l.extras {
                    match k.strip_prefix(&key) {
                        Some("type") => sub_kind = Some(v.as_str()),
                        Some(tag) => {
                            base.insert(tag.to_string(), v.clone());
                        }
                        None => {}
                    }
                }
                let values = settings_values(s, base);
                sub_layers.push(SubLayer {
                    kind: export_kind(sub_kind, &s.mode),
                    index: n as u32 + 1,
                    params: CutParams::from_values(values.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
                });
            }
        } else if !l.sub_layers.is_empty() {
            w.push(format!("layer \"{}\" is a LightBurn tool layer; its sub-layers were not exported", l.name));
        }
        cut_settings.push(CutSetting {
            image: kind == "Image" && extra(l, "image") == Some("1"),
            kind,
            index,
            params: CutParams::from_values(values.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            sub_layers,
        });
    }

    let shift = Affine::translate((field / 2.0, field / 2.0));
    let mut shapes = Vec::new();
    let mut outside = 0;
    for o in &d.objects {
        let Some(&index) = index_of.get(o.layer.as_str()) else {
            w.push(format!("object {} (\"{}\") is on a missing layer and was not exported", o.id, o.name));
            continue;
        };
        let kind = match &o.geometry {
            Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                ShapeKind::Rect { w: *width_mm, h: *height_mm, cr: *corner_radius_mm }
            }
            Geometry::Ellipse { rx_mm, ry_mm } => ShapeKind::Ellipse { rx: *rx_mm, ry: *ry_mm },
            Geometry::Path { path } => {
                let data = PathData::from_bez(path);
                if data.vertices.is_empty() {
                    w.push(format!("object {} (\"{}\") is an empty path and was not exported", o.id, o.name));
                    continue;
                }
                ShapeKind::Path(data)
            }
        };
        if !inside(o.bounds(), field) {
            outside += 1;
        }
        shapes.push(shape(kind, Some(index), shift * o.transform));
    }
    if outside > 0 {
        w.push(format!("{outside} object(s) are partly or wholly outside the {field} mm field"));
    }

    if let Some(stock) = &d.stock {
        match [T1, T1 + 1].into_iter().find(|i| !taken.contains(i)) {
            Some(index) => {
                let kind = match stock.shape {
                    StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                        ShapeKind::Rect { w: width_mm, h: height_mm, cr: corner_radius_mm }
                    }
                    StockShape::Circle { diameter_mm } => {
                        ShapeKind::Ellipse { rx: diameter_mm / 2.0, ry: diameter_mm / 2.0 }
                    }
                };
                shapes.push(shape(kind, Some(index), shift * stock.transform()));
                let values = [("name", palette::default_name(index)), ("priority", cut_settings.len().to_string())];
                cut_settings.push(CutSetting {
                    kind: "Tool".into(),
                    image: false,
                    index,
                    params: CutParams::from_values(values.iter().map(|(k, v)| (*k, v.as_str()))),
                    sub_layers: vec![],
                });
            }
            None => w.push("the stock outline was not exported: both tool layers (T1, T2) are in use"),
        }
    }

    let attributes = [
        ("AppVersion", "1.4.00"),
        ("FormatVersion", "1"),
        ("MaterialHeight", "0"),
        ("MirrorX", "False"),
        ("MirrorY", "False"),
    ]
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .to_vec();
    let value = |name: &str, v: &str| Element {
        name: name.into(),
        attributes: vec![("Value".into(), v.into())],
        text: None,
        children: vec![],
    };
    let variable_text = Element {
        name: "VariableText".into(),
        attributes: vec![],
        text: None,
        children: vec![
            value("Start", "0"),
            value("End", "999"),
            value("Current", "0"),
            value("Increment", "1"),
            value("AutoAdvance", "0"),
        ],
    };
    let notes = Element {
        name: "Notes".into(),
        attributes: vec![("ShowOnLoad".into(), "0".into()), ("Notes".into(), String::new())],
        text: None,
        children: vec![],
    };
    let project = LbrnProject { attributes, cut_settings, shapes, extra_elements: vec![variable_text, notes] };
    Ok((project, w.into_vec()))
}

fn shape(kind: ShapeKind, cut_index: Option<u32>, xform: Affine) -> Shape {
    Shape { kind, cut_index, xform, attributes: vec![], children: vec![] }
}
