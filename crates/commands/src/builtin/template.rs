//! Templates with fill-in fields (Cuttle's project parameters, Cricut's ready-to-make projects; competitor-gaps item
//! 5, building on UX-049's `.lbt` / `.openetch` templates). A designer marks a few things of a document as fields
//! (the name on a coin, a date, the font, the finish of a layer, the first serial number) and can lock everything
//! else; whoever uses the template then fills in a short form (on the phone or the iPad) or an agent calls
//! `template.fill`. Filling is a design edit like any other (one undo step); it never starts anything.
//!
//! The fields live in the document (`extras["openetch.template"]`, JSON of [`TemplateSpec`]), so they travel with
//! the `.openetch` project or template file; LightBurn exports leave them out.
//!
//! Commands: `template.fields` (Query), `template.set_field`, `template.remove_field`, `template.set_lock`,
//! `template.fill`.

use std::collections::BTreeMap;

use doc::{CutSettings, Document, FontSpec, Geometry, ObjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::contact_sheet::overflow_of;
use super::text::{FontInput, rerender};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The document extras key holding the [`TemplateSpec`].
pub const TEMPLATE_KEY: &str = "openetch.template";
/// Most fields a template has.
pub const MAX_FIELDS: usize = 32;
/// Most choices a font or finish field offers.
pub const MAX_CHOICES: usize = 64;
/// Longest text a text field takes.
const MAX_TEXT: usize = 1000;

/// What a field fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    /// The text of the target texts (a name, a message).
    Text,
    /// A date written into the target texts in the field's `format`.
    Date,
    /// The font of the target texts (from `choices`, or any font when there are none).
    Font,
    /// The finish of the target layers: one of `choices`' cut settings (a MOPA colour, frost, black).
    Preset,
    /// The first serial number (the variable text's start and current value; the range keeps its length).
    Serial,
}

/// A font or finish to choose from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    /// What the form shows, e.g. "Gold" or "Script".
    pub label: String,
    /// A font field's font.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<FontInput>,
    /// A finish field's cut settings (complete, as GET /api/document shows a layer's `settings`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<CutSettings>,
    /// A finish field's sub-layers (complete; replace the layer's). Default none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_layers: Vec<CutSettings>,
    /// The swatch colour the form shows for a finish (any CSS colour). Default none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swatch: Option<String>,
}

/// One fill-in field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Short id that `template.fill` uses: 1-40 letters, digits, `_` or `-` (e.g. "name").
    pub id: String,
    /// What the form shows, e.g. "Name on the coin".
    pub label: String,
    pub kind: FieldKind,
    /// text, date, font: text object ids (plain text, not variable text); preset: layer ids; serial: none.
    #[serde(default)]
    pub targets: Vec<String>,
    /// text: a value is required. Default true.
    #[serde(default = "yes")]
    pub required: bool,
    /// text: most characters (0 = no limit).
    #[serde(default)]
    pub max_chars: u32,
    /// text: line breaks allowed. Default false.
    #[serde(default)]
    pub multiline: bool,
    /// date: the format, in LightBurn's date codes (`yyyy-MM-dd`, `dd.MM.yyyy`, `d MMMM yyyy`, ...). Default
    /// `yyyy-MM-dd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// font: the fonts to choose from (none: any font); preset: the finishes (at least one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
    /// A line of help under the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The value last filled in (kept by `template.fill`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

fn yes() -> bool {
    true
}

/// A document's template fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateSpec {
    #[serde(default)]
    pub fields: Vec<Field>,
    /// Everything but the fields' target texts is locked (`template.set_lock`).
    #[serde(default)]
    pub lock_others: bool,
    /// The objects `lock_others` locked (unlocked again when it is turned off).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<ObjectId>,
}

/// The document's template fields (an empty spec when it has none or the stored JSON is unreadable).
pub fn spec(doc: &Document) -> TemplateSpec {
    doc.extras.get(TEMPLATE_KEY).and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default()
}

fn store(cmd: &str, doc: &mut Document, s: &TemplateSpec) -> Result<()> {
    if s.fields.is_empty() && !s.lock_others {
        doc.extras.remove(TEMPLATE_KEY);
        return Ok(());
    }
    let text = serde_json::to_string(s).map_err(|e| CommandError::Failed(format!("{cmd}: {e}")))?;
    doc.extras.insert(TEMPLATE_KEY.into(), text);
    Ok(())
}

