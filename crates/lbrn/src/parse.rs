//! `.lbrn2` (and `.clb` library) text → [`LbrnProject`]. Bounded: input size, XML nodes, nesting depth, total path
//! vertices and total path segments (a shared `VertList` / `PrimList` counts once per use, since every use is resolved
//! into its own copy) are all capped, and DTDs are refused.
//!
//! Besides the values, the parser keeps what [`crate::write`] needs to write the file back as it was: the root's
//! child order, attribute order, the text of numbers and `XForm`s, each path's ids and which lists it carries, the
//! source text of elements OpenEtch does not model, and the declaration and line endings.

use std::collections::HashMap;

use kurbo::Affine;
use roxmltree::{Node, ParsingOptions};

use crate::LbrnError;
use crate::model::{
    Backup, CutParams, CutSetting, Element, Encoded, Format, LbrnProject, PathData, RootItem, Shape, ShapeKind,
    SubLayer,
};
use crate::path::{parse_prims, parse_vertices};

/// Largest accepted file. Embedded bitmaps make real projects a few MB; the engine's upload limit is 50 MB.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: u32 = 4_000_000;
const MAX_DEPTH: usize = 64;
/// Total vertices over all paths, counting shared lists once per use.
const MAX_VERTICES: usize = 4_000_000;
/// Total path segments (`PrimList` entries; a `LineOpen`/`LineClosed` list counts its lines) over all paths, counting
/// shared lists once per use. A few hundred bytes reusing one large list by `PrimID` would otherwise expand to
/// gigabytes.
pub const MAX_SEGMENTS: usize = 2_000_000;

fn malformed(m: impl Into<String>) -> LbrnError {
    LbrnError::Malformed(m.into())
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>, LbrnError> {
    if text.len() > MAX_BYTES {
        return Err(LbrnError::TooLarge(format!("the file is larger than {} MB", MAX_BYTES / (1024 * 1024))));
    }
    let opt = ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..ParsingOptions::default() };
    roxmltree::Document::parse_with_options(text.trim_start_matches('\u{feff}'), opt)
        .map_err(|e| LbrnError::Xml(e.to_string()))
}

/// The source text of `n`, line breaks normalised to `\n`.
fn raw(n: Node<'_, '_>) -> String {
    let text = &n.document().input_text()[n.range()];
    if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.to_string() }
}

/// How the file is laid out (declaration, line endings, empty-element style).
fn format_of(doc: &roxmltree::Document<'_>) -> Format {
    let text = doc.input_text();
    let root = doc.root_element();
    let before = text[..root.range().start].trim();
    let declaration = before
        .starts_with("<?xml")
        .then(|| before.find("?>").map(|end| before[..end + 2].to_string()))
        .flatten();
    let after = &text[root.range().end..];
    Format {
        declaration,
        crlf: text.contains("\r\n"),
        final_newline: after.starts_with('\n') || after.starts_with("\r\n"),
        space_before_slash: text.matches("\" />").count() > text.matches("\"/>").count(),
    }
}

