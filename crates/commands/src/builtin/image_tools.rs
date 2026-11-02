//! Image tools (LightBurn's Image Tools and Image Options): Trace Image (`image.trace`, with `image.trace_preview`
//! for the dialog's live preview, outline and 2.2's centre line), Convert to Bitmap (`art.convert_to_bitmap`), Apply
//! Mask to Image (`image.apply_mask`, `image.remove_mask`, `image.flatten_mask`), Replace Image / Replace Image to Fit
//! (`image.replace`), and the image file itself and as it will be engraved (`image.export`, `image.processed`: Save
//! Processed Bitmap).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{Document, FillMode, Geometry, Object, ObjectId};
use kurbo::{Affine, BezPath, Point, Shape};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::files::{browser_bitmap, file_err};
use super::pathutil;
use super::shape::{add_object, validate_path};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// An image object's geometry, refused with a message for anything else.
fn image_of<'a>(cmd: &str, doc: &'a Document, id: &str) -> Result<&'a Object> {
    let o = pathutil::single_object(cmd, doc, id, false)?;
    let o = doc.object(&o.id).ok_or_else(|| CommandError::Failed(format!("object {id} vanished")))?;
    if !matches!(o.geometry, Geometry::Image { .. }) {
        return Err(invalid(cmd, format!("{} (\"{}\") is not an image", o.id, o.name)));
    }
    Ok(o)
}

/// The decoded file of an image object.
fn decoded(cmd: &str, o: &Object) -> Result<(fileio::image::DynamicImage, Vec<u8>)> {
    let Geometry::Image { data_base64, mime, .. } = &o.geometry else { unreachable!("checked by image_of") };
    let bytes = STANDARD.decode(data_base64.as_bytes()).map_err(|_| invalid(cmd, format!("{}: the image data is not base64", o.id)))?;
    let img = fileio::bitmap::decode(&bytes, Some(mime)).map_err(file_err(cmd))?;
    Ok((img, bytes))
}

/// `{min: [x, y], max: [x, y]}` in the field frame.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FieldRect {
    /// Lower-left corner, field mm.
    pub min: [f64; 2],
    /// Upper-right corner, field mm.
    pub max: [f64; 2],
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceInput {
    /// The image object id.
    pub id: ObjectId,
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
    /// Trace along the middle of lines instead of their edges (LightBurn 2.2's Center Line Trace).
    #[serde(default)]
    pub centerline: bool,
    /// Find edges by local contrast (photos of drawings with uneven light).
    #[serde(default)]
    pub sketch: bool,
    /// Trace the border between transparent and opaque areas.
    #[serde(default)]
    pub transparency: bool,
    /// Trace only inside this field rectangle (LightBurn's boundary drag).
    pub boundary: Option<FieldRect>,
    /// image.trace: delete the image afterwards (default false).
    #[serde(default)]
    pub delete_image: bool,
    /// image.trace: layer of the traced path (default the image's layer).
    pub layer: Option<String>,
    /// image.trace: name (default "Trace of <image name>").
    pub name: Option<String>,
}

/// Trace an image object: the field-frame path, contours, nodes.
pub(super) fn trace_object(cmd: &str, doc: &Document, i: &TraceInput) -> Result<(BezPath, usize, usize)> {
    let o = image_of(cmd, doc, &i.id)?;
    let Geometry::Image { width_mm, height_mm, .. } = o.geometry else { unreachable!("an image") };
    let (img, _) = decoded(cmd, o)?;
    let g = fileio::bitmap::Gray::from_image(&img);
    let to_local = fileio::trace::pixel_to_local(g.width, g.height, width_mm, height_mm);
    let boundary_px = match i.boundary {
        Some(r) => {
            let t = to_local.inverse() * o.transform.inverse();
            let corners = [[r.min[0], r.min[1]], [r.max[0], r.min[1]], [r.max[0], r.max[1]], [r.min[0], r.max[1]]]
                .map(|[x, y]| t * Point::new(x, y));
            let xs = corners.map(|p| p.x);
            let ys = corners.map(|p| p.y);
            let f = |v: [f64; 4], max: bool| v.into_iter().fold(if max { f64::NEG_INFINITY } else { f64::INFINITY }, if max { f64::max } else { f64::min });
            Some([f(xs, false), f(ys, false), f(xs, true), f(ys, true)])
        }
        None => None,
    };
    let d = fileio::trace::TraceOptions::default();
    let opts = fileio::trace::TraceOptions {
        cutoff: i.cutoff.unwrap_or(d.cutoff),
        threshold: i.threshold.unwrap_or(d.threshold),
        ignore_less_than: i.ignore_less_than.unwrap_or(d.ignore_less_than),
        smoothness: i.smoothness.unwrap_or(d.smoothness),
        optimize: i.optimize.unwrap_or(d.optimize),
        centerline: i.centerline,
        sketch: i.sketch,
        transparency: i.transparency,
        boundary_px,
    };
    let t = fileio::trace::trace(&g, &opts).map_err(file_err(cmd))?;
    Ok((o.transform * to_local * t.path, t.contours, t.nodes))
}

