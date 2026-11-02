//! Multi-File Trace (LightBurn's Tools > Multi-File Trace Image, OTH-026): trace several images with one set of
//! Trace Image settings. `image.trace_many` traces image objects of the document (one path object each, one undo
//! step); `image.trace_files` traces image files without adding them to the document and returns one SVG per file
//! (LightBurn writes them next to the source images; a browser cannot, so they come back as text and, on request,
//! as one zip archive). Both use `image.trace`'s settings and code.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{Document, Geometry, ObjectId};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{base64_bytes, file_err};
use super::image_tools::{TraceInput, trace_object};
use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Most images one call traces.
pub const MAX_TRACE_IMAGES: usize = 100;
/// Largest image file `image.trace_files` takes, and the most for all files together, bytes.
const MAX_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// The Trace Image settings shared by every image (the same fields and defaults as `image.trace`).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceSettings {
    /// Darkest brightness traced, 0..=255 (default 0).
    pub cutoff: Option<u8>,
    /// Lightest brightness traced, 0..=255 (default 128).
    pub threshold: Option<u8>,
    /// Drop traced areas and holes (centre line: branches) of fewer pixels (default 2).
    pub ignore_less_than: Option<u32>,
    /// 0 (only lines) ..= 1.33 (only curves); default 1.
    pub smoothness: Option<f64>,
    /// Node reduction tolerance in pixels, 0 or more (default 0.2).
    pub optimize: Option<f64>,
    /// Trace along the middle of lines instead of their edges (Center Line Trace).
    #[serde(default)]
    pub centerline: bool,
    /// Find edges by local contrast.
    #[serde(default)]
    pub sketch: bool,
    /// Trace the border between transparent and opaque areas.
    #[serde(default)]
    pub transparency: bool,
}

impl TraceSettings {
    fn input(&self, id: ObjectId) -> TraceInput {
        TraceInput {
            id,
            cutoff: self.cutoff,
            threshold: self.threshold,
            ignore_less_than: self.ignore_less_than,
            smoothness: self.smoothness,
            optimize: self.optimize,
            centerline: self.centerline,
            sketch: self.sketch,
            transparency: self.transparency,
            boundary: None,
            delete_image: false,
            layer: None,
            name: None,
        }
    }

