//! Live text ([`Geometry::Text`](crate::Geometry::Text)) and the document's variable-text state, modelled on
//! LightBurn's text objects and Variable Text window. Rendering lives in the `text` crate; this is data only. A text
//! object carries its rendered outline ([`Text::outline`], like LightBurn's saved `BackupPath`), so planning, previews
//! and exports never need fonts.

use kurbo::BezPath;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of font a [`FontSpec`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontKind {
    /// An installed TrueType / OpenType font (filled outlines).
    #[default]
    System,
    /// A single-line font bundled with OpenEtch (the Hershey fonts): open strokes, no fill.
    Stroke,
    /// An AutoCAD `.shx` shape font from a font folder: single-line strokes.
    Shx,
    /// An EzCad `.jsf` single-line font from a font folder.
    Jsf,
}

/// A font by family name and kind. The style (bold, italic) is on the [`Text`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FontSpec {
    /// Family name, e.g. "Arial"; for `.shx` / `.jsf` fonts the file name without extension (e.g. "romans").
    pub family: String,
    #[serde(default)]
    pub kind: FontKind,
}

impl FontSpec {
    pub fn system(family: impl Into<String>) -> Self {
        FontSpec { family: family.into(), kind: FontKind::System }
    }
}

/// LightBurn's Align X (`Ah` in `.lbrn2`: 0 left, 1 middle, 2 right). The text's local origin is this point of each
/// line (or, on a path, of the path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlignH {
    Left,
    #[default]
    Middle,
    Right,
}

/// LightBurn's Align Y (`Av` in `.lbrn2`; 1 is middle, as LightBurn's own files show; 0 top and 2 bottom are
/// inferred from the order of its Align Y list and are unverified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlignV {
    /// The tops of the capitals of the first line at the origin.
    Top,
    /// The block of lines centred on the origin (as LightBurn lays it out: see the `text` crate).
    #[default]
    Middle,
    /// The baseline of the last line at the origin.
    Bottom,
}

/// LightBurn's Variable Text modes (the Text Options "Text Mode" list). The text's [`Text::text`] is then a format in
/// that mode's syntax (see the `text` crate and LightBurn's Variable Text Formatting reference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VarMode {
    /// Date and time codes (`yyyy-MM-dd`, `HH:mm`, ...), evaluated when the job is output.
    DateTime,
    /// A serial number (`d`, `0dddd`, `h`, `H`) from the document's [`VariableState`].
    Serial,
    /// Columns of the current CSV row (`%0`, `%1`, ...).
    Csv,
    /// Values of the text's layer, or `C##` another layer (`p`, `s`, `k`, `q`, `n`, `i`, ...).
    CutSetting,
}

/// Variable text settings of one text object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VarText {
    pub mode: VarMode,
    /// Added to the document's current serial number or CSV row for this object (LightBurn's Offset /
    /// `VariableOffset`), so several copies show consecutive values.
    #[serde(default)]
    pub offset: i64,
}

/// Text placed along another object's outline (LightBurn's Apply Path to Text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TextPath {
    /// The object whose outline the text follows (its longest subpath, from its start point).
    pub object: String,
    /// Extra distance along the path in mm, added after the alignment (negative moves back).
    #[serde(default)]
    pub offset_mm: f64,
}

