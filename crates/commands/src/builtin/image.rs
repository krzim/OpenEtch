//! `art.import_image`: a PNG, JPEG, GIF, BMP or WebP file → an image object (`Geometry::Image`). The file is kept as
//! it is (base64); only its header is read, for the pixel size and resolution. Images are shown and exported to
//! LightBurn; planning skips them (image engraving is not supported yet).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use doc::{Document, Geometry};
use kurbo::Affine;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shape::add_object;
use super::util::{self, invalid, transact};
use crate::{Command, Result, Risk};

/// Largest accepted image file (decoded).
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Largest accepted width or height in pixels.
const MAX_PIXELS: u32 = 65_535;
/// Resolution assumed when the file records none: 0.1 mm per pixel, a usual galvo engraving pitch.
const DEFAULT_DPI: f64 = 254.0;
/// Field size assumed for the fit-to-field check when the document has none.
const DEFAULT_FIELD_MM: f64 = 110.0;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportImageInput {
    /// The image file in standard base64 (line breaks are ignored; a `data:image/...;base64,` prefix is accepted).
    pub data_base64: String,
    /// The file's type: image/png, image/jpeg, image/gif, image/bmp or image/webp. The file's own header decides when
    /// they disagree (with a warning).
    pub mime: String,
    /// Width in mm (the height follows the aspect ratio). Default: from the file's resolution (else 254 dpi, 0.1 mm
    /// per pixel), shrunk to fit 80 % of the field.
    pub width_mm: Option<f64>,
    /// Exact layer id. Default: the first layer.
    pub layer: Option<String>,
    /// Display name. Default "Image".
    pub name: Option<String>,
}

/// `art.import_image`.
pub struct ImportImage;

/// What the header of an image file says.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageInfo {
    pub mime: &'static str,
    pub width_px: u32,
    pub height_px: u32,
    /// Horizontal and vertical resolution, when the file records one.
    pub dpi: Option<(f64, f64)>,
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?)))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 3)?;
    Some(u32::from(s[0]) | u32::from(s[1]) << 8 | u32::from(s[2]) << 16)
}

/// Pixels per metre → dots per inch, to 0.01 (files store whole pixels per metre: 300 dpi is 11811).
fn ppm_to_dpi(ppm: u32) -> f64 {
    (f64::from(ppm) * 2.54).round() / 100.0
}

/// Read the type, pixel size and resolution from an image file's header. `None` when it is not a PNG, JPEG, GIF,
/// BMP or WebP file, or its header is cut short.
pub(crate) fn image_info(b: &[u8]) -> Option<ImageInfo> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        if b.get(12..16)? != b"IHDR" {
            return None;
        }
        let (w, h) = (be32(b, 16)?, be32(b, 20)?);
        // pHYs (pixels per unit; unit 1 = metre) comes before the image data.
        let mut at = 8;
        let mut dpi = None;
        while let (Some(len), Some(kind)) = (be32(b, at), b.get(at + 4..at + 8)) {
            if kind == b"IDAT" || kind == b"IEND" {
                break;
            }
            if kind == b"pHYs" && len >= 9 && b.get(at + 16) == Some(&1) {
                dpi = Some((ppm_to_dpi(be32(b, at + 8)?), ppm_to_dpi(be32(b, at + 12)?)));
            }
            at = at.checked_add(12)?.checked_add(usize::try_from(len).ok()?)?;
        }
        return Some(ImageInfo { mime: "image/png", width_px: w, height_px: h, dpi });
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        let mut dpi = None;
        loop {
            // Skip fill bytes to the next marker.
            while b.get(at)? == &0xFF && b.get(at + 1)? == &0xFF {
                at += 1;
            }
            if b.get(at)? != &0xFF {
                return None;
            }
            let marker = *b.get(at + 1)?;
            if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
                at += 2;
                continue;
            }
            let len = usize::try_from(be16(b, at + 2)?).ok()?;
            if marker == 0xE0 && b.get(at + 4..at + 9) == Some(b"JFIF\0") {
                let (unit, x, y) = (*b.get(at + 11)?, be16(b, at + 12)?, be16(b, at + 14)?);
                dpi = match unit {
                    1 => Some((f64::from(x), f64::from(y))),
                    2 => Some((f64::from(x) * 2.54, f64::from(y) * 2.54)),
                    _ => None,
                };
            }
            // Start of frame (baseline, progressive, ...): height, then width.
            if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                let (h, w) = (be16(b, at + 5)?, be16(b, at + 7)?);
                return Some(ImageInfo { mime: "image/jpeg", width_px: w, height_px: h, dpi });
            }
            at = at.checked_add(2 + len)?;
        }
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(ImageInfo { mime: "image/gif", width_px: le16(b, 6)?, height_px: le16(b, 8)?, dpi: None });
    }
    if b.starts_with(b"BM") {
        let w = le32(b, 18)? as i32;
        let h = le32(b, 22)? as i32;
        let (xp, yp) = (le32(b, 38).unwrap_or(0), le32(b, 42).unwrap_or(0));
        let dpi = (xp > 0 && yp > 0).then(|| (ppm_to_dpi(xp), ppm_to_dpi(yp)));
        return Some(ImageInfo { mime: "image/bmp", width_px: w.unsigned_abs(), height_px: h.unsigned_abs(), dpi });
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        let (w, h) = match b.get(12..16)? {
            b"VP8X" => (le24(b, 24)? + 1, le24(b, 27)? + 1),
            b"VP8L" => {
                let bits = le32(b, 21)?;
                ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
            }
            b"VP8 " => (le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF),
            _ => return None,
        };
        return Some(ImageInfo { mime: "image/webp", width_px: w, height_px: h, dpi: None });
    }
    None
}