    fn options(&self) -> fileio::trace::TraceOptions {
        let d = fileio::trace::TraceOptions::default();
        fileio::trace::TraceOptions {
            cutoff: self.cutoff.unwrap_or(d.cutoff),
            threshold: self.threshold.unwrap_or(d.threshold),
            ignore_less_than: self.ignore_less_than.unwrap_or(d.ignore_less_than),
            smoothness: self.smoothness.unwrap_or(d.smoothness),
            optimize: self.optimize.unwrap_or(d.optimize),
            centerline: self.centerline,
            sketch: self.sketch,
            transparency: self.transparency,
            boundary_px: None,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceManyInput {
    /// Image object ids, or group ids (every image in the group). At most 100 images.
    pub ids: Vec<ObjectId>,
    /// Trace settings for all of them (as image.trace's; defaults when left out).
    #[serde(default)]
    pub settings: TraceSettings,
    /// Delete each image after tracing it (default false).
    #[serde(default)]
    pub delete_images: bool,
    /// Layer of the traced paths (default each image's layer).
    pub layer: Option<String>,
}

/// `image.trace_many`.
pub struct TraceMany;

impl Command for TraceMany {
    type Input = TraceManyInput;
    const ID: &'static str = "image.trace_many";
    const TITLE: &'static str = "Trace several images";
    const DESCRIPTION: &'static str = "LightBurn's Multi-File Trace for images in the document: \
        trace every image of ids (groups stand for their images; at most 100) with one set of image.trace settings, \
        one path object per image (\"Trace of <image>\"). Images with nothing traced are listed in `empty`. \
        delete_images, layer. Fails when nothing is traced. One undo step. Returns {created, traced: [{image, path, \
        contours, nodes}], empty, deleted}.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TraceManyInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let ids = util::object_ids(cmd, doc, &i.ids)?;
        let images: Vec<ObjectId> =
            ids.into_iter().filter(|id| doc.object(id).is_some_and(|o| matches!(o.geometry, Geometry::Image { .. }))).collect();
        if images.is_empty() {
            return Err(invalid(cmd, "no image among the ids"));
        }
        if images.len() > MAX_TRACE_IMAGES {
            return Err(invalid(cmd, format!("at most {MAX_TRACE_IMAGES} images at once (got {})", images.len())));
        }
        if let Some(l) = &i.layer {
            util::layer(doc, l)?;
        }
        if i.delete_images {
            util::check_unlocked(cmd, doc, &images)?;
        }
        // Trace them all before changing anything.
        let mut traced = Vec::new();
        let mut empty = Vec::new();
        for id in &images {
            let (path, contours, nodes) = trace_object(cmd, doc, &i.settings.input(id.clone()))?;
            if contours == 0 {
                empty.push(id.clone());
                continue;
            }
            validate_path(cmd, &path)?;
            traced.push((id.clone(), path, contours, nodes));
        }
        if traced.is_empty() {
            return Err(CommandError::Failed(format!("{cmd}: nothing to trace in any image with these settings (try a higher threshold)")));
        }
        transact(doc, |doc| {
            let mut created = Vec::new();
            let mut report = Vec::new();
            let mut deleted = Vec::new();
            for (id, path, contours, nodes) in traced {
                let image = doc.object(&id).cloned().ok_or_else(|| CommandError::Failed(format!("image {id} vanished")))?;
                let layer = match &i.layer {
                    Some(l) => util::layer(doc, l)?.id.clone(),
                    None => image.layer.clone(),
                };
                let c = kurbo::Shape::bounding_box(&path).center();
                let local = kurbo::Affine::translate(-c.to_vec2()) * path;
                let obj = add_object(doc, format!("Trace of {}", image.name), layer, Geometry::Path { path: local }, kurbo::Affine::translate(c.to_vec2()))?;
                if i.delete_images {
                    super::pathutil::delete_objects(doc, std::slice::from_ref(&id));
                    doc.image_masks.remove(&id);
                    deleted.push(id.clone());
                }
                report.push(json!({ "image": id, "path": obj, "contours": contours, "nodes": nodes }));
                created.push(obj);
            }
            let mut r = util::edit_result(doc, &created, &[]);
            r["traced"] = Value::Array(report);
            r["empty"] = json!(empty);
            r["deleted"] = json!(deleted);
            Ok((r, format!("traced {} image(s)", created.len())))
        })
    }
}

/// One image file for `image.trace_files`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceFile {
    /// The file name (used for the SVG's name: `<stem>.svg`).
    pub name: String,
    /// The file's bytes, base64 (a `data:` URL is accepted).
    pub data_base64: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceFilesInput {
    /// The image files (PNG, JPEG, GIF, BMP, WebP, TIFF, TGA and other formats the image import reads), at most 100.
    pub files: Vec<TraceFile>,
    /// Trace settings for all of them (as image.trace's; defaults when left out).
    #[serde(default)]
    pub settings: TraceSettings,
    /// Resolution for files that record none, dots per inch (default 254, 0.1 mm per pixel).
    pub dpi: Option<f64>,
    /// Also return every SVG in one zip archive (`zip_base64`, `zip_name`). Default false.
    #[serde(default)]
    pub zip: bool,
}

/// `image.trace_files`.
pub struct TraceFiles;

impl Command for TraceFiles {
    type Input = TraceFilesInput;
    const ID: &'static str = "image.trace_files";
    const TITLE: &'static str = "Trace image files";
    const DESCRIPTION: &'static str = "LightBurn's Multi-File Trace Image: trace image files (not in \
        the document) with one set of image.trace settings; one SVG per file, sized in mm from the file's resolution \
        (else dpi, default 254). The document is not changed (art.import_svg brings an SVG in). Unreadable files and \
        files with nothing to trace get an `error`. zip: also one archive of all SVGs. Returns {files: [{name, \
        svg_name, svg, contours, nodes, width_mm, height_mm} | {name, error}], traced, zip_base64?, zip_name?}. \
        Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(_doc: &mut Document, i: TraceFilesInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        if i.files.is_empty() {
            return Err(invalid(cmd, "give at least one file"));
        }
        if i.files.len() > MAX_TRACE_IMAGES {
            return Err(invalid(cmd, format!("at most {MAX_TRACE_IMAGES} files at once (got {})", i.files.len())));
        }
        let default_dpi = match i.dpi {
            Some(d) if d.is_finite() && (1.0..=100_000.0).contains(&d) => d,
            Some(d) => return Err(invalid(cmd, format!("dpi must be within 1..=100000 (got {d})"))),
            None => 254.0,
        };
        let opts = i.settings.options();
        let mut out = Vec::new();
        let mut entries: Vec<fileio::zip::Entry> = Vec::new();
        let mut total = 0usize;
        for f in &i.files {
            total += f.data_base64.len() / 4 * 3;
            if total > MAX_TOTAL_BYTES {
                return Err(invalid(cmd, format!("the files are larger than {} MB together", MAX_TOTAL_BYTES / (1024 * 1024))));
            }
            let name = f.name.trim();
            let bytes = base64_bytes(cmd, &f.data_base64, MAX_FILE_BYTES)?;
            match trace_file(cmd, &bytes, name, default_dpi, &opts) {
                Ok((svg, contours, nodes, w, h)) => {
                    let svg_name = unique(&svg_name(name), &entries);
                    if i.zip {
                        entries.push(fileio::zip::Entry { name: svg_name.clone(), data: svg.clone().into_bytes() });
                    } else {
                        entries.push(fileio::zip::Entry { name: svg_name.clone(), data: Vec::new() });
                    }
                    out.push(json!({ "name": name, "svg_name": svg_name, "svg": svg, "contours": contours, "nodes": nodes, "width_mm": util::r4(w), "height_mm": util::r4(h) }));
                }
                Err(e) => out.push(json!({ "name": name, "error": e.to_string() })),
            }
        }
        let traced = out.iter().filter(|f| f.get("svg").is_some()).count();
        let mut r = json!({ "files": out, "traced": traced });
        if i.zip && traced > 0 {
            let zip = fileio::zip::write(&entries).map_err(file_err(cmd))?;
            r["zip_base64"] = json!(STANDARD.encode(zip));
            r["zip_name"] = json!("traced.zip");
        }
        Ok((r, String::new()))
    }
}

/// `<stem>.svg` of a file name (path parts and characters unsafe in file names dropped).
fn svg_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    let clean: String = stem.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')).collect();
    let clean = clean.trim();
    format!("{}.svg", if clean.is_empty() { "trace" } else { clean })
}

