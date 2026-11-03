//! Uploads, imports and downloads of the document (moved from the engine's `state` and `userfiles` modules): which
//! command an uploaded file runs (`POST /api/document/upload`, `POST /api/document/import`), the image sniffing, and
//! the document as a file (`GET /api/document/download`).

use std::collections::HashMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use commands::Invocation;
use doc::Document;
use doc::file::try_to_openetch_json;
use serde_json::{Value, json};

use crate::batch::content_params;
use crate::error::AppError;
use crate::project::{LBRN2_EXT, PROJECT_EXT, display_name};

/// File formats of the download and upload routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// The native `.openetch` JSON project.
    OpenEtch,
    /// A LightBurn `.lbrn2` project.
    Lbrn2,
}

impl FileFormat {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "openetch" => Ok(FileFormat::OpenEtch),
            "lbrn2" => Ok(FileFormat::Lbrn2),
            other => Err(AppError::BadRequest(format!("unknown format '{other}' (expected openetch or lbrn2)"))),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            FileFormat::OpenEtch => PROJECT_EXT,
            FileFormat::Lbrn2 => LBRN2_EXT,
        }
    }

    /// The `Content-Type` of a download.
    pub fn content_type(self) -> &'static str {
        match self {
            FileFormat::OpenEtch => "application/json; charset=utf-8",
            FileFormat::Lbrn2 => "application/xml; charset=utf-8",
        }
    }
}

/// Formats `POST /api/document/upload` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadFormat {
    /// A native `.openetch` project: replaces the document (`doc.load`).
    OpenEtch,
    /// A LightBurn `.lbrn2` project: `doc.import_lbrn2`, replace or merge.
    Lbrn2,
    /// SVG art, added to the document (`art.import_svg`).
    Svg,
    /// DXF drawing, added to the document (`art.import_dxf`: one object per entity, in one group).
    Dxf,
    /// A bitmap (PNG, JPEG, ...), added to the document (`art.import_image`).
    Image,
}

impl UploadFormat {
    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s {
            "openetch" => Ok(UploadFormat::OpenEtch),
            "lbrn2" => Ok(UploadFormat::Lbrn2),
            "svg" => Ok(UploadFormat::Svg),
            "dxf" => Ok(UploadFormat::Dxf),
            "image" => Ok(UploadFormat::Image),
            other => Err(AppError::BadRequest(format!("unknown format '{other}' (expected openetch, lbrn2, svg, dxf or image)"))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            UploadFormat::OpenEtch => "openetch",
            UploadFormat::Lbrn2 => "lbrn2",
            UploadFormat::Svg => "svg",
            UploadFormat::Dxf => "dxf",
            UploadFormat::Image => "image",
        }
    }
}

/// The query parameters of an upload, checked against its format by [`upload_invocation`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UploadOptions {
    /// `mode`: `Some(true)` merge, `Some(false)` replace, `None` not given.
    pub merge: Option<bool>,
    /// LightBurn only: the field the file was laid out for.
    pub field_mm: Option<f64>,
    /// SVG, DXF and images: the layer to put the art on (default the first layer).
    pub layer: Option<String>,
    /// SVG, DXF and images: the new object's name.
    pub name: Option<String>,
    /// SVG, DXF and images: scale to this width in mm.
    pub width_mm: Option<f64>,
    /// Images only: the media type (default: sniffed from the file's first bytes).
    pub mime: Option<String>,
}

/// The query parameters `POST /api/document/upload` accepts.
pub const UPLOAD_QUERY: [&str; 7] = ["format", "mode", "field_mm", "layer", "name", "width_mm", "mime"];

impl UploadOptions {
    /// The format and options of an upload's query (already checked against [`UPLOAD_QUERY`]).
    pub fn from_query(q: &HashMap<String, String>) -> Result<(UploadFormat, UploadOptions), AppError> {
        let format = UploadFormat::parse(q.get("format").map(String::as_str).ok_or_else(|| {
            AppError::BadRequest("the format query parameter is required (openetch, lbrn2, svg, dxf or image)".into())
        })?)?;
        let merge = match q.get("mode").map(String::as_str) {
            None => None,
            Some("replace") => Some(false),
            Some("merge") => Some(true),
            Some(other) => return Err(AppError::BadRequest(format!("unknown mode '{other}' (replace or merge)"))),
        };
        let opts = UploadOptions {
            merge,
            field_mm: q.get("field_mm").map(|v| crate::http::finite_param("field_mm", v)).transpose()?,
            layer: q.get("layer").cloned(),
            name: q.get("name").cloned(),
            width_mm: q.get("width_mm").map(|v| crate::http::finite_param("width_mm", v)).transpose()?,
            mime: q.get("mime").cloned(),
        };
        Ok((format, opts))
    }
}