/// Parse a LightBurn project (`.lbrn2`, or a `.lbt` template, which has the same layout).
pub fn parse(text: &str) -> Result<LbrnProject, LbrnError> {
    let doc = xml(text)?;
    let root = doc.root_element();
    match root.tag_name().name() {
        "LightBurnProject" => {}
        "LightBurnLibrary" => {
            return Err(LbrnError::NotProject(
                "this is a LightBurn material library (.clb), not a project (.lbrn2)".into(),
            ));
        }
        other => return Err(LbrnError::NotProject(format!("the root element is <{other}>, not <LightBurnProject>"))),
    }

    let mut cx = Cx::default();
    // Shared lists: the first element carrying a VertID/PrimID together with the list defines it.
    for n in root.descendants().filter(Node::is_element) {
        if let (Some(id), Some(list)) = (n.attribute("VertID"), child(n, "VertList")) {
            cx.vert_defs.entry(id).or_insert(list.text().unwrap_or_default());
        }
        if let (Some(id), Some(list)) = (n.attribute("PrimID"), child(n, "PrimList")) {
            cx.prim_defs.entry(id).or_insert(list.text().unwrap_or_default());
        }
    }

    let mut p = LbrnProject {
        attributes: root.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect(),
        format: format_of(&doc),
        ..LbrnProject::default()
    };
    for n in root.children().filter(Node::is_element) {
        match n.tag_name().name() {
            "CutSetting" | "CutSetting_Img" => {
                p.order.push(RootItem::CutSetting(p.cut_settings.len()));
                p.cut_settings.push(cut_setting(n)?);
            }
            "Shape" => {
                p.order.push(RootItem::Shape(p.shapes.len()));
                p.shapes.push(cx.shape(n, 0)?);
            }
            _ => {
                p.order.push(RootItem::Element(p.extra_elements.len()));
                p.extra_elements.push(element(n, 0, true)?);
            }
        }
    }
    Ok(p)
}

/// One `<Entry>` of a `.clb` material library.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    /// `Material name`.
    pub material: String,
    /// `Entry NoThickTitle` (or the thickness when given) and `Desc`.
    pub title: String,
    pub description: String,
    pub setting: CutSetting,
}

/// Parse a LightBurn material library (`.clb`): every `Material/Entry/CutSetting`.
pub fn parse_library(text: &str) -> Result<Vec<LibraryEntry>, LbrnError> {
    let doc = xml(text)?;
    let root = doc.root_element();
    if !root.has_tag_name("LightBurnLibrary") {
        return Err(LbrnError::NotProject(format!(
            "the root element is <{}>, not <LightBurnLibrary>",
            root.tag_name().name()
        )));
    }
    let mut out = Vec::new();
    for m in root.children().filter(|n| n.has_tag_name("Material")) {
        for e in m.children().filter(|n| n.has_tag_name("Entry")) {
            for c in e.children().filter(|n| n.has_tag_name("CutSetting") || n.has_tag_name("CutSetting_Img")) {
                let thickness = e.attribute("Thickness").and_then(|t| t.parse::<f64>().ok()).filter(|t| *t > 0.0);
                out.push(LibraryEntry {
                    material: m.attribute("name").unwrap_or_default().to_string(),
                    title: match thickness {
                        Some(t) => format!("{t} mm"),
                        None => e.attribute("NoThickTitle").unwrap_or_default().to_string(),
                    },
                    description: e.attribute("Desc").unwrap_or_default().to_string(),
                    setting: cut_setting(c)?,
                });
            }
        }
    }
    Ok(out)
}

fn child<'a, 'i>(n: Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(tag))
}

fn number(s: &str, what: &str) -> Result<f64, LbrnError> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| malformed(format!("{what} '{s}' is not a finite number")))
}

fn attr_number(n: Node<'_, '_>, key: &str) -> Result<f64, LbrnError> {
    let v = n.attribute(key).ok_or_else(|| {
        malformed(format!("<{} Type=\"{}\"> has no {key}", n.tag_name().name(), n.attribute("Type").unwrap_or("")))
    })?;
    number(v, key)
}

fn index(s: &str, what: &str) -> Result<u32, LbrnError> {
    s.trim().parse().map_err(|_| malformed(format!("{what} '{s}' is not a layer index")))
}

/// The `XForm` child as a transform (identity when absent) and its text.
fn xform(n: Node<'_, '_>) -> Result<(Affine, Option<String>), LbrnError> {
    let Some(x) = child(n, "XForm") else { return Ok((Affine::IDENTITY, None)) };
    let text = x.text().unwrap_or_default();
    Ok((parse_xform(text)?, Some(text.to_string())))
}

/// `a b c d e f` → the transform.
pub(crate) fn parse_xform(text: &str) -> Result<Affine, LbrnError> {
    let v = text.split_whitespace().map(|t| number(t, "XForm value")).collect::<Result<Vec<_>, _>>()?;
    let v: [f64; 6] = v.try_into().map_err(|_| malformed(format!("XForm '{text}' needs six numbers")))?;
    Ok(Affine::new(v))
}

