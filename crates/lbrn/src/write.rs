//! [`LbrnProject`] → `.lbrn2` text, laid out like LightBurn's own files: XML declaration, root attributes, then the
//! root's children in [`LbrnProject::order`] (else the extra elements, `CutSetting`s, `Shape`s, then `Notes`), four
//! spaces of indentation per level, `<tag Value="..."/>` values. What the parser kept is written back as it was read:
//! source text of unmodelled elements, attribute order, the spelling of unchanged numbers and `XForm`s, path ids and
//! shared lists, the declaration and line endings. New paths get fresh `VertID`/`PrimID` values above every id
//! already used, with their lists inline.

use std::collections::HashSet;

use kurbo::Affine;

use crate::model::{
    CutParams, CutSetting, Element, Encoded, Format, LbrnProject, PathData, RootItem, Shape, ShapeKind, num, num_like,
};
use crate::path::{write_prims, write_vertices};

/// Write a project as `.lbrn2` XML.
pub fn write(p: &LbrnProject) -> String {
    let mut w = Writer { out: String::with_capacity(4096), next_id: first_free_id(p), format: p.format.clone() };
    w.out.push_str(p.format.declaration.as_deref().unwrap_or("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    w.out.push('\n');
    w.open(0, "LightBurnProject", &p.attributes, false);
    for item in layout(p) {
        match item {
            RootItem::CutSetting(i) => w.cut_setting(1, &p.cut_settings[i]),
            RootItem::Shape(i) => w.shape(1, &p.shapes[i]),
            RootItem::Element(i) => w.element(1, &p.extra_elements[i]),
        }
    }
    w.out.push_str("</LightBurnProject>");
    if p.format.final_newline {
        w.out.push('\n');
    }
    if p.format.crlf { w.out.replace('\n', "\r\n") } else { w.out }
}

/// The order to write the root's children in: [`LbrnProject::order`] when it names every item exactly once, else
/// LightBurn's usual layout.
fn layout(p: &LbrnProject) -> Vec<RootItem> {
    let total = p.cut_settings.len() + p.shapes.len() + p.extra_elements.len();
    let valid = |i: &RootItem| match *i {
        RootItem::CutSetting(k) => k < p.cut_settings.len(),
        RootItem::Shape(k) => k < p.shapes.len(),
        RootItem::Element(k) => k < p.extra_elements.len(),
    };
    let unique: HashSet<RootItem> = p.order.iter().copied().filter(valid).collect();
    if p.order.len() == total && unique.len() == total {
        return p.order.clone();
    }
    let (tail, head): (Vec<usize>, Vec<usize>) =
        (0..p.extra_elements.len()).partition(|i| p.extra_elements[*i].name == "Notes");
    head.into_iter()
        .map(RootItem::Element)
        .chain((0..p.cut_settings.len()).map(RootItem::CutSetting))
        .chain((0..p.shapes.len()).map(RootItem::Shape))
        .chain(tail.into_iter().map(RootItem::Element))
        .collect()
}

/// One above the largest numeric `VertID`/`PrimID` the project keeps, so fresh ids never collide with them.
fn first_free_id(p: &LbrnProject) -> u64 {
    fn visit(s: &Shape, max: &mut u64) {
        let path = match &s.kind {
            ShapeKind::Path(d) => Some(d),
            ShapeKind::Text { backup: Some(b) } => Some(&b.path),
            ShapeKind::Group(children) => {
                children.iter().for_each(|c| visit(c, max));
                None
            }
            _ => None,
        };
        if let Some(e) = path.and_then(|d| d.encoded.as_ref()) {
            for id in [&e.vert_id, &e.prim_id].into_iter().flatten() {
                if let Ok(n) = id.trim().parse::<u64>() {
                    *max = (*max).max(n.saturating_add(1));
                }
            }
        }
    }
    let mut max = 0;
    p.shapes.iter().for_each(|s| visit(s, &mut max));
    max
}

struct Writer {
    out: String,
    next_id: u64,
    format: Format,
}

/// Whether XML 1.0 allows `c` in a document at all (its `Char` production). Other characters are dropped on writing,
/// since no escape can express them.
fn xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// Whether `name` can be written as an element or attribute name without changing the markup around it. Names come
/// from parsed XML or from [`doc::is_plain_name`]-checked extras; this guards the public model against anything else.
fn safe_name(name: &str) -> bool {
    name.chars().next().is_some_and(|c| !c.is_ascii_digit() && !matches!(c, '-' | '.'))
        && name.chars().all(|c| {
            xml_char(c) && !c.is_whitespace() && !matches!(c, '<' | '>' | '&' | '"' | '\'' | '/' | '=' | '?' | '!')
        })
}

pub(crate) fn escape_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| xml_char(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            '\t' => o.push_str("&#9;"),
            c => o.push(c),
        }
    }
    o
}

