//! Files (ARCHITECTURE.md "Files and image tools"; crate `fileio`): exports (`doc.export`: SVG, DXF, AI, PDF; and
//! `doc.print`: print-ready SVG/PDF, black only or keeping colours), imports of further vector formats
//! (`art.import_pdf`, `art.import_ai`, `art.import_eps`, `art.import_hpgl`, `art.import_gerber`) and 3D models as depth
//! maps (`art.import_model`), plus helpers the LightBurn import uses (legacy `.lbrn`, project notes).

use std::borrow::Cow;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{Document, Geometry, Group, ObjectId};
use fileio::svg_out::{Area, ColorMode};
use fileio::{Art, FileError};
use kurbo::{Affine, BezPath, Rect, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Largest file (decoded) the import commands take.
pub const MAX_FILE_BYTES: usize = 100 * 1024 * 1024;
/// Most objects one import creates (split imports beyond this become one object).
const MAX_SPLIT: usize = 10_000;
/// Farthest imported art may lie from the field origin after placing (mm).
const MAX_PLACED_MM: f64 = 100_000.0;

pub(super) fn file_err(cmd: &str) -> impl Fn(FileError) -> CommandError + '_ {
    move |e| match e {
        FileError::TooLarge(m) | FileError::Invalid(m) | FileError::Unsupported(m) => invalid(cmd, m),
    }
}

/// Decode base64 (a `data:` prefix and whitespace are accepted), at most `max` bytes.
pub(super) fn base64_bytes(cmd: &str, text: &str, max: usize) -> Result<Vec<u8>> {
    let body = match text.trim_start().strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map(|(_, b)| b).ok_or_else(|| invalid(cmd, "the data: URL has no ','"))?,
        None => text,
    };
    if body.len() > max / 3 * 4 + 1024 * 1024 {
        return Err(invalid(cmd, format!("the file is larger than {} MB", max / (1024 * 1024))));
    }
    let compact: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = STANDARD.decode(compact.as_bytes()).map_err(|e| invalid(cmd, format!("data_base64 is not valid base64: {e}")))?;
    if bytes.len() > max {
        return Err(invalid(cmd, format!("the file is larger than {} MB", max / (1024 * 1024))));
    }
    Ok(bytes)
}

/// A bitmap browsers can show and the planner can decode: PNG, JPEG, GIF, BMP and WebP files are kept as they are;
/// TIFF, TGA and other formats the `image` crate reads are converted to PNG (a TIFF keeps its resolution). Returns
/// the file and a note when it was converted.
pub(crate) fn browser_bitmap(cmd: &str, bytes: Vec<u8>, mime: &str) -> Result<(Vec<u8>, Option<String>)> {
    if super::image::image_info(&bytes).is_some() {
        return Ok((bytes, None));
    }
    let format = fileio::bitmap::sniff(&bytes, Some(mime));
    let Some(format) = format.filter(|f| fileio::bitmap::needs_conversion(*f)) else { return Ok((bytes, None)) };
    let img = fileio::bitmap::decode(&bytes, Some(mime)).map_err(file_err(cmd))?;
    let dpi = fileio::bitmap::tiff_dpi(&bytes).map(|(x, _)| x);
    let png = fileio::bitmap::png(&img, dpi).map_err(file_err(cmd))?;
    Ok((png, Some(format!("converted from {format:?} to PNG"))))
}

/// The text of a LightBurn file as `.lbrn2`: a legacy `.lbrn` (vertex elements) is rewritten
/// ([`fileio::lbrn_legacy`]).
pub(crate) fn legacy_lbrn<'a>(cmd: &str, text: &'a str) -> Result<Cow<'a, str>> {
    if fileio::lbrn_legacy::is_legacy(text) {
        return fileio::lbrn_legacy::to_lbrn2(text).map(Cow::Owned).map_err(file_err(cmd));
    }
    Ok(Cow::Borrowed(text))
}

/// The attributes of a `<Notes ShowOnLoad=".." Notes=".."/>` element.
fn notes_of(xml: &str) -> Option<doc::Notes> {
    let d = roxmltree::Document::parse(xml).ok()?;
    let n = d.root_element();
    n.has_tag_name("Notes").then(|| doc::Notes {
        text: n.attribute("Notes").unwrap_or_default().to_string(),
        show_on_load: matches!(n.attribute("ShowOnLoad"), Some("1" | "true" | "True")),
    })
}

/// Project notes kept from a LightBurn file (its `Notes` root element, kept in `extras`).
pub(crate) fn notes_from_extras(extras: &std::collections::BTreeMap<String, String>) -> doc::Notes {
    extras.values().filter(|v| v.trim_start().starts_with("<Notes")).find_map(|v| notes_of(v)).unwrap_or_default()
}

/// Write the document's notes into a LightBurn project being exported: the kept `Notes` element is replaced when the
/// notes changed (else written as it was), and one is added when the file has none.
pub(crate) fn notes_into_project(p: &mut lbrn::LbrnProject, notes: &doc::Notes) {
    let element = || lbrn::Element {
        name: "Notes".into(),
        attributes: vec![("ShowOnLoad".into(), if notes.show_on_load { "1" } else { "0" }.into()), ("Notes".into(), notes.text.clone())],
        ..lbrn::Element::default()
    };
    if let Some(e) = p.extra_elements.iter_mut().find(|e| e.name == "Notes") {
        let current = match &e.raw {
            Some(raw) => notes_of(raw),
            None => Some(doc::Notes {
                text: e.attributes.iter().find(|(k, _)| k == "Notes").map(|(_, v)| v.clone()).unwrap_or_default(),
                show_on_load: e.attributes.iter().any(|(k, v)| k == "ShowOnLoad" && v == "1"),
            }),
        };
        if current.as_ref() != Some(notes) {
            *e = element();
        }
    } else if !notes.is_default() {
        p.order.push(lbrn::RootItem::Element(p.extra_elements.len()));
        p.extra_elements.push(element());
    }
}

/// Where and how imported art is placed (shared by the import commands).
pub(super) struct Placement<'a> {
    pub layer: Option<&'a str>,
    pub name: Option<&'a str>,
    pub width_mm: Option<f64>,
    pub keep_position: bool,
    pub split: bool,
    /// Put the paths on the layers the file carries (EzCad pens and hatches) even when `layer` is given.
    pub file_layers: bool,
}

