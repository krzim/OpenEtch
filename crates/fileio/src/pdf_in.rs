//! PDF and PDF-based Adobe Illustrator import (LightBurn's Import of `.pdf` and `.ai`, Illustrator 9 and later):
//! the vector paths of one page (default the first) in mm, +Y up, in page coordinates.
//!
//! The file is parsed with lopdf (MIT). The page's content streams run through a small interpreter of the path and
//! graphics-state operators (`q Q cm m l c v y h re S s f F f* B B* b b* n`, the colour operators for the
//! colour each path gets, and `Do` of form XObjects, recursively with their `Matrix`). Marked content of optional
//! content groups that are off by default (hidden Illustrator layers) is skipped unless `hidden_layers` is set
//! (LightBurn's "import hidden layers"). Text (`BT`...`ET`), embedded images and shadings are left out with a warning:
//! convert text to outlines before saving. An Illustrator file saved without "Create PDF Compatible File" holds no
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
    images: usize,
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
                "BI" => self.images += usize::from(!hidden),
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
                        Some(b"Image") => self.images += 1,
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
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
    let (res, _) = doc.get_page_resources(page).map_err(|e| FileError::Invalid(format!("bad page resources ({e})")))?;
    let mut ctx = Ctx {
        doc: &doc,
        off: hidden_groups(&doc),
        hidden_layers: o.hidden_layers,
        art: Art::default(),
        ops: 0,
        content_bytes: 0,
        texts: 0,
        images: 0,
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
        art.warn(format!("{} embedded image(s) left out (vectors only)", ctx.images));
    }
    if art.paths.is_empty() {
        let ai_without_pdf = bytes.windows(13).any(|w| w == b"AIPrivateData");
        return invalid(if ai_without_pdf && ctx.texts > 0 {
            "this Illustrator file has no PDF content; save it again with \"Create PDF Compatible File\""
        } else {
            "no vector paths found on the page"
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
