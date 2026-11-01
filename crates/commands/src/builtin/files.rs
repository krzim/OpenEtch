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
}

/// Add imported art to the document: scaled to `width_mm`, centred on the field origin unless `keep_position`, one
/// object per path in a group named after the file (`split`, the default, as LightBurn groups imports) or one
/// combined object. Returns `{created, group, bounds, warnings, size_mm}`.
pub(super) fn place_art(cmd: &str, doc: &mut Document, art: Art, p: &Placement, default_name: &str) -> Result<(Value, String)> {
    let Art { paths, mut warnings } = art;
    if paths.is_empty() {
        return Err(CommandError::Failed(format!("{cmd}: the file contains no vector paths")));
    }
    let base = p.name.map(|n| util::name(cmd, "name", n)).transpose()?.unwrap_or_else(|| default_name.to_string());
    let bounds = paths.iter().map(|a| a.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
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
        let mut created: Vec<ObjectId> = Vec::new();
        let mut group = None;
        if p.split && paths.len() > 1 && paths.len() <= MAX_SPLIT {
            let gid = doc.alloc_id("group")?;
            doc.groups.push(Group::new(gid.clone(), base.clone(), None));
            group = Some(gid);
            for (n, a) in paths.into_iter().enumerate() {
                let path = fit * a.path;
                validate_path(cmd, &path)?;
                let obj = add_object(doc, format!("{base} {} {}", a.label, n + 1), layer.clone(), Geometry::Path { path }, Affine::IDENTITY)?;
                if let Some(o) = doc.object_mut(&obj) {
                    o.group = group.clone();
                }
                created.push(obj);
            }
        } else {
            let mut all = BezPath::new();
            for a in paths {
                all.extend(fit * a.path);
            }
            validate_path(cmd, &all)?;
            created.push(add_object(doc, base.clone(), layer, Geometry::Path { path: all }, Affine::IDENTITY)?);
        }
        let mut r = util::edit_result(doc, &created, &[]);
        r["group"] = json!(group);
        r["warnings"] = json!(warnings);
        r["size_mm"] = json!([util::r4(size.width), util::r4(size.height)]);
        Ok((r, format!("imported {base} ({} object(s))", created.len())))
    })
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
        "Import the vector paths of one page (default 1) of a PDF (base64) in mm: paths and rectangles of the page \
         and its forms; text, images and gradients are left out with a warning. hidden_layers imports optional \
         content that is off. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportPdfInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, MAX_FILE_BYTES)?;
        let art = fileio::pdf_in::parse(&bytes, &fileio::pdf_in::PdfImportOptions { page: i.page, hidden_layers: i.hidden_layers })
            .map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
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
         layers), older PostScript-based ones through the EPS reader. Vectors only; text and images are left out with \
         a warning. ",
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
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
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

/// `art.import_ezd`.
pub struct ImportEzd;

impl Command for ImportEzd {
    type Input = ImportEpsInput;
    const ID: &'static str = "art.import_ezd";
    const TITLE: &'static str = "Import EzCad project";
    const DESCRIPTION: &'static str = concat!(
        "Import the vector objects of an EzCad 2 .ezd project (base64; not in LightBurn): curves, rectangles, \
         circles, ellipses, polygons, groups and the outlines of hatches, one path per object named after its pen. \
         Text, images, hatch and pen settings are not read (a warning lists them). keep_position defaults to false; \
         EzCad's field frame matches OpenEtch's, so true keeps every object where EzCad had it. ",
        placement_doc!()
    );
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportEpsInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = base64_bytes(id, &i.data_base64, fileio::ezd::MAX_BYTES)?;
        let art = fileio::ezd::parse(&bytes).map_err(file_err(id))?;
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
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
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
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
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
        place_art(id, doc, art, &p, "Imported HPGL")
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
        let p = Placement { layer: i.layer.as_deref(), name: i.name.as_deref(), width_mm: i.width_mm, keep_position: i.keep_position, split: i.split.unwrap_or(true) };
        place_art(id, doc, art, &p, default)
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// 3D models.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
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
        let format = i.format.map(|f| match f {
            ModelFormat::Stl => fileio::mesh::Format::Stl,
            ModelFormat::Obj => fileio::mesh::Format::Obj,
            ModelFormat::Off => fileio::mesh::Format::Off,
            ModelFormat::Ply => fileio::mesh::Format::Ply,
            ModelFormat::ThreeMf => fileio::mesh::Format::ThreeMf,
            ModelFormat::Amf => fileio::mesh::Format::Amf,
        });
        let (mesh, mut warnings) = fileio::mesh::load(&bytes, format).map_err(file_err(id))?;
        drop(bytes);
        let dpi = i.dpi.map(|d| util::positive(id, "dpi", d)).transpose()?.unwrap_or(254.0);
        let o = fileio::mesh::DepthOptions {
            rotation_deg: i.rotation_deg.unwrap_or([0.0; 3]),
            width_mm: i.width_mm,
            px_per_mm: dpi / 25.4,
            sixteen_bit: i.sixteen_bit,
            invert: i.invert,
            transparent_background: i.transparent_background,
            depth_cutoff_mm: i.depth_cutoff_mm,
            equalize: i.equalize,
        };
        let map = fileio::mesh::depth_map(&mesh, &o).map_err(file_err(id))?;
        if map.width_mm > 300.0 || map.height_mm > 300.0 {
            warnings.push(format!("the depth map is {:.1} x {:.1} mm; pass width_mm to scale the model", map.width_mm, map.height_mm));
        }
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
            if i.outline && !map.outline.is_empty() {
                let mut p = BezPath::new();
                for ring in &map.outline {
                    p.move_to(ring[0]);
                    for q in &ring[1..] {
                        p.line_to(*q);
                    }
                    p.close_path();
                }
                validate_path(id, &p)?;
                let o = add_object(doc, format!("{name} outline"), layer, Geometry::Path { path: p }, Affine::IDENTITY)?;
                created.push(o.clone());
                outline = Some(o);
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["image"] = json!(image);
            r["outline"] = json!(outline);
            r["width_px"] = json!(map.width_px);
            r["height_px"] = json!(map.height_px);
            r["depth_mm"] = json!(util::r4(map.depth_mm));
            r["size_mm"] = json!([util::r4(map.width_mm), util::r4(map.height_mm)]);
            r["triangles"] = json!(mesh.triangles.len());
            r["warnings"] = json!(warnings);
            Ok((r, format!("imported a 3D model as depth map \"{name}\" ({} x {} px)", map.width_px, map.height_px)))
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
    }

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