/// Add imported art to the document: scaled to `width_mm`, centred on the field origin unless `keep_position`, one
/// object per path in a group named after the file (`split`, the default, as LightBurn groups imports) or one
/// combined object. Returns `{created, group, bounds, warnings, size_mm}`.
pub(super) fn place_art(cmd: &str, doc: &mut Document, art: Art, p: &Placement, default_name: &str) -> Result<(Value, String)> {
    let Art { paths, images, layers: art_layers, texts, mut warnings } = art;
    if paths.is_empty() && images.is_empty() && texts.is_empty() {
        return Err(CommandError::Failed(format!("{cmd}: the file contains no vector paths")));
    }
    let base = p.name.map(|n| util::name(cmd, "name", n)).transpose()?.unwrap_or_else(|| default_name.to_string());
    let text_box = |t: &fileio::ArtText| Rect::from_points(t.at, t.at + kurbo::Vec2::new(text_width(t), t.height_mm));
    let bounds = paths
        .iter()
        .map(|a| a.path.bounding_box())
        .chain(images.iter().map(fileio::ArtImage::bounds))
        .chain(texts.iter().map(text_box))
        .reduce(|a, b| a.union(b))
        .unwrap_or(Rect::ZERO);
    let k = match p.width_mm {
        Some(w) => {
            let w = util::positive(cmd, "width_mm", w)?;
            if bounds.width() <= 1e-9 {
                return Err(invalid(cmd, "cannot scale to a width: the art has zero width"));
            }
            w / bounds.width()
        }
        None => 1.0,
    };
    let fit = if p.keep_position { Affine::scale(k) } else { Affine::scale(k) * Affine::translate(-bounds.center().to_vec2()) };
    let size = bounds.size() * k;
    // Art placed beyond 100 m (or not finite) is a broken or hostile file: curves that large are flattened into
    // astronomically many segments later (previews, jobs, offsets).
    let placed = fit.transform_rect_bbox(bounds);
    if ![placed.x0, placed.y0, placed.x1, placed.y1].iter().all(|v| v.is_finite() && v.abs() <= MAX_PLACED_MM) {
        return Err(invalid(
            cmd,
            format!("the art is {:.0} x {:.0} mm or lies that far out; pass width_mm to scale it (at most {MAX_PLACED_MM} mm from the origin)", size.width, size.height),
        ));
    }
    if size.width > 300.0 || size.height > 300.0 {
        warnings.push(format!("the art is {:.1} x {:.1} mm, larger than a galvo field; pass width_mm to scale it", size.width, size.height));
    }
    transact(doc, |doc| {
        let layer = util::layer_or_default(doc, p.layer)?;
        // Layers the file carries (EzCad pens and hatches), made only for what uses them and only when the caller
        // named no layer.
        let mut made_layers: Vec<String> = Vec::new();
        let mut file_layers: Vec<Option<String>> = vec![None; art_layers.len()];
        if p.file_layers || p.layer.is_none() {
            let used: Vec<usize> = paths.iter().filter_map(|a| a.layer).chain(texts.iter().filter_map(|t| t.layer)).collect();
            for (i, l) in art_layers.iter().enumerate().filter(|(i, _)| used.contains(i)) {
                let id = doc.alloc_id("layer")?;
                let color = format!("#{:02x}{:02x}{:02x}", l.color[0], l.color[1], l.color[2]);
                let mut layer = doc::Layer::new(id.clone(), &l.name, &color);
                let usable = |s: &doc::CutSettings| super::layer::validate_settings(cmd, s).map_err(|e| e.to_string());
                match usable(&l.settings) {
                    Ok(()) => layer.settings = l.settings.clone(),
                    Err(e) => warnings.push(format!("layer \"{}\": the file's settings are not usable ({e}); defaults used", l.name)),
                }
                for s in &l.sub_layers {
                    match usable(s) {
                        Ok(()) => layer.sub_layers.push(s.clone()),
                        Err(e) => warnings.push(format!("layer \"{}\": a further hatch is left out ({e})", l.name)),
                    }
                }
                doc.layers.push(layer);
                made_layers.push(id.clone());
                file_layers[i] = Some(id);
            }
        }
        let layer_of = |l: Option<usize>| l.and_then(|i| file_layers.get(i).cloned().flatten()).unwrap_or_else(|| layer.clone());
        let mut created: Vec<ObjectId> = Vec::new();
        let mut group = None;
        // Images and texts are objects of their own, grouped with the art.
        let mut extra = place_images(cmd, doc, &images, fit, &base, &layer)?;
        for t in &texts {
            extra.push(place_text(cmd, doc, t, fit, k, &layer_of(t.layer), &mut warnings)?);
        }
        if p.split && paths.len() > 1 && paths.len() <= MAX_SPLIT {
            let gid = doc.alloc_id("group")?;
            doc.groups.push(Group::new(gid.clone(), base.clone(), None));
            group = Some(gid);
            for (n, a) in paths.into_iter().enumerate() {
                let path = fit * a.path;
                validate_path(cmd, &path)?;
                let obj = add_object(doc, format!("{base} {} {}", a.label, n + 1), layer_of(a.layer), Geometry::Path { path }, Affine::IDENTITY)?;
                if let Some(o) = doc.object_mut(&obj) {
                    o.group = group.clone();
                }
                created.push(obj);
            }
        } else if !paths.is_empty() {
            // One object per layer the paths are on.
            let mut by_layer: Vec<(String, BezPath)> = Vec::new();
            for a in paths {
                let l = layer_of(a.layer);
                let path = fit * a.path;
                match by_layer.iter_mut().find(|(x, _)| *x == l) {
                    Some((_, all)) => all.extend(path),
                    None => by_layer.push((l, path)),
                }
            }
            let several = by_layer.len() > 1;
            for (l, all) in by_layer {
                validate_path(cmd, &all)?;
                let name = if several { format!("{base} {}", doc.layer(&l).map_or("", |x| x.name.as_str())) } else { base.clone() };
                created.push(add_object(doc, name, l, Geometry::Path { path: all }, Affine::IDENTITY)?);
            }
        }
        if !extra.is_empty() || (group.is_none() && created.len() > 1) {
            let gid = match &group {
                Some(g) => g.clone(),
                None if created.len() + extra.len() > 1 => {
                    let gid = doc.alloc_id("group")?;
                    doc.groups.push(Group::new(gid.clone(), base.clone(), None));
                    for id in &created {
                        if let Some(o) = doc.object_mut(id) {
                            o.group = Some(gid.clone());
                        }
                    }
                    group = Some(gid.clone());
                    gid
                }
                None => String::new(),
            };
            for id in &extra {
                if let Some(o) = doc.object_mut(id).filter(|_| !gid.is_empty()) {
                    o.group = Some(gid.clone());
                }
            }
            created.extend(extra);
        }
        let mut r = util::edit_result(doc, &created, &[]);
        r["group"] = json!(group);
        r["layers"] = json!(made_layers);
        r["warnings"] = json!(warnings);
        r["size_mm"] = json!([util::r4(size.width), util::r4(size.height)]);
        Ok((r, format!("imported {base} ({} object(s))", created.len())))
    })
}

/// A text's width before it is set: about 0.6 of the height per character.
fn text_width(t: &fileio::ArtText) -> f64 {
    0.6 * t.height_mm * t.text.lines().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f64
}

/// A text the file holds without outlines: live text in its font when that is installed, else a placeholder
/// rectangle where it goes (named after the text) and a warning.
fn place_text(cmd: &str, doc: &mut Document, t: &fileio::ArtText, fit: Affine, k: f64, layer: &str, warnings: &mut Vec<String>) -> Result<ObjectId> {
    let at = fit * t.at;
    let height = t.height_mm * k;
    let input = json!({
        "text": t.text, "font": { "family": t.font, "kind": "system" }, "height_mm": height,
        "center": [at.x, at.y], "rotation_deg": t.angle_deg, "layer": layer, "align_h": "left", "align_v": "bottom",
    });
    let set = serde_json::from_value::<<super::text::ShapeText as Command>::Input>(input)
        .map_err(|e| e.to_string())
        .and_then(|i| <super::text::ShapeText as Command>::apply(doc, i).map_err(|e| e.to_string()));
    match set {
        Ok((r, _)) => {
            warnings.push(format!("text \"{}\" was set in {} (its place in the file is the start of the baseline; unverified against EzCad)", t.text, t.font));
            r["created"][0].as_str().map(str::to_owned).ok_or_else(|| CommandError::Failed("the text was not created".into()))
        }
        Err(e) => {
            warnings.push(format!("text \"{}\" in font \"{}\" has no outline in the file and the font cannot be used here ({e}): a placeholder marks its place", t.text, t.font));
            let w = text_width(t) * k;
            let rect = Geometry::Rect { width_mm: w, height_mm: height, corner_radius_mm: 0.0 };
            let place = Affine::translate(at.to_vec2()) * Affine::rotate(t.angle_deg.to_radians()) * Affine::translate((w / 2.0, height / 2.0));
            let _ = cmd;
            add_object(doc, format!("Text placeholder: {}", t.text.chars().take(60).collect::<String>()), layer.to_owned(), rect, place)
        }
    }
}