/// `image.trace`.
pub struct TraceImage;

impl Command for TraceImage {
    type Input = TraceInput;
    const ID: &'static str = "image.trace";
    const TITLE: &'static str = "Trace image";
    const DESCRIPTION: &'static str = "LightBurn's Trace Image: vector outlines of the pixels whose brightness is in \
        cutoff..=threshold (0-255, default 0..128) of an image object, as one path object (field frame, following the \
        image's transform). ignore_less_than drops specks (pixels, default 2), smoothness 0..1.33 turns corners into \
        curves (default 1), optimize reduces nodes (default 0.2); centerline traces the middle of strokes instead (open \
        paths, LightBurn 2.2); sketch uses local contrast; transparency traces the opaque area; boundary limits it to a \
        field rectangle. delete_image, layer, name. Returns {created, contours, nodes, deleted}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: TraceInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let (path, contours, nodes) = trace_object(id, doc, &i)?;
        if contours == 0 {
            return Err(CommandError::Failed(format!("{id}: nothing to trace with these settings (try a higher threshold)")));
        }
        validate_path(id, &path)?;
        let image = doc.object(&i.id).cloned().ok_or_else(|| CommandError::Failed("the image vanished".into()))?;
        if i.delete_image {
            util::check_unlocked(id, doc, std::slice::from_ref(&image.id))?;
        }
        transact(doc, |doc| {
            let layer = match &i.layer {
                Some(l) => util::layer(doc, l)?.id.clone(),
                None => image.layer.clone(),
            };
            let name = match &i.name {
                Some(n) => util::name(id, "name", n)?,
                None => format!("Trace of {}", image.name),
            };
            let c = path.bounding_box().center();
            let local = Affine::translate(-c.to_vec2()) * path.clone();
            let obj = add_object(doc, name, layer, Geometry::Path { path: local }, Affine::translate(c.to_vec2()))?;
            let mut deleted = Vec::new();
            if i.delete_image {
                pathutil::delete_objects(doc, std::slice::from_ref(&image.id));
                doc.image_masks.remove(&image.id);
                deleted.push(image.id.clone());
            }
            let mut r = util::edit_result(doc, std::slice::from_ref(&obj), &[]);
            r["contours"] = json!(contours);
            r["nodes"] = json!(nodes);
            r["deleted"] = json!(deleted);
            Ok((r, format!("traced image {} ({contours} contour(s))", image.id)))
        })
    }
}

/// `image.trace_preview`.
pub struct TracePreview;