/// The media type of a bitmap from its first bytes, for the formats browsers and LightBurn commonly use.
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if starts(b"BM") {
        Some("image/bmp")
    } else if bytes.len() >= 12 && starts(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        Some("image/tiff")
    } else {
        None
    }
}

/// UTF-8 text of an uploaded file without a byte-order mark (LightBurn and some editors write one). Moves the
/// bytes, never copies them.
pub fn upload_text(bytes: Vec<u8>, what: &str) -> Result<String, AppError> {
    let mut text = String::from_utf8(bytes).map_err(|_| AppError::BadRequest(format!("the file is not UTF-8 text (expected {what})")))?;
    if text.starts_with('\u{feff}') {
        text.drain(..'\u{feff}'.len_utf8());
    }
    Ok(text)
}

/// The command an uploaded file runs, and the display name the document gets (see the engine's `AppState::upload`):
/// `.openetch` through `doc.load` (replace only), `.lbrn2` through `doc.import_lbrn2` (`replace` or `merge`,
/// optional `field_mm`), SVG through `art.import_svg`, DXF through `art.import_dxf` and bitmaps through
/// `art.import_image` (merge only, optional `layer`, `name`, `width_mm`; images also `mime`, else sniffed). Options
/// that do not apply to the format are refused (400).
pub fn upload_invocation(format: UploadFormat, opts: UploadOptions, bytes: Vec<u8>) -> Result<(Invocation, Option<String>), AppError> {
    let refuse = |what: &str, only: &str| Err(AppError::BadRequest(format!("{what} is only for {only} uploads, not {}", format.name())));
    let art = matches!(format, UploadFormat::Svg | UploadFormat::Dxf | UploadFormat::Image);
    let display = match format {
        UploadFormat::OpenEtch | UploadFormat::Lbrn2 => opts.name.as_deref().and_then(display_name),
        _ => None,
    };
    if !art {
        if opts.layer.is_some() {
            return refuse("layer", "svg, dxf and image");
        }
        if opts.width_mm.is_some() {
            return refuse("width_mm", "svg, dxf and image");
        }
    }
    if format != UploadFormat::Image && opts.mime.is_some() {
        return refuse("mime", "image");
    }
    if format != UploadFormat::Lbrn2 && opts.field_mm.is_some() {
        return refuse("field_mm", "LightBurn");
    }
    let inv = match format {
        UploadFormat::OpenEtch => {
            if opts.merge == Some(true) {
                return Err(AppError::BadRequest(
                    "mode=merge is only for LightBurn imports; an .openetch project replaces the document".into(),
                ));
            }
            let text = upload_text(bytes, ".openetch JSON or .lbrn2 XML")?;
            Invocation { id: "doc.load".into(), params: content_params(text) }
        }
        UploadFormat::Lbrn2 => {
            let mut params = content_params(upload_text(bytes, ".openetch JSON or .lbrn2 XML")?);
            params["mode"] = json!(if opts.merge == Some(true) { "merge" } else { "replace" });
            if let Some(f) = opts.field_mm {
                params["field_mm"] = json!(f);
            }
            if let Some(n) = &opts.name {
                params["name"] = json!(n);
            }
            Invocation { id: "doc.import_lbrn2".into(), params }
        }
        UploadFormat::Svg | UploadFormat::Dxf | UploadFormat::Image => {
            if opts.merge == Some(false) {
                return Err(AppError::BadRequest(format!(
                    "{} uploads add to the document; use mode=merge or leave mode out",
                    format.name()
                )));
            }
            let mut params = serde_json::Map::new();
            let id = match format {
                UploadFormat::Svg => {
                    params.insert("svg".into(), Value::String(upload_text(bytes, "SVG")?));
                    "art.import_svg"
                }
                UploadFormat::Dxf => {
                    params.insert("content".into(), Value::String(upload_text(bytes, "DXF")?));
                    "art.import_dxf"
                }
                _ => {
                    let mime = match opts.mime {
                        Some(m) => m,
                        None => sniff_image(&bytes)
                            .ok_or_else(|| {
                                AppError::BadRequest(
                                    "not an image the engine recognises (PNG, JPEG, GIF, BMP, WebP or TIFF); pass \
                                     mime=image/... if it is one"
                                        .into(),
                                )
                            })?
                            .to_owned(),
                    };
                    let data = STANDARD.encode(&bytes);
                    drop(bytes);
                    params.insert("data_base64".into(), Value::String(data));
                    params.insert("mime".into(), Value::String(mime));
                    "art.import_image"
                }
            };
            if let Some(l) = opts.layer {
                params.insert("layer".into(), Value::String(l));
            }
            if let Some(n) = opts.name {
                params.insert("name".into(), Value::String(n));
            }
            if let Some(w) = opts.width_mm {
                params.insert("width_mm".into(), json!(w));
            }
            Invocation { id: id.into(), params: Value::Object(params) }
        }
    };
    Ok((inv, display))
}

