//! PDF and PDF-based Adobe Illustrator import (LightBurn's Import of `.pdf` and `.ai`, Illustrator 9 and later):
//! the vector paths of one page (default the first) in mm, +Y up, in page coordinates.
//!
//! The file is parsed with lopdf (MIT). The page's content streams run through a small interpreter of the path and
//! graphics-state operators (`q Q cm m l c v y h re S s f F f* B B* b b* n`, the colour operators for the
//! colour each path gets, and `Do` of form XObjects, recursively with their `Matrix`). Marked content of optional
//! content groups that are off by default (hidden Illustrator layers) is skipped unless `hidden_layers` is set
//! (LightBurn's "import hidden layers"). Image XObjects drawn with `Do` are imported as images (LightBurn imports them
//! too), placed by the CTM: JPEG (`DCTDecode`) data as it is, 8-bit gray, RGB, CMYK, ICC-based and indexed samples
//! (and 1-bit gray) decoded to PNG with their soft mask as alpha. Stencil masks, inline images (`BI`) and other
//! codecs (JPEG 2000, CCITT, JBIG2) are left out with a warning. Text (`BT`...`ET`) and shadings are left out with a
//! warning: convert text to outlines before saving. An Illustrator file saved without "Create PDF Compatible File" holds no
//! PDF page content and is refused with that advice.

use std::collections::BTreeSet;

use kurbo::{Affine, BezPath, Point};
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::{Art, FileError, Result, invalid};

/// Largest file read.
pub const MAX_BYTES: usize = 100 * 1024 * 1024;
/// Largest decompressed content per page or form (decoding holds every operation in memory, ~70 bytes each).
const MAX_CONTENT: usize = 32 * 1024 * 1024;
/// Most content decoded in total: a form drawn many times (`Do` in a form tree) is decoded each time.
const MAX_TOTAL_CONTENT: usize = 256 * 1024 * 1024;
/// Most operators interpreted.
const MAX_OPS: usize = 20_000_000;
/// Most pixels of one embedded image decoded.
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
/// Most embedded images imported (each further one is left out with a warning).
const MAX_IMAGES: usize = 200;

const MM_PER_PT: f64 = 25.4 / 72.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PdfImportOptions {
    /// 1-based page number (default 1).
    pub page: Option<u32>,
    /// Import content of hidden optional-content groups (hidden Illustrator layers).
    pub hidden_layers: bool,
}

#[derive(Clone)]
struct GState {
    ctm: Affine,
    stroke: Option<[u8; 3]>,
    fill: Option<[u8; 3]>,
}

struct Ctx<'a> {
    doc: &'a Document,
    off: BTreeSet<ObjectId>,
    hidden_layers: bool,
    art: Art,
    ops: usize,
    /// Content bytes decoded so far.
    content_bytes: usize,
    texts: usize,
    /// Images left out (inline, stencil masks, codecs not decoded).
    images: usize,
    /// Why images were left out.
    image_problems: BTreeSet<String>,
    forms_depth: usize,
}

fn f(o: &Object) -> Option<f64> {
    match *o {
        Object::Integer(i) => Some(i as f64),
        Object::Real(r) => Some(f64::from(r)),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Vec<f64> {
    ops.iter().filter_map(f).collect()
}

fn rgb(v: &[f64]) -> Option<[u8; 3]> {
    let c = |x: f64| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    match v.len() {
        1 => Some([c(v[0]); 3]),
        3 => Some([c(v[0]), c(v[1]), c(v[2])]),
        4 => {
            let k = 1.0 - v[3];
            Some([c((1.0 - v[0]) * k), c((1.0 - v[1]) * k), c((1.0 - v[2]) * k)])
        }
        _ => None,
    }
}

/// Resolve a reference (or return the object).
fn deref<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    doc.dereference(o).ok().map(|(_, o)| o)
}

fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().and_then(|o| deref(doc, o))
}

/// The ids of optional content groups that are off by default.
fn hidden_groups(doc: &Document) -> BTreeSet<ObjectId> {
    let mut off = BTreeSet::new();
    let Ok(cat) = doc.catalog() else { return off };
    let Some(Object::Dictionary(props)) = dict_get(doc, cat, b"OCProperties") else { return off };
    let Some(Object::Dictionary(d)) = dict_get(doc, props, b"D") else { return off };
    if let Some(Object::Array(a)) = dict_get(doc, d, b"OFF") {
        off.extend(a.iter().filter_map(|o| o.as_reference().ok()));
    }
    off
}

