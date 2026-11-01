//! LightBurn Text shapes ⇄ [`doc::Text`].
//!
//! Attributes seen in LightBurn 1.x-2.x files (the owner's samples; nucarve.com's notes show the same set):
//! `Font` (Qt's `QFont::toString`: family, point size, pixel size, style hint, weight, style, underline, strike out,
//! fixed pitch, raw mode; weight 50/75 in Qt 5's scale, 400/700 in Qt 6's, style 1 italic), `Str` (the text, lines
//! separated by a newline), `H` (font size: the saved outlines are exactly `H` per em), `LS` / `LnS` (HSpace /
//! VSpace, %), `Ah` / `Av` (Align X 0 left / 1 middle / 2 right; Align Y 1 middle, 0 top and 2 bottom assumed from
//! the order of LightBurn's list), `Weld` (0/1), `HasBackupPath` and, on copies made by arrays, `VariableOffset`.
//! LightBurn's attribute names for upper case, bend, distort, text on path, max width, squeeze, ignore empty vars and
//! the variable text mode are not known (no sample uses them), so those settings are not written; the outline
//! (`BackupPath`) still shows the text as OpenEtch rendered it, but LightBurn renders the text again from the
//! attributes when it has the font.

use doc::{AlignH, AlignV, FontSpec, Text};
use kurbo::BezPath;

use crate::model::num_like;

/// `(family, bold, italic)` of a Qt font string.
pub fn parse_font(s: &str) -> (String, bool, bool) {
    let parts: Vec<&str> = s.split(',').collect();
    let family = parts.first().map(|f| f.trim().to_string()).unwrap_or_default();
    let weight = parts.get(4).and_then(|w| w.trim().parse::<i32>().ok()).unwrap_or(50);
    // Qt 5 weights run 0..=99 (50 normal, 75 bold), Qt 6 ones 1..=1000 (400 normal, 700 bold).
    let bold = if weight > 99 { weight >= 600 } else { weight >= 63 };
    let italic = parts.get(5).and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(0) != 0;
    (family, bold, italic)
}

/// A Qt font string for `family` in the given style, keeping the other fields (and the weight scale) of `source`.
pub fn font_string(family: &str, bold: bool, italic: bool, source: Option<&str>) -> String {
    let base = source.unwrap_or("Arial,-1,100,5,50,0,0,0,0,0");
    let mut parts: Vec<String> = base.split(',').map(str::to_string).collect();
    while parts.len() < 10 {
        parts.push("0".into());
    }
    let qt6 = parts[4].trim().parse::<i32>().is_ok_and(|w| w > 99);
    parts[0] = family.replace(',', " ");
    parts[4] = match (qt6, bold) {
        (true, true) => "700",
        (true, false) => "400",
        (false, true) => "75",
        (false, false) => "50",
    }
    .into();
    parts[5] = if italic { "1" } else { "0" }.into();
    parts.join(",")
}