fn escape_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars().filter(|c| xml_char(*c)) {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c => o.push(c),
        }
    }
    o
}

/// `a b c d e f`, or `text` when it reads as exactly `a`.
fn xform_text(a: Affine, text: Option<&str>) -> String {
    if let Some(t) = text
        && crate::parse::parse_xform(t).is_ok_and(|b| b.as_coeffs() == a.as_coeffs())
    {
        return t.to_string();
    }
    a.as_coeffs().map(num).join(" ")
}

/// Source text that is safe to paste into the output as one element: it must start a tag and must not hold
/// characters XML cannot carry. (Raw text comes from parsed files and is well-formed; this guards the public model.)
fn raw_ok(raw: &str) -> bool {
    raw.starts_with('<') && raw.ends_with('>') && raw.chars().all(xml_char)
}

impl Writer {
    fn indent(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
    }

    fn empty_end(&self) -> &'static str {
        if self.format.space_before_slash { " />\n" } else { "/>\n" }
    }

    fn open(&mut self, depth: usize, name: &str, attrs: &[(String, String)], empty: bool) {
        self.indent(depth);
        self.out.push('<');
        self.out.push_str(name);
        self.attributes(attrs);
        let end = if empty { self.empty_end() } else { ">\n" };
        self.out.push_str(end);
    }

    fn attributes(&mut self, attrs: &[(String, String)]) {
        for (k, v) in attrs.iter().filter(|(k, _)| safe_name(k)) {
            self.out.push_str(&format!(" {k}=\"{}\"", escape_attr(v)));
        }
    }

    fn close(&mut self, depth: usize, name: &str) {
        self.indent(depth);
        self.out.push_str(&format!("</{name}>\n"));
    }

    fn text_element(&mut self, depth: usize, name: &str, text: &str) {
        self.indent(depth);
        self.out.push_str(&format!("<{name}>{}</{name}>\n", escape_text(text)));
    }

    fn value(&mut self, depth: usize, tag: &str, value: &str) {
        self.indent(depth);
        self.out.push_str(&format!("<{tag} Value=\"{}\"", escape_attr(value)));
        let end = self.empty_end();
        self.out.push_str(end);
    }

    fn element(&mut self, depth: usize, e: &Element) {
        if let Some(raw) = e.raw.as_deref().filter(|r| raw_ok(r)) {
            self.indent(depth);
            self.out.push_str(raw);
            self.out.push('\n');
            return;
        }
        if !safe_name(&e.name) {
            return;
        }
        match (&e.text, e.children.is_empty()) {
            (None, true) => self.open(depth, &e.name, &e.attributes, true),
            (Some(t), true) => {
                self.indent(depth);
                self.out.push('<');
                self.out.push_str(&e.name);
                self.attributes(&e.attributes);
                self.out.push_str(&format!(">{}</{}>\n", escape_text(t), e.name));
            }
            (text, false) => {
                self.open(depth, &e.name, &e.attributes, false);
                if let Some(t) = text {
                    self.indent(depth + 1);
                    self.out.push_str(&escape_text(t));
                    self.out.push('\n');
                }
                for c in &e.children {
                    self.element(depth + 1, c);
                }
                self.close(depth, &e.name);
            }
        }
    }

    fn params(&mut self, depth: usize, index: Option<u32>, p: &CutParams) {
        if let Some(i) = index {
            self.value(depth, "index", &i.to_string());
        }
        for (tag, value) in p.to_values().into_iter().filter(|(tag, _)| safe_name(tag)) {
            self.value(depth, &tag, &value);
        }
    }

    fn cut_setting(&mut self, depth: usize, c: &CutSetting) {
        let name = if c.image { "CutSetting_Img" } else { "CutSetting" };
        self.open(depth, name, &[("type".into(), c.kind.clone())], false);
        self.params(depth + 1, Some(c.index), &c.params);
        for s in &c.sub_layers {
            let attrs = [("type".to_string(), s.kind.clone()), ("index".to_string(), s.index.to_string())];
            self.open(depth + 1, "SubLayer", &attrs, false);
            self.params(depth + 2, None, &s.params);
            self.close(depth + 1, "SubLayer");
        }
        self.close(depth, name);
    }

    fn fresh_id(&mut self) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        id
    }

    /// The ids and lists to write for a path: as read, or both lists for a new path, with fresh ids when `ids` (a
    /// Text's backup outline has none in LightBurn's files).
    fn encoding(&mut self, d: &PathData, ids: bool) -> Encoded {
        match &d.encoded {
            Some(e) => e.clone(),
            None => {
                let id = ids.then(|| self.fresh_id());
                Encoded {
                    vert_id: id.clone(),
                    prim_id: id,
                    vert_list: Some(write_vertices(&d.vertices)),
                    prim_list: Some(write_prims(&d.prims)),
                }
            }
        }
    }

    fn path_lists(&mut self, depth: usize, e: &Encoded) {
        if let Some(v) = &e.vert_list {
            self.text_element(depth, "VertList", v);
        }
        if let Some(p) = &e.prim_list {
            self.text_element(depth, "PrimList", p);
        }
    }

    /// `Type`, then `attributes` in their order with the modelled ones (`model`, in the model's spelling unless the
    /// source spelling reads the same) in place, then modelled ones the source did not have.
    fn shape_attributes(
        type_name: &str,
        attributes: &[(String, String)],
        model: Vec<(&'static str, Option<ModelValue>)>,
    ) -> Vec<(String, String)> {
        let mut out = vec![("Type".to_string(), type_name.to_string())];
        let modelled = |k: &str| model.iter().find(|(m, _)| *m == k);
        for (k, v) in attributes {
            match modelled(k) {
                Some((_, Some(m))) => out.push((k.clone(), m.text(Some(v)))),
                Some((_, None)) => {}
                None if k == "Type" => {}
                None => out.push((k.clone(), v.clone())),
            }
        }
        for (k, m) in &model {
            if let Some(m) = m
                && !attributes.iter().any(|(a, _)| a == k)
            {
                out.push(((*k).to_string(), m.text(None)));
            }
        }
        out
    }

    fn shape(&mut self, depth: usize, s: &Shape) {
        let cut = s.cut_index.map(|i| ModelValue::Int(u64::from(i)));
        let mut model: Vec<(&'static str, Option<ModelValue>)> = vec![("CutIndex", cut)];
        let mut path_encoding = None;
        match &s.kind {
            ShapeKind::Rect { w, h, cr } => model.extend([
                ("W", Some(ModelValue::Num(*w))),
                ("H", Some(ModelValue::Num(*h))),
                ("Cr", Some(ModelValue::Num(*cr))),
            ]),
            ShapeKind::Ellipse { rx, ry } => {
                model.extend([("Rx", Some(ModelValue::Num(*rx))), ("Ry", Some(ModelValue::Num(*ry)))]);
            }
            ShapeKind::Bitmap { w, h } => {
                model.extend([("W", Some(ModelValue::Num(*w))), ("H", Some(ModelValue::Num(*h)))]);
            }
            ShapeKind::Path(d) => {
                let e = self.encoding(d, true);
                model.extend([
                    ("VertID", e.vert_id.clone().map(ModelValue::Text)),
                    ("PrimID", e.prim_id.clone().map(ModelValue::Text)),
                ]);
                path_encoding = Some(e);
            }
            ShapeKind::Group(_) | ShapeKind::Text { .. } | ShapeKind::Other { .. } => {}
        }
        let attrs = Self::shape_attributes(s.type_name(), &s.attributes, model);
        self.open(depth, "Shape", &attrs, false);
        let d = depth + 1;
        let xform = xform_text(s.xform, s.xform_text.as_deref());
        match &s.kind {
            ShapeKind::Text { backup: Some(b) } => {
                let e = self.encoding(&b.path, false);
                let model = vec![
                    ("CutIndex", b.cut_index.map(|i| ModelValue::Int(u64::from(i)))),
                    ("VertID", e.vert_id.clone().map(ModelValue::Text)),
                    ("PrimID", e.prim_id.clone().map(ModelValue::Text)),
                ];
                let battrs = Self::shape_attributes("Path", &b.attributes, model);
                self.open(d, "BackupPath", &battrs, false);
                self.text_element(d + 1, "XForm", &xform_text(b.xform, b.xform_text.as_deref()));
                self.path_lists(d + 1, &e);
                self.close(d, "BackupPath");
                self.text_element(d, "XForm", &xform);
            }
            ShapeKind::Path(_) => {
                self.text_element(d, "XForm", &xform);
                if let Some(e) = &path_encoding {
                    self.path_lists(d, e);
                }
            }
            ShapeKind::Group(children) => {
                self.text_element(d, "XForm", &xform);
                self.open(d, "Children", &[], false);
                for c in children {
                    self.shape(d + 1, c);
                }
                self.close(d, "Children");
            }
            _ => self.text_element(d, "XForm", &xform),
        }
        for c in &s.children {
            self.element(d, c);
        }
        self.close(depth, "Shape");
    }
}

/// A modelled attribute's value.
enum ModelValue {
    Num(f64),
    Int(u64),
    Text(String),
}

impl ModelValue {
    /// The text to write, keeping `source`'s spelling when it reads as the same value.
    fn text(&self, source: Option<&str>) -> String {
        match self {
            ModelValue::Num(v) => num_like(source, *v),
            ModelValue::Int(n) => match source {
                Some(s) if s.trim().parse::<u64>().ok() == Some(*n) => s.to_string(),
                _ => n.to_string(),
            },
            ModelValue::Text(t) => t.clone(),
        }
    }
}
