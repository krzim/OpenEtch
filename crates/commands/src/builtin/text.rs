//! Text, variable text and barcodes (LightBurn's Text tool, Variable Text window and Create Bar Code), rendered by
//! the `text` crate. A text object keeps its settings and its rendered outline ([`doc::Text`]); every edit renders it
//! again, so planning and exports only ever read the outline. Commands: `shape.text`, `text.update`,
//! `text.apply_path`, `text.detach_path`, `text.convert_to_path`, `text.refresh`, `var.set_serial`, `var.advance`,
//! `var.load_csv`, `var.clear_csv`, `var.preview_row`, `shape.barcode`, `font.list`.

use doc::{AlignH, AlignV, Document, FontKind, FontSpec, Geometry, ObjectId, Text, TextPath, VarMode, VarText};
use kurbo::Affine;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::shape::add_object;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Object extras key holding a barcode's parameters (JSON of [`BarcodeInput`]), so it can be edited and follow
/// variable text.
pub const BARCODE_KEY: &str = "openetch.barcode";
/// Longest text accepted, in characters.
const MAX_TEXT: usize = 10_000;
/// Most cells a loaded CSV file may have (the rows live in the document, which every edit copies for undo).
const MAX_CSV_CELLS: usize = 1_000_000;

/// The moment date/time variable text shows: now on native builds, none (the format is shown) in the browser.
pub fn now() -> Option<text::Moment> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(text::Moment::now())
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// The device name the cut-setting `L` code shows: the LightBurn device the document came from, if any.
fn laser_name(doc: &Document) -> String {
    doc.extras.get("lbrn2._root_attrs").and_then(|r| super::lbrn2_device_name(r)).unwrap_or_default()
}

/// A barcode object's kept settings.
fn barcode_params(o: &doc::Object) -> Option<BarcodeInput> {
    serde_json::from_str(o.extras.get(BARCODE_KEY)?).ok()
}

/// Render text object `id` again (its settings, the document's variables, its path) and store the outline.
fn rerender(cmd: &str, doc: &mut Document, id: &str, moment: Option<&text::Moment>) -> Result<Vec<String>> {
    let laser = laser_name(doc);
    let o = util::object(doc, id)?;
    let r = text::render_object(doc, o, moment, &laser).map_err(|e| CommandError::Failed(format!("{cmd}: {id}: {e}")))?;
    let o = doc.object_mut(id).expect("looked up above");
    if let Geometry::Text(t) = &mut o.geometry {
        t.shown = (r.shown != t.text).then(|| r.shown.clone());
        t.outline = r.outline;
    }
    Ok(r.warnings)
}

/// Draw barcode `id` again from its kept parameters (after its variable text changed).
fn redraw_barcode(cmd: &str, doc: &mut Document, id: &str, moment: Option<&text::Moment>) -> Result<()> {
    let o = util::object(doc, id)?;
    let Some(p) = o.extras.get(BARCODE_KEY) else { return Ok(()) };
    let input: BarcodeInput =
        serde_json::from_str(p).map_err(|e| CommandError::Failed(format!("{cmd}: {id}: barcode settings: {e}")))?;
    let data = barcode_data(doc, &o.layer.clone(), &input, moment);
    let drawn = draw_barcode(cmd, &input, &data)?;
    let o = doc.object_mut(id).expect("looked up above");
    o.geometry = Geometry::Path { path: drawn.path };
    Ok(())
}

/// Every variable text and variable barcode of `doc` rendered again for `moment`. Returns the ids touched. Plain
/// texts (on a path or not) keep the outline the canvas shows: re-rendering a text on a path here would snap it back
/// onto its path after the text was moved, or fail once the path was deleted, so a run would burn something other
/// than the preview (`text.refresh` follows an edited path on request).
pub fn refresh_variables(cmd: &str, doc: &mut Document, moment: Option<&text::Moment>) -> Result<Vec<ObjectId>> {
    let ids: Vec<(ObjectId, bool)> = doc
        .objects
        .iter()
        .filter_map(|o| match &o.geometry {
            Geometry::Text(t) if t.variable.is_some() => Some((o.id.clone(), true)),
            _ if barcode_params(o).is_some_and(|b| b.variable.as_ref().and_then(var_of).is_some()) => {
                Some((o.id.clone(), false))
            }
            _ => None,
        })
        .collect();
    for (id, is_text) in &ids {
        if *is_text {
            rerender(cmd, doc, id, moment)?;
        } else {
            redraw_barcode(cmd, doc, id, moment)?;
        }
    }
    Ok(ids.into_iter().map(|(id, _)| id).collect())
}