/// The query parameters `POST /api/document/import` accepts.
pub const IMPORT_QUERY: [&str; 28] = [
    "format", "mode", "name", "layer", "width_mm", "keep_position", "split", "page", "hidden_layers", "units_per_mm", "invert", "kind",
    "model_format", "dpi", "outline", "sixteen_bit", "transparent_background", "depth_cutoff_mm", "equalize", "rotation_deg", "mime",
    "field_mm", "tool_layers", "pen_layers", "rd_origin", "bed_width_mm", "bed_height_mm", "rd_layers",
];

/// A query value as JSON: numbers, booleans, `a,b` lists for `ids` / `paper`, else text.
pub fn qvalue(key: &str, v: &str) -> Result<Value, AppError> {
    if matches!(key, "ids") {
        return Ok(json!(v.split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>()));
    }
    if matches!(key, "paper" | "rotation_deg") {
        let n: Result<Vec<f64>, _> = v.split(',').map(|s| s.trim().parse::<f64>()).collect();
        return match n {
            Ok(n) if n.iter().all(|x| x.is_finite()) => Ok(json!(n)),
            _ => Err(AppError::BadRequest(format!("{key} must be numbers separated by commas"))),
        };
    }
    if v == "true" || v == "false" {
        return Ok(json!(v == "true"));
    }
    if let Ok(n) = v.parse::<f64>()
        && !matches!(key, "name" | "layer" | "kind" | "format" | "colors" | "area" | "mime" | "render")
    {
        if !n.is_finite() {
            return Err(AppError::BadRequest(format!("{key} '{v}' is not a finite number")));
        }
        // Whole numbers stay integers, so integer options (a PDF `page`, a `dpi`) read; float options take them too.
        if let Ok(i) = v.trim().parse::<i64>() {
            return Ok(json!(i));
        }
        return Ok(json!(n));
    }
    Ok(json!(v))
}