/// Every attribute but `Type`, in file order.
fn attributes(n: Node<'_, '_>) -> Vec<(String, String)> {
    n.attributes().filter(|a| a.name() != "Type").map(|a| (a.name().to_string(), a.value().to_string())).collect()
}

fn cut_setting(n: Node<'_, '_>) -> Result<CutSetting, LbrnError> {
    let kind = n.attribute("type").unwrap_or_default().to_string();
    let mut values = Vec::new();
    let mut idx = None;
    let mut sub_layers = Vec::new();
    for c in n.children().filter(Node::is_element) {
        if c.has_tag_name("SubLayer") {
            sub_layers.push(SubLayer {
                kind: c.attribute("type").unwrap_or_default().to_string(),
                index: c.attribute("index").map(|i| index(i, "SubLayer index")).transpose()?.unwrap_or(0),
                params: params(c)?,
            });
            continue;
        }
        let tag = c.tag_name().name();
        let value = c.attribute("Value").unwrap_or_default();
        if tag == "index" {
            idx = Some(index(value, "CutSetting index")?);
        } else {
            values.push((tag, value));
        }
    }
    Ok(CutSetting {
        kind,
        image: n.has_tag_name("CutSetting_Img"),
        index: idx.ok_or_else(|| malformed("a CutSetting has no index"))?,
        params: CutParams::from_values(values),
        sub_layers,
    })
}

fn params(n: Node<'_, '_>) -> Result<CutParams, LbrnError> {
    let values: Vec<(&str, &str)> = n
        .children()
        .filter(Node::is_element)
        .map(|c| (c.tag_name().name(), c.attribute("Value").unwrap_or_default()))
        .collect();
    Ok(CutParams::from_values(values))
}

/// A generic element. `keep_raw`: also keep its source text (for elements written back on their own; their
/// children are covered by it).
fn element(n: Node<'_, '_>, depth: usize, keep_raw: bool) -> Result<Element, LbrnError> {
    if depth > MAX_DEPTH {
        return Err(LbrnError::TooLarge(format!("elements nested deeper than {MAX_DEPTH}")));
    }
    let text: String = n.children().filter(Node::is_text).filter_map(|t| t.text()).collect();
    let text = text.trim();
    Ok(Element {
        name: n.tag_name().name().to_string(),
        attributes: n.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect(),
        text: (!text.is_empty()).then(|| text.to_string()),
        children: n
            .children()
            .filter(Node::is_element)
            .map(|c| element(c, depth + 1, false))
            .collect::<Result<_, _>>()?,
        raw: keep_raw.then(|| raw(n)),
    })
}

/// A per-list size error restated as the file-wide limit it hit.
fn total(e: LbrnError, what: String) -> LbrnError {
    match e {
        LbrnError::TooLarge(_) => LbrnError::TooLarge(format!("{what} (a shared list counts once per use)")),
        other => other,
    }
}

#[derive(Default)]
struct Cx<'a> {
    vert_defs: HashMap<&'a str, &'a str>,
    prim_defs: HashMap<&'a str, &'a str>,
    vertices: usize,
    segments: usize,
}