/// The base64 text with an optional `data:` prefix and whitespace removed, and the decoded bytes.
fn decode(cmd: &str, text: &str) -> Result<Vec<u8>> {
    let body = match text.trim_start().strip_prefix("data:") {
        Some(rest) => rest.split_once(',').map(|(_, b)| b).ok_or_else(|| invalid(cmd, "the data: URL has no ','"))?,
        None => text,
    };
    if body.len() > MAX_IMAGE_BYTES / 3 * 4 + 1024 * 1024 {
        return Err(invalid(cmd, format!("the image is larger than {} MB", MAX_IMAGE_BYTES / (1024 * 1024))));
    }
    let compact: String = body.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = STANDARD
        .decode(compact.as_bytes())
        .map_err(|e| invalid(cmd, format!("data_base64 is not valid base64: {e}")))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(invalid(cmd, format!("the image is larger than {} MB", MAX_IMAGE_BYTES / (1024 * 1024))));
    }
    Ok(bytes)
}

impl Command for ImportImage {
    type Input = ImportImageInput;
    const ID: &'static str = "art.import_image";
    const TITLE: &'static str = "Import image";
    const DESCRIPTION: &'static str = "Import a bitmap (PNG, JPEG, GIF, BMP or WebP, as base64) as an image object \
        centred on the field origin. Size: width_mm (height keeps the aspect ratio), else the file's resolution \
        (else 254 dpi), shrunk to fit 80 % of the field. Images are shown and exported to LightBurn; jobs skip them \
        (image engraving is not supported yet). Returns {created, bounds, width_px, height_px, dpi, warnings}. \
        Undoable.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportImageInput) -> Result<(Value, String)> {
        let id = Self::ID;
        let bytes = decode(id, &i.data_base64)?;
        let info = image_info(&bytes)
            .ok_or_else(|| invalid(id, "the data is not a PNG, JPEG, GIF, BMP or WebP image (or its header is damaged)"))?;
        if info.width_px == 0 || info.height_px == 0 || info.width_px > MAX_PIXELS || info.height_px > MAX_PIXELS {
            return Err(invalid(id, format!("the image is {} x {} pixels; 1..={MAX_PIXELS} are supported", info.width_px, info.height_px)));
        }
        let mut warnings = Vec::new();
        let given = i.mime.trim().to_ascii_lowercase();
        let given = if given == "image/jpg" { "image/jpeg".to_string() } else { given };
        if given != info.mime {
            warnings.push(format!("the file is {}, not {}; imported as {}", info.mime, i.mime.trim(), info.mime));
        }
        let name = i.name.as_deref().map(|n| util::name(id, "name", n)).transpose()?.unwrap_or_else(|| "Image".into());
        let (wpx, hpx) = (f64::from(info.width_px), f64::from(info.height_px));
        let (width_mm, height_mm) = match i.width_mm {
            Some(w) => {
                let w = util::positive(id, "width_mm", w)?;
                (w, w * hpx / wpx)
            }
            None => {
                let (dx, dy) = info.dpi.filter(|(x, y)| *x >= 1.0 && *y >= 1.0).unwrap_or((DEFAULT_DPI, DEFAULT_DPI));
                let (w, h) = (wpx / dx * 25.4, hpx / dy * 25.4);
                let fit = doc.field_mm.unwrap_or(DEFAULT_FIELD_MM) * 0.8;
                let k = (fit / w).min(fit / h).min(1.0);
                if k < 1.0 {
                    warnings.push(format!(
                        "the image is {w:.1} x {h:.1} mm at its resolution; scaled to {:.1} x {:.1} mm to fit the field",
                        w * k,
                        h * k
                    ));
                }
                (w * k, h * k)
            }
        };
        let data = STANDARD.encode(&bytes);
        transact(doc, |doc| {
            let layer = util::layer_or_default(doc, i.layer.as_deref())?;
            let geometry = Geometry::Image { width_mm, height_mm, mime: info.mime.into(), data_base64: data.into() };
            let obj = add_object(doc, name.clone(), layer, geometry, Affine::IDENTITY)?;
            let mut r = util::edit_result(doc, std::slice::from_ref(&obj), &[]);
            r["width_px"] = json!(info.width_px);
            r["height_px"] = json!(info.height_px);
            r["dpi"] = json!(info.dpi.map(|(x, y)| [util::r4(x), util::r4(y)]));
            r["warnings"] = json!(warnings);
            Ok((r, format!("imported image \"{name}\" ({} x {} px)", info.width_px, info.height_px)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 x 2 PNG with a pHYs chunk of 300 dpi (11811 px/m) and no real image data (the header is all that is read).
    pub(crate) fn png(w: u32, h: u32, ppm: Option<u32>) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend(13u32.to_be_bytes());
        b.extend(b"IHDR");
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
        if let Some(p) = ppm {
            b.extend(9u32.to_be_bytes());
            b.extend(b"pHYs");
            b.extend(p.to_be_bytes());
            b.extend(p.to_be_bytes());
            b.extend([1, 0, 0, 0, 0]);
        }
        b.extend(0u32.to_be_bytes());
        b.extend(b"IEND");
        b.extend([0xAE, 0x42, 0x60, 0x82]);
        b
    }

    fn import(d: &mut Document, v: Value) -> Result<(Value, String)> {
        ImportImage::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    #[test]
    fn reads_headers_of_every_supported_type() {
        let info = image_info(&png(3, 2, Some(11811))).unwrap();
        assert_eq!((info.mime, info.width_px, info.height_px), ("image/png", 3, 2));
        let (dx, dy) = info.dpi.unwrap();
        assert!((dx - 300.0).abs() < 0.01 && (dy - 300.0).abs() < 0.01);
        assert_eq!(image_info(&png(3, 2, None)).unwrap().dpi, None);
        // JPEG: SOI, JFIF APP0 at 72 dpi, SOF0 40 x 30.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        jpeg.extend(b"JFIF\0");
        jpeg.extend([1, 1, 1, 0, 72, 0, 72, 0, 0]);
        jpeg.extend([0xFF, 0xC0, 0, 17, 8, 0, 30, 0, 40, 3]);
        let info = image_info(&jpeg).unwrap();
        assert_eq!((info.mime, info.width_px, info.height_px, info.dpi), ("image/jpeg", 40, 30, Some((72.0, 72.0))));
        let gif = [b"GIF89a".as_slice(), &[10, 0, 5, 0]].concat();
        assert_eq!(image_info(&gif).map(|i| (i.width_px, i.height_px)), Some((10, 5)));
        let mut bmp = vec![0u8; 54];
        bmp[..2].copy_from_slice(b"BM");
        bmp[18..22].copy_from_slice(&7i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&(-4i32).to_le_bytes());
        assert_eq!(image_info(&bmp).map(|i| (i.width_px, i.height_px, i.dpi)), Some((7, 4, None)));
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.extend([10, 0, 0, 0, 0, 0, 0, 0, 99, 0, 0, 49, 0, 0]);
        assert_eq!(image_info(&webp).map(|i| (i.mime, i.width_px, i.height_px)), Some(("image/webp", 100, 50)));
        for bad in [b"".as_slice(), b"<svg/>", b"\x89PNG\r\n\x1a\n", &[0xFF, 0xD8, 0xFF]] {
            assert_eq!(image_info(bad), None);
        }
    }

    #[test]
    fn imports_at_the_file_resolution_or_a_given_width() {
        let mut d = Document { field_mm: Some(150.0), ..Document::default() };
        let data = STANDARD.encode(png(300, 150, Some(11811)));
        let (r, _) = import(&mut d, json!({"data_base64": data, "mime": "image/png", "name": "logo"})).unwrap();
        assert_eq!(r["created"], json!(["obj-2"]));
        assert_eq!((r["width_px"].clone(), r["height_px"].clone()), (json!(300), json!(150)));
        // 300 px at 300 dpi = 1 in.
        assert_eq!(r["bounds"]["size"], json!([25.4, 12.7]));
        assert_eq!(r["bounds"]["center"], json!([0.0, 0.0]));
        assert_eq!(r["warnings"], json!([]));
        let o = &d.objects[0];
        assert_eq!(o.name, "logo");
        assert!(matches!(&o.geometry, Geometry::Image { mime, data_base64, .. } if mime == "image/png" && **data_base64 == *data));

        let (r, _) = import(&mut d, json!({"data_base64": data, "mime": "image/png", "width_mm": 40})).unwrap();
        assert_eq!(r["bounds"]["size"], json!([40.0, 20.0]));
        // A data URL with line breaks is fine; a wrong type is corrected with a warning.
        let url = format!("data:image/jpeg;base64,{}\n{}", &data[..20], &data[20..]);
        let (r, _) = import(&mut d, json!({"data_base64": url, "mime": "image/jpg"})).unwrap();
        assert!(r["warnings"][0].as_str().unwrap().contains("image/png"), "{}", r["warnings"]);
        // No resolution in the file: 254 dpi, then shrunk to 80 % of the field.
        let big = STANDARD.encode(png(4000, 1000, None));
        let (r, _) = import(&mut d, json!({"data_base64": big, "mime": "image/png"})).unwrap();
        assert_eq!(r["bounds"]["size"], json!([120.0, 30.0]));
        assert!(r["warnings"][0].as_str().unwrap().contains("400.0 x 100.0 mm"), "{}", r["warnings"]);
        assert_eq!(d.objects.len(), 4);
    }

    #[test]
    fn real_image_files_import() {
        let Some(files) = super::super::dxf::sample_files(&["png", "jpg", "jpeg", "gif", "bmp", "webp"]) else {
            println!("OPENETCH_LBRN_SAMPLES is not set; skipping the real-file test");
            return;
        };
        for f in files {
            let bytes = std::fs::read(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let mut d = Document { field_mm: Some(150.0), ..Document::default() };
            let (r, _) = import(&mut d, json!({"data_base64": STANDARD.encode(&bytes), "mime": "image/png"}))
                .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            println!(
                "== {}: {} x {} px, dpi {}, size {} mm, warnings {}",
                f.file_name().unwrap_or_default().to_string_lossy(),
                r["width_px"],
                r["height_px"],
                r["dpi"],
                r["bounds"]["size"],
                r["warnings"]
            );
        }
    }

    #[test]
    fn refuses_what_is_not_an_image() {
        let mut d = Document::default();
        for (data, mime) in [
            ("not base64!".to_string(), "image/png"),
            ("PHN2Zy8+".to_string(), "image/svg+xml"),
            (STANDARD.encode(png(0, 5, None)), "image/png"),
            (STANDARD.encode(png(70_000, 5, None)), "image/png"),
        ] {
            assert!(import(&mut d, json!({"data_base64": data, "mime": mime})).is_err(), "{data}");
        }
        let ok = STANDARD.encode(png(3, 2, None));
        assert!(import(&mut d, json!({"data_base64": ok, "mime": "image/png", "width_mm": -1})).is_err());
        assert!(import(&mut d, json!({"data_base64": ok, "mime": "image/png", "layer": "layer-9"})).is_err());
        assert!(d.objects.is_empty());
    }
}