/// A live text object. Lengths in mm in the object's own frame (its transform places it). See the `text` crate for
/// how it is laid out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    /// The text, lines separated by `\n`. With [`Text::variable`] it is a format (LightBurn's `Str`).
    pub text: String,
    pub font: FontSpec,
    /// Font size (em) in mm: LightBurn's `H`. Capitals are about 0.7 of it in most fonts (LightBurn: "generally the
    /// height of a capital X"; its saved outlines are exactly `H` per em).
    pub height_mm: f64,
    /// Extra space between characters, % of the height (LightBurn's HSpace, `LS`). Negative tightens.
    #[serde(default)]
    pub h_spacing: f64,
    /// Extra space between lines, % of the line height (LightBurn's VSpace, `LnS`).
    #[serde(default)]
    pub v_spacing: f64,
    #[serde(default)]
    pub align_h: AlignH,
    #[serde(default)]
    pub align_v: AlignV,
    /// Use the family's bold face (if it has one; LightBurn does not fake it).
    #[serde(default)]
    pub bold: bool,
    /// Use the family's italic face (single-line fonts are slanted instead).
    #[serde(default)]
    pub italic: bool,
    /// Render in upper case (the stored text keeps its case).
    #[serde(default)]
    pub upper: bool,
    /// Merge touching and overlapping glyphs into one outline (LightBurn's Welded, `Weld`).
    #[serde(default)]
    pub welded: bool,
    /// Bend (LightBurn's bend handle): the angle in degrees the widest line spans on a circle; positive arches the
    /// text upward (centre below), negative downward. 0 is straight.
    #[serde(default)]
    pub bend_deg: f64,
    /// With bend: warp every glyph along the arc (on) or keep glyphs rigid and only rotate them (off).
    #[serde(default)]
    pub distort: bool,
    /// Follow another object's outline (Apply Path to Text) instead of straight lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<TextPath>,
    /// Lines wider than this many mm are scaled down to fit (LightBurn's Max Width); 0 is no limit.
    #[serde(default)]
    pub max_width_mm: f64,
    /// With max width: scale only horizontally (on) or keep the proportions (off).
    #[serde(default)]
    pub squeeze: bool,
    /// Drop lines that evaluate to nothing (LightBurn's Ignore Empty Vars, e.g. a blank CSV cell).
    #[serde(default)]
    pub ignore_empty_vars: bool,
    /// Variable text (serial numbers, date/time, CSV merge, cut settings); `None` is LightBurn's Normal mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<VarText>,
    /// The rendered outline in the object's own frame (LightBurn's `BackupPath`). Planning, previews and exports use
    /// only this, so they never need the font.
    pub outline: BezPath,
    /// The text the outline shows (the evaluated variable text), when it differs from [`Text::text`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<String>,
}

impl Text {
    /// A text with LightBurn's usual defaults (middle alignment, welded, no spacing) and an empty outline; the
    /// `text` crate fills the outline in.
    pub fn new(text: impl Into<String>, font: FontSpec, height_mm: f64) -> Self {
        Text {
            text: text.into(),
            font,
            height_mm,
            h_spacing: 0.0,
            v_spacing: 0.0,
            align_h: AlignH::Middle,
            align_v: AlignV::Middle,
            bold: false,
            italic: false,
            upper: false,
            welded: true,
            bend_deg: 0.0,
            distort: false,
            path: None,
            max_width_mm: 0.0,
            squeeze: false,
            ignore_empty_vars: false,
            variable: None,
            outline: BezPath::new(),
            shown: None,
        }
    }

