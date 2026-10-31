//! [`LbrnProject`] → `.lbrn2` text, laid out like LightBurn's own files: XML declaration, root attributes, the
//! kept root elements, `CutSetting`s, `Shape`s, then `Notes`. No thumbnail is written. Every path gets fresh, unique
//! `VertID`/`PrimID` values and its lists inline.

use kurbo::Affine;

use crate::model::{CutParams, CutSetting, Element, LbrnProject, PathData, Shape, ShapeKind, num};
use crate::path::{write_prims, write_vertices};

/// Write a project as `.lbrn2` XML.
pub fn write(p: &LbrnProject) -> String {
    let mut w = Writer { out: String::with_capacity(4096), next_id: 0 };
    w.out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    w.open(0, "LightBurnProject", &p.attributes, false);
    let (tail, head): (Vec<&Element>, Vec<&Element>) = p.extra_elements.iter().partition(|e| e.name == "Notes");
    for e in head {
        w.element(1, e);
    }
    for c in &p.cut_settings {
        w.cut_setting(1, c);
    }
    for s in &p.shapes {
        w.shape(1, s);
    }
    for e in tail {
        w.element(1, e);
    }
    w.out.push_str("</LightBurnProject>\n");
    w.out
}

struct Writer {
    out: String,
    next_id: u64,
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

fn escape_attr(s: &str) -> String {
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

fn xform_text(a: Affine) -> String {
    a.as_coeffs().map(num).join(" ")
}

impl Writer {
    fn indent(&mut self, depth: usize) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
    }

    fn open(&mut self, depth: usize, name: &str, attrs: &[(String, String)], empty: bool) {
        self.indent(depth);
        self.out.push('<');
        self.out.push_str(name);
        self.attributes(attrs);
        self.out.push_str(if empty { "/>\n" } else { ">\n" });
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
        self.out.push_str(&format!("<{tag} Value=\"{}\"/>\n", escape_attr(value)));
    }

    fn element(&mut self, depth: usize, e: &Element) {
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

    fn path_lists(&mut self, depth: usize, d: &PathData) {
        self.text_element(depth, "VertList", &write_vertices(&d.vertices));
        self.text_element(depth, "PrimList", &write_prims(&d.prims));
    }

    fn shape(&mut self, depth: usize, s: &Shape) {
        let mut attrs: Vec<(String, String)> = vec![("Type".into(), s.type_name().to_string())];
        if let Some(i) = s.cut_index {
            attrs.push(("CutIndex".into(), i.to_string()));
        }
        match &s.kind {
            ShapeKind::Rect { w, h, cr } => {
                attrs.extend([("W".into(), num(*w)), ("H".into(), num(*h)), ("Cr".into(), num(*cr))]);
            }
            ShapeKind::Ellipse { rx, ry } => attrs.extend([("Rx".into(), num(*rx)), ("Ry".into(), num(*ry))]),
            ShapeKind::Bitmap { w, h } => attrs.extend([("W".into(), num(*w)), ("H".into(), num(*h))]),
            ShapeKind::Path(_) => {
                let id = self.next_id.to_string();
                self.next_id += 1;
                attrs.extend([("VertID".into(), id.clone()), ("PrimID".into(), id)]);
            }
            ShapeKind::Group(_) | ShapeKind::Text { .. } | ShapeKind::Other { .. } => {}
        }
        attrs.extend(s.attributes.iter().cloned());
        self.open(depth, "Shape", &attrs, false);
        let d = depth + 1;
        match &s.kind {
            ShapeKind::Text { backup: Some(b) } => {
                let mut battrs = vec![("Type".to_string(), "Path".to_string())];
                if let Some(i) = b.cut_index {
                    battrs.push(("CutIndex".into(), i.to_string()));
                }
                self.open(d, "BackupPath", &battrs, false);
                self.text_element(d + 1, "XForm", &xform_text(b.xform));
                self.path_lists(d + 1, &b.path);
                self.close(d, "BackupPath");
                self.text_element(d, "XForm", &xform_text(s.xform));
            }
            ShapeKind::Path(p) => {
                self.text_element(d, "XForm", &xform_text(s.xform));
                self.path_lists(d, p);
            }
            ShapeKind::Group(children) => {
                self.text_element(d, "XForm", &xform_text(s.xform));
                self.open(d, "Children", &[], false);
                for c in children {
                    self.shape(d + 1, c);
                }
                self.close(d, "Children");
            }
            _ => self.text_element(d, "XForm", &xform_text(s.xform)),
        }
        for c in &s.children {
            self.element(d, c);
        }
        self.close(depth, "Shape");
    }
}