/// The document as a job should burn it now: variable text (serial numbers, CSV rows, cut settings, the date and
/// time) rendered again; everything else burns as shown. The engine plans runs from this, so a date shows the time
/// of the run and a serial number the current counter even if the canvas was not refreshed. Fails when a text cannot be rendered
/// (its font is missing), rather than burn a stale value.
pub fn prepare_output(doc: &Document) -> std::result::Result<Document, String> {
    let mut out = doc.clone();
    refresh_variables("run", &mut out, now().as_ref()).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Object extras key on a copy made by a grid array with Auto-Increment: the data its barcode was drawn with, so a
/// virtual array's relayout can tell whether it must draw it again.
const BARCODE_SHOWN_KEY: &str = "openetch.barcode_shown";

/// Raise the variable-text Offset of `o` (a copy not yet in the document: a serial-number or CSV text or barcode) by
/// `delta` and draw it again: LightBurn's Grid Array "Auto-Increment Variable Text". `prev` is what the copy was
/// before (a virtual array's clone laid out again): when it already shows exactly this, its drawing is reused and no
/// font is loaded. Other objects, and texts on a path (their outline follows the path object, not the copy), are
/// left alone. Returns whether `o` changed.
pub(crate) fn bump_copy(cmd: &str, doc: &Document, o: &mut doc::Object, delta: i64, prev: Option<&doc::Object>) -> Result<bool> {
    if delta == 0 {
        return Ok(false);
    }
    let counts = |m: VarMode| matches!(m, VarMode::Serial | VarMode::Csv);
    let laser = laser_name(doc);
    if let Geometry::Text(t) = &o.geometry {
        if t.path.is_some() || !t.variable.as_ref().is_some_and(|v| counts(v.mode)) {
            return Ok(false);
        }
        let mut nt = (**t).clone();
        if let Some(v) = &mut nt.variable {
            v.offset = v.offset.saturating_add(delta);
        }
        let shown = text::shown_text(doc, o, &nt, None, &laser);
        let bare = |t: &Text| Text { outline: kurbo::BezPath::new(), shown: None, ..t.clone() };
        let reuse = prev.and_then(|p| match &p.geometry {
            Geometry::Text(pt) if bare(pt) == bare(&nt) && pt.shown.as_deref().unwrap_or(&pt.text) == shown => {
                Some((pt.outline.clone(), pt.shown.clone()))
            }
            _ => None,
        });
        match reuse {
            Some((outline, s)) => {
                nt.outline = outline;
                nt.shown = s;
            }
            None => {
                let r = text::render_text(&nt, &shown, None).map_err(|e| CommandError::Failed(format!("{cmd}: {}: {e}", o.id)))?;
                nt.shown = (shown != nt.text).then_some(shown);
                nt.outline = r.outline;
            }
        }
        o.geometry = Geometry::Text(Box::new(nt));
        return Ok(true);
    }
    let Some(mut b) = barcode_params(o) else { return Ok(false) };
    let Some(v) = b.variable.as_mut().filter(|v| var_of(v).is_some_and(|v| counts(v.mode))) else { return Ok(false) };
    v.offset = v.offset.saturating_add(delta);
    let params = serde_json::to_string(&b).map_err(|e| CommandError::Failed(format!("{cmd}: {}: {e}", o.id)))?;
    let data = barcode_data(doc, &o.layer, &b, None);
    let same = prev.filter(|p| {
        p.extras.get(BARCODE_KEY) == Some(&params) && p.extras.get(BARCODE_SHOWN_KEY) == Some(&data)
    });
    o.geometry = match same {
        Some(p) => p.geometry.clone(),
        None => Geometry::Path { path: draw_barcode(cmd, &b, &data)?.path },
    };
    o.extras.insert(BARCODE_KEY.into(), params);
    o.extras.insert(BARCODE_SHOWN_KEY.into(), data);
    Ok(true)
}

/// Whether a finished run should advance the serial number / CSV row (`var.advance {auto: true}`): auto-advance is on
/// and some text or barcode uses it.
pub fn auto_advance_due(doc: &Document) -> bool {
    doc.variables.auto_advance && doc.objects.iter().any(|o| match &o.geometry {
        Geometry::Text(t) => t.variable.as_ref().is_some_and(|v| matches!(v.mode, VarMode::Serial | VarMode::Csv)),
        _ => barcode_params(o)
            .and_then(|b| b.variable.as_ref().and_then(var_of))
            .is_some_and(|v| matches!(v.mode, VarMode::Serial | VarMode::Csv)),
    })
}

// ---------------------------------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontInput {
    /// Family name, e.g. "Arial"; for `.shx` / `.jsf` fonts the file name without extension. `font.list` lists them.
    pub family: String,
    /// `system` (installed TrueType/OpenType, default), `stroke` (bundled single-line Hershey fonts), `shx`, `jsf`.
    #[serde(default)]
    pub kind: FontKind,
}

/// LightBurn's Text Mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModeInput {
    /// Plain text (no variable text).
    Normal,
    DateTime,
    Serial,
    Csv,
    CutSetting,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VarInput {
    pub mode: ModeInput,
    /// Added to the current serial number / CSV row for this object (LightBurn's Offset). Default 0.
    #[serde(default)]
    pub offset: i64,
}

fn var_of(v: &VarInput) -> Option<VarText> {
    let mode = match v.mode {
        ModeInput::Normal => return None,
        ModeInput::DateTime => VarMode::DateTime,
        ModeInput::Serial => VarMode::Serial,
        ModeInput::Csv => VarMode::Csv,
        ModeInput::CutSetting => VarMode::CutSetting,
    };
    Some(VarText { mode, offset: v.offset })
}