fn field_id_ok(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Check `f` against `doc`; returns it cleaned (trimmed label and hint).
fn check_field(cmd: &str, doc: &Document, mut f: Field) -> Result<Field> {
    if !field_id_ok(&f.id) {
        return Err(invalid(cmd, format!("field id '{}' must be 1-40 letters, digits, '_' or '-'", f.id)));
    }
    f.label = util::name(cmd, "label", &f.label)?;
    f.hint = f.hint.as_deref().map(str::trim).filter(|h| !h.is_empty()).map(|h| util::name(cmd, "hint", h)).transpose()?;
    if f.choices.len() > MAX_CHOICES {
        return Err(invalid(cmd, format!("a field offers at most {MAX_CHOICES} choices")));
    }
    let texts = |f: &Field| -> Result<()> {
        if f.targets.is_empty() {
            return Err(invalid(cmd, format!("field '{}' needs at least one text object in targets", f.id)));
        }
        for id in &f.targets {
            let o = util::object(doc, id)?;
            let Geometry::Text(t) = &o.geometry else {
                return Err(invalid(cmd, format!("{id} (\"{}\") is not a text object", o.name)));
            };
            if f.kind != FieldKind::Font && t.variable.is_some() {
                return Err(invalid(cmd, format!("{id} is variable text; its value comes from the counter or the CSV file")));
            }
        }
        Ok(())
    };
    match f.kind {
        FieldKind::Text | FieldKind::Date | FieldKind::Font => texts(&f)?,
        FieldKind::Preset => {
            if f.targets.is_empty() {
                return Err(invalid(cmd, format!("field '{}' needs at least one layer id in targets", f.id)));
            }
            for id in &f.targets {
                util::layer(doc, id)?;
            }
        }
        FieldKind::Serial => {
            if !f.targets.is_empty() {
                return Err(invalid(cmd, "a serial field has no targets (it sets the variable text's start)"));
            }
        }
    }
    if f.kind == FieldKind::Date {
        let fmt = f.format.get_or_insert_with(|| "yyyy-MM-dd".into());
        if fmt.trim().is_empty() || fmt.chars().count() > 60 || fmt.contains(char::is_control) {
            return Err(invalid(cmd, "a date format is 1-60 characters without line breaks"));
        }
    } else if f.format.is_some() {
        return Err(invalid(cmd, "format is for date fields"));
    }
    for c in &mut f.choices {
        c.label = util::name(cmd, "choices.label", &c.label)?;
        if let Some(s) = &c.swatch {
            c.swatch = Some(util::color(cmd, s)?);
        }
        match f.kind {
            FieldKind::Font => {
                let Some(font) = &mut c.font else { return Err(invalid(cmd, format!("font choice '{}' needs a font", c.label))) };
                font.family = util::name(cmd, "choices.font.family", &font.family)?;
                if c.settings.is_some() || !c.sub_layers.is_empty() {
                    return Err(invalid(cmd, "a font choice has no cut settings"));
                }
            }
            FieldKind::Preset => {
                let Some(s) = &c.settings else { return Err(invalid(cmd, format!("finish '{}' needs settings", c.label))) };
                super::layer::validate_settings(cmd, s)?;
                for s in &c.sub_layers {
                    super::layer::validate_settings(cmd, s)?;
                }
                if c.font.is_some() {
                    return Err(invalid(cmd, "a finish has no font"));
                }
            }
            _ => return Err(invalid(cmd, "only font and preset fields have choices")),
        }
    }
    if f.kind == FieldKind::Preset && f.choices.is_empty() {
        return Err(invalid(cmd, format!("finish field '{}' needs at least one choice", f.id)));
    }
    let mut seen = Vec::new();
    for c in &f.choices {
        let k = c.label.to_lowercase();
        if seen.contains(&k) {
            return Err(invalid(cmd, format!("two choices are called '{}'", c.label)));
        }
        seen.push(k);
    }
    if f.kind != FieldKind::Text && (f.max_chars != 0 || f.multiline) {
        return Err(invalid(cmd, "max_chars and multiline are for text fields"));
    }
    Ok(f)
}

/// The text objects the fields fill (never locked by `lock_others`).
fn field_texts(s: &TemplateSpec) -> Vec<ObjectId> {
    s.fields
        .iter()
        .filter(|f| matches!(f.kind, FieldKind::Text | FieldKind::Date | FieldKind::Font))
        .flat_map(|f| f.targets.iter().cloned())
        .collect()
}

/// Apply `lock_others`: lock every object but the field texts (remembering which ones it locked), or unlock those.
fn relock(doc: &mut Document, s: &mut TemplateSpec) {
    for id in s.locked.drain(..) {
        doc.locked.remove(&id);
    }
    if s.lock_others {
        let keep = field_texts(s);
        for o in &doc.objects {
            if !keep.contains(&o.id) && !doc.locked.contains(&o.id) {
                s.locked.push(o.id.clone());
            }
        }
        doc.locked.extend(s.locked.iter().cloned());
    }
}

/// The current value of field `f` in `doc`, and what is wrong with it.
fn current(doc: &Document, f: &Field) -> (Value, Vec<String>) {
    let mut problems = Vec::new();
    for id in &f.targets {
        let gone = match f.kind {
            FieldKind::Preset => doc.layer(id).is_none(),
            _ => !matches!(doc.object(id).map(|o| &o.geometry), Some(Geometry::Text(_))),
        };
        if gone {
            problems.push(format!("target {id} no longer exists"));
        }
    }
    let first_text = || {
        f.targets.iter().find_map(|id| match doc.object(id).map(|o| &o.geometry) {
            Some(Geometry::Text(t)) => Some(t),
            _ => None,
        })
    };
    let v = match f.kind {
        FieldKind::Text => first_text().map_or(Value::Null, |t| json!(t.text)),
        FieldKind::Date => f.value.clone().unwrap_or(Value::Null),
        FieldKind::Font => first_text().map_or(Value::Null, |t| {
            let label = f.choices.iter().find(|c| c.font.as_ref().is_some_and(|x| x.family.eq_ignore_ascii_case(&t.font.family) && x.kind == t.font.kind));
            match label {
                Some(c) => json!(c.label),
                None => json!({ "family": t.font.family, "kind": t.font.kind }),
            }
        }),
        FieldKind::Preset => f
            .targets
            .iter()
            .find_map(|id| doc.layer(id))
            .and_then(|l| f.choices.iter().find(|c| c.settings.as_ref() == Some(&l.settings) && c.sub_layers == l.sub_layers))
            .map_or(Value::Null, |c| json!(c.label)),
        FieldKind::Serial => json!(doc.variables.start),
    };
    (v, problems)
}

// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Empty {}

/// `template.fields`.
pub struct Fields;

impl Command for Fields {
    type Input = Empty;
    const ID: &'static str = "template.fields";
    const TITLE: &'static str = "Template fields";
    const DESCRIPTION: &'static str = "The document's fill-in fields (a template's parameters): `{fields: [{id, \
        label, kind, targets, required, max_chars, multiline, format, choices, hint, current, problems}], \
        lock_others}`. kind: text (the target texts' text), date (written in `format`), font (from `choices`, or \
        any font), preset (a finish: one of `choices`' cut settings for the target layers) or serial (the first \
        serial number). `current` is the value now (a choice's label where one matches). Fill them with \
        template.fill. Changes nothing.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, _: Empty) -> Result<(Value, String)> {
        let s = spec(doc);
        let fields: Vec<Value> = s
            .fields
            .iter()
            .map(|f| {
                let (cur, problems) = current(doc, f);
                let mut v = serde_json::to_value(f).unwrap_or(Value::Null);
                v["current"] = cur;
                v["problems"] = json!(problems);
                v
            })
            .collect();
        let n = fields.len();
        Ok((json!({ "fields": fields, "lock_others": s.lock_others }), format!("{n} template fields")))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetFieldInput {
    /// The field; one with the same id is replaced (it keeps its place), else it is added at the end.
    pub field: Field,
    /// Move it to this position (0 = first). Default: its place, or the end.
    pub index: Option<usize>,
}

/// `template.set_field`.
pub struct SetField;

impl Command for SetField {
    type Input = SetFieldInput;
    const ID: &'static str = "template.set_field";
    const TITLE: &'static str = "Set template field";
    const DESCRIPTION: &'static str = "Add a fill-in field to the document (making it a template) or replace the \
        one with the same id: `{id, label, kind: text|date|font|preset|serial, targets, required?, max_chars?, \
        multiline?, format?, choices?, hint?}`. text / date / font target plain text objects; preset targets layers \
        and needs `choices: [{label, settings, sub_layers?, swatch?}]` (complete cut settings, e.g. a copy of a \
        layer's); font `choices: [{label, font: {family, kind}}]` (none: any font). At most 32 fields. With \
        lock_others on, the new targets are unlocked and the rest stays locked.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetFieldInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let f = check_field(cmd, doc, i.field)?;
            let mut s = spec(doc);
            let id = f.id.clone();
            let at = s.fields.iter().position(|x| x.id == id);
            let replaced = at.is_some();
            let mut f = f;
            if let Some(k) = at {
                // A refilled value stays while the kind is the same.
                if f.value.is_none() && s.fields[k].kind == f.kind {
                    f.value = s.fields[k].value.clone();
                }
                s.fields.remove(k);
            }
            if s.fields.len() >= MAX_FIELDS {
                return Err(invalid(cmd, format!("a template has at most {MAX_FIELDS} fields")));
            }
            let index = i.index.or(at).unwrap_or(s.fields.len()).min(s.fields.len());
            s.fields.insert(index, f);
            relock(doc, &mut s);
            store(cmd, doc, &s)?;
            let verb = if replaced { "changed" } else { "added" };
            Ok((json!({ "created": [], "affected": [], "bounds": Value::Null, "field": id, "fields": s.fields.len() }), format!("{verb} template field {id}")))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveFieldInput {
    /// The field id.
    pub id: String,
}

/// `template.remove_field`.
pub struct RemoveField;

impl Command for RemoveField {
    type Input = RemoveFieldInput;
    const ID: &'static str = "template.remove_field";
    const TITLE: &'static str = "Remove template field";
    const DESCRIPTION: &'static str = "Remove a fill-in field (the objects stay as they are). With lock_others on, \
        its texts are locked like the rest.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: RemoveFieldInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let mut s = spec(doc);
            let Some(k) = s.fields.iter().position(|f| f.id == i.id) else {
                return Err(not_found(&s, &i.id));
            };
            s.fields.remove(k);
            relock(doc, &mut s);
            store(cmd, doc, &s)?;
            Ok((json!({ "created": [], "affected": [], "bounds": Value::Null }), format!("removed template field {}", i.id)))
        })
    }
}