/// Add `images` as image objects placed by `fit` after their own placement. Returns their ids.
fn place_images(cmd: &str, doc: &mut Document, images: &[fileio::ArtImage], fit: Affine, base: &str, layer: &str) -> Result<Vec<ObjectId>> {
    let mut out = Vec::new();
    for (n, im) in images.iter().enumerate() {
        let m = fit * im.placement;
        let c = m.as_coeffs();
        let (w, h) = (c[0].hypot(c[1]), c[2].hypot(c[3]));
        if !(w.is_finite() && h.is_finite() && w > 1e-6 && h > 1e-6) || m.determinant().abs() < 1e-12 {
            continue;
        }
        // The image is centred on its local origin: unit square = local / size + 0.5.
        let transform = m * Affine::translate((0.5, 0.5)) * Affine::scale_non_uniform(1.0 / w, 1.0 / h);
        let geometry = Geometry::Image {
            width_mm: w,
            height_mm: h,
            mime: im.mime.into(),
            data_base64: STANDARD.encode(&im.data).into(),
            adjust: doc::ImageAdjust::default(),
        };
        let name = if images.len() == 1 { format!("{base} {}", im.label) } else { format!("{base} {} {}", im.label, n + 1) };
        let _ = cmd;
        out.push(add_object(doc, name, layer.to_owned(), geometry, transform)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------------
// Exports.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Svg,
    Dxf,
    /// Adobe Illustrator 9+ (a PDF-compatible `.ai`).
    Ai,
    Pdf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Colors {
    /// Each layer's colour (LightBurn's "Print (keep colors)").
    #[default]
    Layers,
    /// Everything black (LightBurn's "Print (black only)").
    Black,
}

impl From<Colors> for ColorMode {
    fn from(c: Colors) -> Self {
        match c {
            Colors::Layers => ColorMode::Layers,
            Colors::Black => ColorMode::Black,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportArea {
    /// The bounds of what is exported.
    Bounds,
    /// The device workspace (galvo field or bed), centred on the field origin.
    Workspace,
}

impl From<ExportArea> for Area {
    fn from(a: ExportArea) -> Self {
        match a {
            ExportArea::Bounds => Area::Bounds,
            ExportArea::Workspace => Area::Workspace,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    pub format: ExportFormat,
    /// Objects or groups to export (LightBurn exports the selection); empty or absent: the whole project.
    #[serde(default)]
    pub ids: Vec<ObjectId>,
    #[serde(default)]
    pub colors: Colors,
    /// SVG/AI/PDF stroke width in mm (default 0.1).
    pub stroke_mm: Option<f64>,
    /// SVG/AI/PDF: embed images (default true). DXF never holds images.
    pub images: Option<bool>,
    /// DXF: curve flattening tolerance in mm (default 0.01).
    pub tolerance_mm: Option<f64>,
    /// DXF: write circles as CIRCLE entities (default true; false converts them to lines too).
    pub keep_circles: Option<bool>,
    /// Page / view box: `bounds` (default) or `workspace`.
    pub area: Option<ExportArea>,
}

/// `doc.export`.
pub struct DocExport;

fn export_result(format: &str, mime: &str, ext: &str, content: Value, warnings: Vec<String>, bounds: Rect) -> Value {
    let mut r = json!({ "format": format, "mime": mime, "extension": ext, "warnings": warnings, "bounds": util::rect_json(bounds) });
    if let Value::Object(m) = &mut r
        && let Value::Object(c) = content
    {
        m.extend(c);
    }
    r
}

impl Command for DocExport {
    type Input = ExportInput;
    const ID: &'static str = "doc.export";
    const TITLE: &'static str = "Export";
    const DESCRIPTION: &'static str = "Export the project, or the objects/groups in ids (LightBurn's Export: the \
        selection, else everything), as svg (mm, layers as Inkscape layer groups in their colours, fill layers filled \
        even-odd, layers with output off hidden, text as outlines, images embedded), dxf (R12, mm, one DXF layer per \
        layer with the nearest AutoCAD colour, curves flattened to tolerance_mm, circles as CIRCLE unless \
        keep_circles: false; no images), ai (Adobe Illustrator 9+ PDF-compatible .ai) or pdf (one page of the area). \
        colors: layers | black. Field frame mm. Returns {format, mime, extension, content (svg, dxf) or \
        content_base64 (ai, pdf), warnings, bounds}. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ExportInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let ids = (!i.ids.is_empty()).then(|| i.ids.clone());
        let area: Area = i.area.map_or(Area::Bounds, Into::into);
        let r = match i.format {
            ExportFormat::Svg => {
                let o = fileio::svg_out::SvgOptions {
                    ids,
                    colors: i.colors.into(),
                    stroke_mm: i.stroke_mm.unwrap_or(0.1),
                    images: i.images.unwrap_or(true),
                    hidden_layers: true,
                    area,
                };
                let e = fileio::svg_out::export_svg(doc, &o).map_err(file_err(id))?;
                export_result("svg", "image/svg+xml", "svg", json!({ "content": e.content }), e.warnings, e.bounds)
            }
            ExportFormat::Dxf => {
                let o = fileio::dxf_out::DxfOptions { ids, tolerance_mm: i.tolerance_mm.unwrap_or(0.01), keep_circles: i.keep_circles.unwrap_or(true) };
                let e = fileio::dxf_out::export_dxf(doc, &o).map_err(file_err(id))?;
                export_result("dxf", "image/vnd.dxf", "dxf", json!({ "content": e.content }), e.warnings, e.bounds)
            }
            ExportFormat::Ai | ExportFormat::Pdf => {
                let o = fileio::pdf_out::PdfOptions {
                    ids,
                    colors: i.colors.into(),
                    stroke_mm: i.stroke_mm.unwrap_or(0.1),
                    images: i.images.unwrap_or(true),
                    area,
                    title: doc.name.clone(),
                    ..fileio::pdf_out::PdfOptions::default()
                };
                let e = fileio::pdf_out::export_pdf(doc, &o).map_err(file_err(id))?;
                let (f, mime) = if i.format == ExportFormat::Ai { ("ai", "application/postscript") } else { ("pdf", "application/pdf") };
                export_result(f, mime, f, json!({ "content_base64": STANDARD.encode(&e.content) }), e.warnings, e.bounds)
            }
        };
        Ok((r, String::new()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrintFormat {
    Svg,
    Pdf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrintInput {
    /// pdf (default) or svg.
    pub format: Option<PrintFormat>,
    /// black (LightBurn's "Print (black only)", the default) or layers ("Print (keep colors)").
    pub colors: Option<Colors>,
    /// Line width in mm (LightBurn 2.1 Print Settings > Line Width; default 0.25).
    pub line_width_mm: Option<f64>,
    /// PDF: paper `[width, height]` in mm; the design is centred and scaled down to fit inside the margins. Default:
    /// the page is the area at 1:1.
    pub paper_mm: Option<[f64; 2]>,
    /// PDF: paper margin in mm (default 10).
    pub margin_mm: Option<f64>,
    /// workspace (default: the whole work area, as LightBurn prints it) or bounds.
    pub area: Option<ExportArea>,
    /// Objects or groups to print (default all).
    #[serde(default)]
    pub ids: Vec<ObjectId>,
}

/// `doc.print`.
pub struct DocPrint;

impl Command for DocPrint {
    type Input = PrintInput;
    const ID: &'static str = "doc.print";
    const TITLE: &'static str = "Print";
    const DESCRIPTION: &'static str = "A print-ready file of the work area (LightBurn's File > Print): pdf (default) \
        or svg, colors black (default, 'black only') or layers ('keep colors'), line_width_mm (default 0.25), PDF \
        paper_mm [w, h] with margin_mm (the design scaled down to fit), area workspace (default) or bounds, optional \
        ids. Hidden layers and layers with output off are left out. Returns like doc.export. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: PrintInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let ids = (!i.ids.is_empty()).then(|| i.ids.clone());
        let colors: ColorMode = i.colors.unwrap_or(Colors::Black).into();
        let stroke = i.line_width_mm.unwrap_or(0.25);
        let area: Area = i.area.unwrap_or(ExportArea::Workspace).into();
        let r = match i.format.unwrap_or(PrintFormat::Pdf) {
            PrintFormat::Svg => {
                let o = fileio::svg_out::SvgOptions { ids, colors, stroke_mm: stroke, images: true, hidden_layers: false, area };
                let e = fileio::svg_out::export_svg(doc, &o).map_err(file_err(id))?;
                export_result("svg", "image/svg+xml", "svg", json!({ "content": e.content }), e.warnings, e.bounds)
            }
            PrintFormat::Pdf => {
                let o = fileio::pdf_out::PdfOptions {
                    ids,
                    colors,
                    stroke_mm: stroke,
                    images: true,
                    skip_hidden: true,
                    area,
                    paper_mm: i.paper_mm,
                    margin_mm: i.margin_mm.unwrap_or(10.0),
                    title: doc.name.clone(),
                };
                let e = fileio::pdf_out::export_pdf(doc, &o).map_err(file_err(id))?;
                export_result("pdf", "application/pdf", "pdf", json!({ "content_base64": STANDARD.encode(&e.content) }), e.warnings, e.bounds)
            }
        };
        Ok((r, String::new()))
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Vector imports.

/// The placement fields every vector import takes.
macro_rules! placement_doc {
    () => {
        "Placement: layer, name, width_mm, keep_position (default centred on the field origin), split (default one \
         object per path in a group). Returns {created, group, bounds, warnings, size_mm}."
    };
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPdfInput {
    /// The file in base64.
    pub data_base64: String,
    /// 1-based page (default 1).
    pub page: Option<u32>,
    /// Import hidden layers (optional content that is off; LightBurn's "Import hidden AI layers"). Default false.
    #[serde(default)]
    pub hidden_layers: bool,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per path in a group (default true); false makes one object.
    pub split: Option<bool>,
}

/// `art.import_pdf`.
#[cfg(not(target_arch = "wasm32"))]
pub struct ImportPdf;

#[cfg(not(target_arch = "wasm32"))]
impl Command for ImportPdf {
    type Input = ImportPdfInput;
    const ID: &'static str = "art.import_pdf";
    const TITLE: &'static str = "Import PDF";
    const DESCRIPTION: &'static str = concat!(
        "Import one page (default 1) of a PDF (base64) in mm: paths and rectangles of the page and its forms, and \
         its embedded images as image objects where the page draws them; text and gradients are left out with a \
         warning. hidden_layers imports optional content that is off. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportPdfInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, MAX_FILE_BYTES)?;
        let art = fileio::pdf_in::parse(&bytes, &fileio::pdf_in::PdfImportOptions { page: i.page, hidden_layers: i.hidden_layers })
            .map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, "Imported PDF")
    }
}

/// `art.import_ai`.
pub struct ImportAi;

impl Command for ImportAi {
    type Input = ImportPdfInput;
    const ID: &'static str = "art.import_ai";
    const TITLE: &'static str = "Import Adobe Illustrator";
    const DESCRIPTION: &'static str = concat!(
        "Import an Adobe Illustrator .ai file (base64): Illustrator 9 and later files are read through their PDF \
         content (saved with 'Create PDF Compatible File', LightBurn's requirement too; hidden_layers imports hidden \
         layers; embedded images as image objects), older PostScript-based ones through the EPS reader (vectors \
         only). Text is left out with a warning. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportPdfInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, MAX_FILE_BYTES)?;
        let pdf = bytes.windows(5).take(1024).any(|w| w == b"%PDF-");
        let art = if pdf {
            #[cfg(not(target_arch = "wasm32"))]
            {
                fileio::pdf_in::parse(&bytes, &fileio::pdf_in::PdfImportOptions { page: i.page, hidden_layers: i.hidden_layers })
                    .map_err(file_err(id))?
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (i.page, i.hidden_layers);
                return Err(invalid(id, "PDF-based Illustrator files are read by the engine only"));
            }
        } else {
            fileio::postscript::parse(&bytes).map_err(file_err(id))?
        };
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, "Imported AI")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportEpsInput {
    /// The file in base64 (EPS files may carry a binary preview).
    pub data_base64: String,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per path in a group (default true); false makes one object.
    pub split: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportEzdInput {
    /// The .ezd file in base64.
    pub data_base64: String,
    /// Layer id for what carries no settings of its own (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per path in a group (default true); false makes one object per layer.
    pub split: Option<bool>,
    /// Put the objects on layers made from the file's pens and hatches (default true); false puts everything on
    /// `layer`.
    pub pen_layers: Option<bool>,
}

/// `art.import_ezd`.
pub struct ImportEzd;

impl Command for ImportEzd {
    type Input = ImportEzdInput;
    const ID: &'static str = "art.import_ezd";
    const TITLE: &'static str = "Import EzCad project";
    const DESCRIPTION: &'static str = concat!(
        "Import an EzCad 2 .ezd project (base64; not in LightBurn): curves, rectangles, circles, ellipses, polygons, \
         groups, hatch outlines, text outlines and images, each pen a line layer with its speed, power, frequency, \
         loops and pulse width, each hatch a fill layer (further hatches as sub-layers; pen_layers: false puts \
         everything on `layer`). A text without outlines is set in its font when installed, else a placeholder \
         marks it (warned). keep_position keeps EzCad's field positions. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportEzdInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, fileio::ezd::MAX_BYTES)?;
        let art = fileio::ezd::parse(&bytes).map_err(file_err(id))?;
        let pen_layers = i.pen_layers.unwrap_or(true);
        let mut art = art;
        if !pen_layers {
            art.layers.clear();
            for a in &mut art.paths {
                a.layer = None;
            }
            for t in &mut art.texts {
                t.layer = None;
            }
        }
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: pen_layers };
        place_art(id, doc, art, &p, "Imported EzCad")
    }
}

/// `art.import_eps`.
pub struct ImportEps;

impl Command for ImportEps {
    type Input = ImportEpsInput;
    const ID: &'static str = "art.import_eps";
    const TITLE: &'static str = "Import EPS";
    const DESCRIPTION: &'static str = concat!(
        "Import an EPS / PostScript file (base64) with OpenEtch's PostScript subset: paths, arcs, rectangles, \
         procedures, gsave/grestore and transforms, colours; text, images and fonts are left out with a warning. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportEpsInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, MAX_FILE_BYTES)?;
        let art = fileio::postscript::parse(&bytes).map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, "Imported EPS")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportHpglInput {
    /// The HPGL/PLT text.
    pub content: String,
    /// Plotter units per mm (LightBurn's HPGL precision; default 40, i.e. 0.025 mm).
    pub units_per_mm: Option<f64>,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per path in a group (default true); false makes one object.
    pub split: Option<bool>,
}

/// `art.import_hpgl`.
pub struct ImportHpgl;

impl Command for ImportHpgl {
    type Input = ImportHpglInput;
    const ID: &'static str = "art.import_hpgl";
    const TITLE: &'static str = "Import HPGL / PLT";
    const DESCRIPTION: &'static str = concat!(
        "Import an HPGL / PLT plotter file: pen moves, circles, arcs and rectangles, one path set per pen; labels and \
         scaling commands are skipped with a warning. units_per_mm (default 40). ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportHpglInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let art = fileio::hpgl::parse(&i.content, i.units_per_mm.unwrap_or(fileio::hpgl::UNITS_PER_MM)).map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, "Imported HPGL")
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportGcodeInput {
    /// The G-code program text (.gcode, .nc, .gc, .ngc).
    pub content: String,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the program's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per power (S value) in a group (default true); false makes one object.
    pub split: Option<bool>,
}

/// `art.import_gcode`.
pub struct ImportGcode;

impl Command for ImportGcode {
    type Input = ImportGcodeInput;
    const ID: &'static str = "art.import_gcode";
    const TITLE: &'static str = "Import G-code";
    const DESCRIPTION: &'static str = concat!(
        "Import the cutting moves of a G-code program (a laser's, plotter's or router's) as paths: G1 lines and G2/G3 \
         arcs while the laser is on (M3/M4 with S > 0; every feed move when the program never switches a laser), one \
         path set per power (S value); travel (G0), Z and feeds are not imported. G20/G21 and G90/G91 are honoured; \
         work offsets and machine-frame moves are approximated with a warning. Nothing is sent to a machine. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportGcodeInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let art = fileio::gcode_in::parse(&i.content).map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, "Imported G-code")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GerberKind {
    /// Excellon when the file looks like a drill file, else Gerber.
    #[default]
    Auto,
    Gerber,
    Excellon,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportGerberInput {
    /// The Gerber (.gbr, .gtl, .gbl, ...) or Excellon drill (.drl) text.
    pub content: String,
    #[serde(default)]
    pub kind: GerberKind,
    /// Gerber: the board area outside the copper instead of the copper (isolation / etch-resist removal).
    #[serde(default)]
    pub invert: bool,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name of the group (or the object).
    pub name: Option<String>,
    /// Scale uniformly to this width in mm.
    pub width_mm: Option<f64>,
    /// Keep the file's own coordinates instead of centring on the field origin.
    #[serde(default)]
    pub keep_position: bool,
    /// One object per path in a group (default true); false makes one object.
    pub split: Option<bool>,
}

/// `art.import_gerber`.
pub struct ImportGerber;

impl Command for ImportGerber {
    type Input = ImportGerberInput;
    const ID: &'static str = "art.import_gerber";
    const TITLE: &'static str = "Import Gerber / drill";
    const DESCRIPTION: &'static str = concat!(
        "Import a PCB layer (LightBurn 2.2's Gerber import): Gerber RS-274X copper as closed outlines (tracks \
         stroked with their aperture, pads flashed, regions filled, clear polarity cut out; put them on a fill layer \
         to burn the copper area), invert for the board area outside the copper; or an Excellon drill file as one \
         circle per hole. Aperture macros and step-and-repeat are skipped with a warning. Use keep_position: true to \
         keep several layers of one board aligned. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportGerberInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let kind = match i.kind {
            GerberKind::Auto => fileio::gerber::detect(&i.content),
            GerberKind::Gerber => fileio::gerber::Kind::Gerber,
            GerberKind::Excellon => fileio::gerber::Kind::Excellon,
        };
        let art = match kind {
            fileio::gerber::Kind::Gerber => fileio::gerber::parse_gerber(&i.content, i.invert),
            fileio::gerber::Kind::Excellon => fileio::gerber::parse_excellon(&i.content),
        }
        .map_err(file_err(id))?;
        let default = if kind == fileio::gerber::Kind::Excellon { "Drill" } else { "Gerber" };
        // Copper outlines with their holes belong together: one object per island.
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true), file_layers: false };
        place_art(id, doc, art, &p, default)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 3D models.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    Stl,
    Obj,
    Off,
    Ply,
    #[serde(rename = "3mf")]
    ThreeMf,
    Amf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportModelInput {
    /// The model file in base64.
    pub data_base64: String,
    /// Default: from the content.
    pub format: Option<ModelFormat>,
    /// Rotation about X, then Y, then Z in degrees (default none: seen from above, +Z up).
    pub rotation_deg: Option<[f64; 3]>,
    /// Width of the depth map in mm (default the model's own size, file units taken as mm).
    pub width_mm: Option<f64>,
    /// Depth map resolution in dots per inch (default 254: 0.1 mm per pixel).
    pub dpi: Option<f64>,
    /// 16-bit grey levels (default 8-bit).
    #[serde(default)]
    pub sixteen_bit: bool,
    /// Swap light and dark on the model.
    #[serde(default)]
    pub invert: bool,
    /// Background transparent instead of black (black engraves full depth).
    #[serde(default)]
    pub transparent_background: bool,
    /// Floor this many mm below the top (anything lower is background); default automatic (the lowest point).
    pub depth_cutoff_mm: Option<f64>,
    /// Spread the model's grey levels evenly to bring out fine detail.
    #[serde(default)]
    pub equalize: bool,
    /// Also add the model's outline seen from above as a path (LightBurn's Generate Outline).
    #[serde(default)]
    pub outline: bool,
    /// Layer id (default the first layer).
    pub layer: Option<String>,
    /// Name (default "Depth map").
    pub name: Option<String>,
}

/// Object `extras` of a depth map made by `art.import_model` (LightBurn's Edit 3D Model reopens its dialog on it):
/// the options it was rendered with (JSON of [`ModelOptions`]).
pub const MODEL_OPTIONS_KEY: &str = "model3d.options";
/// The model file itself, zipped (deflate) and in base64, when it fits [`MAX_KEPT_MODEL_BYTES`].
pub const MODEL_SOURCE_KEY: &str = "model3d.source";
/// The id of the outline object made with it, if any.
pub const MODEL_OUTLINE_KEY: &str = "model3d.outline";
/// Largest zipped model kept on the image (bigger models import, but cannot be edited again).
pub const MAX_KEPT_MODEL_BYTES: usize = 16 * 1024 * 1024;

/// The options a depth map is rendered with (the import dialog's fields).
#[derive(Debug, Clone, PartialEq, Deserialize, serde::Serialize)]
pub struct ModelOptions {
    #[serde(default)]
    pub format: Option<ModelFormat>,
    #[serde(default)]
    pub rotation_deg: [f64; 3],
    #[serde(default)]
    pub width_mm: Option<f64>,
    pub dpi: f64,
    #[serde(default)]
    pub sixteen_bit: bool,
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub transparent_background: bool,
    #[serde(default)]
    pub depth_cutoff_mm: Option<f64>,
    #[serde(default)]
    pub equalize: bool,
    #[serde(default)]
    pub outline: bool,
}

/// Render `bytes` (a model file) with `o`: the depth map, the triangle count and warnings.
fn render_model(id: &str, bytes: &[u8], o: &ModelOptions) -> Result<(fileio::mesh::DepthMap, usize, Vec<String>)> {
    let format = o.format.map(|f| match f {
        ModelFormat::Stl => fileio::mesh::Format::Stl,
        ModelFormat::Obj => fileio::mesh::Format::Obj,
        ModelFormat::Off => fileio::mesh::Format::Off,
        ModelFormat::Ply => fileio::mesh::Format::Ply,
        ModelFormat::ThreeMf => fileio::mesh::Format::ThreeMf,
        ModelFormat::Amf => fileio::mesh::Format::Amf,
    });
    let (mesh, mut warnings) = fileio::mesh::load(bytes, format).map_err(file_err(id))?;
    let dpi = util::positive(id, "dpi", o.dpi)?;
    for (k, v) in o.rotation_deg.iter().enumerate() {
        util::finite(id, ["rotation_deg[0]", "rotation_deg[1]", "rotation_deg[2]"][k], *v)?;
    }
    let d = fileio::mesh::DepthOptions {
        rotation_deg: o.rotation_deg,
        width_mm: o.width_mm,
        px_per_mm: dpi / 25.4,
        sixteen_bit: o.sixteen_bit,
        invert: o.invert,
        transparent_background: o.transparent_background,
        depth_cutoff_mm: o.depth_cutoff_mm,
        equalize: o.equalize,
    };
    let map = fileio::mesh::depth_map(&mesh, &d).map_err(file_err(id))?;
    if map.width_mm > 300.0 || map.height_mm > 300.0 {
        warnings.push(format!("the depth map is {:.1} x {:.1} mm; pass width_mm to scale the model", map.width_mm, map.height_mm));
    }
    Ok((map, mesh.triangles.len(), warnings))
}

/// The model's outline rings as one path (`None` when it has none).
fn outline_path(id: &str, map: &fileio::mesh::DepthMap) -> Result<Option<BezPath>> {
    if map.outline.is_empty() {
        return Ok(None);
    }
    let mut p = BezPath::new();
    for ring in &map.outline {
        p.move_to(ring[0]);
        for q in &ring[1..] {
            p.line_to(*q);
        }
        p.close_path();
    }
    validate_path(id, &p)?;
    Ok(Some(p))
}

/// The model file zipped and in base64 for [`MODEL_SOURCE_KEY`], or why it is not kept.
fn keep_model(bytes: &[u8]) -> std::result::Result<String, String> {
    let zipped = fileio::zip::write(&[fileio::zip::Entry { name: "model".into(), data: bytes.to_vec() }]).map_err(|e| e.to_string())?;
    if zipped.len() > MAX_KEPT_MODEL_BYTES {
        return Err(format!(
            "the model is too large to keep with the image ({} MB zipped, at most {} MB): Edit 3D Model needs it imported again",
            zipped.len() / (1024 * 1024),
            MAX_KEPT_MODEL_BYTES / (1024 * 1024)
        ));
    }
    Ok(STANDARD.encode(zipped))
}

/// The depth map's result fields.
fn model_result(r: &mut Value, map: &fileio::mesh::DepthMap, triangles: usize, warnings: &[String]) {
    r["width_px"] = json!(map.width_px);
    r["height_px"] = json!(map.height_px);
    r["depth_mm"] = json!(util::r4(map.depth_mm));
    r["size_mm"] = json!([util::r4(map.width_mm), util::r4(map.height_mm)]);
    r["triangles"] = json!(triangles);
    r["warnings"] = json!(warnings);
}

/// `art.import_model`.
pub struct ImportModel;

impl Command for ImportModel {
    type Input = ImportModelInput;
    const ID: &'static str = "art.import_model";
    const TITLE: &'static str = "Import 3D model as depth map";
    const DESCRIPTION: &'static str = "LightBurn 2.2's Import 3D Model: an STL, OBJ, OFF, PLY, 3MF or AMF model \
        (base64) rendered from above into a grey depth map image (highest point white, floor black; black engraves \
        deepest), centred on the field origin. Options: rotation_deg [x, y, z], width_mm, dpi (default 254), \
        sixteen_bit, invert, transparent_background, depth_cutoff_mm (default automatic), equalize, outline (also add \
        the silhouette as a path). Engrave it with the layer's image mode set to 3D Sliced (layer.update settings.image \
        {dither: \"slice3d\"}) and as many passes as depth slices. Returns {created, image, outline, width_px, \
        height_px, depth_mm, size_mm, warnings}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportModelInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, fileio::mesh::MAX_BYTES)?;
        let options = ModelOptions {
            format: i.format,
            rotation_deg: i.rotation_deg.unwrap_or([0.0; 3]),
            width_mm: i.width_mm,
            dpi: i.dpi.unwrap_or(254.0),
            sixteen_bit: i.sixteen_bit,
            invert: i.invert,
            transparent_background: i.transparent_background,
            depth_cutoff_mm: i.depth_cutoff_mm,
            equalize: i.equalize,
            outline: i.outline,
        };
        let (map, triangles, mut warnings) = render_model(id, &bytes, &options)?;
        // Kept so Edit 3D Model can render it again with other options.
        let kept = keep_model(&bytes).map_err(|w| warnings.push(w)).ok();
        drop(bytes);
        let options_json = serde_json::to_string(&options).map_err(|e| CommandError::Failed(e.to_string()))?;
        let name = i.name.as_deref().map(|n| util::name(id, "name", n)).transpose()?.unwrap_or_else(|| "Depth map".into());
        transact(doc, |doc| {
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let geometry = Geometry::Image {
                width_mm: map.width_mm,
                height_mm: map.height_mm,
                mime: "image/png".into(),
                data_base64: STANDARD.encode(&map.png).into(),
                adjust: doc::ImageAdjust::default(),
            };
            let image = add_object(doc, name.clone(), layer.clone(), geometry, Affine::IDENTITY)?;
            let mut created = vec![image.clone()];
            let mut outline = None;
            if i.outline
                && let Some(p) = outline_path(id, &map)?
            {
                let o = add_object(doc, format!("{name} outline"), layer, Geometry::Path { path: p }, Affine::IDENTITY)?;
                created.push(o.clone());
                outline = Some(o);
            }
            if let Some(obj) = doc.objects.iter_mut().find(|o| o.id == image) {
                obj.extras.insert(MODEL_OPTIONS_KEY.into(), options_json.clone());
                if let Some(k) = &kept {
                    obj.extras.insert(MODEL_SOURCE_KEY.into(), k.clone());
                }
                if let Some(o) = &outline {
                    obj.extras.insert(MODEL_OUTLINE_KEY.into(), o.clone());
                }
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["image"] = json!(image);
            r["outline"] = json!(outline);
            r["editable"] = json!(kept.is_some());
            model_result(&mut r, &map, triangles, &warnings);
            Ok((r, format!("imported a 3D model as depth map \"{name}\" ({} x {} px)", map.width_px, map.height_px)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditModelInput {
    /// The depth map image made by art.import_model.
    pub id: String,
    /// Rotation about X, then Y, then Z in degrees. Default: as before (also for every field below).
    pub rotation_deg: Option<[f64; 3]>,
    /// Width of the depth map in mm; null for the model's own size.
    #[serde(default, deserialize_with = "super::layer::double_option")]
    #[schemars(with = "Option<f64>")]
    pub width_mm: Option<Option<f64>>,
    /// Resolution in dots per inch.
    pub dpi: Option<f64>,
    /// 16-bit grey levels.
    pub sixteen_bit: Option<bool>,
    /// Swap light and dark on the model.
    pub invert: Option<bool>,
    /// Background transparent instead of black.
    pub transparent_background: Option<bool>,
    /// Floor this many mm below the top; null for automatic.
    #[serde(default, deserialize_with = "super::layer::double_option")]
    #[schemars(with = "Option<f64>")]
    pub depth_cutoff_mm: Option<Option<f64>>,
    /// Spread the grey levels evenly.
    pub equalize: Option<bool>,
    /// Keep an outline object: updated when the import made one, added when there is none.
    pub outline: Option<bool>,
}

/// `image.edit_model`.
pub struct EditModel;

impl Command for EditModel {
    type Input = EditModelInput;
    const ID: &'static str = "image.edit_model";
    const TITLE: &'static str = "Edit 3D model";
    const DESCRIPTION: &'static str = "LightBurn's Edit 3D Model: render a depth map made by art.import_model again \
        from the model kept on it, with changed options (rotation_deg, width_mm, dpi, sixteen_bit, invert, \
        transparent_background, depth_cutoff_mm, equalize, outline; the others stay as they were, see the image's \
        extras model3d.options). The image keeps its place and transform; its outline object is redrawn (or added \
        with outline: true). Refused for images without a kept model. Returns {image, outline, width_px, height_px, \
        depth_mm, size_mm, options, warnings}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: EditModelInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let obj = util::object(doc, &i.id)?;
        if !matches!(obj.geometry, Geometry::Image { .. }) {
            return Err(invalid(id, format!("{} is not an image", i.id)));
        }
        let (Some(opts), Some(source)) = (obj.extras.get(MODEL_OPTIONS_KEY), obj.extras.get(MODEL_SOURCE_KEY)) else {
            return Err(invalid(id, format!("{} was not made from a 3D model kept with it; import the model again", i.id)));
        };
        let mut o: ModelOptions =
            serde_json::from_str(opts).map_err(|e| invalid(id, format!("the image's model options are not usable: {e}")))?;
        let zipped = base64_bytes(id, source, MAX_KEPT_MODEL_BYTES * 2)?;
        let bytes = fileio::zip::read_one(&zipped, "model")
            .map_err(|e| invalid(id, format!("the kept model is not readable: {e}")))?
            .ok_or_else(|| invalid(id, "the kept model is missing"))?;
        drop(zipped);
        if let Some(v) = i.rotation_deg {
            o.rotation_deg = v;
        }
        if let Some(v) = i.width_mm {
            o.width_mm = v;
        }
        if let Some(v) = i.dpi {
            o.dpi = v;
        }
        if let Some(v) = i.sixteen_bit {
            o.sixteen_bit = v;
        }
        if let Some(v) = i.invert {
            o.invert = v;
        }
        if let Some(v) = i.transparent_background {
            o.transparent_background = v;
        }
        if let Some(v) = i.depth_cutoff_mm {
            o.depth_cutoff_mm = v;
        }
        if let Some(v) = i.equalize {
            o.equalize = v;
        }
        if let Some(v) = i.outline {
            o.outline = v;
        }
        let (map, triangles, warnings) = render_model(id, &bytes, &o)?;
        drop(bytes);
        let options_json = serde_json::to_string(&o).map_err(|e| CommandError::Failed(e.to_string()))?;
        transact(doc, |doc| {
            let image = i.id.clone();
            let (layer, transform, name, outline_id) = {
                let obj = util::object(doc, &image)?;
                (obj.layer.clone(), obj.transform, obj.name.clone(), obj.extras.get(MODEL_OUTLINE_KEY).cloned())
            };
            let obj = doc.objects.iter_mut().find(|x| x.id == image).ok_or_else(|| CommandError::Failed(format!("{image} vanished")))?;
            if let Geometry::Image { width_mm, height_mm, mime, data_base64, .. } = &mut obj.geometry {
                *width_mm = map.width_mm;
                *height_mm = map.height_mm;
                *mime = "image/png".into();
                *data_base64 = STANDARD.encode(&map.png).into();
            }
            obj.extras.insert(MODEL_OPTIONS_KEY.into(), options_json.clone());
            let mut created = Vec::new();
            let mut affected = vec![image.clone()];
            // The outline the import (or an earlier edit) made, while it is still a path.
            let existing = outline_id.filter(|o| doc.object(o).is_some_and(|x| matches!(x.geometry, Geometry::Path { .. })));
            let mut outline = None;
            if let Some(p) = outline_path(id, &map)? {
                match existing {
                    Some(oid) => {
                        if let Some(x) = doc.objects.iter_mut().find(|x| x.id == oid) {
                            x.geometry = Geometry::Path { path: p };
                        }
                        affected.push(oid.clone());
                        outline = Some(oid);
                    }
                    None if o.outline => {
                        let oid = add_object(doc, format!("{name} outline"), layer, Geometry::Path { path: p }, transform)?;
                        created.push(oid.clone());
                        outline = Some(oid);
                    }
                    None => {}
                }
            }
            if let Some(obj) = doc.objects.iter_mut().find(|x| x.id == image) {
                match &outline {
                    Some(oid) => obj.extras.insert(MODEL_OUTLINE_KEY.into(), oid.clone()),
                    None => obj.extras.remove(MODEL_OUTLINE_KEY),
                };
            }
            let mut r = util::edit_result(doc, &created, &affected);
            r["image"] = json!(image);
            r["outline"] = json!(outline);
            r["options"] = serde_json::to_value(&o).unwrap_or(Value::Null);
            model_result(&mut r, &map, triangles, &warnings);
            Ok((r, format!("rendered the 3D model of \"{name}\" again ({} x {} px)", map.width_px, map.height_px)))
        })
    }
}

#[cfg(test)]
mod tests {
    use doc::Document;
    use serde_json::{Value, json};

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: Value) -> Value {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).unwrap_or_else(|e| panic!("{id}: {e}")).result
    }

    pub(crate) fn session() -> Session {
        let mut s = Session::new(Document { field_mm: Some(100.0), ..Document::default() });
        run(&mut s, "shape.rect", json!({"width_mm": 20, "height_mm": 10, "center": [10, 5]}));
        run(&mut s, "shape.ellipse", json!({"rx_mm": 5, "ry_mm": 5, "center": [-20, 0]}));
        s
    }

    #[test]
    fn exports_every_format_and_selection() {
        let mut s = session();
        let svg = run(&mut s, "doc.export", json!({"format": "svg"}));
        assert_eq!(svg["mime"], "image/svg+xml");
        assert!(svg["content"].as_str().unwrap().contains("<svg"));
        let dxf = run(&mut s, "doc.export", json!({"format": "dxf", "ids": ["obj-2"]}));
        assert!(dxf["content"].as_str().unwrap().contains("POLYLINE") && !dxf["content"].as_str().unwrap().contains("CIRCLE"));
        let ai = run(&mut s, "doc.export", json!({"format": "ai"}));
        assert_eq!(ai["extension"], "ai");
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, ai["content_base64"].as_str().unwrap()).unwrap();
        assert!(bytes.starts_with(b"%PDF"));
        let print = run(&mut s, "doc.print", json!({"format": "svg"}));
        assert!(print["content"].as_str().unwrap().contains(r#"width="100mm""#), "the work area");
        let s2 = Session::new(s.document().clone());
        assert_eq!(s2.document().objects.len(), 2, "exports change nothing");
    }

    #[test]
    fn imports_hpgl_gerber_eps_and_round_trips_our_pdf_and_ai() {
        let mut s = session();
        let r = run(&mut s, "art.import_hpgl", json!({"content": "IN;SP1;PU0,0;PD400,0,400,400;SP2;PU800,0;PD1200,0;"}));
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        assert!(r["group"].is_string());
        let g = run(&mut s, "art.import_gerber", json!({"content": "%FSLAX24Y24*%%MOMM*%%ADD10C,1*%D10*X0Y0D03*M02*", "keep_position": true}));
        assert_eq!(g["created"].as_array().unwrap().len(), 1);
        let eps = "%!PS-Adobe-3.0 EPSF-3.0\nnewpath 0 0 moveto 72 0 lineto 72 72 lineto closepath stroke\n";
        let e = run(&mut s, "art.import_eps", json!({"data_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, eps), "width_mm": 10}));
        assert_eq!(e["size_mm"], json!([10.0, 10.0]));
        let pdf = run(&mut s, "doc.export", json!({"format": "pdf", "ids": ["obj-2"]}));
        let p = run(&mut s, "art.import_pdf", json!({"data_base64": pdf["content_base64"], "split": false}));
        let size: Vec<f64> = p["size_mm"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        assert!((size[0] - 20.0).abs() < 1e-3 && (size[1] - 10.0).abs() < 1e-3, "{size:?} (PDF reals are 32-bit)");
        let ai = run(&mut s, "art.import_ai", json!({"data_base64": pdf["content_base64"]}));
        assert_eq!(ai["created"].as_array().unwrap().len(), 1);
        assert!(s.execute(Invocation { id: "art.import_hpgl".into(), params: json!({"content": "nothing"}) }, Origin::Ui).is_err());
    }

    #[test]
    fn imports_gcode_cuts_by_power() {
        let mut s = session();
        let g = "G90 G21\nM4 S0\nG0 X10 Y10\nG1 X20 Y10 S500 F1200\nG1 Y20\nG0 X50 Y50\nG1 X60 S250\nM5\n";
        let r = run(&mut s, "art.import_gcode", json!({"content": g, "keep_position": true}));
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        assert_eq!(r["size_mm"], json!([50.0, 40.0]));
        let one = run(&mut s, "art.import_gcode", json!({"content": g, "split": false, "width_mm": 10}));
        assert_eq!(one["size_mm"], json!([10.0, 8.0]));
        let e = s.execute(Invocation { id: "art.import_gcode".into(), params: json!({"content": "M3 S0\nG1 X5 F100\n"}) }, Origin::Ui).unwrap_err();
        assert!(e.to_string().contains("cuts nothing"), "{e}");
    }

    /// Art kilometres wide (or far out with keep_position) is refused unless scaled down with width_mm.
    #[test]
    fn far_out_art_is_refused_unless_scaled() {
        let mut s = session();
        let huge = json!({"content": "IN;PU0,0;PD4000000000000,0,4000000000000,4000000000000;"});
        let e = s.execute(Invocation { id: "art.import_hpgl".into(), params: huge.clone() }, Origin::Ui).unwrap_err();
        assert!(e.to_string().contains("width_mm"), "{e}");
        let mut scaled = huge;
        scaled["width_mm"] = json!(50);
        assert_eq!(run(&mut s, "art.import_hpgl", scaled)["size_mm"], json!([50.0, 50.0]));
        let far = json!({"content": "IN;PU4000000000000,0;PD4000000000040,0,4000000000040,40;", "keep_position": true});
        assert!(s.execute(Invocation { id: "art.import_hpgl".into(), params: far }, Origin::Ui).is_err());
    }

    #[test]
    fn imports_a_model_as_a_depth_map_with_outline() {
        let mut s = session();
        let obj = "v 0 0 0\nv 10 0 0\nv 10 10 5\nv 0 10 5\nf 1 2 3 4\n";
        let r = run(&mut s, "art.import_model", json!({
            "data_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, obj),
            "dpi": 25.4, "outline": true
        }));
        assert_eq!((r["width_px"].as_u64(), r["height_px"].as_u64()), (Some(10), Some(10)));
        assert!(r["outline"].is_string());
        let img = s.document().object(r["image"].as_str().unwrap()).unwrap();
        assert!(matches!(img.geometry, doc::Geometry::Image { width_mm, .. } if (width_mm - 10.0).abs() < 1e-9));
        assert_eq!(r["editable"], json!(true));
        let opts: Value = serde_json::from_str(&img.extras[super::MODEL_OPTIONS_KEY]).unwrap();
        assert_eq!((opts["dpi"].clone(), opts["outline"].clone()), (json!(25.4), json!(true)));
    }

    /// UX-030: images of imported art (PDF, EzCad) become image objects where the file placed them, grouped with
    /// the paths.
    #[test]
    fn art_images_become_image_objects_grouped_with_the_paths() {
        use base64::Engine as _;
        let mut d = Document { field_mm: Some(100.0), ..Document::default() };
        let png = base64::engine::general_purpose::STANDARD.decode(PNG_1X1).unwrap();
        let mut path = kurbo::BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.line_to((10.0, 10.0));
        path.close_path();
        let art = fileio::Art {
            paths: vec![fileio::ArtPath { path, label: "path".into(), color: None, layer: None }],
            images: vec![fileio::ArtImage { data: png, mime: "image/png", placement: kurbo::Affine::new([20.0, 0.0, 0.0, 10.0, 10.0, 0.0]), label: "image".into() }],
            ..Default::default()
        };
        let p = super::Placement { layer: None, name: Some("Art"), width_mm: None, keep_position: true, split: false, file_layers: false };
        let (r, _) = super::place_art("t", &mut d, art, &p, "Art").unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 2);
        let img = d.objects.iter().find(|o| matches!(o.geometry, doc::Geometry::Image { .. })).unwrap();
        let b = img.bounds();
        assert!((b.x0 - 10.0).abs() < 1e-9 && (b.x1 - 30.0).abs() < 1e-9 && b.y0.abs() < 1e-9 && (b.y1 - 10.0).abs() < 1e-9, "{b:?}");
        assert!(matches!(img.geometry, doc::Geometry::Image { width_mm, height_mm, .. } if (width_mm - 20.0).abs() < 1e-9 && (height_mm - 10.0).abs() < 1e-9));
        assert!(img.group.is_some() && d.objects.iter().all(|o| o.group == img.group), "grouped with the path");
    }

    /// UX-038: layers an imported file carries (EzCad pens and hatches) become document layers for the paths on
    /// them; a text without outlines whose font is not installed leaves a placeholder and a warning.
    #[test]
    fn file_layers_and_texts_are_placed() {
        let mut d = Document { field_mm: Some(100.0), ..Document::default() };
        let sq = |x: f64| {
            let mut p = kurbo::BezPath::new();
            p.move_to((x, 0.0));
            p.line_to((x + 5.0, 0.0));
            p.line_to((x + 5.0, 5.0));
            p.close_path();
            p
        };
        let fill = doc::CutSettings { mode: doc::FillMode::Fill, power_pct: 40.0, ..doc::CutSettings::default() };
        let line = doc::CutSettings { mode: doc::FillMode::Line, speed_mm_s: 3000.0, ..doc::CutSettings::default() };
        let art = fileio::Art {
            paths: vec![
                fileio::ArtPath { path: sq(0.0), label: "Pen 1".into(), color: None, layer: Some(1) },
                fileio::ArtPath { path: sq(10.0), label: "hatch".into(), color: None, layer: Some(2) },
            ],
            layers: vec![
                fileio::ArtLayer { name: "Unused".into(), color: [0, 0, 0], settings: line.clone(), sub_layers: vec![] },
                fileio::ArtLayer { name: "Pen 1".into(), color: [255, 0, 0], settings: line, sub_layers: vec![] },
                fileio::ArtLayer { name: "Hatch".into(), color: [0, 0, 255], settings: fill.clone(), sub_layers: vec![fill] },
            ],
            texts: vec![fileio::ArtText { text: "Hi".into(), font: "No Such Font 123".into(), height_mm: 4.0, at: kurbo::Point::new(20.0, 0.0), angle_deg: 0.0, layer: Some(1) }],
            ..Default::default()
        };
        let p = super::Placement { layer: None, name: Some("E"), width_mm: None, keep_position: true, split: true, file_layers: true };
        let (r, _) = super::place_art("t", &mut d, art, &p, "E").unwrap();
        assert_eq!(r["layers"].as_array().unwrap().len(), 2, "only the used layers: {r}");
        let layer_named = |n: &str| d.layers.iter().find(|l| l.name == n).unwrap().clone();
        let (pen, hatch) = (layer_named("Pen 1"), layer_named("Hatch"));
        assert_eq!((pen.color.as_str(), pen.settings.speed_mm_s), ("#ff0000", 3000.0));
        assert_eq!((hatch.settings.mode.clone(), hatch.sub_layers.len()), (doc::FillMode::Fill, 1));
        let on = |l: &str| d.objects.iter().filter(|o| o.layer == l).count();
        assert_eq!((on(&pen.id), on(&hatch.id)), (2, 1), "the text placeholder goes on its pen's layer too");
        let ph = d.objects.iter().find(|o| o.name.starts_with("Text placeholder")).expect("a placeholder");
        assert!((ph.bounds().height() - 4.0).abs() < 1e-9 && (ph.bounds().x0 - 20.0).abs() < 1e-9, "{:?}", ph.bounds());
        assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("placeholder")), "{r}");
    }

    /// UX-040: Edit 3D Model renders the kept model again with new options; the image keeps its place, the outline
    /// follows, undo restores the old map, and an ordinary image is refused.
    #[test]
    fn edit_model_renders_the_kept_model_again() {
        let mut s = session();
        let obj = "v 0 0 0\nv 10 0 0\nv 10 10 5\nv 0 10 5\nf 1 2 3 4\n";
        let r = run(&mut s, "art.import_model", json!({
            "data_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, obj), "dpi": 25.4, "outline": true
        }));
        let (image, outline) = (r["image"].as_str().unwrap().to_owned(), r["outline"].as_str().unwrap().to_owned());
        run(&mut s, "object.transform", json!({"ids": [image], "translate": [5, 7]}));
        let before = s.document().clone();
        let r = run(&mut s, "image.edit_model", json!({"id": image, "width_mm": 20, "dpi": 50.8, "invert": true}));
        assert_eq!((r["width_px"].as_u64(), r["size_mm"].clone()), (Some(40), json!([20.0, 20.0])));
        assert_eq!((r["options"]["invert"].clone(), r["options"]["dpi"].clone(), r["outline"].clone()), (json!(true), json!(50.8), json!(outline)));
        let d = s.document();
        let img = d.object(&image).unwrap();
        assert_eq!(img.transform, before.object(&image).unwrap().transform, "the image stays where it was");
        assert!(matches!(img.geometry, doc::Geometry::Image { width_mm, .. } if (width_mm - 20.0).abs() < 1e-9));
        assert!((d.object(&outline).unwrap().bounds().width() - 20.0).abs() < 1e-6, "the outline follows");
        // width_mm null goes back to the model's own size.
        let r = run(&mut s, "image.edit_model", json!({"id": image, "width_mm": null}));
        assert_eq!(r["size_mm"], json!([10.0, 10.0]));
        s.undo().unwrap();
        s.undo().unwrap();
        assert_eq!(s.document().object(&image), before.object(&image));
        // Not a model: refused.
        run(&mut s, "art.import_image", json!({"mime": "image/png", "data_base64": PNG_1X1}));
        let plain = s.document().objects.last().unwrap().id.clone();
        assert!(s.execute(Invocation { id: "image.edit_model".into(), params: json!({"id": plain}) }, Origin::Ui).is_err());
        assert!(s.execute(Invocation { id: "image.edit_model".into(), params: json!({"id": outline}) }, Origin::Ui).is_err());
        assert!(s.execute(Invocation { id: "image.edit_model".into(), params: json!({"id": image, "dpi": -1}) }, Origin::Ui).is_err());
    }

    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAAAAAA6fptVAAAACklEQVR4nGNgAAAAAgABSK+kcQAAAABJRU5ErkJggg==";

    #[test]
    fn notes_are_read_from_and_written_into_lightburn_files() {
        let notes = doc::Notes { text: "Use 2 mm slate".into(), show_on_load: true };
        let mut extras = std::collections::BTreeMap::new();
        extras.insert("lbrn2._el.3".to_string(), r#"<Notes ShowOnLoad="1" Notes="Use 2 mm slate"/>"#.to_string());
        assert_eq!(super::notes_from_extras(&extras), notes);
        let mut p = lbrn::LbrnProject::default();
        super::notes_into_project(&mut p, &notes);
        let xml = lbrn::write(&p);
        assert!(xml.contains("Use 2 mm slate") && xml.contains(r#"ShowOnLoad="1""#), "{xml}");
    }
}