/// The text model of a Text shape with attributes `attr`, its outline `outline` (the `BackupPath`). `None` when the
/// size is unusable, so the caller keeps the shape as a plain outline.
pub fn from_attributes(attr: &dyn Fn(&str) -> Option<String>, outline: BezPath) -> Option<Text> {
    let num = |k: &str| attr(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let height = num("H").filter(|h| *h > 0.0 && *h <= 10_000.0)?;
    let (family, bold, italic) = parse_font(&attr("Font").unwrap_or_default());
    let mut t = Text::new(attr("Str").unwrap_or_default(), FontSpec::system(family), height);
    t.h_spacing = num("LS").unwrap_or(0.0).clamp(-100.0, 1000.0);
    t.v_spacing = num("LnS").unwrap_or(0.0).clamp(-100.0, 1000.0);
    t.align_h = match num("Ah").map(|v| v as i64) {
        Some(0) => AlignH::Left,
        Some(2) => AlignH::Right,
        _ => AlignH::Middle,
    };
    t.align_v = match num("Av").map(|v| v as i64) {
        Some(0) => AlignV::Top,
        Some(2) => AlignV::Bottom,
        _ => AlignV::Middle,
    };
    t.welded = attr("Weld").is_some_and(|w| w.trim() == "1");
    t.bold = bold;
    t.italic = italic;
    t.outline = outline;
    Some(t)
}

/// Write `t`'s modelled attributes into `attrs` (the kept attributes in file order, or empty for a new text),
/// keeping each kept value's spelling while it reads the same. Returns the settings LightBurn's file cannot carry.
pub fn to_attributes(t: &Text, attrs: &mut Vec<(String, String)>) -> Vec<&'static str> {
    let kept = |attrs: &Vec<(String, String)>, k: &str| attrs.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    let new = attrs.is_empty();
    let font_src = kept(attrs, "Font");
    let font = match &font_src {
        Some(f) if parse_font(f) == (t.font.family.clone(), t.bold, t.italic) => f.clone(),
        _ => font_string(&t.font.family, t.bold, t.italic, font_src.as_deref()),
    };
    let ah = match t.align_h {
        AlignH::Left => 0.0,
        AlignH::Middle => 1.0,
        AlignH::Right => 2.0,
    };
    let av = match t.align_v {
        AlignV::Top => 0.0,
        AlignV::Middle => 1.0,
        AlignV::Bottom => 2.0,
    };
    let weld = if t.welded { "1" } else { "0" };
    // Set `k` to `v`. An attribute the source left out stays out while the value is what its absence means (the
    // import's default), so unchanged files keep their attribute set; new ones go before `HasBackupPath`, as
    // LightBurn orders them.
    let put = |attrs: &mut Vec<(String, String)>, k: &str, v: String, absent_means_this: bool| {
        match attrs.iter_mut().find(|(a, _)| a == k) {
            Some((_, old)) => *old = v,
            None if !new && absent_means_this => {}
            None => {
                let at = attrs.iter().position(|(a, _)| a == "HasBackupPath").unwrap_or(attrs.len());
                attrs.insert(at, (k.to_string(), v));
            }
        }
    };
    if new {
        put(attrs, "CutIndex", String::new(), false);
    }
    put(attrs, "Font", font, t.font.family.is_empty() && !t.bold && !t.italic);
    put(attrs, "Str", t.text.clone(), t.text.is_empty());
    let numbers = [("H", t.height_mm, f64::NAN), ("LS", t.h_spacing, 0.0), ("LnS", t.v_spacing, 0.0), ("Ah", ah, 1.0), ("Av", av, 1.0)];
    for (k, v, absent) in numbers {
        let src = kept(attrs, k);
        put(attrs, k, num_like(src.as_deref(), v), v == absent);
    }
    let weld_src = kept(attrs, "Weld");
    let weld_text = if weld_src.as_deref().map(str::trim) == Some(weld) { weld_src.unwrap_or_default() } else { weld.into() };
    put(attrs, "Weld", weld_text, !t.welded);
    if let Some(v) = &t.variable {
        let src = kept(attrs, "VariableOffset");
        let text = match src {
            Some(s) if s.trim().parse::<i64>().ok() == Some(v.offset) => s,
            _ => v.offset.to_string(),
        };
        // LightBurn writes it before HasBackupPath.
        match attrs.iter().position(|(a, _)| a == "VariableOffset") {
            Some(i) => attrs[i].1 = text,
            None => {
                let at = attrs.iter().position(|(a, _)| a == "HasBackupPath").unwrap_or(attrs.len());
                attrs.insert(at, ("VariableOffset".into(), text));
            }
        }
    }
    if !attrs.iter().any(|(a, _)| a == "HasBackupPath") {
        attrs.push(("HasBackupPath".into(), "1".into()));
    }
    let mut lost = Vec::new();
    if t.upper {
        lost.push("upper case");
    }
    if t.bend_deg != 0.0 {
        lost.push("bend");
    }
    if t.path.is_some() {
        lost.push("text on a path");
    }
    if t.max_width_mm > 0.0 {
        lost.push("max width");
    }
    if t.ignore_empty_vars {
        lost.push("ignore empty vars");
    }
    if t.variable.is_some() {
        lost.push("the variable text mode");
    }
    if t.font.kind != doc::FontKind::System {
        lost.push("its single-line font");
    }
    lost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qt_font_strings() {
        assert_eq!(parse_font("Arial,-1,100,5,50,0,0,0,0,0"), ("Arial".into(), false, false));
        assert_eq!(parse_font("Arial,-1,4096,5,400,0,0,0,0,0"), ("Arial".into(), false, false));
        assert_eq!(parse_font("Arial,-1,100,5,75,1,0,0,0,0"), ("Arial".into(), true, true));
        assert_eq!(parse_font("Segoe UI,-1,4096,5,700,0,0,0,0,0"), ("Segoe UI".into(), true, false));
        assert_eq!(font_string("Times New Roman", true, false, Some("Arial,-1,4096,5,400,0,0,0,0,0")), "Times New Roman,-1,4096,5,700,0,0,0,0,0");
        assert_eq!(font_string("Arial", false, true, None), "Arial,-1,100,5,50,1,0,0,0,0");
    }

    #[test]
    fn attributes_round_trip_with_their_spelling() {
        let src: Vec<(String, String)> = [
            ("CutIndex", "1"),
            ("Font", "Arial,-1,100,5,50,0,0,0,0,0"),
            ("Str", "Line one\nLine two"),
            ("H", "3.0123374"),
            ("LS", "0"),
            ("LnS", "0"),
            ("Ah", "1"),
            ("Av", "1"),
            ("Weld", "1"),
            ("VariableOffset", "2"),
            ("HasBackupPath", "1"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .to_vec();
        let get = |k: &str| src.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
        let t = from_attributes(&get, BezPath::new()).unwrap();
        assert_eq!((t.height_mm, t.align_h, t.align_v, t.welded), (3.0123374, AlignH::Middle, AlignV::Middle, true));
        let mut out = src.clone();
        assert!(to_attributes(&t, &mut out).is_empty());
        assert_eq!(out, src, "unchanged text writes its attributes back as read (VariableOffset kept)");
        let mut t2 = t.clone();
        t2.text = "New".into();
        t2.height_mm = 5.0;
        t2.align_h = AlignH::Right;
        t2.bold = true;
        let mut out = src.clone();
        to_attributes(&t2, &mut out);
        let get = |k: &str| out.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!((get("Str"), get("H"), get("Ah"), get("Font")), ("New".into(), "5".into(), "2".into(), "Arial,-1,100,5,75,0,0,0,0,0".into()));
        assert!(from_attributes(&|k| (k == "H").then(|| "0".to_string()), BezPath::new()).is_none());
    }

    #[test]
    fn new_texts_get_lightburns_attribute_set() {
        let mut t = Text::new("A", FontSpec::system("Arial"), 4.0);
        t.bend_deg = 30.0;
        let mut attrs = Vec::new();
        let lost = to_attributes(&t, &mut attrs);
        let names: Vec<&str> = attrs.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(names, ["CutIndex", "Font", "Str", "H", "LS", "LnS", "Ah", "Av", "Weld", "HasBackupPath"]);
        assert_eq!(lost, ["bend"]);
    }
}

/// The `<VariableText>` element's children OpenEtch models, with the [`doc::VariableState`] field each sets.
const VARIABLE_TAGS: [&str; 5] = ["Start", "End", "Current", "Increment", "AutoAdvance"];

fn tag_value<'a>(xml: &'a str, tag: &str) -> Option<(usize, &'a str)> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(k) = xml[from..].find(&open) {
        let at = from + k + open.len();
        if xml[at..].starts_with(|c: char| c.is_whitespace() || c == '/' || c == '>') {
            let end = at + xml[at..].find('>')?;
            let v = xml[at..end].find("Value=\"")? + at + "Value=\"".len();
            let close = v + xml[v..end].find('"')?;
            return Some((v, &xml[v..close]));
        }
        from = at;
    }
    None
}