/// Text settings, all optional (`shape.text` fills in LightBurn's defaults; `text.update` changes only what is given).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct TextFields {
    /// The text; `\n` starts a new line. With variable text, its format (see `variable`).
    pub text: Option<String>,
    pub font: Option<FontInput>,
    /// Font size in mm (LightBurn's Height: capitals are about 0.7 of it). Default 5.
    pub height_mm: Option<f64>,
    /// HSpace: extra space between characters, % of the height (negative tightens). Default 0.
    pub h_spacing: Option<f64>,
    /// VSpace: extra space between lines, % of the line height. Default 0.
    pub v_spacing: Option<f64>,
    /// Align X: left, middle (default), right. The object's position is this point of each line.
    pub align_h: Option<AlignH>,
    /// Align Y: top (capitals' tops), middle (default), bottom (last baseline).
    pub align_v: Option<AlignV>,
    /// Bold face (if the family has one).
    pub bold: Option<bool>,
    /// Italic face (slanted when the family has none, and for single-line fonts).
    pub italic: Option<bool>,
    /// Render in upper case.
    pub upper: Option<bool>,
    /// Merge overlapping glyphs (LightBurn's Welded). Default true.
    pub welded: Option<bool>,
    /// Bend: degrees the widest line spans on a circle (positive arches up, negative down, 0 straight).
    pub bend_deg: Option<f64>,
    /// With bend: warp the glyphs (true) or keep them rigid (false, default).
    pub distort: Option<bool>,
    /// Max Width in mm (0 = none): wider lines are scaled to fit.
    pub max_width_mm: Option<f64>,
    /// With max width: scale only horizontally.
    pub squeeze: Option<bool>,
    /// Drop lines whose variable text is empty.
    pub ignore_empty_vars: Option<bool>,
    /// Variable text: `{mode: normal|date_time|serial|csv|cut_setting, offset?}`. Formats follow LightBurn: serial
    /// `d`/`0dddd`/`h`/`H`; date `yyyy-MM-dd HH:mm`; CSV `%0`, `%1`; cut settings `p` `P` `s` `S` `k` `K` `q` `Q` `n`
    /// `i` `d` `C03p`; single quotes copy text as it is.
    pub variable: Option<VarInput>,
    /// Distance along the path in mm, for text on a path (`text.apply_path`).
    pub path_offset_mm: Option<f64>,
}

fn check_text(cmd: &str, s: &str) -> Result<()> {
    if s.chars().count() > MAX_TEXT {
        return Err(invalid(cmd, format!("text is longer than {MAX_TEXT} characters")));
    }
    if s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
        return Err(invalid(cmd, "text must not contain control characters other than line breaks and tabs"));
    }
    Ok(())
}

impl TextFields {
    /// Apply the given fields to `t`.
    fn apply_to(&self, cmd: &str, t: &mut Text) -> Result<()> {
        if let Some(s) = &self.text {
            check_text(cmd, s)?;
            t.text = s.replace("\r\n", "\n");
        }
        if let Some(f) = &self.font {
            let family = util::name(cmd, "font.family", &f.family)?;
            t.font = FontSpec { family, kind: f.kind };
        }
        let num = |field: &str, v: Option<f64>| v.map(|v| util::finite(cmd, field, v)).transpose();
        if let Some(v) = num("height_mm", self.height_mm)? {
            t.height_mm = v;
        }
        if let Some(v) = num("h_spacing", self.h_spacing)? {
            t.h_spacing = v;
        }
        if let Some(v) = num("v_spacing", self.v_spacing)? {
            t.v_spacing = v;
        }
        if let Some(v) = num("bend_deg", self.bend_deg)? {
            t.bend_deg = v;
        }
        if let Some(v) = num("max_width_mm", self.max_width_mm)? {
            t.max_width_mm = v;
        }
        if let Some(v) = num("path_offset_mm", self.path_offset_mm)? {
            match &mut t.path {
                Some(p) => p.offset_mm = v,
                None => return Err(invalid(cmd, "path_offset_mm needs a text on a path (text.apply_path)")),
            }
        }
        t.align_h = self.align_h.unwrap_or(t.align_h);
        t.align_v = self.align_v.unwrap_or(t.align_v);
        t.bold = self.bold.unwrap_or(t.bold);
        t.italic = self.italic.unwrap_or(t.italic);
        t.upper = self.upper.unwrap_or(t.upper);
        t.welded = self.welded.unwrap_or(t.welded);
        t.distort = self.distort.unwrap_or(t.distort);
        t.squeeze = self.squeeze.unwrap_or(t.squeeze);
        t.ignore_empty_vars = self.ignore_empty_vars.unwrap_or(t.ignore_empty_vars);
        if let Some(v) = &self.variable {
            t.variable = var_of(v);
        }
        if let Some(p) = t.problem() {
            return Err(invalid(cmd, p));
        }
        Ok(())
    }
}

/// The name `shape.text` gives a text when none is given: its first non-blank line (at most 100 characters).
fn auto_name(text: &str) -> String {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Text").trim().chars().take(100).collect()
}

/// The name `shape.barcode` gives a barcode of `kind` when none is given.
fn barcode_auto_name(kind: &str) -> String {
    format!("{} barcode", kind.trim())
}

/// The default font for new text: the first of a few common families that is installed, else the bundled
/// single-line Hershey Sans.
fn default_font() -> FontSpec {
    for family in ["Arial", "Liberation Sans", "DejaVu Sans", "Helvetica"] {
        let spec = FontSpec::system(family);
        if text::resolve(&spec, false, false).is_ok() {
            return spec;
        }
    }
    FontSpec { family: text::hershey::DEFAULT.into(), kind: FontKind::Stroke }
}

fn text_result(doc: &Document, created: &[ObjectId], affected: &[ObjectId], warnings: Vec<String>) -> Value {
    let mut r = util::edit_result(doc, created, affected);
    let shown: Vec<Value> = created
        .iter()
        .chain(affected)
        .filter_map(|id| doc.object(id))
        .filter_map(|o| match &o.geometry {
            Geometry::Text(t) => Some(json!({ "id": o.id, "shown": t.shown.as_deref().unwrap_or(&t.text) })),
            _ => None,
        })
        .collect();
    r["texts"] = json!(shown);
    r["warnings"] = json!(warnings);
    r
}

// ---------------------------------------------------------------------------------------------------------------
// shape.text, text.update
// ---------------------------------------------------------------------------------------------------------------

/// (Not `deny_unknown_fields`: serde cannot combine it with the flattened fields.)
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShapeTextInput {
    #[serde(flatten)]
    pub fields: TextFields,
    /// Where the alignment point goes, field frame mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Counter-clockwise rotation in degrees. Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default: the text's first line.
    pub name: Option<String>,
}

/// `shape.text`.
pub struct ShapeText;