/// The import command and its parameters for a format (the file in `content` or `data_base64`).
pub fn import_invocation(format: &str, bytes: Vec<u8>, q: &HashMap<String, String>) -> Result<Invocation, AppError> {
    let text = |bytes: Vec<u8>| -> Result<String, AppError> {
        let mut t = String::from_utf8(bytes).map_err(|_| AppError::BadRequest(format!("a {format} file must be UTF-8 text")))?;
        if t.starts_with('\u{feff}') {
            t.drain(..3);
        }
        Ok(t)
    };
    let mut params = serde_json::Map::new();
    let id = match format {
        "svg" => {
            params.insert("svg".into(), json!(text(bytes)?));
            "art.import_svg"
        }
        "dxf" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_dxf"
        }
        "hpgl" | "plt" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_hpgl"
        }
        "gcode" | "nc" | "gc" | "ngc" | "tap" => {
            params.insert("content".into(), json!(text(bytes)?));
            "art.import_gcode"
        }
        "rd" => {
            params.insert("data_base64".into(), json!(STANDARD.encode(&bytes)));
            "art.import_rd"
        }
        "gerber" | "drill" => {
            params.insert("content".into(), json!(text(bytes)?));
            if format == "drill" {
                params.insert("kind".into(), json!("excellon"));
            }
            "art.import_gerber"
        }
        "lbrn" | "lbrn2" | "lbt" => {
            params.insert("content".into(), json!(text(bytes)?));
            "doc.import_lbrn2"
        }
        "image" | "tiff" | "tga" => {
            let mime = match q.get("mime") {
                Some(m) => m.clone(),
                None => sniff_image(&bytes)
                    .map(str::to_owned)
                    .or_else(|| (format == "tga").then(|| "image/x-tga".to_string()))
                    .ok_or_else(|| AppError::BadRequest("not an image the engine recognises; pass mime=".into()))?,
            };
            params.insert("mime".into(), json!(mime));
            params.insert("data_base64".into(), json!(STANDARD.encode(&bytes)));
            "art.import_image"
        }
        "pdf" | "ai" | "eps" | "ezd" | "model" => {
            params.insert("data_base64".into(), json!(STANDARD.encode(&bytes)));
            match format {
                "pdf" => "art.import_pdf",
                "ai" => "art.import_ai",
                "eps" => "art.import_eps",
                "ezd" => "art.import_ezd",
                _ => "art.import_model",
            }
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown format '{other}' (svg, dxf, image, tiff, tga, pdf, ai, eps, ezd, hpgl, plt, gcode, nc, gerber, drill, model, lbrn, rd)"
            )));
        }
    };
    for (k, v) in q {
        match k.as_str() {
            "format" | "mode" | "mime" => {}
            "name" if id == "doc.import_lbrn2" => {
                params.insert("name".into(), json!(v));
            }
            // Art imports name their group (or object) after the file, without its extension.
            "name" => {
                let stem = v.rsplit_once('.').map_or(v.as_str(), |(s, _)| s);
                params.insert("name".into(), json!(stem));
            }
            "model_format" => {
                params.insert("format".into(), json!(v));
            }
            "rd_origin" => {
                params.insert("origin".into(), json!(v));
            }
            _ => {
                params.insert(k.clone(), qvalue(k, v)?);
            }
        }
    }
    Ok(Invocation { id: id.into(), params: Value::Object(params) })
}

/// The import's target layer under LightBurn's "Allow Importing to Tool Layers" (`tool_layers`): with it, `layer`
/// may be `T1` / `T2` (the tool layer, added in the same step when the document has none; returned as the commands
/// to run first); without it, those names are refused and an import without `layer` goes to the first ordinary
/// layer. A layer given by id is used as given. `doc` is the document the import goes into (a replacing import
/// starts from an empty one).
pub fn import_layer(doc: &Document, q: &mut HashMap<String, String>, tool_layers: bool) -> Result<Vec<Invocation>, AppError> {
    let off = || AppError::BadRequest("importing to tool layers is off: pass tool_layers=true (LightBurn's \"Allow Importing to Tool Layers\")".into());
    match q.get("layer").cloned() {
        Some(l) if doc.layer(&l).is_none() && (l.eq_ignore_ascii_case("t1") || l.eq_ignore_ascii_case("t2")) => {
            if !tool_layers {
                return Err(off());
            }
            let t = if l.eq_ignore_ascii_case("t1") { doc::ToolLayer::T1 } else { doc::ToolLayer::T2 };
            q.insert("layer".into(), t.name().into());
            if doc.layers.iter().any(|x| x.tool == Some(t)) {
                Ok(Vec::new())
            } else {
                Ok(vec![Invocation { id: "layer.tool".into(), params: json!({ "tool": t.name() }) }])
            }
        }
        // An existing layer named by id is the caller's choice (the web UI applies the preference itself).
        Some(_) => Ok(Vec::new()),
        None => {
            if !tool_layers && doc.layers.first().is_some_and(|x| x.tool.is_some()) {
                let plain = doc.layers.iter().find(|x| x.tool.is_none()).ok_or_else(|| {
                    AppError::BadRequest("the document has only tool layers: add a layer, or pass tool_layers=true".into())
                })?;
                q.insert("layer".into(), plain.id.clone());
            }
            Ok(Vec::new())
        }
    }
}

/// How an import request runs, from its query (`format`, `mode`, `tool_layers`, `name`; already checked against
/// [`IMPORT_QUERY`]).
#[derive(Debug, Clone)]
pub struct ImportRequest {
    pub format: String,
    pub replace: bool,
    pub tool_layers: bool,
    /// The file's name without its extension: names the document (and the import's group).
    pub display: Option<String>,
    /// The rest of the query (the command's options).
    pub query: HashMap<String, String>,
}