/// The serial counter of a kept `<VariableText>` element (LightBurn's Variable Text window: Start, End, Current,
/// Increment = advance by, AutoAdvance). `None` when `xml` is not one.
pub fn variables_from_xml(xml: &str) -> Option<doc::VariableState> {
    if !xml.trim_start().starts_with("<VariableText") {
        return None;
    }
    let num = |t: &str| tag_value(xml, t).and_then(|(_, v)| v.trim().parse::<i64>().ok());
    let d = doc::VariableState::default();
    Some(doc::VariableState {
        start: num("Start").unwrap_or(d.start),
        end: num("End").unwrap_or(d.end),
        current: num("Current").unwrap_or(d.current),
        advance_by: num("Increment").unwrap_or(d.advance_by),
        auto_advance: num("AutoAdvance").is_some_and(|v| v != 0),
        csv: None,
    })
}

/// The variable text state a LightBurn import kept in the document's extras, if the file had a `<VariableText>`.
pub fn variables_from_extras(extras: &std::collections::BTreeMap<String, String>) -> Option<doc::VariableState> {
    extras
        .iter()
        .filter(|(k, _)| k.starts_with("lbrn2._el."))
        .find_map(|(_, v)| variables_from_xml(v))
}

/// A kept `<VariableText>` element with `v`'s values written in (each value's spelling kept while it is the same).
pub fn variables_into_xml(xml: &str, v: &doc::VariableState) -> String {
    let values = [v.start, v.end, v.current, v.advance_by, i64::from(v.auto_advance)];
    let mut out = xml.to_string();
    for (tag, want) in VARIABLE_TAGS.into_iter().zip(values) {
        if let Some((at, old)) = tag_value(&out, tag) {
            let same = old.trim().parse::<i64>().ok().map(|o| if tag == "AutoAdvance" { i64::from(o != 0) } else { o }) == Some(want);
            if !same {
                let len = old.len();
                out.replace_range(at..at + len, &want.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod variable_tests {
    use super::*;

    #[test]
    fn variable_text_element_round_trips() {
        let xml = "<VariableText>\n        <Start Value=\"0\"/>\n        <End Value=\"999\"/>\n        <Current Value=\"05\"/>\n        <Increment Value=\"1\"/>\n        <AutoAdvance Value=\"0\"/>\n    </VariableText>";
        let v = variables_from_xml(xml).unwrap();
        assert_eq!((v.start, v.end, v.current, v.advance_by, v.auto_advance), (0, 999, 5, 1, false));
        assert_eq!(variables_into_xml(xml, &v), xml, "unchanged values keep their spelling");
        let v2 = doc::VariableState { current: 42, auto_advance: true, ..v };
        let out = variables_into_xml(xml, &v2);
        assert!(out.contains("<Current Value=\"42\"/>") && out.contains("<AutoAdvance Value=\"1\"/>"), "{out}");
        assert_eq!(variables_from_xml(&out).unwrap(), v2);
        assert!(variables_from_xml("<Notes/>").is_none());
    }
}
