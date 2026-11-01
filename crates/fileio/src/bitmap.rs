//! Bitmap helpers: decoding any importable raster (PNG, JPEG, GIF, BMP, WebP, TIFF, TGA) and converting the ones
//! browsers cannot show (TIFF, TGA) to PNG, PNG writing with a resolution, grey levels for tracing, and baking an
//! image mask into the alpha channel (LightBurn's Flatten Image Mask / Crop Image).

use std::io::Cursor;

use image::{DynamicImage, ImageFormat};
use kurbo::{Affine, BezPath, Point};

use crate::{FileError, Result, crc32, invalid};

/// Most pixels a bitmap tool decodes (as the planner).
pub const MAX_PIXELS: u64 = 64_000_000;

/// The format of an image file from its first bytes (TGA has no signature: give `hint`, e.g. `image/x-tga` or
/// `tga`).
pub fn sniff(bytes: &[u8], hint: Option<&str>) -> Option<ImageFormat> {
    if let Ok(f) = image::guess_format(bytes) {
        return Some(f);
    }
    let h = hint.unwrap_or_default().to_ascii_lowercase();
    (h.contains("tga") || h.contains("targa")).then_some(ImageFormat::Tga)
}

/// Decode an image file (any supported format), refusing ones larger than [`MAX_PIXELS`].
pub fn decode(bytes: &[u8], hint: Option<&str>) -> Result<DynamicImage> {
    let format = sniff(bytes, hint).ok_or_else(|| FileError::Unsupported("not an image format OpenEtch reads".into()))?;
    let mut r = image::ImageReader::with_format(Cursor::new(bytes), format);
    let (w, h) = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|e| FileError::Invalid(format!("the image cannot be read ({e})")))?;
    if u64::from(w) * u64::from(h) > MAX_PIXELS || w == 0 || h == 0 {
        return Err(FileError::TooLarge(format!("the image is {w} x {h} pixels (at most {MAX_PIXELS} pixels)")));
    }
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1 << 31);
    r.limits(limits);
    r.decode().map_err(|e| FileError::Invalid(format!("the image cannot be decoded ({e})")))
}

/// Whether an image of this format must be converted to PNG to be shown and engraved (TIFF, TGA, ...).
pub fn needs_conversion(format: ImageFormat) -> bool {
    !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Bmp | ImageFormat::WebP)
}

/// The resolution a TIFF records (XResolution / YResolution with ResolutionUnit inch or cm), if any.
pub fn tiff_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    let le = bytes.starts_with(b"II*\0");
    if !le && !bytes.starts_with(b"MM\0*") {
        return None;
    }
    let u16_at = |at: usize| -> Option<u32> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(u32::from(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }))
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    let (mut x, mut y, mut unit) = (None, None, 2);
    for k in 0..n.min(512) {
        let e = ifd + 2 + 12 * k;
        let tag = u16_at(e)?;
        let rational = |at: usize| -> Option<f64> {
            let off = u32_at(at)? as usize;
            let (a, b) = (u32_at(off)?, u32_at(off + 4)?);
            (b != 0).then(|| f64::from(a) / f64::from(b))
        };
        match tag {
            282 => x = rational(e + 8),
            283 => y = rational(e + 8),
            296 => unit = u16_at(e + 8)?,
            _ => {}
        }
    }
    let k = match unit {
        2 => 1.0,
        3 => 2.54,
        _ => return None,
    };
    Some((x? * k, y.or(x)? * k))
}

/// Encode as PNG, with `dpi` (if any) in a `pHYs` chunk.
pub fn png(img: &DynamicImage, dpi: Option<f64>) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| FileError::Invalid(format!("PNG encoding failed ({e})")))?;
    if let Some(d) = dpi.filter(|d| d.is_finite() && *d > 0.0) {
        set_png_dpi(&mut out, d);
    }
    Ok(out)
}

/// Insert a `pHYs` chunk right after a PNG's IHDR.
pub fn set_png_dpi(png: &mut Vec<u8>, dpi: f64) {
    let ppm = (dpi / 0.0254).round();
    let at = 8 + 25;
    if png.len() < at || !(1.0..4e9).contains(&ppm) {
        return;
    }
    let ppm = ppm as u32;
    let mut body = b"pHYs".to_vec();
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1);
    let mut chunk = 9u32.to_be_bytes().to_vec();
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc32(&body).to_be_bytes());
    png.splice(at..at, chunk);
}

/// Grey levels (0 black ..= 255 white, composited on white) and alpha (0..=255), row-major from the top.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub value: Vec<u8>,
    pub alpha: Vec<u8>,
}

impl Gray {
    pub fn from_image(img: &DynamicImage) -> Gray {
        let la = img.to_luma_alpha8();
        let (w, h) = (la.width() as usize, la.height() as usize);
        let mut value = Vec::with_capacity(w * h);
        let mut alpha = Vec::with_capacity(w * h);
        for p in la.pixels() {
            let [l, a] = p.0;
            let v = (u32::from(l) * u32::from(a) + 255 * (255 - u32::from(a))) / 255;
            value.push(v as u8);
            alpha.push(a);
        }
        Gray { width: w, height: h, value, alpha }
    }
}