    /// The numbers of this text that must be finite, with their names.
    pub fn numbers(&self) -> [(&'static str, f64); 6] {
        [
            ("height_mm", self.height_mm),
            ("h_spacing", self.h_spacing),
            ("v_spacing", self.v_spacing),
            ("bend_deg", self.bend_deg),
            ("max_width_mm", self.max_width_mm),
            ("path.offset_mm", self.path.as_ref().map_or(0.0, |p| p.offset_mm)),
        ]
    }

    /// Why these settings cannot be rendered (a number that is not finite or out of range), if so.
    pub fn problem(&self) -> Option<String> {
        if let Some((name, v)) = self.numbers().into_iter().find(|(_, v)| !v.is_finite()) {
            return Some(format!("text {name} is not a finite number ({v})"));
        }
        if !(self.height_mm > 0.0 && self.height_mm <= 10_000.0) {
            return Some(format!("text height_mm must be within 0..=10000 and above 0 (got {})", self.height_mm));
        }
        if !(-100.0..=1000.0).contains(&self.h_spacing) || !(-100.0..=1000.0).contains(&self.v_spacing) {
            return Some("text h_spacing and v_spacing must be within -100..=1000 %".into());
        }
        if !(-360.0..=360.0).contains(&self.bend_deg) {
            return Some(format!("text bend_deg must be within -360..=360 (got {})", self.bend_deg));
        }
        if self.max_width_mm < 0.0 {
            return Some("text max_width_mm must not be negative".into());
        }
        None
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// The document's variable-text state (LightBurn's Variable Text window; `<VariableText>` in `.lbrn2`). One serial
/// number / CSV row counter shared by every variable text of the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VariableState {
    /// First serial number or CSV row.
    pub start: i64,
    /// Last serial number or CSV row; after it the counter wraps to `start`.
    pub end: i64,
    /// The value used for the next output.
    pub current: i64,
    /// How far `current` moves per output (with `auto_advance`) or per next / previous.
    pub advance_by: i64,
    /// Advance `current` after every finished run (LightBurn: on Start / Send).
    pub auto_advance: bool,
    /// Stop at `end` instead of wrapping back to `start` (OpenEtch's production option): a slot whose value would
    /// pass `end` is not marked, and once every value is used (`current > end`) runs that need the counter are
    /// refused until the counter is reset or `end` raised. Left out of JSON while off.
    #[serde(skip_serializing_if = "is_false")]
    pub stop_at_end: bool,
    /// Slots (the variable-text offsets, LightBurn's per-text Offset) whose pockets are empty: their objects are
    /// left out of runs and the other slots take consecutive values (see `commands::slots`). Left out while empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub empty_slots: Vec<i64>,
    /// The loaded CSV file for merge mode, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csv: Option<CsvData>,
}

impl Default for VariableState {
    fn default() -> Self {
        VariableState {
            start: 0,
            end: 999,
            current: 0,
            advance_by: 1,
            auto_advance: false,
            stop_at_end: false,
            empty_slots: Vec::new(),
            csv: None,
        }
    }
}

impl VariableState {
    pub fn is_default(&self) -> bool {
        *self == VariableState::default()
    }

    /// `current` moved by `steps` × `advance_by`, wrapping between `start` and `end` (both ways, as LightBurn's
    /// Next / Previous do).
    pub fn advanced(&self, steps: i64) -> i64 {
        let (lo, hi) = if self.start <= self.end { (self.start, self.end) } else { (self.end, self.start) };
        if self.stop_at_end {
            // No wrap: forward ends one past `end` (every value used), back stops at `start`.
            let moved = i128::from(self.current) + i128::from(steps) * i128::from(self.advance_by);
            let clamped = moved.clamp(i128::from(lo), i128::from(hi) + 1);
            return i64::try_from(clamped).unwrap_or(hi);
        }
        let span = i128::from(hi) - i128::from(lo) + 1;
        let moved = i128::from(self.current) + i128::from(steps) * i128::from(self.advance_by);
        let wrapped = (moved - i128::from(lo)).rem_euclid(span) + i128::from(lo);
        i64::try_from(wrapped).unwrap_or(lo)
    }
}

/// A CSV file loaded for merge mode: its rows (row 0 first, as LightBurn counts them) and the file name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CsvData {
    pub name: String,
    pub rows: Vec<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_at_end_never_wraps() {
        let v = VariableState { start: 1, end: 5, current: 4, advance_by: 1, stop_at_end: true, ..VariableState::default() };
        assert_eq!(v.advanced(1), 5);
        assert_eq!(v.advanced(2), 6, "one past End: every value is used");
        assert_eq!(v.advanced(9), 6);
        assert_eq!(v.advanced(-9), 1);
        let back: VariableState = serde_json::from_value(serde_json::to_value(&v).unwrap()).unwrap();
        assert_eq!(back, v);
        assert!(serde_json::to_value(VariableState::default()).unwrap().get("stop_at_end").is_none());
    }

    #[test]
    fn counter_wraps_both_ways() {
        let v = VariableState { start: 1, end: 5, current: 5, advance_by: 1, ..VariableState::default() };
        assert_eq!(v.advanced(1), 1);
        assert_eq!(v.advanced(-1), 4);
        let v = VariableState { current: 1, ..v };
        assert_eq!(v.advanced(-1), 5);
        let v = VariableState { start: 0, end: 999, current: 998, advance_by: 4, ..VariableState::default() };
        assert_eq!(v.advanced(1), 2);
        let v = VariableState { start: i64::MIN, end: i64::MAX, current: i64::MAX, advance_by: i64::MAX, ..v };
        let _ = v.advanced(3);
    }

    #[test]
    fn text_problems_are_reported() {
        let mut t = Text::new("A", FontSpec::system("Arial"), 5.0);
        assert_eq!(t.problem(), None);
        t.height_mm = 0.0;
        assert!(t.problem().unwrap().contains("height_mm"));
        t.height_mm = 5.0;
        t.bend_deg = f64::NAN;
        assert!(t.problem().unwrap().contains("bend_deg"));
    }
}