fn not_found(s: &TemplateSpec, id: &str) -> CommandError {
    let ids: Vec<String> = s.fields.iter().map(|f| f.id.clone()).collect();
    CommandError::NotFound {
        message: format!("no template field '{id}' (fields: {})", if ids.is_empty() { "none".into() } else { ids.join(", ") }),
        candidates: ids,
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetLockInput {
    /// Lock every object except the fields' texts (true), or unlock what this locked (false).
    pub lock_others: bool,
}

/// `template.set_lock`.
pub struct SetLock;

impl Command for SetLock {
    type Input = SetLockInput;
    const ID: &'static str = "template.set_lock";
    const TITLE: &'static str = "Lock the rest of the template";
    const DESCRIPTION: &'static str = "lock_others true locks every object that is not a field's text (LightBurn's \
        lock: it cannot be moved, edited or deleted), so whoever fills the template changes only the fields; false \
        unlocks the objects it locked (objects locked by hand stay locked).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetLockInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let mut s = spec(doc);
            if s.lock_others == i.lock_others {
                return Err(invalid(cmd, format!("lock_others is already {}", i.lock_others)));
            }
            s.lock_others = i.lock_others;
            relock(doc, &mut s);
            store(cmd, doc, &s)?;
            let n = s.locked.len();
            let summary = if i.lock_others { format!("locked {n} objects outside the template fields") } else { "unlocked the template".into() };
            Ok((json!({ "created": [], "affected": [], "bounds": Value::Null, "locked": s.locked }), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FillInput {
    /// Field id → value. text: a string; date: "yyyy-MM-dd" or "today"; font: a choice's label, a family name or
    /// `{family, kind}`; preset: a choice's label; serial: the first number. Fields left out keep their value.
    pub values: BTreeMap<String, Value>,
}

/// `template.fill`.
pub struct Fill;

/// Set the text of text object `id` (rendering it again) unless it already shows `text`.
fn set_text(cmd: &str, doc: &mut Document, id: &str, text: &str, warnings: &mut Vec<String>) -> Result<bool> {
    let o = doc.object_mut(id).ok_or_else(|| CommandError::Failed(format!("{cmd}: text {id} is gone")))?;
    let Geometry::Text(t) = &mut o.geometry else { return Err(CommandError::Failed(format!("{cmd}: {id} is not text"))) };
    if t.text == text {
        return Ok(false);
    }
    let old = std::mem::replace(&mut t.text, text.to_string());
    // A name that followed the old text follows the new one (as text.update).
    let first = |s: &str| s.lines().find(|l| !l.trim().is_empty()).unwrap_or("Text").trim().chars().take(100).collect::<String>();
    if o.name == first(&old) {
        o.name = first(text);
    }
    warnings.extend(rerender(cmd, doc, id, None)?);
    Ok(true)
}

fn font_of(f: &Field, v: &Value, cmd: &str) -> Result<FontSpec> {
    let by_label = |s: &str| f.choices.iter().find(|c| c.label.eq_ignore_ascii_case(s.trim())).and_then(|c| c.font.clone());
    let font = match v {
        Value::String(s) => match by_label(s) {
            Some(fi) => FontSpec { family: fi.family, kind: fi.kind },
            None if f.choices.is_empty() => FontSpec::system(util::name(cmd, "font", s)?),
            None => FontSpec { family: s.clone(), kind: doc::FontKind::System },
        },
        Value::Object(_) => {
            let fi: FontInput = serde_json::from_value(v.clone()).map_err(|e| invalid(cmd, format!("{}: {e}", f.id)))?;
            FontSpec { family: util::name(cmd, "font.family", &fi.family)?, kind: fi.kind }
        }
        _ => return Err(invalid(cmd, format!("{}: a font is a choice's label or {{family, kind}}", f.id))),
    };
    if !f.choices.is_empty() && !f.choices.iter().any(|c| c.font.as_ref().is_some_and(|x| x.family.eq_ignore_ascii_case(&font.family) && x.kind == font.kind)) {
        let labels: Vec<&str> = f.choices.iter().map(|c| c.label.as_str()).collect();
        return Err(invalid(cmd, format!("{}: choose one of {}", f.id, labels.join(", "))));
    }
    Ok(font)
}

/// `yyyy-MM-dd` (or "today" on the engine) as a moment.
fn date_of(cmd: &str, f: &Field, v: &Value) -> Result<text::Moment> {
    let s = v.as_str().ok_or_else(|| invalid(cmd, format!("{}: a date is \"yyyy-MM-dd\" or \"today\"", f.id)))?.trim();
    if s.eq_ignore_ascii_case("today") {
        return super::text::now().ok_or_else(|| invalid(cmd, format!("{}: \"today\" needs the engine's clock; give the date", f.id)));
    }
    let parts: Vec<&str> = s.split('-').collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<i64>().ok());
    match (parts.len(), num(0), num(1), num(2)) {
        (3, Some(y), Some(m), Some(d)) => i32::try_from(y)
            .ok()
            .zip(u32::try_from(m).ok().zip(u32::try_from(d).ok()))
            .and_then(|(y, (m, d))| text::Moment::on_day(y, m, d))
            .ok_or_else(|| invalid(cmd, format!("{}: {s} is not a day", f.id))),
        _ => Err(invalid(cmd, format!("{}: a date is \"yyyy-MM-dd\" or \"today\" (got \"{s}\")", f.id))),
    }
}

impl Command for Fill {
    type Input = FillInput;
    const ID: &'static str = "template.fill";
    const TITLE: &'static str = "Fill in template";
    const DESCRIPTION: &'static str = "Fill in the document's template fields (template.fields lists them) in one \
        undoable step: `values: {field id: value}`; text: the text (required fields refuse an empty one, max_chars \
        and multiline apply); date: \"yyyy-MM-dd\" or \"today\", written in the field's format; font: a choice's \
        label (or a family name / {family, kind} when the field has no choices); preset: a finish's label (the target \
        layers get its cut settings); serial: the first serial number (the range keeps its length). Design only: it \
        never starts or approves a run. Returns the values set, render `warnings` (missing glyphs) and `overflow`: \
        texts that now run outside their shape, the stock or the workspace (check before asking for a run).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FillInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let mut s = spec(doc);
            if s.fields.is_empty() {
                return Err(CommandError::Failed(format!("{cmd}: the document has no template fields")));
            }
            if i.values.is_empty() {
                return Err(invalid(cmd, "values must name at least one field"));
            }
            let mut affected: Vec<ObjectId> = Vec::new();
            let mut warnings = Vec::new();
            let mut set = Vec::new();
            for (id, v) in &i.values {
                let k = s.fields.iter().position(|f| &f.id == id).ok_or_else(|| not_found(&s, id))?;
                let f = s.fields[k].clone();
                let stored = match f.kind {
                    FieldKind::Text => {
                        let text = v.as_str().ok_or_else(|| invalid(cmd, format!("{id}: a text field takes a string")))?;
                        let text = text.replace("\r\n", "\n");
                        if f.required && text.trim().is_empty() {
                            return Err(invalid(cmd, format!("{id}: \"{}\" must not be empty", f.label)));
                        }
                        if !f.multiline && text.contains('\n') {
                            return Err(invalid(cmd, format!("{id}: \"{}\" is one line", f.label)));
                        }
                        let n = text.chars().count();
                        if (f.max_chars > 0 && n > f.max_chars as usize) || n > MAX_TEXT {
                            let max = if f.max_chars > 0 { f.max_chars as usize } else { MAX_TEXT };
                            return Err(invalid(cmd, format!("{id}: \"{}\" takes at most {max} characters (got {n})", f.label)));
                        }
                        if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                            return Err(invalid(cmd, format!("{id}: control characters are not allowed")));
                        }
                        // An optional field left empty: a text object cannot be empty, so it shows a space.
                        let shown = if text.trim().is_empty() { " ".to_string() } else { text.clone() };
                        for t in &f.targets {
                            if set_text(cmd, doc, t, &shown, &mut warnings)? {
                                affected.push(t.clone());
                            }
                        }
                        json!(text)
                    }
                    FieldKind::Date => {
                        let m = date_of(cmd, &f, v)?;
                        let text = text::vars::format_date(f.format.as_deref().unwrap_or("yyyy-MM-dd"), &m);
                        for t in &f.targets {
                            if set_text(cmd, doc, t, &text, &mut warnings)? {
                                affected.push(t.clone());
                            }
                        }
                        json!(m.iso_day())
                    }
                    FieldKind::Font => {
                        let font = font_of(&f, v, cmd)?;
                        for t in &f.targets {
                            let o = doc.object_mut(t).ok_or_else(|| CommandError::Failed(format!("{cmd}: text {t} is gone")))?;
                            let Geometry::Text(tx) = &mut o.geometry else { continue };
                            if tx.font != font {
                                tx.font = font.clone();
                                warnings.extend(rerender(cmd, doc, t, None)?);
                                affected.push(t.clone());
                            }
                        }
                        json!({ "family": font.family, "kind": font.kind })
                    }
                    FieldKind::Preset => {
                        let label = v.as_str().ok_or_else(|| invalid(cmd, format!("{id}: a finish is a choice's label")))?;
                        let c = f.choices.iter().find(|c| c.label.eq_ignore_ascii_case(label.trim())).ok_or_else(|| {
                            let labels: Vec<&str> = f.choices.iter().map(|c| c.label.as_str()).collect();
                            invalid(cmd, format!("{id}: choose one of {}", labels.join(", ")))
                        })?;
                        for l in &f.targets {
                            let layer = doc.layer_mut(l).ok_or_else(|| CommandError::Failed(format!("{cmd}: layer {l} is gone")))?;
                            if let Some(st) = &c.settings {
                                layer.settings = st.clone();
                            }
                            layer.sub_layers = c.sub_layers.clone();
                            // The finish replaces any library link (LightBurn's Assign).
                            layer.link = None;
                            layer.extras.remove(super::preset::LINK_PATH_KEY);
                        }
                        // Cut-setting variable texts on those layers show the new values.
                        super::text::refresh_variables(cmd, doc, super::text::now().as_ref())?;
                        json!(c.label)
                    }
                    FieldKind::Serial => {
                        let n = v.as_i64().ok_or_else(|| invalid(cmd, format!("{id}: a serial field takes a whole number")))?;
                        let vs = &mut doc.variables;
                        let len = vs.end.saturating_sub(vs.start).max(0);
                        vs.start = n;
                        vs.current = n;
                        vs.end = n.saturating_add(len);
                        affected.extend(super::text::refresh_variables(cmd, doc, super::text::now().as_ref())?);
                        json!(n)
                    }
                };
                s.fields[k].value = Some(stored.clone());
                set.push(json!({ "id": id, "value": stored }));
            }
            store(cmd, doc, &s)?;
            affected.sort();
            affected.dedup();
            warnings.sort();
            warnings.dedup();
            let overflow: Vec<Value> = field_texts(&s)
                .iter()
                .flat_map(|id| overflow_of(doc, id).into_iter().map(move |m| json!({ "id": id, "message": m })))
                .collect();
            let mut r = util::edit_result(doc, &[], &affected);
            r["values"] = json!(set);
            r["warnings"] = json!(warnings);
            r["overflow"] = json!(overflow);
            let names: Vec<&str> = i.values.keys().map(String::as_str).collect();
            Ok((r, format!("filled in {}", names.join(", "))))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(doc: &mut Document, id: &str, params: Value) -> Result<Value> {
        let mut s = crate::Session::new(doc.clone());
        let out = s.execute(crate::Invocation { id: id.into(), params }, crate::Origin::Agent("t".into()))?;
        *doc = s.document().clone();
        Ok(out.result)
    }

    fn ok(doc: &mut Document, id: &str, params: Value) -> Value {
        run(doc, id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// A coin: a rim, a name text and a date text, on two layers.
    fn coin() -> (Document, String, String, String, String) {
        let mut d = Document { field_mm: Some(150.0), ..Document::default() };
        let rim = ok(&mut d, "shape.ellipse", json!({"rx_mm": 20, "ry_mm": 20, "name": "Rim"}))["created"][0].as_str().unwrap().to_string();
        let hershey = json!({"family": "Hershey Sans", "kind": "stroke"});
        let name = ok(&mut d, "shape.text", json!({"text": "NAME", "font": hershey, "height_mm": 6}))["created"][0].as_str().unwrap().to_string();
        let date = ok(&mut d, "shape.text", json!({"text": "DATE", "font": hershey, "height_mm": 3, "center": [0, -10]}))["created"][0].as_str().unwrap().to_string();
        let layer = d.layers[0].id.clone();
        (d, rim, name, date, layer)
    }

    #[test]
    fn define_fill_and_list_fields() {
        let (mut d, rim, name, date, layer) = coin();
        ok(&mut d, "template.set_field", json!({"field": {"id": "name", "label": "Name", "kind": "text", "targets": [name], "max_chars": 20}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "day", "label": "Date", "kind": "date", "targets": [date], "format": "dd.MM.yyyy"}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "font", "label": "Font", "kind": "font", "targets": [name], "choices": [
            {"label": "Plain", "font": {"family": "Hershey Sans", "kind": "stroke"}},
            {"label": "Script", "font": {"family": "Hershey Script", "kind": "stroke"}}]}}));
        let mut gold = d.layers[0].settings.clone();
        gold.power_pct = 31.0;
        gold.frequency_khz = 500.0;
        ok(&mut d, "template.set_field", json!({"field": {"id": "finish", "label": "Finish", "kind": "preset", "targets": [layer], "choices": [
            {"label": "Gold", "settings": gold, "swatch": "#d4a017"}]}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "sn", "label": "First number", "kind": "serial"}}));

        let r = ok(&mut d, "template.fill", json!({"values": {"name": "Ada", "day": "2026-11-03", "font": "script", "finish": "Gold", "sn": 100}}));
        assert_eq!(r["overflow"], json!([]));
        let text = |d: &Document, id: &str| match &d.object(id).unwrap().geometry {
            Geometry::Text(t) => (t.text.clone(), t.font.family.clone()),
            _ => panic!(),
        };
        assert_eq!(text(&d, &name), ("Ada".into(), "Hershey Script".into()));
        assert_eq!(text(&d, &date).0, "03.11.2026");
        assert_eq!(d.object(&name).unwrap().name, "Ada", "the auto name follows the text");
        assert_eq!(d.layers[0].settings.power_pct, 31.0);
        assert_eq!(d.variables.start, 100);
        assert_eq!(d.variables.current, 100);

        let f = ok(&mut d, "template.fields", json!({}));
        let cur: Vec<Value> = f["fields"].as_array().unwrap().iter().map(|x| x["current"].clone()).collect();
        assert_eq!(cur, [json!("Ada"), json!("2026-11-03"), json!("Script"), json!("Gold"), json!(100)]);
        assert!(f["fields"][0]["problems"].as_array().unwrap().is_empty());
        // The rim is no field: it stays out of reach of the form, and a deleted target is reported.
        assert!(run(&mut d, "template.set_field", json!({"field": {"id": "rim", "label": "Rim", "kind": "text", "targets": [rim]}})).is_err());
        ok(&mut d, "object.delete", json!({"ids": [date]}));
        let f = ok(&mut d, "template.fields", json!({}));
        assert!(f["fields"][1]["problems"][0].as_str().unwrap().contains("no longer exists"));
    }

    #[test]
    fn fill_refuses_bad_values_and_changes_nothing_then() {
        let (mut d, _, name, _, _) = coin();
        ok(&mut d, "template.set_field", json!({"field": {"id": "name", "label": "Name", "kind": "text", "targets": [name], "max_chars": 5}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "font", "label": "Font", "kind": "font", "targets": [name], "choices": [
            {"label": "Plain", "font": {"family": "Hershey Sans", "kind": "stroke"}}]}}));
        let before = d.clone();
        for v in [json!({"name": ""}), json!({"name": "Too long"}), json!({"name": "a\nb"}), json!({"font": "Comic"}), json!({"nope": 1}), json!({"name": 5}), json!({})] {
            assert!(run(&mut d, "template.fill", json!({"values": v})).is_err(), "{v}");
            assert_eq!(d, before);
        }
        let e = run(&mut d, "template.fill", json!({"values": {"nme": "x"}})).unwrap_err();
        assert!(e.to_string().contains("fields: name, font"), "{e}");
        // A long name runs off the rim: reported, not refused.
        ok(&mut d, "template.set_field", json!({"field": {"id": "name", "label": "Name", "kind": "text", "targets": [name]}}));
        let r = ok(&mut d, "template.fill", json!({"values": {"name": "A VERY LONG NAME FOR A COIN"}}));
        assert!(r["overflow"][0]["message"].as_str().is_some_and(|m| m.contains("Rim")), "{r}");
        // Dates: a day that does not exist, a wrong form.
        let mut e = d.clone();
        ok(&mut e, "template.set_field", json!({"field": {"id": "day", "label": "Day", "kind": "date", "targets": [name]}}));
        assert!(run(&mut e, "template.fill", json!({"values": {"day": "2026-02-30"}})).is_err());
        assert!(run(&mut e, "template.fill", json!({"values": {"day": "3.11.2026"}})).is_err());
    }

    #[test]
    fn field_definitions_are_checked() {
        let (mut d, rim, name, _, layer) = coin();
        let bad = [
            json!({"id": "a b", "label": "x", "kind": "text", "targets": [name]}),
            json!({"id": "n", "label": " ", "kind": "text", "targets": [name]}),
            json!({"id": "n", "label": "x", "kind": "text", "targets": []}),
            json!({"id": "n", "label": "x", "kind": "text", "targets": [rim]}),
            json!({"id": "n", "label": "x", "kind": "preset", "targets": [layer]}),
            json!({"id": "n", "label": "x", "kind": "preset", "targets": ["nope"], "choices": []}),
            json!({"id": "n", "label": "x", "kind": "serial", "targets": [name]}),
            json!({"id": "n", "label": "x", "kind": "text", "targets": [name], "format": "yyyy"}),
            json!({"id": "n", "label": "x", "kind": "font", "targets": [name], "choices": [{"label": "A"}]}),
            json!({"id": "n", "label": "x", "kind": "date", "targets": [name], "multiline": true}),
        ];
        for f in bad {
            assert!(run(&mut d, "template.set_field", json!({ "field": f })).is_err(), "{f}");
        }
        assert!(run(&mut d, "template.remove_field", json!({"id": "n"})).is_err());
        assert!(run(&mut d, "template.fill", json!({"values": {"n": "x"}})).is_err(), "no fields yet");
        // Replacing keeps the place; index moves it.
        ok(&mut d, "template.set_field", json!({"field": {"id": "a", "label": "A", "kind": "text", "targets": [name]}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "b", "label": "B", "kind": "serial"}}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "a", "label": "A2", "kind": "text", "targets": [name]}}));
        assert_eq!(spec(&d).fields.iter().map(|f| f.label.as_str()).collect::<Vec<_>>(), ["A2", "B"]);
        ok(&mut d, "template.set_field", json!({"field": {"id": "b", "label": "B", "kind": "serial"}, "index": 0}));
        assert_eq!(spec(&d).fields[0].id, "b");
        ok(&mut d, "template.remove_field", json!({"id": "a"}));
        ok(&mut d, "template.remove_field", json!({"id": "b"}));
        assert!(!d.extras.contains_key(TEMPLATE_KEY), "no fields: no template");
    }

    #[test]
    fn lock_others_locks_all_but_the_field_texts() {
        let (mut d, rim, name, date, _) = coin();
        ok(&mut d, "object.lock", json!({"ids": [date], "locked": true}));
        ok(&mut d, "template.set_field", json!({"field": {"id": "name", "label": "Name", "kind": "text", "targets": [name]}}));
        ok(&mut d, "template.set_lock", json!({"lock_others": true}));
        assert!(d.is_locked(&rim) && d.is_locked(&date) && !d.is_locked(&name));
        assert!(run(&mut d, "object.delete", json!({"ids": [rim]})).is_err(), "the rest is locked");
        ok(&mut d, "template.fill", json!({"values": {"name": "Bo"}}));
        assert!(run(&mut d, "template.set_lock", json!({"lock_others": true})).is_err(), "no-op refused");
        ok(&mut d, "template.set_lock", json!({"lock_others": false}));
        assert!(!d.is_locked(&rim));
        assert!(d.is_locked(&date), "locked by hand before: stays locked");
    }
}