impl ImportRequest {
    /// Parse the query of `POST /api/document/import` (`body_empty`: the upload had no bytes).
    pub fn parse(mut q: HashMap<String, String>, body_empty: bool) -> Result<Self, AppError> {
        // LightBurn's "Allow Importing to Tool Layers" (off by default): only then may art land on T1 / T2.
        let tool_layers = match q.remove("tool_layers").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(v) => return Err(AppError::BadRequest(format!("tool_layers must be true or false (got '{v}')"))),
        };
        if body_empty {
            return Err(AppError::BadRequest("the upload is empty".into()));
        }
        let format = q.get("format").cloned().ok_or_else(|| AppError::BadRequest("format is required".into()))?;
        let replace = match q.get("mode").map(String::as_str) {
            None | Some("merge") => false,
            Some("replace") => true,
            Some(m) => return Err(AppError::BadRequest(format!("unknown mode '{m}' (merge or replace)"))),
        };
        let display = q.get("name").and_then(|n| display_name(n.rsplit_once('.').map_or(n.as_str(), |(stem, _)| stem)));
        Ok(ImportRequest { format, replace, tool_layers, display, query: q })
    }

    /// Whether the file is a LightBurn project (imported by `doc.import_lbrn2` itself, replace or merge).
    pub fn lightburn(&self) -> bool {
        matches!(self.format.as_str(), "lbrn" | "lbrn2" | "lbt")
    }

    /// The batch the import runs (`current`: the document now): a LightBurn file alone with its mode; otherwise the
    /// tool layer it needs, then the import, after a `doc.new` named after the file when it replaces the document.
    pub fn invocations(mut self, current: &Document, bytes: Vec<u8>) -> Result<Vec<Invocation>, AppError> {
        if self.lightburn() {
            let mut inv = import_invocation(&self.format, bytes, &self.query)?;
            inv.params["mode"] = json!(if self.replace { "replace" } else { "merge" });
            return Ok(vec![inv]);
        }
        let empty;
        let doc = if self.replace {
            empty = Document::default();
            &empty
        } else {
            current
        };
        let before = import_layer(doc, &mut self.query, self.tool_layers)?;
        let inv = import_invocation(&self.format, bytes, &self.query)?;
        let mut invs = Vec::new();
        if self.replace {
            invs.push(Invocation { id: "doc.new".into(), params: json!({ "name": self.display.clone().unwrap_or_else(|| "Untitled".into()) }) });
        }
        invs.extend(before);
        invs.push(inv);
        Ok(invs)
    }
}

/// The document as `.openetch` JSON, refusing one that would not load again (never written as `null`s). The
/// session refuses to commit such a document, so this is a backstop.
pub fn saveable(doc: &Document) -> Result<String, AppError> {
    try_to_openetch_json(doc).map_err(|e| AppError::Internal(e.to_string()))
}

/// The content of a `doc.export` / `doc.print` result: `content` text, else `content_base64` bytes.
pub fn decoded_content(r: &Value) -> Result<Vec<u8>, AppError> {
    if let Some(t) = r["content"].as_str() {
        return Ok(t.as_bytes().to_vec());
    }
    let b = r["content_base64"].as_str().unwrap_or_default();
    STANDARD.decode(b).map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_parse() {
        assert_eq!(FileFormat::parse("lbrn2").unwrap().extension(), "lbrn2");
        assert!(FileFormat::parse("svg").is_err(), "downloads are openetch or lbrn2");
        assert_eq!(UploadFormat::parse("svg").unwrap(), UploadFormat::Svg);
        assert!(UploadFormat::parse("pdf").is_err());
    }

    #[test]
    fn images_are_sniffed() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0"), Some("image/png"));
        assert_eq!(sniff_image(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a.."), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"II*\0...."), Some("image/tiff"));
        assert_eq!(sniff_image(b"<svg/>"), None);
        assert_eq!(sniff_image(b""), None);
    }

    #[test]
    fn saving_refuses_a_document_that_would_not_load_again() {
        let good = Document::default();
        assert!(doc::file::from_openetch_json(&saveable(&good).unwrap()).is_ok());
        let mut bad = Document::default();
        bad.layers[0].settings.power_pct = f64::NAN;
        assert_eq!(saveable(&bad).unwrap_err().status(), 500);
    }
}