/// `name`, or `name` with " (2)", " (3)", ... when an entry already has it.
fn unique(name: &str, entries: &[fileio::zip::Entry]) -> String {
    if !entries.iter().any(|e| e.name == name) {
        return name.to_owned();
    }
    let stem = name.trim_end_matches(".svg");
    (2..).map(|n| format!("{stem} ({n}).svg")).find(|c| !entries.iter().any(|e| &e.name == c)).unwrap_or_else(|| name.to_owned())
}

/// Trace one image file into an SVG document: (svg, contours, nodes, width mm, height mm).
fn trace_file(cmd: &str, bytes: &[u8], name: &str, default_dpi: f64, o: &fileio::trace::TraceOptions) -> Result<(String, usize, usize, f64, f64)> {
    let img = fileio::bitmap::decode(bytes, Some(name)).map_err(file_err(cmd))?;
    let g = fileio::bitmap::Gray::from_image(&img);
    let dpi = super::image::image_info(bytes)
        .and_then(|i| i.dpi)
        .or_else(|| fileio::bitmap::tiff_dpi(bytes))
        .filter(|(x, y)| *x >= 1.0 && *y >= 1.0)
        .unwrap_or((default_dpi, default_dpi));
    let (w, h) = (g.width as f64 * 25.4 / dpi.0, g.height as f64 * 25.4 / dpi.1);
    let t = fileio::trace::trace(&g, o).map_err(file_err(cmd))?;
    if t.contours == 0 {
        return Err(CommandError::Failed("nothing to trace with these settings (try a higher threshold)".into()));
    }
    // Pixels (y down) to mm with +Y up; svg_out::path_data flips Y back for the SVG.
    let to_mm = kurbo::Affine::new([25.4 / dpi.0, 0.0, 0.0, -25.4 / dpi.1, 0.0, 0.0]);
    let d = fileio::svg_out::path_data(&(to_mm * t.path));
    let (ws, hs) = (util::r4(w), util::r4(h));
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{ws}mm\" height=\"{hs}mm\" viewBox=\"0 0 {ws} {hs}\">\n  <path d=\"{d}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"0.1\" fill-rule=\"evenodd\"/>\n</svg>\n"
    );
    Ok((svg, t.contours, t.nodes, w, h))
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use doc::Document;
    use serde_json::json;

    use crate::{Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: serde_json::Value) -> crate::Result<serde_json::Value> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    /// A 20 x 10 px PNG: white with a black 6 x 4 block (at 254 dpi, 2 x 1 mm with a 0.6 x 0.4 mm block).
    fn png() -> Vec<u8> {
        let mut img = fileio::image::RgbaImage::from_pixel(20, 10, fileio::image::Rgba([255, 255, 255, 255]));
        for y in 3..7 {
            for x in 4..10 {
                img.put_pixel(x, y, fileio::image::Rgba([0, 0, 0, 255]));
            }
        }
        fileio::bitmap::png(&fileio::image::DynamicImage::ImageRgba8(img), None).unwrap()
    }

    fn blank() -> Vec<u8> {
        let img = fileio::image::RgbaImage::from_pixel(8, 8, fileio::image::Rgba([255, 255, 255, 255]));
        fileio::bitmap::png(&fileio::image::DynamicImage::ImageRgba8(img), None).unwrap()
    }

    #[test]
    fn images_in_the_document_are_traced_in_one_step() {
        let mut s = Session::new(Document { field_mm: Some(100.0), ..Document::default() });
        let mut ids = Vec::new();
        for (bytes, at) in [(png(), 0), (png(), 20), (blank(), 40)] {
            let r = run(&mut s, "art.import_image", json!({"data_base64": STANDARD.encode(bytes), "mime": "image/png", "width_mm": 10})).unwrap();
            let id = r["created"][0].as_str().unwrap().to_owned();
            run(&mut s, "object.transform", json!({"ids": [id], "translate": [at, 0]})).unwrap();
            ids.push(id);
        }
        let r = run(&mut s, "image.trace_many", json!({"ids": ids, "settings": {"smoothness": 0}, "delete_images": true})).unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["empty"], json!([ids[2]]), "the blank image has nothing to trace and stays");
        assert_eq!(r["deleted"].as_array().unwrap().len(), 2);
        let names: Vec<String> = s.document().objects.iter().map(|o| o.name.clone()).collect();
        assert_eq!(names.iter().filter(|n| n.starts_with("Trace of")).count(), 2, "{names:?}");
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 3, "one undo step brings the images back");
        assert!(run(&mut s, "image.trace_many", json!({"ids": [ids[2]]})).is_err(), "nothing traced at all");
    }

    #[test]
    fn files_are_traced_to_svg_without_touching_the_document() {
        let mut s = Session::new(Document { field_mm: Some(100.0), ..Document::default() });
        let files = json!([
            {"name": "C:\\art\\logo.png", "data_base64": STANDARD.encode(png())},
            {"name": "logo.png", "data_base64": STANDARD.encode(png())},
            {"name": "blank.png", "data_base64": STANDARD.encode(blank())},
            {"name": "junk.png", "data_base64": STANDARD.encode(b"not an image")},
        ]);
        let r = run(&mut s, "image.trace_files", json!({"files": files, "settings": {"smoothness": 0, "optimize": 0}, "zip": true})).unwrap();
        assert_eq!(r["traced"], 2, "{r}");
        let f = &r["files"];
        assert_eq!((f[0]["svg_name"].as_str(), f[1]["svg_name"].as_str()), (Some("logo.svg"), Some("logo (2).svg")));
        assert!(f[2]["error"].as_str().unwrap().contains("nothing to trace"));
        assert!(f[3]["error"].is_string());
        let svg = f[0]["svg"].as_str().unwrap();
        assert!(svg.contains("width=\"2mm\" height=\"1mm\""), "{svg}");
        // The block is 0.4 to 1.0 mm across and 0.3 to 0.7 mm down.
        assert!(svg.contains("M0.4 0.3") || svg.contains("0.4 0.3"), "{svg}");
        let zip = STANDARD.decode(r["zip_base64"].as_str().unwrap()).unwrap();
        assert_eq!(fileio::zip::names(&zip).unwrap(), vec!["logo.svg".to_string(), "logo (2).svg".to_string()]);
        assert!(s.document().objects.is_empty());
        assert!(run(&mut s, "image.trace_files", json!({"files": []})).is_err());
    }
}