impl<'a> Cx<'a> {
    fn shape(&mut self, n: Node<'a, '_>, depth: usize) -> Result<Shape, LbrnError> {
        if depth > MAX_DEPTH {
            return Err(LbrnError::TooLarge(format!("groups nested deeper than {MAX_DEPTH}")));
        }
        let type_name = n.attribute("Type").ok_or_else(|| malformed("a Shape has no Type"))?;
        let cut_index = n.attribute("CutIndex").map(|i| index(i, "CutIndex")).transpose()?;
        let (kind, modelled_children): (ShapeKind, &[&str]) = match type_name {
            "Rect" => (
                ShapeKind::Rect {
                    w: attr_number(n, "W")?,
                    h: attr_number(n, "H")?,
                    cr: n.attribute("Cr").map(|c| number(c, "Cr")).transpose()?.unwrap_or(0.0),
                },
                &[],
            ),
            "Ellipse" => (ShapeKind::Ellipse { rx: attr_number(n, "Rx")?, ry: attr_number(n, "Ry")? }, &[]),
            "Path" => (ShapeKind::Path(self.path(n)?), &["VertList", "PrimList"]),
            "Group" => {
                let mut children = Vec::new();
                if let Some(c) = child(n, "Children") {
                    for s in c.children().filter(|c| c.has_tag_name("Shape")) {
                        children.push(self.shape(s, depth + 1)?);
                    }
                }
                (ShapeKind::Group(children), &["Children"])
            }
            "Text" => {
                let backup = match child(n, "BackupPath") {
                    Some(b) => {
                        // Some versions nest the outline in a Shape inside BackupPath.
                        let src = child(b, "Shape").unwrap_or(b);
                        let (xform, xform_text) = xform(src)?;
                        Some(Backup {
                            cut_index: src.attribute("CutIndex").map(|i| index(i, "CutIndex")).transpose()?,
                            xform,
                            xform_text,
                            attributes: attributes(src),
                            path: self.path(src)?,
                        })
                    }
                    None => None,
                };
                (ShapeKind::Text { backup }, &["BackupPath"])
            }
            "Bitmap" => (ShapeKind::Bitmap { w: attr_number(n, "W")?, h: attr_number(n, "H")? }, &[]),
            other => (ShapeKind::Other { type_name: other.to_string() }, &[]),
        };
        let children = n
            .children()
            .filter(|c| c.is_element() && !c.has_tag_name("XForm"))
            .filter(|c| !modelled_children.contains(&c.tag_name().name()))
            .map(|c| element(c, depth + 1, true))
            .collect::<Result<_, _>>()?;
        let (xform, xform_text) = xform(n)?;
        Ok(Shape { kind, cut_index, xform, xform_text, attributes: attributes(n), children })
    }

    /// The path's own lists, or the shared ones its `VertID`/`PrimID` refer to.
    fn path(&mut self, n: Node<'a, '_>) -> Result<PathData, LbrnError> {
        let own_verts = child(n, "VertList").map(|l| l.text().unwrap_or_default());
        let vert_text = match (own_verts, n.attribute("VertID")) {
            (Some(t), _) => t,
            (None, Some(id)) => {
                self.vert_defs.get(id).copied().ok_or_else(|| malformed(format!("VertID {id} is never defined")))?
            }
            (None, None) => return Err(malformed("a Path has no VertList")),
        };
        let vertices = parse_vertices(vert_text, MAX_VERTICES - self.vertices)
            .map_err(|e| total(e, format!("more than {MAX_VERTICES} path vertices in total")))?;
        self.vertices += vertices.len();
        let own_prims = child(n, "PrimList").map(|l| l.text().unwrap_or_default());
        let prim_text = match (own_prims, n.attribute("PrimID")) {
            (Some(t), _) => t,
            (None, Some(id)) => {
                self.prim_defs.get(id).copied().ok_or_else(|| malformed(format!("PrimID {id} is never defined")))?
            }
            (None, None) => return Err(malformed("a Path has no PrimList")),
        };
        let (prims, segments) = parse_prims(prim_text, vertices.len(), MAX_SEGMENTS - self.segments)
            .map_err(|e| total(e, format!("more than {MAX_SEGMENTS} path segments in total")))?;
        self.segments += segments;
        let encoded = Encoded {
            vert_id: n.attribute("VertID").map(str::to_string),
            prim_id: n.attribute("PrimID").map(str::to_string),
            vert_list: own_verts.map(str::to_string),
            prim_list: own_prims.map(str::to_string),
        };
        Ok(PathData { vertices, prims, encoded: Some(encoded) })
    }
}