impl Command for TracePreview {
    type Input = TraceInput;
    const ID: &'static str = "image.trace_preview";
    const TITLE: &'static str = "Trace image preview";
    const DESCRIPTION: &'static str = "What image.trace would make with the same settings, without changing the \
        document (the Trace Image dialog's live preview): {d (SVG path data, field mm, +Y up), contours, nodes}. \
        delete_image, layer and name are ignored. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: TraceInput) -> Result<(Value, String)> {
        let (path, contours, nodes) = trace_object(Self::ID, doc, &i)?;
        Ok((json!({ "d": path.to_svg(), "contours": contours, "nodes": nodes }), String::new()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderType {
    /// Only the outlines.
    Outlines,
    /// The inside of closed shapes.
    FillAll,
    /// Outline or fill by each shape's layer mode (LightBurn's "Use Cut Settings").
    #[default]
    CutSettings,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToBitmapInput {
    /// Vector objects or groups to convert (images in the selection are skipped).
    pub ids: Vec<ObjectId>,
    /// Resolution in dots per inch (default 254).
    pub dpi: Option<f64>,
    #[serde(default)]
    pub render: RenderType,
    /// Grey of the drawn pixels, 0 black ..= 255 white (default 128: LightBurn's 50 % grey).
    pub gray: Option<u8>,
    /// Outline width in mm (default one pixel).
    pub line_width_mm: Option<f64>,
    /// Keep the vectors (LightBurn deletes them; default false).
    #[serde(default)]
    pub keep_original: bool,
    /// Layer of the bitmap (default the first converted object's layer).
    pub layer: Option<String>,
    /// Name (default "Bitmap").
    pub name: Option<String>,
}

/// `art.convert_to_bitmap`.
pub struct ConvertToBitmap;

impl Command for ConvertToBitmap {
    type Input = ToBitmapInput;
    const ID: &'static str = "art.convert_to_bitmap";
    const TITLE: &'static str = "Convert to bitmap";
    const DESCRIPTION: &'static str = "LightBurn's Convert to Bitmap: render vector objects into one image object at \
        dpi (default 254) covering their bounds. render: cut_settings (default: line layers outlined, fill layers \
        filled), fill_all or outlines; drawn pixels get gray (default 128, 50 %), the rest is white; line_width_mm \
        (default one pixel). The vectors are deleted unless keep_original. Returns {created, deleted, width_px, \
        height_px}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ToBitmapInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let ids = util::object_ids(id, doc, &i.ids)?;
        let vectors: Vec<&Object> = ids.iter().filter_map(|x| doc.object(x)).filter(|o| !matches!(o.geometry, Geometry::Image { .. })).collect();
        if vectors.is_empty() {
            return Err(invalid(id, "no vector objects to convert"));
        }
        if !i.keep_original {
            let v: Vec<ObjectId> = vectors.iter().map(|o| o.id.clone()).collect();
            util::check_unlocked(id, doc, &v)?;
        }
        let shapes: Vec<fileio::raster::RasterShape> = vectors
            .iter()
            .map(|o| {
                let mode = doc.layer(&o.layer).map_or(FillMode::Line, |l| l.settings.mode.clone());
                let (fill, outline) = match i.render {
                    RenderType::Outlines => (false, true),
                    RenderType::FillAll => (true, false),
                    RenderType::CutSettings => match mode {
                        FillMode::Line => (false, true),
                        FillMode::Fill | FillMode::OffsetFill => (true, false),
                        FillMode::FillThenLine => (true, true),
                    },
                };
                fileio::raster::RasterShape { path: fileio::clean_field_path(o), fill, outline }
            })
            .collect();
        let d = fileio::raster::RasterOptions::default();
        let o = fileio::raster::RasterOptions { dpi: i.dpi.unwrap_or(d.dpi), gray: i.gray.unwrap_or(d.gray), line_width_mm: i.line_width_mm };
        let r = fileio::raster::rasterize(&shapes, &o).map_err(file_err(id))?;
        let first_layer = vectors[0].layer.clone();
        let removed: Vec<ObjectId> = if i.keep_original { vec![] } else { vectors.iter().map(|o| o.id.clone()).collect() };
        transact(doc, |doc| {
            let layer = match &i.layer {
                Some(l) => util::layer(doc, l)?.id.clone(),
                None => first_layer,
            };
            let name = i.name.as_deref().map(|n| util::name(id, "name", n)).transpose()?.unwrap_or_else(|| "Bitmap".into());
            let g = Geometry::Image {
                width_mm: r.bounds.width(),
                height_mm: r.bounds.height(),
                mime: "image/png".into(),
                data_base64: STANDARD.encode(&r.png).into(),
                adjust: doc::ImageAdjust::default(),
            };
            let obj = add_object(doc, name, layer, g, Affine::translate(r.bounds.center().to_vec2()))?;
            pathutil::delete_objects(doc, &removed);
            let mut out = util::edit_result(doc, std::slice::from_ref(&obj), &[]);
            out["deleted"] = json!(removed);
            out["width_px"] = json!(r.width_px);
            out["height_px"] = json!(r.height_px);
            Ok((out, format!("converted {} object(s) to a bitmap", vectors_len(&shapes))))
        })
    }
}

fn vectors_len(s: &[fileio::raster::RasterShape]) -> usize {
    s.len()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyMaskInput {
    /// The image object id.
    pub image: ObjectId,
    /// The mask: a closed vector object, or a group whose members (all closed vectors, on one layer) mask together.
    pub mask: ObjectId,
}

/// `image.apply_mask`.
pub struct ApplyMask;

impl Command for ApplyMask {
    type Input = ApplyMaskInput;
    const ID: &'static str = "image.apply_mask";
    const TITLE: &'static str = "Apply mask to image";
    const DESCRIPTION: &'static str = "LightBurn's Apply Mask to Image: only the part of the image inside the mask \
        (a closed vector object, or a group on one layer; even-odd, so an inner shape cuts a hole) is engraved and \
        shown. Non-destructive: the mask stays an editable shape (put it on a tool layer, output off, so it does not \
        burn); deleting it or image.remove_mask shows the whole image again; image.flatten_mask bakes it in. Returns \
        {image, mask}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ApplyMaskInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let image = image_of(id, doc, &i.image)?.id.clone();
        let members = util::object_ids(id, doc, std::slice::from_ref(&i.mask))?;
        let mut layers = std::collections::BTreeSet::new();
        for m in &members {
            let o = util::object(doc, m)?;
            if matches!(o.geometry, Geometry::Image { .. }) {
                return Err(invalid(id, format!("the mask {m} is an image; use a closed vector shape")));
            }
            let open = geom::region::has_open_subpaths(&o.field_path()).unwrap_or(true);
            if open {
                return Err(invalid(id, format!("the mask {m} (\"{}\") is not a closed shape", o.name)));
            }
            layers.insert(o.layer.clone());
        }
        if members.contains(&image) {
            return Err(invalid(id, "an image cannot mask itself"));
        }
        if layers.len() > 1 {
            return Err(invalid(id, "every shape of a mask group must be on the same layer"));
        }
        doc.image_masks.insert(image.clone(), i.mask.clone());
        let mut r = util::edit_result(doc, &[], std::slice::from_ref(&image));
        r["image"] = json!(image);
        r["mask"] = json!(i.mask);
        Ok((r, format!("masked image {image} with {}", i.mask)))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaskIdsInput {
    /// Image object ids (or groups of them).
    pub ids: Vec<ObjectId>,
    /// image.flatten_mask: also cut the image down to the masked area (LightBurn's Crop Image). Default false.
    #[serde(default)]
    pub crop: bool,
}

/// `image.remove_mask`.
pub struct RemoveMask;

impl Command for RemoveMask {
    type Input = MaskIdsInput;
    const ID: &'static str = "image.remove_mask";
    const TITLE: &'static str = "Remove mask from image";
    const DESCRIPTION: &'static str = "LightBurn's Remove Mask from Image: the images in ids show and engrave whole \
        again (their mask shapes stay). Returns {affected}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MaskIdsInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let ids = util::object_ids(id, doc, &i.ids)?;
        let affected: Vec<ObjectId> = ids.into_iter().filter(|x| doc.image_masks.remove(x).is_some()).collect();
        if affected.is_empty() {
            return Err(invalid(id, "none of these images has a mask"));
        }
        let r = util::edit_result(doc, &[], &affected);
        Ok((r, format!("removed the mask of {}", affected.join(", "))))
    }
}

/// `image.flatten_mask`.
pub struct FlattenMask;

impl Command for FlattenMask {
    type Input = MaskIdsInput;
    const ID: &'static str = "image.flatten_mask";
    const TITLE: &'static str = "Flatten image mask";
    const DESCRIPTION: &'static str = "LightBurn's Flatten Image Mask / Crop Image: bake each masked image's mask into \
        the image (pixels outside become transparent, which never fires) and drop the link; with crop the image is \
        also cut down to the masked area (its placement is kept). As in LightBurn, the mask shapes are deleted (one \
        still masking another image stays; a locked one refuses). Returns {affected, deleted}. Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MaskIdsInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let ids = util::object_ids(id, doc, &i.ids)?;
        util::check_unlocked(id, doc, &ids)?;
        let mut changes = Vec::new();
        for x in &ids {
            let Some(mask) = doc.image_mask(x).cloned() else { continue };
            let o = image_of(id, doc, x)?;
            let Geometry::Image { width_mm, height_mm, .. } = o.geometry else { unreachable!("an image") };
            let (img, _) = decoded(id, o)?;
            let to_px = fileio::bitmap::local_to_pixel(img.width(), img.height(), width_mm, height_mm) * o.transform.inverse();
            let mut polys = Vec::new();
            for m in doc.expand_group_ids(std::slice::from_ref(&mask)) {
                if let Some(mo) = doc.object(&m) {
                    polys.extend(fileio::bitmap::polygons(&(to_px * mo.field_path()), 0.25));
                }
            }
            let (png, [x0, y0, x1, y1]) = fileio::bitmap::apply_mask(&img, &polys, i.crop).map_err(file_err(id))?;
            let (w, h) = (f64::from(img.width()), f64::from(img.height()));
            let (nw, nh) = (width_mm * f64::from(x1 - x0) / w, height_mm * f64::from(y1 - y0) / h);
            // The kept rectangle's centre in the image's local frame.
            let c = fileio::trace::pixel_to_local(img.width() as usize, img.height() as usize, width_mm, height_mm)
                * Point::new(f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0);
            changes.push((x.clone(), png, nw, nh, c));
        }
        if changes.is_empty() {
            return Err(invalid(id, "none of these images has a mask"));
        }
        let affected: Vec<ObjectId> = changes.iter().map(|c| c.0.clone()).collect();
        // LightBurn: flattening "also deletes the mask shape" (else it would still burn on its layer). A shape that
        // still masks an image not flattened here stays.
        let still_used: std::collections::BTreeSet<ObjectId> = doc
            .image_masks
            .iter()
            .filter(|(img, _)| !affected.contains(img))
            .flat_map(|(_, m)| doc.expand_group_ids(std::slice::from_ref(m)))
            .collect();
        let mut masks: Vec<ObjectId> = Vec::new();
        for x in &affected {
            if let Some(m) = doc.image_mask(x).cloned() {
                for id in doc.expand_group_ids(std::slice::from_ref(&m)) {
                    let shape = doc.object(&id).is_some_and(|o| !matches!(o.geometry, Geometry::Image { .. }));
                    if shape && !still_used.contains(&id) && !masks.contains(&id) {
                        masks.push(id);
                    }
                }
            }
        }
        util::check_unlocked(id, doc, &masks)?;
        for (x, png, nw, nh, c) in changes {
            doc.image_masks.remove(&x);
            if let Some(o) = doc.object_mut(&x) {
                o.transform *= Affine::translate(c.to_vec2());
                if let Geometry::Image { width_mm, height_mm, mime, data_base64, .. } = &mut o.geometry {
                    *width_mm = nw;
                    *height_mm = nh;
                    *mime = "image/png".into();
                    *data_base64 = STANDARD.encode(&png).into();
                }
            }
        }
        super::pathutil::delete_objects(doc, &masks);
        let mut r = util::edit_result(doc, &[], &affected);
        r["deleted"] = json!(masks);
        Ok((r, format!("flattened the mask of {}", affected.join(", "))))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReplaceFit {
    /// The new file at its own size (from its resolution, else 254 dpi), centred where the old one was
    /// (LightBurn's Replace Image).
    #[default]
    FileSize,
    /// Scaled to fit inside the old image's width and height, keeping its aspect ratio (Replace Image to Fit).
    Fit,
    /// Stretched to the old width and height.
    Stretch,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceInput {
    /// The image object id.
    pub id: ObjectId,
    /// The new image file in base64 (PNG, JPEG, GIF, BMP, WebP, TIFF, TGA).
    pub data_base64: String,
    /// Its type (default sniffed from the data).
    pub mime: Option<String>,
    #[serde(default)]
    pub fit: ReplaceFit,
}

/// `image.replace`.
pub struct ReplaceImage;

impl Command for ReplaceImage {
    type Input = ReplaceInput;
    const ID: &'static str = "image.replace";
    const TITLE: &'static str = "Replace image";
    const DESCRIPTION: &'static str = "LightBurn's Image Options > Replace Image / Replace Image to Fit / Refresh \
        Image: put a new file into an image object, keeping its place, rotation, layer, Adjust Image values, mask and \
        cut settings. fit: file_size (default, the file's own size), fit (inside the old size, keeping the aspect \
        ratio) or stretch. Refresh is a replace with the edited file. Returns {affected, width_px, height_px}. \
        Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ReplaceInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let o = image_of(id, doc, &i.id)?.clone();
        util::check_unlocked(id, doc, std::slice::from_ref(&o.id))?;
        let bytes = super::image::decode(id, &i.data_base64)?;
        let (bytes, converted) = browser_bitmap(id, bytes, i.mime.as_deref().unwrap_or_default())?;
        let info = super::image::image_info(&bytes).ok_or_else(|| invalid(id, "the data is not an image OpenEtch reads"))?;
        let Geometry::Image { width_mm: ow, height_mm: oh, .. } = o.geometry else { unreachable!("an image") };
        let (wpx, hpx) = (f64::from(info.width_px), f64::from(info.height_px));
        if wpx < 1.0 || hpx < 1.0 {
            return Err(invalid(id, "the image has no pixels"));
        }
        // The old transform's scale stays on the object: sizes here are local.
        let (w, h) = match i.fit {
            ReplaceFit::FileSize => {
                let (dx, dy) = info.dpi.filter(|(x, y)| *x >= 1.0 && *y >= 1.0).unwrap_or((254.0, 254.0));
                (wpx / dx * 25.4, hpx / dy * 25.4)
            }
            ReplaceFit::Fit => {
                let k = (ow / wpx).min(oh / hpx);
                (wpx * k, hpx * k)
            }
            ReplaceFit::Stretch => (ow, oh),
        };
        if let Some(Geometry::Image { width_mm, height_mm, mime, data_base64, .. }) = doc.object_mut(&o.id).map(|o| &mut o.geometry) {
            *width_mm = w;
            *height_mm = h;
            *mime = info.mime.into();
            *data_base64 = STANDARD.encode(&bytes).into();
        }
        let mut r = util::edit_result(doc, &[], std::slice::from_ref(&o.id));
        r["width_px"] = json!(info.width_px);
        r["height_px"] = json!(info.height_px);
        r["warnings"] = json!(converted.into_iter().collect::<Vec<_>>());
        Ok((r, format!("replaced the image of {}", o.id)))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageIdInput {
    /// The image object id.
    pub id: ObjectId,
    /// image.processed: 0 for the layer's main settings (default), n for its n-th sub-layer.
    pub sub_layer: Option<usize>,
}

/// `image.export`.
pub struct ExportImage;

impl Command for ExportImage {
    type Input = ImageIdInput;
    const ID: &'static str = "image.export";
    const TITLE: &'static str = "Export image";
    const DESCRIPTION: &'static str = "The file of an image object as it was imported (LightBurn's Export of a single \
        selected image; no cut settings or adjustments): {mime, extension, data_base64, width_px, height_px}. \
        Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ImageIdInput) -> Result<(Value, String)> {
        let o = image_of(Self::ID, doc, &i.id)?;
        let Geometry::Image { mime, data_base64, .. } = &o.geometry else { unreachable!("an image") };
        let bytes = STANDARD.decode(data_base64.as_bytes()).unwrap_or_default();
        let info = super::image::image_info(&bytes);
        let ext = mime.trim_start_matches("image/").replace("jpeg", "jpg");
        Ok((
            json!({
                "mime": mime,
                "extension": ext,
                "data_base64": data_base64.as_ref(),
                "width_px": info.as_ref().map(|i| i.width_px),
                "height_px": info.as_ref().map(|i| i.height_px),
            }),
            String::new(),
        ))
    }
}

/// `image.processed`.
pub struct ProcessedImage;

impl Command for ProcessedImage {
    type Input = ImageIdInput;
    const ID: &'static str = "image.processed";
    const TITLE: &'static str = "Save processed bitmap";
    const DESCRIPTION: &'static str = "LightBurn's Save Processed Bitmap: the image as it will be engraved with its \
        layer's image settings (sub_layer 0 = main, n = n-th sub-layer): Adjust Image, negative and the dither mode at \
        the line interval, as a PNG (fired dots black; grayscale and 3D Sliced as grey levels) with its dpi. Returns \
        {png_base64, width_px, height_px, dpi}. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ImageIdInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let o = image_of(id, doc, &i.id)?;
        let layer = util::layer(doc, &o.layer)?;
        let k = i.sub_layer.unwrap_or(0);
        let cut = if k == 0 {
            &layer.settings
        } else {
            layer.sub_layers.get(k - 1).ok_or_else(|| invalid(id, format!("layer {} has no sub-layer {k}", layer.id)))?
        };
        let b = plan::processed_bitmap(o, &cut.image, cut.passes).map_err(|e| invalid(id, e))?;
        let png = plan::processed_png(&b).map_err(|e| invalid(id, e))?;
        Ok((
            json!({ "png_base64": STANDARD.encode(png), "width_px": b.width, "height_px": b.height, "dpi": util::r4(b.dpi) }),
            String::new(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use doc::Document;
    use serde_json::{Value, json};

    use crate::{CommandError, Invocation, Origin, Session};

    fn run(s: &mut Session, id: &str, params: Value) -> Result<Value, CommandError> {
        s.execute(Invocation { id: id.into(), params }, Origin::Ui).map(|o| o.result)
    }

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> String {
        let img = image::GrayImage::from_fn(w, h, |x, y| image::Luma([f(x, y)]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out)
    }

    /// A 20 x 20 mm image (40 x 40 px) with a black square in the middle, centred at (10, 0).
    fn with_image() -> (Session, String) {
        let mut s = Session::new(Document { field_mm: Some(100.0), ..Document::default() });
        let data = png(40, 40, |x, y| if (10..30).contains(&x) && (10..30).contains(&y) { 0 } else { 255 });
        let r = run(&mut s, "art.import_image", json!({"data_base64": data, "mime": "image/png", "width_mm": 20})).unwrap();
        let id = r["created"][0].as_str().unwrap().to_string();
        run(&mut s, "object.transform", json!({"ids": [id], "translate": [10, 0]})).unwrap();
        (s, id)
    }

    #[test]
    fn trace_follows_the_image_transform_and_preview_changes_nothing() {
        let (mut s, img) = with_image();
        let p = run(&mut s, "image.trace_preview", json!({"id": img, "smoothness": 0})).unwrap();
        assert_eq!(p["contours"], 1);
        assert_eq!(s.document().objects.len(), 1);
        let r = run(&mut s, "image.trace", json!({"id": img, "smoothness": 0, "delete_image": true})).unwrap();
        // The black square is 10 x 10 mm around (10, 0).
        assert_eq!(r["bounds"]["min"], json!([5.0, -5.0]));
        assert_eq!(r["bounds"]["max"], json!([15.0, 5.0]));
        assert_eq!(r["deleted"], json!([img]));
        assert_eq!(s.document().objects.len(), 1);
        s.undo().unwrap();
        assert_eq!(s.document().objects.len(), 1, "undo brings the image back (and removes the trace)");
        // Boundary: the right half only.
        let r = run(&mut s, "image.trace", json!({"id": img, "smoothness": 0, "boundary": {"min": [10, -10], "max": [20, 10]}})).unwrap();
        assert_eq!(r["bounds"]["min"], json!([10.0, -5.0]));
        // Centre line of a 2 mm thick bar is one open stroke along its middle.
        let bar = png(40, 40, |x, y| if (4..36).contains(&x) && (18..22).contains(&y) { 0 } else { 255 });
        let b = run(&mut s, "art.import_image", json!({"data_base64": bar, "mime": "image/png", "width_mm": 20})).unwrap();
        let bar = b["created"][0].as_str().unwrap();
        let r = run(&mut s, "image.trace", json!({"id": bar, "centerline": true})).unwrap();
        assert_eq!(r["contours"], 1);
        assert!(r["bounds"]["size"][1].as_f64().unwrap() < 1.0 && r["bounds"]["size"][0].as_f64().unwrap() > 10.0, "{r}");
        assert!(run(&mut s, "image.trace", json!({"id": img, "threshold": 0, "cutoff": 1})).is_err());
    }

    #[test]
    fn convert_to_bitmap_masks_and_flatten() {
        let mut s = Session::new(Document { field_mm: Some(100.0), ..Document::default() });
        let r = run(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10, "center": [0, 0]})).unwrap();
        let rect = r["created"][0].as_str().unwrap().to_string();
        let b = run(&mut s, "art.convert_to_bitmap", json!({"ids": [rect], "dpi": 25.4, "render": "fill_all", "gray": 0})).unwrap();
        assert_eq!(b["deleted"], json!([rect]));
        assert_eq!((b["width_px"].as_u64(), b["height_px"].as_u64()), (Some(10), Some(10)));
        let img = b["created"][0].as_str().unwrap().to_string();
        // Mask with a 4 mm circle on a tool layer.
        let l = run(&mut s, "layer.add", json!({"name": "Mask"})).unwrap();
        run(&mut s, "layer.update", json!({"id": l["id"], "output": false})).unwrap();
        let c = run(&mut s, "shape.ellipse", json!({"rx_mm": 2, "ry_mm": 2, "layer": l["id"]})).unwrap();
        let circle = c["created"][0].as_str().unwrap().to_string();
        run(&mut s, "image.apply_mask", json!({"image": img, "mask": circle})).unwrap();
        assert_eq!(s.document().image_mask(&img), Some(&circle));
        assert!(run(&mut s, "image.apply_mask", json!({"image": circle, "mask": img})).is_err());
        let r = run(&mut s, "image.flatten_mask", json!({"ids": [img], "crop": true})).unwrap();
        assert_eq!(r["affected"], json!([img]));
        assert!(s.document().image_mask(&img).is_none());
        // LightBurn deletes the mask shape on flatten (left in place it would still burn on an output layer).
        assert_eq!(r["deleted"], json!([circle]));
        assert!(s.document().object(&circle).is_none());
        let doc::Geometry::Image { width_mm, .. } = s.document().object(&img).unwrap().geometry else { panic!() };
        assert!((width_mm - 4.0).abs() < 1.01, "cropped to the circle: {width_mm}");
        s.undo().unwrap();
        assert!(s.document().object(&circle).is_some(), "undo brings the mask back");
        // A shape masking two images stays while the other still uses it.
        let o = run(&mut s, "art.convert_to_bitmap", json!({"ids": [circle], "dpi": 25.4, "render": "fill_all", "keep_original": true})).unwrap();
        let img2 = o["created"][0].as_str().unwrap().to_string();
        run(&mut s, "image.apply_mask", json!({"image": img2, "mask": circle})).unwrap();
        let r = run(&mut s, "image.flatten_mask", json!({"ids": [img]})).unwrap();
        assert_eq!(r["deleted"], json!([]));
        assert!(s.document().object(&circle).is_some());
        for _ in 0..3 {
            s.undo().unwrap();
        }
        run(&mut s, "image.remove_mask", json!({"ids": [img]})).unwrap();
        assert!(run(&mut s, "image.remove_mask", json!({"ids": [img]})).is_err());
    }

    #[test]
    fn replace_export_and_processed() {
        let (mut s, img) = with_image();
        let new = png(10, 20, |_, _| 0);
        run(&mut s, "image.replace", json!({"id": img, "data_base64": new, "fit": "fit"})).unwrap();
        let o = s.document().object(&img).unwrap();
        let doc::Geometry::Image { width_mm, height_mm, .. } = o.geometry else { panic!() };
        assert_eq!((width_mm, height_mm), (10.0, 20.0));
        assert_eq!(o.bounds().center(), kurbo::Point::new(10.0, 0.0), "kept its place");
        let e = run(&mut s, "image.export", json!({"id": img})).unwrap();
        assert_eq!((e["extension"].as_str(), e["width_px"].as_u64()), (Some("png"), Some(10)));
        run(&mut s, "layer.update", json!({"id": "layer-1", "settings": {"image": {"dither": "threshold", "interval_mm": 1}}})).unwrap();
        let p = run(&mut s, "image.processed", json!({"id": img})).unwrap();
        assert_eq!((p["width_px"].as_u64(), p["height_px"].as_u64()), (Some(10), Some(20)));
        assert!(run(&mut s, "image.processed", json!({"id": img, "sub_layer": 3})).is_err());
        // TIFF converts to PNG on replace.
        let mut tiff = Vec::new();
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(4, 4, image::Luma([0])))
            .write_to(&mut std::io::Cursor::new(&mut tiff), image::ImageFormat::Tiff)
            .unwrap();
        let t = base64::engine::general_purpose::STANDARD.encode(tiff);
        let r = run(&mut s, "image.replace", json!({"id": img, "data_base64": t, "mime": "image/tiff"})).unwrap();
        assert!(r["warnings"][0].as_str().unwrap().contains("PNG"));
        let r = run(&mut s, "art.import_image", json!({"data_base64": t, "mime": "image/tiff"})).unwrap();
        let o = s.document().object(r["created"][0].as_str().unwrap()).unwrap();
        assert!(matches!(&o.geometry, doc::Geometry::Image { mime, .. } if mime == "image/png"));
    }
}