impl Command for ShapeText {
    type Input = ShapeTextInput;
    const ID: &'static str = "shape.text";
    const TITLE: &'static str = "Text";
    const DESCRIPTION: &'static str = "Add live text (LightBurn's Text tool) at `center` (field mm) with an installed \
        font (`font: {family, kind: system}`; `font.list` lists fonts), a bundled single-line Hershey font (`kind: \
        stroke`) or an `.shx` / `.jsf` single-line font from a font folder. `text` is required; lines are separated \
        by \\n. Defaults as LightBurn: height 5 mm (the font size), middle/middle alignment, welded. Other fields: \
        see the schema (spacing, bend, max width, `variable` text). The text stays editable (`text.update`); its \
        rendered outline is what burns. Returns the id, bounds, what it shows and warnings (missing glyphs or \
        faces). Fails when the font is not installed.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ShapeTextInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let Some(s) = i.fields.text.clone() else { return Err(invalid(cmd, "text is required")) };
            if s.trim().is_empty() {
                return Err(invalid(cmd, "text must not be empty"));
            }
            let font = match &i.fields.font {
                Some(_) => FontSpec::system(""),
                None => default_font(),
            };
            let mut t = Text::new(String::new(), font, 5.0);
            i.fields.apply_to(cmd, &mut t)?;
            if i.fields.path_offset_mm.is_some() {
                return Err(invalid(cmd, "path_offset_mm: use text.apply_path to put a text on a path"));
            }
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let [x, y] = util::finite2(cmd, "center", i.center)?;
            let rot = util::finite(cmd, "rotation_deg", i.rotation_deg)?;
            let name = match &i.name {
                Some(n) => util::name(cmd, "name", n)?,
                None => auto_name(&s),
            };
            let transform = Affine::translate((x, y)) * Affine::rotate(rot.to_radians());
            let id = add_object(doc, name, layer, Geometry::Text(Box::new(t)), transform)?;
            let warnings = rerender(cmd, doc, &id, now().as_ref())?;
            let summary = format!("added text {id} \"{}\"", s.lines().next().unwrap_or_default());
            Ok((text_result(doc, &[id], &[], warnings), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TextUpdateInput {
    /// Exact text object id.
    pub id: ObjectId,
    #[serde(flatten)]
    pub fields: TextFields,
}

/// `text.update`.
pub struct TextUpdate;

impl Command for TextUpdate {
    type Input = TextUpdateInput;
    const ID: &'static str = "text.update";
    const TITLE: &'static str = "Edit text";
    const DESCRIPTION: &'static str = "Change a text object's settings (any of shape.text's fields; only those given \
        change; `variable: {mode: normal}` turns variable text off) and render it again. A text imported from LightBurn \
        is re-rendered with its font, which must be installed (else change `font`). Returns affected ids, bounds, \
        what it shows and warnings.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TextUpdateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let o = util::object(doc, &i.id)?;
            util::check_unlocked(cmd, doc, std::slice::from_ref(&i.id))?;
            let Geometry::Text(t) = &o.geometry else {
                return Err(invalid(cmd, format!("{} (\"{}\") is not a text object", o.id, o.name)));
            };
            let before = (**t).clone();
            let mut t = before.clone();
            i.fields.apply_to(cmd, &mut t)?;
            if t.text.trim().is_empty() {
                return Err(invalid(cmd, "text must not be empty (delete the object instead)"));
            }
            // An update that changes nothing is refused, so it does not leave an empty undo step.
            if t == before {
                return Err(invalid(cmd, format!("{}: these settings are already set; nothing changed", i.id)));
            }
            // A name still derived from the old text follows the new text.
            let rename = (t.text != before.text && o.name == auto_name(&before.text)).then(|| auto_name(&t.text));
            let o = doc.object_mut(&i.id).expect("looked up");
            o.geometry = Geometry::Text(Box::new(t));
            if let Some(name) = rename {
                o.name = name;
            }
            let warnings = rerender(cmd, doc, &i.id, now().as_ref())?;
            Ok((text_result(doc, &[], std::slice::from_ref(&i.id), warnings), format!("edited text {}", i.id)))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Text on a path, convert, refresh
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPathInput {
    /// Exact text object id.
    pub id: ObjectId,
    /// Exact id of the vector object to follow (its longest subpath, from its start point).
    pub path: ObjectId,
    /// Extra distance along the path in mm. Default 0.
    #[serde(default)]
    pub offset_mm: f64,
}

/// `text.apply_path`.
pub struct ApplyPath;

impl Command for ApplyPath {
    type Input = ApplyPathInput;
    const ID: &'static str = "text.apply_path";
    const TITLE: &'static str = "Apply path to text";
    const DESCRIPTION: &'static str = "Make a text follow another object's outline (LightBurn's Apply Path to Text). \
        The text moves into the path's frame and stays editable. Align X places it from the path's start (left), \
        centred between its ends (middle) or ending at its end (right); Align Y puts the baseline (bottom), the middle \
        of the capitals (middle) or their tops (top) on the path. The path object stays (put it on a layer with \
        output off to keep it from burning); `text.refresh` follows it after it was edited (a run burns the text as \
        shown). `text.detach_path` undoes this.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ApplyPathInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            util::check_unlocked(cmd, doc, std::slice::from_ref(&i.id))?;
            util::finite(cmd, "offset_mm", i.offset_mm)?;
            let p = util::object(doc, &i.path)?;
            if matches!(p.geometry, Geometry::Image { .. } | Geometry::Text(_)) || i.path == i.id {
                return Err(invalid(cmd, format!("{} cannot be a text's path: use a vector shape", i.path)));
            }
            let path_tf = p.transform;
            let o = doc.object_mut(&i.id).ok_or_else(|| invalid(cmd, format!("no object {}", i.id)))?;
            let Geometry::Text(t) = &mut o.geometry else {
                return Err(invalid(cmd, format!("{} is not a text object", i.id)));
            };
            t.path = Some(TextPath { object: i.path.clone(), offset_mm: i.offset_mm });
            // The text now lives in the path's frame (LightBurn moves the text onto the path).
            o.transform = path_tf;
            let warnings = rerender(cmd, doc, &i.id, now().as_ref())?;
            Ok((text_result(doc, &[], std::slice::from_ref(&i.id), warnings), format!("put text {} on {}", i.id, i.path)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdInput {
    /// Exact text object id.
    pub id: ObjectId,
}

/// `text.detach_path`.
pub struct DetachPath;

impl Command for DetachPath {
    type Input = IdInput;
    const ID: &'static str = "text.detach_path";
    const TITLE: &'static str = "Detach text from path";
    const DESCRIPTION: &'static str = "Take a text off its path: it is laid out on straight lines again, where the \
        path was. Returns affected ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: IdInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            util::check_unlocked(cmd, doc, std::slice::from_ref(&i.id))?;
            let o = doc.object_mut(&i.id).ok_or_else(|| invalid(cmd, format!("no object {}", i.id)))?;
            let Geometry::Text(t) = &mut o.geometry else {
                return Err(invalid(cmd, format!("{} is not a text object", i.id)));
            };
            if t.path.take().is_none() {
                return Err(CommandError::Failed(format!("{cmd}: {} is not on a path", i.id)));
            }
            let warnings = rerender(cmd, doc, &i.id, now().as_ref())?;
            Ok((text_result(doc, &[], std::slice::from_ref(&i.id), warnings), format!("took text {} off its path", i.id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdsInput {
    /// Exact object or group ids (a group stands for all its members).
    pub ids: Vec<ObjectId>,
}

/// `text.convert_to_path`.
pub struct ConvertToPath;

impl Command for ConvertToPath {
    type Input = IdsInput;
    const ID: &'static str = "text.convert_to_path";
    const TITLE: &'static str = "Convert text to path";
    const DESCRIPTION: &'static str = "Turn text objects into plain paths of their current outline (LightBurn's \
        Convert to Path: permanent, the text settings are gone). Other objects in `ids` are skipped. Returns affected \
        ids, `skipped` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: IdsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let mut affected = Vec::new();
            let mut skipped = Vec::new();
            for id in &ids {
                let o = doc.object_mut(id).expect("checked");
                match &o.geometry {
                    Geometry::Text(t) => {
                        o.geometry = Geometry::Path { path: t.outline.clone() };
                        o.extras.clear();
                        affected.push(id.clone());
                    }
                    _ => skipped.push(json!({ "id": id, "reason": "not text" })),
                }
            }
            if affected.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: none of the objects is text")));
            }
            let mut r = util::edit_result(doc, &[], &affected);
            r["skipped"] = json!(skipped);
            Ok((r, format!("converted text {} to paths", affected.join(", "))))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RefreshInput {
    /// Exact text object ids (groups allowed). Default: every text object.
    #[serde(default)]
    pub ids: Option<Vec<ObjectId>>,
}

/// `text.refresh`.
pub struct Refresh;

impl Command for Refresh {
    type Input = RefreshInput;
    const ID: &'static str = "text.refresh";
    const TITLE: &'static str = "Refresh text";
    const DESCRIPTION: &'static str = "Render text objects again: after fonts were installed, a text's path or its \
        layer's cut settings changed, or to update dates. Texts whose font is missing keep their outline and are \
        listed in `failed`. Returns affected ids, `failed` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RefreshInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = match &i.ids {
                Some(ids) => util::object_ids(cmd, doc, ids)?,
                None => doc.objects.iter().filter(|o| matches!(o.geometry, Geometry::Text(_))).map(|o| o.id.clone()).collect(),
            };
            let moment = now();
            let mut affected = Vec::new();
            let mut failed = Vec::new();
            let mut warnings = Vec::new();
            for id in ids {
                if !matches!(util::object(doc, &id)?.geometry, Geometry::Text(_)) {
                    continue;
                }
                match rerender(cmd, doc, &id, moment.as_ref()) {
                    Ok(w) => {
                        warnings.extend(w);
                        affected.push(id);
                    }
                    Err(e) => failed.push(json!({ "id": id, "error": e.to_string() })),
                }
            }
            let mut r = text_result(doc, &[], &affected, warnings);
            r["failed"] = json!(failed);
            Ok((r, format!("refreshed {} text object(s)", affected.len())))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Variable text state
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSerialInput {
    /// First serial number / CSV row.
    pub start: Option<i64>,
    /// Last one; after it the counter wraps to `start`.
    pub end: Option<i64>,
    /// The value the next output uses.
    pub current: Option<i64>,
    /// Steps per output (auto-advance) or per `var.advance`.
    pub advance_by: Option<i64>,
    /// Advance after every finished run.
    pub auto_advance: Option<bool>,
    /// Set `current` back to `start` (LightBurn's Reset), after the other fields.
    #[serde(default)]
    pub reset: bool,
}

fn variables_json(doc: &Document) -> Value {
    let v = &doc.variables;
    json!({
        "start": v.start, "end": v.end, "current": v.current, "advance_by": v.advance_by,
        "auto_advance": v.auto_advance,
        "csv": v.csv.as_ref().map(|c| json!({ "name": c.name, "rows": c.rows.len() })),
    })
}

/// `var.set_serial`.
pub struct SetSerial;

impl Command for SetSerial {
    type Input = SetSerialInput;
    const ID: &'static str = "var.set_serial";
    const TITLE: &'static str = "Variable text settings";
    const DESCRIPTION: &'static str = "Set the document's serial number / CSV row counter (LightBurn's Variable Text \
        window): start, end, current, advance_by, auto_advance (advance after every finished run), reset (current = \
        start). Every variable text is rendered again. Returns `variables` and the refreshed ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetSerialInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let v = &mut doc.variables;
            v.start = i.start.unwrap_or(v.start);
            v.end = i.end.unwrap_or(v.end);
            v.current = i.current.unwrap_or(v.current);
            v.advance_by = i.advance_by.unwrap_or(v.advance_by);
            v.auto_advance = i.auto_advance.unwrap_or(v.auto_advance);
            if v.start > v.end {
                return Err(invalid(cmd, format!("start {} is after end {}", v.start, v.end)));
            }
            if i.reset {
                v.current = v.start;
            }
            let ids = refresh_variables(cmd, doc, now().as_ref())?;
            let mut r = util::edit_result(doc, &[], &ids);
            r["variables"] = variables_json(doc);
            Ok((r, format!("variable text: current {}", doc.variables.current)))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdvanceInput {
    /// How many steps of `advance_by` (negative goes back, LightBurn's Previous). Default 1.
    pub steps: Option<i64>,
    /// Only advance when auto-advance is on and a text uses the counter (what the engine sends after a finished
    /// run). Default false.
    #[serde(default)]
    pub auto: bool,
}

/// `var.advance`.
pub struct Advance;

impl Command for Advance {
    type Input = AdvanceInput;
    const ID: &'static str = "var.advance";
    const TITLE: &'static str = "Next serial number";
    const DESCRIPTION: &'static str = "Move the serial number / CSV row counter by `steps` × advance_by (default one \
        step; wraps between start and end, LightBurn's Next / Previous) and render variable text again. With `auto: \
        true` it only advances when auto-advance is on (the engine sends this after each finished run). Returns \
        `advanced` and `variables`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AdvanceInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.auto && !auto_advance_due(doc) {
                let mut r = util::edit_result(doc, &[], &[]);
                r["advanced"] = json!(false);
                r["variables"] = variables_json(doc);
                return Ok((r, "auto-advance is off".into()));
            }
            doc.variables.current = doc.variables.advanced(i.steps.unwrap_or(1));
            let ids = refresh_variables(cmd, doc, now().as_ref())?;
            let mut r = util::edit_result(doc, &[], &ids);
            r["advanced"] = json!(true);
            r["variables"] = variables_json(doc);
            Ok((r, format!("variable text advanced to {}", doc.variables.current)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoadCsvInput {
    /// The CSV file's text (UTF-8; commas, double quotes as in RFC 4180). Row 0 is the first line.
    pub content: String,
    /// File name, for display. Default "data.csv".
    pub name: Option<String>,
    /// Also set the counter to rows 0..last (start 0, end = rows - 1, current 0). Default true.
    #[serde(default = "yes")]
    pub reset_range: bool,
}

fn yes() -> bool {
    true
}

/// `var.load_csv`.
pub struct LoadCsv;

impl Command for LoadCsv {
    type Input = LoadCsvInput;
    const ID: &'static str = "var.load_csv";
    const TITLE: &'static str = "Load CSV for merge";
    const DESCRIPTION: &'static str = "Load a CSV file for variable text in csv mode (`%0` is the first column of the \
        current row). The rows are kept in the document. By default the counter is set to run over every row (start \
        0, end the last row, current 0). Variable text is rendered again. Returns `variables`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: LoadCsvInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.content.len() > 10_000_000 {
                return Err(invalid(cmd, "the CSV file is larger than 10 MB"));
            }
            let rows = text::vars::parse_csv(&i.content).map_err(|e| invalid(cmd, e))?;
            if rows.is_empty() {
                return Err(invalid(cmd, "the CSV file has no rows"));
            }
            let cells: usize = rows.iter().map(|r| r.len().max(1)).sum();
            if cells > MAX_CSV_CELLS {
                return Err(invalid(cmd, format!("the CSV file has {cells} cells; at most {MAX_CSV_CELLS} can be loaded")));
            }
            let name = match &i.name {
                Some(n) => util::name(cmd, "name", n)?,
                None => "data.csv".into(),
            };
            let n = rows.len();
            doc.variables.csv = Some(doc::CsvData { name: name.clone(), rows });
            if i.reset_range {
                doc.variables.start = 0;
                doc.variables.end = n as i64 - 1;
                doc.variables.current = 0;
            }
            let ids = refresh_variables(cmd, doc, now().as_ref())?;
            let mut r = util::edit_result(doc, &[], &ids);
            r["variables"] = variables_json(doc);
            Ok((r, format!("loaded {name} ({n} rows)")))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `var.clear_csv`.
pub struct ClearCsv;

impl Command for ClearCsv {
    type Input = Empty;
    const ID: &'static str = "var.clear_csv";
    const TITLE: &'static str = "Clear CSV";
    const DESCRIPTION: &'static str = "Unload the CSV file (csv-mode texts then show nothing for their columns).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, _: Empty) -> Result<(Value, String)> {
        transact(doc, |doc| {
            if doc.variables.csv.take().is_none() {
                return Err(CommandError::NothingTo("clear: no CSV file is loaded"));
            }
            let ids = refresh_variables(Self::ID, doc, now().as_ref())?;
            let mut r = util::edit_result(doc, &[], &ids);
            r["variables"] = variables_json(doc);
            Ok((r, "cleared the CSV file".into()))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    /// The serial number / CSV row to show (each text adds its offset). Default: the current one.
    pub value: Option<i64>,
}

/// `var.preview_row`.
pub struct PreviewRow;

impl Command for PreviewRow {
    type Input = PreviewInput;
    const ID: &'static str = "var.preview_row";
    const TITLE: &'static str = "Test variable text";
    const DESCRIPTION: &'static str = "What every variable text (and variable barcode) would show for serial number \
        / CSV row `value` (default the current one), without changing anything (LightBurn's Test button). Returns \
        `{value, items: [{id, shown}], variables}`.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: PreviewInput) -> Result<(Value, String)> {
        let mut d = doc.clone();
        if let Some(v) = i.value {
            d.variables.current = v;
        }
        let moment = now();
        let laser = laser_name(&d);
        let mut items = Vec::new();
        for o in &d.objects {
            match &o.geometry {
                Geometry::Text(t) if t.variable.is_some() => {
                    items.push(json!({ "id": o.id, "shown": text::shown_text(&d, o, t, moment.as_ref(), &laser) }));
                }
                _ => {
                    if let Some(p) = o.extras.get(BARCODE_KEY)
                        && let Ok(b) = serde_json::from_str::<BarcodeInput>(p)
                        && b.variable.is_some()
                    {
                        items.push(json!({ "id": o.id, "shown": barcode_data(&d, &o.layer, &b, moment.as_ref()) }));
                    }
                }
            }
        }
        let r = json!({ "value": d.variables.current, "items": items, "variables": variables_json(&d) });
        Ok((r, "previewed variable text".into()))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Barcodes
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BarcodeInput {
    /// Redraw this existing barcode object with these settings (LightBurn's Edit Bar Code) instead of adding one.
    #[serde(default, skip_serializing)]
    pub id: Option<ObjectId>,
    /// qr, data_matrix, aztec, pdf417, pdf417_compact, micro_qr, rmqr, code128, code39, code93, codabar, ean8, ean13,
    /// upc_a, upc_e, itf, industrial_2of5, iata_2of5, isbn, vin, plessey, telepen, apriltag_16h5, apriltag_25h9,
    /// apriltag_36h11. A tag's data is its id.
    pub kind: String,
    /// The content (with `variable`, a variable-text format).
    pub data: String,
    /// Module (2D) or narrowest bar (1D) size in mm. Default 0.5.
    pub module_mm: Option<f64>,
    /// 1D bar height in mm. Default 15.
    pub height_mm: Option<f64>,
    /// Quiet zone in modules (drawn only when inverted). Default 2.
    pub quiet_zone: Option<u32>,
    /// Mark the light modules and the quiet zone instead of the dark ones.
    #[serde(default)]
    pub invert: bool,
    /// QR: L, M, Q, H; Micro QR: L, M, Q; rMQR: M, H; Aztec: % of error words; PDF417: 0-8.
    pub error_correction: Option<String>,
    /// Data Matrix: square symbols only (LightBurn's Force Square). Default true.
    pub force_square: Option<bool>,
    /// 1D codes: write the data under the bars (LightBurn's Show Bar Code Text), in the default font, a fifth of the
    /// bar height. Default false.
    #[serde(default)]
    pub show_text: bool,
    /// Variable text for the data (like text: serial, date_time, csv, cut_setting).
    pub variable: Option<VarInput>,
    /// Centre in the field frame, mm. Default [0, 0].
    #[serde(default)]
    pub center: [f64; 2],
    /// Counter-clockwise rotation in degrees. Default 0.
    #[serde(default)]
    pub rotation_deg: f64,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "<kind> barcode".
    pub name: Option<String>,
}

/// What a barcode's drawing depends on: its kept settings without the placement (unused by a redraw) and name.
fn barcode_settings(b: &BarcodeInput) -> Value {
    let mut v = serde_json::to_value(b).unwrap_or(Value::Null);
    if let Some(m) = v.as_object_mut() {
        for k in ["center", "rotation_deg", "layer", "name"] {
            m.remove(k);
        }
    }
    v
}

fn barcode_data(doc: &Document, layer: &str, b: &BarcodeInput, moment: Option<&text::Moment>) -> String {
    match b.variable.as_ref().and_then(var_of) {
        Some(v) => text::vars::evaluate(doc, layer, &b.data, &v, moment, &laser_name(doc)),
        None => b.data.clone(),
    }
}

fn draw_barcode(cmd: &str, b: &BarcodeInput, data: &str) -> Result<text::barcode::Drawn> {
    let kind = text::barcode::Kind::from_name(&b.kind).ok_or_else(|| {
        let names: Vec<&str> = text::barcode::Kind::ALL.iter().map(|k| k.name()).collect();
        invalid(cmd, format!("unknown barcode kind '{}'; one of {}", b.kind, names.join(", ")))
    })?;
    let d = text::barcode::Options::default();
    let o = text::barcode::Options {
        module_mm: b.module_mm.unwrap_or(d.module_mm),
        height_mm: b.height_mm.unwrap_or(d.height_mm),
        quiet_zone: b.quiet_zone.unwrap_or(d.quiet_zone),
        invert: b.invert,
        error_correction: b.error_correction.clone(),
        force_square: b.force_square.unwrap_or(true),
    };
    let mut drawn = text::barcode::draw(kind, data, &o).map_err(|e| CommandError::Failed(format!("{cmd}: {e}")))?;
    if b.show_text && !kind.is_2d() {
        // The text under the bars: its capitals' tops one module below them, centred; the outline stays centred.
        let size = (o.height_mm / 5.0).max(o.module_mm * 4.0);
        let mut t = Text::new(data, default_font(), size);
        t.align_v = AlignV::Top;
        let r = text::render_text(&t, data, None).map_err(|e| CommandError::Failed(format!("{cmd}: barcode text: {e}")))?;
        let below = -drawn.height_mm / 2.0 - o.module_mm;
        let label = Affine::translate((0.0, below)) * r.outline;
        let Some(lb) = geom::bounds(&label) else { return Ok(drawn) };
        let mut path = drawn.path.clone();
        path.extend(label.iter());
        let top = drawn.height_mm / 2.0;
        let bottom = lb.y0.min(-drawn.height_mm / 2.0);
        drawn.path = Affine::translate((0.0, -(top + bottom) / 2.0)) * path;
        drawn.height_mm = top - bottom;
        drawn.width_mm = drawn.width_mm.max(lb.width());
    }
    Ok(drawn)
}

/// `shape.barcode`.
pub struct ShapeBarcode;

impl Command for ShapeBarcode {
    type Input = BarcodeInput;
    const ID: &'static str = "shape.barcode";
    const TITLE: &'static str = "Barcode";
    const DESCRIPTION: &'static str = "Add a barcode (LightBurn's Create Bar Code) as a path of its dark modules \
        (welded into one outline per island: put it on a fill layer); `kind` lists the types (QR, Micro QR, rMQR, \
        Data Matrix, Aztec, PDF417, 1D codes, AprilTag ids). Sizes in mm: module_mm (default 0.5), \
        height_mm for 1D codes (default 15). invert marks the light modules and a quiet zone instead. `variable` \
        makes `data` a variable-text format (serial numbers, CSV, date). With `id` it redraws that barcode in place. \
        Returns the id, bounds, size and module count.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: BarcodeInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            check_text(cmd, &i.data)?;
            let layer = match (&i.id, &i.layer) {
                (Some(id), None) => util::object(doc, id)?.layer.clone(),
                _ => util::layer_or_default(doc, i.layer.as_deref())?,
            };
            let data = barcode_data(doc, &layer, &i, now().as_ref());
            let drawn = draw_barcode(cmd, &i, &data)?;
            let params = serde_json::to_string(&i).expect("serializes");
            let geometry = Geometry::Path { path: drawn.path.clone() };
            let (created, affected, id) = match &i.id {
                Some(id) => {
                    util::check_unlocked(cmd, doc, std::slice::from_ref(id))?;
                    let o = doc.object_mut(id).expect("looked up");
                    let Some(old) = o.extras.get(BARCODE_KEY).cloned() else {
                        return Err(invalid(cmd, format!("{id} is not a barcode")));
                    };
                    let old: Option<BarcodeInput> = serde_json::from_str(&old).ok();
                    let name = match (&i.name, &old) {
                        (Some(n), _) => Some(util::name(cmd, "name", n)?),
                        // A name still derived from the old kind follows the new kind.
                        (None, Some(old)) if o.name == barcode_auto_name(&old.kind) => Some(barcode_auto_name(&i.kind)),
                        _ => None,
                    };
                    // A redraw that changes nothing is refused, so it does not leave an empty undo step.
                    let unchanged = old.as_ref().is_some_and(|old| barcode_settings(old) == barcode_settings(&i))
                        && o.layer == layer
                        && name.as_ref().is_none_or(|n| *n == o.name);
                    if unchanged {
                        return Err(invalid(cmd, format!("{id}: this barcode already has these settings; nothing changed")));
                    }
                    o.geometry = geometry;
                    o.layer = layer;
                    if let Some(n) = name {
                        o.name = n;
                    }
                    o.extras.clear();
                    (vec![], vec![id.clone()], id.clone())
                }
                None => {
                    let [x, y] = util::finite2(cmd, "center", i.center)?;
                    let rot = util::finite(cmd, "rotation_deg", i.rotation_deg)?;
                    let name = match &i.name {
                        Some(n) => util::name(cmd, "name", n)?,
                        None => barcode_auto_name(&i.kind),
                    };
                    let t = Affine::translate((x, y)) * Affine::rotate(rot.to_radians());
                    let id = add_object(doc, name, layer, geometry, t)?;
                    (vec![id.clone()], vec![], id)
                }
            };
            doc.object_mut(&id).expect("just made").extras.insert(BARCODE_KEY.into(), params);
            let mut r = util::edit_result(doc, &created, &affected);
            r["size_mm"] = json!([util::r4(drawn.width_mm), util::r4(drawn.height_mm)]);
            r["modules"] = json!([drawn.modules.0, drawn.modules.1]);
            r["data"] = json!(data);
            Ok((r, format!("{} barcode {id}", i.kind.trim())))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// font.list
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontListInput {
    /// Only fonts of this kind.
    pub kind: Option<FontKind>,
}

/// `font.list`.
pub struct FontList;

impl Command for FontList {
    type Input = FontListInput;
    const ID: &'static str = "font.list";
    const TITLE: &'static str = "Fonts";
    const DESCRIPTION: &'static str = "List the fonts text can use: installed TrueType/OpenType families (`system`), \
        the bundled single-line Hershey fonts (`stroke`), and `.shx` / `.jsf` single-line fonts from the folders in \
        OPENETCH_FONT_DIRS. Returns `[{family, kind, bold, italic}]` sorted by name (bold/italic: the family has \
        such a face).";
    const RISK: Risk = Risk::Query;

    fn apply(_doc: &mut Document, i: FontListInput) -> Result<(Value, String)> {
        let list: Vec<Value> = text::families()
            .into_iter()
            .filter(|f| i.kind.is_none_or(|k| k == f.kind))
            .map(|f| json!({ "family": f.family, "kind": f.kind, "bold": f.bold, "italic": f.italic }))
            .collect();
        let n = list.len();
        Ok((json!(list), format!("{n} fonts")))
    }
}

#[cfg(test)]
mod tests;