impl Ctx<'_> {
    /// Run a content stream with `resources` and the starting graphics state.
    fn run(&mut self, content: &[u8], resources: Option<&Dictionary>, gs0: GState) -> Result<()> {
        self.content_bytes = self.content_bytes.saturating_add(content.len());
        if content.len() > MAX_CONTENT || self.content_bytes > MAX_TOTAL_CONTENT {
            return Err(FileError::TooLarge("the PDF page content is too large".into()));
        }
        // Each call costs an operation, so forms that draw each other many times end.
        self.ops += 1;
        let ops = lopdf::content::Content::decode(content)
            .map_err(|e| FileError::Invalid(format!("the page content cannot be read ({e})")))?
            .operations;
        let mut gs = gs0;
        let mut stack: Vec<GState> = Vec::new();
        let mut path = BezPath::new();
        let mut cur = Point::ZERO;
        let mut start = Point::ZERO;
        // Marked content: whether each open section hides its content.
        let mut marked: Vec<bool> = Vec::new();
        let mut in_text = false;
        for op in &ops {
            self.ops += 1;
            if self.ops > MAX_OPS {
                return Err(FileError::TooLarge("the PDF has too many drawing operations".into()));
            }
            let hidden = marked.iter().any(|h| *h);
            let a = &op.operands;
            let n = nums(a);
            let p = |i: usize, gs: &GState| gs.ctm * Point::new(n[i], n[i + 1]);
            match op.operator.as_str() {
                "q" => stack.push(gs.clone()),
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" if n.len() == 6 => gs.ctm *= Affine::new([n[0], n[1], n[2], n[3], n[4], n[5]]),
                "m" if n.len() == 2 => {
                    cur = p(0, &gs);
                    start = cur;
                    path.move_to(cur);
                }
                "l" if n.len() == 2 => {
                    cur = p(0, &gs);
                    path.line_to(cur);
                }
                "c" if n.len() == 6 => {
                    let (a1, b1, c1) = (p(0, &gs), p(2, &gs), p(4, &gs));
                    path.curve_to(a1, b1, c1);
                    cur = c1;
                }
                "v" if n.len() == 4 => {
                    let (b1, c1) = (p(0, &gs), p(2, &gs));
                    path.curve_to(cur, b1, c1);
                    cur = c1;
                }
                "y" if n.len() == 4 => {
                    let (a1, c1) = (p(0, &gs), p(2, &gs));
                    path.curve_to(a1, c1, c1);
                    cur = c1;
                }
                "h" => {
                    path.close_path();
                    cur = start;
                }
                "re" if n.len() == 4 => {
                    let (x, y, w, h) = (n[0], n[1], n[2], n[3]);
                    let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)].map(|(u, v)| gs.ctm * Point::new(u, v));
                    path.move_to(pts[0]);
                    for q in &pts[1..] {
                        path.line_to(*q);
                    }
                    path.close_path();
                    cur = pts[0];
                    start = cur;
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    if matches!(op.operator.as_str(), "s" | "b" | "b*") {
                        path.close_path();
                    }
                    let taken = std::mem::take(&mut path);
                    if !hidden {
                        let color = if op.operator.starts_with(['S', 's']) { gs.stroke } else { gs.fill.or(gs.stroke) };
                        self.art.push(Affine::scale(MM_PER_PT) * taken, "path", color);
                    }
                }
                "n" => path = BezPath::new(),
                "RG" | "G" | "K" => gs.stroke = rgb(&n).or(gs.stroke),
                "rg" | "g" | "k" => gs.fill = rgb(&n).or(gs.fill),
                "SC" | "SCN" => gs.stroke = rgb(&n).or(gs.stroke),
                "sc" | "scn" => gs.fill = rgb(&n).or(gs.fill),
                "BT" => in_text = true,
                "ET" => in_text = false,
                "Tj" | "TJ" | "'" | "\"" => {
                    if in_text && !hidden {
                        self.texts += 1;
                    }
                }
                "BI" if !hidden => {
                    self.images += 1;
                    self.image_problems.insert("inline images".into());
                }
                "sh" => self.art.warn("shadings (gradients) are left out"),
                "BMC" => marked.push(false),
                "BDC" => {
                    let off = !self.hidden_layers
                        && a.first().and_then(|o| o.as_name().ok()) == Some(b"OC".as_slice())
                        && a.get(1)
                            .and_then(|o| match o {
                                Object::Name(name) => resources
                                    .and_then(|r| dict_get(self.doc, r, b"Properties"))
                                    .and_then(|p| p.as_dict().ok())
                                    .and_then(|p| p.get(name).ok())
                                    .and_then(|o| o.as_reference().ok()),
                                Object::Reference(id) => Some(*id),
                                _ => None,
                            })
                            .is_some_and(|id| self.off.contains(&id));
                    marked.push(off);
                }
                "EMC" => {
                    marked.pop();
                }
                "Do" if !hidden => {
                    let Some(name) = a.first().and_then(|o| o.as_name().ok()) else { continue };
                    let xobj = resources
                        .and_then(|r| dict_get(self.doc, r, b"XObject"))
                        .and_then(|x| x.as_dict().ok())
                        .and_then(|x| x.get(name).ok())
                        .and_then(|o| deref(self.doc, o))
                        .and_then(|o| o.as_stream().ok());
                    let Some(stream) = xobj else { continue };
                    match stream.dict.get(b"Subtype").and_then(Object::as_name).ok() {
                        Some(b"Form") => {
                            if self.forms_depth > 32 {
                                continue;
                            }
                            let m = dict_get(self.doc, &stream.dict, b"Matrix")
                                .and_then(|o| o.as_array().ok())
                                .map(|v| nums(v))
                                .filter(|v| v.len() == 6)
                                .map_or(Affine::IDENTITY, |v| Affine::new([v[0], v[1], v[2], v[3], v[4], v[5]]));
                            let res = dict_get(self.doc, &stream.dict, b"Resources").and_then(|o| o.as_dict().ok()).or(resources);
                            let content = stream
                                .decompressed_content_with_limit(MAX_CONTENT)
                                .unwrap_or_else(|_| stream.content.clone());
                            let inner = GState { ctm: gs.ctm * m, ..gs.clone() };
                            self.forms_depth += 1;
                            let r = self.run(&content, res, inner);
                            self.forms_depth -= 1;
                            r?;
                        }
                        Some(b"Image") => {
                            if self.art.images.len() >= MAX_IMAGES {
                                self.images += 1;
                                self.image_problems.insert(format!("more than {MAX_IMAGES} images"));
                                continue;
                            }
                            match decode_image(self.doc, stream) {
                                Ok((data, mime)) => self.art.images.push(crate::ArtImage {
                                    data,
                                    mime,
                                    placement: Affine::scale(MM_PER_PT) * gs.ctm,
                                    label: "image".into(),
                                }),
                                Err(why) => {
                                    self.images += 1;
                                    self.image_problems.insert(why);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// A name or the first name of an array (a `Filter` or `ColorSpace` entry).
fn first_name<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a [u8]> {
    match deref(doc, o)? {
        Object::Name(n) => Some(n),
        Object::Array(a) => a.first().and_then(|o| deref(doc, o)).and_then(|o| o.as_name().ok()),
        _ => None,
    }
}

/// An integer entry of `d`.
fn int(doc: &Document, d: &Dictionary, key: &[u8]) -> Option<i64> {
    dict_get(doc, d, key).and_then(|o| o.as_i64().ok())
}

/// How samples of a colour space become RGB.
enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// Base space and lookup table (base components per entry).
    Indexed(Box<Space>, Vec<u8>),
}

impl Space {
    fn components(&self) -> usize {
        match self {
            Space::Gray | Space::Indexed(..) => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
        }
    }

    fn rgb(&self, s: &[u8]) -> [u8; 3] {
        match self {
            Space::Gray => [s[0]; 3],
            Space::Rgb => [s[0], s[1], s[2]],
            Space::Cmyk => {
                // Naive CMYK → RGB (no ICC profile): good enough to engrave from.
                let k = 255 - u16::from(s[3]);
                let c = |v: u8| ((255 - u16::from(v)) * k / 255) as u8;
                [c(s[0]), c(s[1]), c(s[2])]
            }
            Space::Indexed(base, table) => {
                let n = base.components();
                let i = usize::from(s[0]) * n;
                match table.get(i..i + n) {
                    Some(e) => base.rgb(e),
                    None => [0; 3],
                }
            }
        }
    }
}

fn color_space(doc: &Document, o: &Object, depth: usize) -> std::result::Result<Space, String> {
    let o = deref(doc, o).ok_or("an unreadable colour space")?;
    let unsupported = |n: &[u8]| format!("images in the {} colour space", String::from_utf8_lossy(n));
    match o {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Ok(Space::Gray),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Ok(Space::Rgb),
            b"DeviceCMYK" | b"CMYK" => Ok(Space::Cmyk),
            other => Err(unsupported(other)),
        },
        Object::Array(a) if depth < 4 => {
            let kind = a.first().and_then(|o| deref(doc, o)).and_then(|o| o.as_name().ok()).ok_or("an unreadable colour space")?;
            match kind {
                b"ICCBased" => {
                    let n = a.get(1).and_then(|o| deref(doc, o)).and_then(|o| o.as_stream().ok()).and_then(|s| int(doc, &s.dict, b"N"));
                    match n {
                        Some(1) => Ok(Space::Gray),
                        Some(3) => Ok(Space::Rgb),
                        Some(4) => Ok(Space::Cmyk),
                        _ => Err("ICC-based images that are not gray, RGB or CMYK".into()),
                    }
                }
                b"Indexed" | b"I" => {
                    let base = color_space(doc, a.get(1).ok_or("an indexed colour space without a base")?, depth + 1)?;
                    if matches!(base, Space::Indexed(..)) {
                        return Err("nested indexed colour spaces".into());
                    }
                    let table = match a.get(3).and_then(|o| deref(doc, o)) {
                        Some(Object::String(s, _)) => s.clone(),
                        Some(Object::Stream(s)) => s.decompressed_content_with_limit(1 << 20).unwrap_or_else(|_| s.content.clone()),
                        _ => return Err("an indexed colour space without a table".into()),
                    };
                    Ok(Space::Indexed(Box::new(base), table))
                }
                b"CalGray" => Ok(Space::Gray),
                b"CalRGB" => Ok(Space::Rgb),
                other => Err(unsupported(other)),
            }
        }
        _ => Err("an unreadable colour space".into()),
    }
}

/// An image XObject as an encoded file (`image/jpeg` as stored, else decoded to `image/png`), or why it is left out.
fn decode_image(doc: &Document, s: &lopdf::Stream) -> std::result::Result<(Vec<u8>, &'static str), String> {
    let d = &s.dict;
    if dict_get(doc, d, b"ImageMask").and_then(|o| o.as_bool().ok()) == Some(true) {
        return Err("stencil masks".into());
    }
    let (w, h) = (int(doc, d, b"Width").unwrap_or(0), int(doc, d, b"Height").unwrap_or(0));
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_IMAGE_PIXELS {
        return Err(format!("images with no or more than {} megapixels", MAX_IMAGE_PIXELS / (1024 * 1024)));
    }
    let (w, h) = (w as u32, h as u32);
    let filter = dict_get(doc, d, b"Filter").and_then(|f| first_name(doc, f));
    let filters = match dict_get(doc, d, b"Filter").and_then(|o| deref(doc, o)) {
        Some(Object::Array(a)) => a.len(),
        Some(_) => 1,
        None => 0,
    };
    let has_smask = dict_get(doc, d, b"SMask").is_some();
    if filter == Some(b"DCTDecode".as_slice()) && filters == 1 && !has_smask {
        if s.content.starts_with(&[0xff, 0xd8]) {
            return Ok((s.content.clone(), "image/jpeg"));
        }
        return Err("JPEG images that do not read as JPEG".into());
    }
    if let Some(f) = filter.filter(|f| matches!(*f, b"JPXDecode" | b"CCITTFaxDecode" | b"JBIG2Decode" | b"DCTDecode")) {
        return Err(format!("{} images", String::from_utf8_lossy(f)));
    }
    let bits = int(doc, d, b"BitsPerComponent").unwrap_or(8);
    let space = match dict_get(doc, d, b"ColorSpace") {
        Some(cs) => color_space(doc, cs, 0)?,
        None => return Err("images without a colour space".into()),
    };
    let n = space.components();
    let samples = s
        .decompressed_content_with_limit((w as usize) * (h as usize) * n + 1024)
        .map_err(|e| format!("images whose data cannot be decoded ({e})"))?;
    let row_bytes = match bits {
        8 => w as usize * n,
        1 if n == 1 => (w as usize).div_ceil(8),
        _ => return Err(format!("{bits}-bit images")),
    };
    if samples.len() < row_bytes * h as usize {
        return Err("images with missing data".into());
    }
    let alpha = if has_smask {
        dict_get(doc, d, b"SMask").and_then(|o| deref(doc, o)).and_then(|o| o.as_stream().ok()).and_then(|m| {
            let (mw, mh) = (int(doc, &m.dict, b"Width")?, int(doc, &m.dict, b"Height")?);
            let data = m.decompressed_content_with_limit((w as usize) * (h as usize) + 1024).ok()?;
            (mw == i64::from(w) && mh == i64::from(h) && int(doc, &m.dict, b"BitsPerComponent").unwrap_or(8) == 8 && data.len() >= (w * h) as usize).then_some(data)
        })
    } else {
        None
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = &samples[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..w as usize {
            let c = if bits == 1 {
                let on = (row[x / 8] >> (7 - (x % 8))) & 1 == 1;
                let v = if matches!(space, Space::Indexed(..)) { u8::from(on) } else if on { 255 } else { 0 };
                space.rgb(&[v])
            } else {
                space.rgb(&row[x * n..x * n + n])
            };
            let a = alpha.as_ref().map_or(255, |m| m[y * w as usize + x]);
            rgba.extend_from_slice(&[c[0], c[1], c[2], a]);
        }
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or("images of an impossible size")?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("images that cannot be written as PNG ({e})"))?;
    Ok((png, "image/png"))
}

/// Import page `o.page` (default 1) of a PDF (or PDF-based AI) file.
pub fn parse(bytes: &[u8], o: &PdfImportOptions) -> Result<Art> {
    if bytes.len() > MAX_BYTES {
        return Err(FileError::TooLarge("the file is larger than 100 MB".into()));
    }
    if !bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        return invalid("this is not a PDF file (an Illustrator file must be saved with \"Create PDF Compatible File\")");
    }
    // Object and cross-reference streams are decoded while loading: bounded against decompression bombs.
    let doc = Document::load_mem_with_options(bytes, lopdf::LoadOptions::with_max_decompressed_size(MAX_TOTAL_CONTENT)).map_err(|e| FileError::Invalid(format!("the PDF cannot be read ({e})")))?;
    let pages = doc.get_pages();
    let n = o.page.unwrap_or(1);
    let &page = pages.get(&n).ok_or_else(|| FileError::Invalid(format!("the PDF has no page {n} (it has {})", pages.len())))?;
    let content = doc
        .get_page_content_with_limit(page, MAX_CONTENT)
        .map_err(|e| FileError::Invalid(format!("the page content cannot be read ({e})")))?;
    let (res, inherited) = doc.get_page_resources(page).map_err(|e| FileError::Invalid(format!("bad page resources ({e})")))?;
    // Resources given by reference (or inherited from the page tree) come as object ids.
    let res = res.or_else(|| inherited.iter().find_map(|id| doc.get_dictionary(*id).ok()));
    let mut ctx = Ctx {
        doc: &doc,
        off: hidden_groups(&doc),
        hidden_layers: o.hidden_layers,
        art: Art::default(),
        ops: 0,
        content_bytes: 0,
        texts: 0,
        images: 0,
        image_problems: BTreeSet::new(),
        forms_depth: 0,
    };
    ctx.run(&content, res, GState { ctm: Affine::IDENTITY, stroke: None, fill: None })?;
    let mut art = ctx.art;
    if pages.len() > 1 {
        art.warn(format!("the PDF has {} pages; page {n} was imported", pages.len()));
    }
    if ctx.texts > 0 {
        art.warn(format!("{} text run(s) left out (convert text to outlines before saving)", ctx.texts));
    }
    if ctx.images > 0 {
        let why: Vec<String> = ctx.image_problems.into_iter().collect();
        let why = if why.is_empty() { "inline images".to_string() } else { why.join(", ") };
        art.warn(format!("{} embedded image(s) left out ({why} are not read)", ctx.images));
    }
    if art.paths.is_empty() && art.images.is_empty() {
        let ai_without_pdf = bytes.windows(13).any(|w| w == b"AIPrivateData");
        return invalid(if ai_without_pdf && ctx.texts > 0 {
            "this Illustrator file has no PDF content; save it again with \"Create PDF Compatible File\""
        } else {
            "no vector paths or images found on the page"
        });
    }
    Ok(art)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_out::{PdfOptions, export_pdf};
    use crate::testutil::sample;
    use kurbo::Shape;

    #[test]
    fn our_own_pdf_reads_back_in_page_millimetres() {
        let d = sample();
        let pdf = export_pdf(&d, &PdfOptions::default()).unwrap().content;
        let art = parse(&pdf, &PdfImportOptions::default()).unwrap();
        assert_eq!(art.paths.len(), 3);
        let bounds = art.paths.iter().map(|p| p.path.bounding_box()).reduce(|a, b| a.union(b)).unwrap();
        // The page is the art's bounds: 45 x 40 mm from the origin.
        assert!((bounds.width() - 45.0).abs() < 0.01 && (bounds.height() - 40.0).abs() < 0.01, "{bounds:?}");
        assert_eq!(art.paths[0].color, Some([255, 0, 0]));
    }

    #[test]
    fn refuses_non_pdf_and_missing_pages() {
        assert!(parse(b"%!PS-Adobe", &PdfImportOptions::default()).is_err());
        let pdf = export_pdf(&sample(), &PdfOptions::default()).unwrap().content;
        assert!(parse(&pdf, &PdfImportOptions { page: Some(2), ..Default::default() }).is_err());
    }

    /// A one-page PDF whose page draws form `F`, which draws itself twice after a megabyte of padding: 2^32 form
    /// runs, each decoding the padding again, before the total content budget.
    fn self_drawing_form_pdf() -> Vec<u8> {
        use lopdf::{Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let form_id = doc.new_object_id();
        let mut content = b"0 0 m 10 10 l S /F Do /F Do\n".to_vec();
        content.extend(std::iter::repeat_n(b' ', 1 << 20));
        let bbox = vec![0.into(), 0.into(), 10.into(), 10.into()];
        let form = Stream::new(
            dictionary! {"Type" => "XObject", "Subtype" => "Form", "BBox" => bbox.clone(), "Resources" => dictionary! {"XObject" => dictionary! {"F" => form_id}}},
            content,
        );
        doc.objects.insert(form_id, Object::Stream(form));
        let page_content = doc.add_object(Stream::new(dictionary! {}, b"/F Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => bbox, "Contents" => page_content,
            "Resources" => dictionary! {"XObject" => dictionary! {"F" => form_id}},
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1}));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    /// A page drawing a 2 x 2 RGB image (uncompressed) scaled to 72 x 36 pt at (72, 0), an indexed 1 x 1 image, a
    /// stencil mask and an inline image.
    fn image_pdf() -> Vec<u8> {
        use lopdf::{Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let rgb = doc.add_object(Stream::new(
            dictionary! {"Type" => "XObject", "Subtype" => "Image", "Width" => 2, "Height" => 2, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8},
            vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
        ));
        let table = Object::String(vec![10, 20, 30, 40, 50, 60], lopdf::StringFormat::Hexadecimal);
        let cs = vec![Object::Name(b"Indexed".to_vec()), Object::Name(b"DeviceRGB".to_vec()), 1.into(), table];
        let indexed = doc.add_object(Stream::new(
            dictionary! {"Type" => "XObject", "Subtype" => "Image", "Width" => 1, "Height" => 1, "ColorSpace" => cs, "BitsPerComponent" => 8},
            vec![1],
        ));
        let mask = doc.add_object(Stream::new(
            dictionary! {"Type" => "XObject", "Subtype" => "Image", "Width" => 8, "Height" => 1, "ImageMask" => true, "BitsPerComponent" => 1},
            vec![0xff],
        ));
        let content = b"q 72 0 0 36 72 0 cm /A Do Q q 10 0 0 10 0 0 cm /B Do Q q 5 0 0 5 0 0 cm /M Do Q BI /W 1 /H 1 /CS /G /BPC 8 ID A EI".to_vec();
        let page_content = doc.add_object(Stream::new(dictionary! {}, content));
        let bbox = vec![0.into(), 0.into(), 200.into(), 100.into()];
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "MediaBox" => bbox, "Contents" => page_content,
            "Resources" => dictionary! {"XObject" => dictionary! {"A" => rgb, "B" => indexed, "M" => mask}},
        });
        doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1}));
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn embedded_images_are_imported_where_the_page_draws_them() {
        let art = parse(&image_pdf(), &PdfImportOptions::default()).unwrap();
        assert_eq!(art.images.len(), 2, "{:?}", art.warnings);
        let a = &art.images[0];
        assert_eq!(a.mime, "image/png");
        let b = a.bounds();
        assert!((b.x0 - 25.4).abs() < 1e-9 && (b.width() - 25.4).abs() < 1e-9 && (b.height() - 12.7).abs() < 1e-9, "{b:?}");
        let img = image::load_from_memory(&a.data).unwrap().to_rgba8();
        // Row 0 is the image's top: red, green.
        assert_eq!((img.get_pixel(0, 0).0, img.get_pixel(1, 0).0, img.get_pixel(1, 1).0), ([255, 0, 0, 255], [0, 255, 0, 255], [255, 255, 255, 255]));
        let idx = image::load_from_memory(&art.images[1].data).unwrap().to_rgba8();
        assert_eq!(idx.get_pixel(0, 0).0, [40, 50, 60, 255]);
        assert!(art.warnings.iter().any(|w| w.contains("2 embedded image(s) left out") && w.contains("stencil") && w.contains("inline")), "{:?}", art.warnings);
    }

    /// The owner's B6 manual (in OPENETCH_LBRN_SAMPLES, read in place): its images come in (117 on its 18 pages; its
    /// page resources are given by reference, which the importer once missed).
    #[test]
    fn real_pdf_images_import() {
        let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else { return };
        let mut n = 0;
        for d in std::env::split_paths(&dirs) {
            for name in ["Users' manual ComMarker B6.pdf"] {
                let p = d.join(name);
                let Ok(bytes) = std::fs::read(&p) else { continue };
                let mut images = 0;
                for page in 1..=6 {
                    match parse(&bytes, &PdfImportOptions { page: Some(page), ..Default::default() }) {
                        Ok(art) => {
                            for im in &art.images {
                                assert!(image::load_from_memory(&im.data).is_ok(), "{}: page {page}", p.display());
                            }
                            images += art.images.len();
                        }
                        Err(e) => eprintln!("{} page {page}: {e}", p.display()),

                    }
                }
                eprintln!("{}: {images} image(s) on the first pages", p.display());
                assert!(images > 0, "{}", p.display());
                n += 1;
            }
        }
        eprintln!("{n} PDF sample(s) with images");
    }

    #[test]
    fn forms_drawing_themselves_end_quickly() {
        let pdf = self_drawing_form_pdf();
        let t = std::time::Instant::now();
        let r = parse(&pdf, &PdfImportOptions::default());
        assert!(t.elapsed().as_secs() < 20, "took {:?}", t.elapsed());
        assert!(matches!(r, Err(FileError::TooLarge(_))), "{r:?}");
    }

    /// The owner's sample `.ai` / `.pdf` files (OPENETCH_LBRN_SAMPLES), read in place.
    #[test]
    fn real_ai_and_pdf_samples_import() {
        let Some(dirs) = std::env::var_os("OPENETCH_LBRN_SAMPLES") else { return };
        let mut n = 0;
        for d in std::env::split_paths(&dirs) {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                        check(&e.path(), &mut n);
                    }
                } else {
                    check(&p, &mut n);
                }
            }
        }
        fn check(p: &std::path::Path, n: &mut usize) {
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ai")) {
                return;
            }
            let bytes = std::fs::read(p).unwrap();
            let art = parse(&bytes, &PdfImportOptions::default()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(!art.paths.is_empty(), "{}", p.display());
            *n += 1;
        }
        eprintln!("{n} AI sample(s) imported");
    }
}
