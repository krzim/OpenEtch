//! `.lbrn2` (and `.clb` library) text → [`LbrnProject`]. Bounded: input size, XML nodes, nesting depth, and total
//! path vertices (a shared `VertList` counts once per use) are all capped, and DTDs are refused.

use std::collections::HashMap;

use kurbo::Affine;
use roxmltree::{Node, ParsingOptions};

use crate::LbrnError;
use crate::model::{Backup, CutParams, CutSetting, Element, LbrnProject, PathData, Shape, ShapeKind, SubLayer};
use crate::path::{parse_prims, parse_vertices};

/// Largest accepted file. Embedded bitmaps make real projects a few MB; the engine's upload limit is 50 MB.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_NODES: u32 = 4_000_000;
const MAX_DEPTH: usize = 64;
/// Total vertices over all paths, counting shared lists once per use.
const MAX_VERTICES: usize = 10_000_000;

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

/// Parse a LightBurn project.
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
        cut_settings: Vec::new(),
        shapes: Vec::new(),
        extra_elements: Vec::new(),
    };
    for n in root.children().filter(Node::is_element) {
        match n.tag_name().name() {
            "CutSetting" | "CutSetting_Img" => p.cut_settings.push(cut_setting(n)?),
            "Shape" => p.shapes.push(cx.shape(n, 0)?),
            "Thumbnail" => {}
            _ => p.extra_elements.push(element(n, 0)?),
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

fn xform(n: Node<'_, '_>) -> Result<Affine, LbrnError> {
    let Some(x) = child(n, "XForm") else { return Ok(Affine::IDENTITY) };
    let text = x.text().unwrap_or_default();
    let v = text.split_whitespace().map(|t| number(t, "XForm value")).collect::<Result<Vec<_>, _>>()?;
    let v: [f64; 6] = v.try_into().map_err(|_| malformed(format!("XForm '{text}' needs six numbers")))?;
    Ok(Affine::new(v))
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

fn element(n: Node<'_, '_>, depth: usize) -> Result<Element, LbrnError> {
    if depth > MAX_DEPTH {
        return Err(LbrnError::TooLarge(format!("elements nested deeper than {MAX_DEPTH}")));
    }
    let text: String = n.children().filter(Node::is_text).filter_map(|t| t.text()).collect();
    let text = text.trim();
    Ok(Element {
        name: n.tag_name().name().to_string(),
        attributes: n.attributes().map(|a| (a.name().to_string(), a.value().to_string())).collect(),
        text: (!text.is_empty()).then(|| text.to_string()),
        children: n.children().filter(Node::is_element).map(|c| element(c, depth + 1)).collect::<Result<_, _>>()?,
    })
}

#[derive(Default)]
struct Cx<'a> {
    vert_defs: HashMap<&'a str, &'a str>,
    prim_defs: HashMap<&'a str, &'a str>,
    vertices: usize,
}

impl<'a> Cx<'a> {
    fn shape(&mut self, n: Node<'a, '_>, depth: usize) -> Result<Shape, LbrnError> {
        if depth > MAX_DEPTH {
            return Err(LbrnError::TooLarge(format!("groups nested deeper than {MAX_DEPTH}")));
        }
        let type_name = n.attribute("Type").ok_or_else(|| malformed("a Shape has no Type"))?;
        let cut_index = n.attribute("CutIndex").map(|i| index(i, "CutIndex")).transpose()?;
        let (kind, modelled_attrs, modelled_children): (ShapeKind, &[&str], &[&str]) = match type_name {
            "Rect" => (
                ShapeKind::Rect {
                    w: attr_number(n, "W")?,
                    h: attr_number(n, "H")?,
                    cr: n.attribute("Cr").map(|c| number(c, "Cr")).transpose()?.unwrap_or(0.0),
                },
                &["W", "H", "Cr"],
                &[],
            ),
            "Ellipse" => {
                (ShapeKind::Ellipse { rx: attr_number(n, "Rx")?, ry: attr_number(n, "Ry")? }, &["Rx", "Ry"], &[])
            }
            "Path" => (ShapeKind::Path(self.path(n)?), &["VertID", "PrimID"], &["VertList", "PrimList"]),
            "Group" => {
                let mut children = Vec::new();
                if let Some(c) = child(n, "Children") {
                    for s in c.children().filter(|c| c.has_tag_name("Shape")) {
                        children.push(self.shape(s, depth + 1)?);
                    }
                }
                (ShapeKind::Group(children), &[], &["Children"])
            }
            "Text" => {
                let backup = match child(n, "BackupPath") {
                    Some(b) => {
                        // Some versions nest the outline in a Shape inside BackupPath.
                        let src = child(b, "Shape").unwrap_or(b);
                        Some(Backup {
                            cut_index: src.attribute("CutIndex").map(|i| index(i, "CutIndex")).transpose()?,
                            xform: xform(src)?,
                            path: self.path(src)?,
                        })
                    }
                    None => None,
                };
                (ShapeKind::Text { backup }, &[], &["BackupPath"])
            }
            "Bitmap" => (ShapeKind::Bitmap { w: attr_number(n, "W")?, h: attr_number(n, "H")? }, &["W", "H"], &[]),
            other => (ShapeKind::Other { type_name: other.to_string() }, &[], &[]),
        };
        let attributes = n
            .attributes()
            .filter(|a| !matches!(a.name(), "Type" | "CutIndex") && !modelled_attrs.contains(&a.name()))
            .map(|a| (a.name().to_string(), a.value().to_string()))
            .collect();
        let children = n
            .children()
            .filter(|c| c.is_element() && !c.has_tag_name("XForm"))
            .filter(|c| !modelled_children.contains(&c.tag_name().name()))
            .map(|c| element(c, depth + 1))
            .collect::<Result<_, _>>()?;
        Ok(Shape { kind, cut_index, xform: xform(n)?, attributes, children })
    }

    /// The path's own lists, or the shared ones its `VertID`/`PrimID` refer to.
    fn path(&mut self, n: Node<'a, '_>) -> Result<PathData, LbrnError> {
        let vert_text = match (child(n, "VertList"), n.attribute("VertID")) {
            (Some(l), _) => l.text().unwrap_or_default(),
            (None, Some(id)) => {
                self.vert_defs.get(id).copied().ok_or_else(|| malformed(format!("VertID {id} is never defined")))?
            }
            (None, None) => return Err(malformed("a Path has no VertList")),
        };
        let vertices = parse_vertices(vert_text, MAX_VERTICES - self.vertices)?;
        self.vertices += vertices.len();
        let prim_text = match (child(n, "PrimList"), n.attribute("PrimID")) {
            (Some(l), _) => l.text().unwrap_or_default(),
            (None, Some(id)) => {
                self.prim_defs.get(id).copied().ok_or_else(|| malformed(format!("PrimID {id} is never defined")))?
            }
            (None, None) => return Err(malformed("a Path has no PrimList")),
        };
        let prims = parse_prims(prim_text, vertices.len())?;
        Ok(PathData { vertices, prims })
    }
}