/// Image-local mm (centred, +Y up) → pixel-edge coordinates (x right, y down from the top-left).
pub fn local_to_pixel(width_px: u32, height_px: u32, width_mm: f64, height_mm: f64) -> Affine {
    let (w, h) = (f64::from(width_px), f64::from(height_px));
    Affine::new([w / width_mm, 0.0, 0.0, -h / height_mm, w / 2.0, h / 2.0])
}

/// Even-odd point in polygons.
pub fn inside(p: Point, polys: &[Vec<Point>]) -> bool {
    let mut odd = false;
    for poly in polys {
        let n = poly.len();
        for i in 0..n {
            let (a, b) = (poly[i], poly[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                odd = !odd;
            }
        }
    }
    odd
}

/// Flattened closed polygons of a path (every subpath implicitly closed).
pub fn polygons(p: &BezPath, tol: f64) -> Vec<Vec<Point>> {
    crate::dxf_out::polylines(p, tol).into_iter().map(|(pts, _)| pts).filter(|p| p.len() >= 3).collect()
}

/// Bake a mask into an image (LightBurn's Flatten Image Mask): pixels whose centre is outside `mask_px` (polygons
/// in pixel-edge coordinates, even-odd) become fully transparent; with `crop`, the image is also cut down to the
/// pixels the mask covers (Crop Image). Returns the PNG and the kept pixel rectangle `[x0, y0, x1, y1)`.
pub fn apply_mask(img: &DynamicImage, mask_px: &[Vec<Point>], crop: bool) -> Result<(Vec<u8>, [u32; 4])> {
    let mut rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    // Scan rows with crossing lists: fast for large images.
    for y in 0..h {
        let cy = f64::from(y) + 0.5;
        let mut xs: Vec<f64> = Vec::new();
        for poly in mask_px {
            let n = poly.len();
            for i in 0..n {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                if (a.y > cy) != (b.y > cy) {
                    xs.push(a.x + (cy - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        let mut k = 0;
        for x in 0..w {
            let cx = f64::from(x) + 0.5;
            while k < xs.len() && xs[k] <= cx {
                k += 1;
            }
            if k % 2 == 1 {
                if rgba.get_pixel(x, y).0[3] > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            } else {
                rgba.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return invalid("the mask does not cover any of the image");
    }
    let out = DynamicImage::ImageRgba8(rgba);
    let (out, rect) = if crop { (out.crop_imm(x0, y0, x1 - x0, y1 - y0), [x0, y0, x1, y1]) } else { (out, [0, 0, w, h]) };
    Ok((png(&out, None)?, rect))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(w, h, image::Luma([10])));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Tiff).unwrap();
        out
    }

    #[test]
    fn tiff_and_tga_decode_and_convert() {
        let t = tiff(3, 2);
        assert_eq!(sniff(&t, None), Some(ImageFormat::Tiff));
        assert!(needs_conversion(ImageFormat::Tiff) && !needs_conversion(ImageFormat::Png));
        let img = decode(&t, None).unwrap();
        let p = png(&img, Some(300.0)).unwrap();
        assert!(p.windows(4).any(|w| w == b"pHYs"));
        let back = image::load_from_memory(&p).unwrap();
        assert_eq!((back.width(), back.height()), (3, 2));
        let mut tga = Vec::new();
        img.write_to(&mut Cursor::new(&mut tga), ImageFormat::Tga).unwrap();
        assert_eq!(decode(&tga, Some("image/x-tga")).unwrap().width(), 3);
        assert!(decode(&tga, None).is_err() || sniff(&tga, None).is_some());
        assert_eq!(tiff_dpi(&t), None, "no resolution written");
    }

    #[test]
    fn grey_composites_on_white() {
        let img = DynamicImage::ImageLumaA8(image::GrayAlphaImage::from_raw(2, 1, vec![0, 255, 0, 0]).unwrap());
        let g = Gray::from_image(&img);
        assert_eq!(g.value, [0, 255]);
        assert_eq!(g.alpha, [255, 0]);
    }

    #[test]
    fn masks_hide_outside_and_crop() {
        let img = DynamicImage::ImageLuma8(image::GrayImage::from_pixel(10, 10, image::Luma([0])));
        let square = vec![Point::new(2.0, 2.0), Point::new(6.0, 2.0), Point::new(6.0, 6.0), Point::new(2.0, 6.0)];
        let (p, rect) = apply_mask(&img, std::slice::from_ref(&square), false).unwrap();
        assert_eq!(rect, [0, 0, 10, 10]);
        let back = image::load_from_memory(&p).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0).0[3], 0);
        assert_eq!(back.get_pixel(3, 3).0[3], 255);
        let (p, rect) = apply_mask(&img, &[square], true).unwrap();
        assert_eq!(rect, [2, 2, 6, 6]);
        assert_eq!(image::load_from_memory(&p).unwrap().width(), 4);
        assert!(apply_mask(&img, &[vec![Point::new(20.0, 20.0), Point::new(30.0, 20.0), Point::new(30.0, 30.0)]], false).is_err());
        assert!(inside(Point::new(3.0, 3.0), &[vec![Point::new(2.0, 2.0), Point::new(6.0, 2.0), Point::new(6.0, 6.0), Point::new(2.0, 6.0)]]));
    }
}
