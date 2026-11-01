//! Frame capture from a UVC camera on the engine's computer (feature `camera`, `nokhwa` without its default
//! features, Apache-2.0). Frames are converted here (MJPEG through the `image` crate, YUYV, NV12, grey and raw RGB by
//! hand), so no JPEG library under another licence is pulled in. Without the feature, frames come from a browser
//! (`POST /api/camera/frame`).

/// YUV (BT.601, video range) to RGB.
#[cfg_attr(not(feature = "camera"), allow(dead_code))]
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let c = f32::from(y) - 16.0;
    let d = f32::from(u) - 128.0;
    let e = f32::from(v) - 128.0;
    let q = |x: f32| x.round().clamp(0.0, 255.0) as u8;
    [q(1.164 * c + 1.596 * e), q(1.164 * c - 0.392 * d - 0.813 * e), q(1.164 * c + 2.017 * d)]
}

/// Packed YUYV 4:2:2 to RGB.
#[cfg_attr(not(feature = "camera"), allow(dead_code))]
pub fn yuyv_to_rgb(data: &[u8], width: u32, height: u32) -> Option<image::RgbImage> {
    let (w, h) = (width as usize, height as usize);
    if data.len() < w * h * 2 || w % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(w * h * 3);
    for px in data[..w * h * 2].as_chunks::<4>().0 {
        out.extend_from_slice(&yuv_to_rgb(px[0], px[1], px[3]));
        out.extend_from_slice(&yuv_to_rgb(px[2], px[1], px[3]));
    }
    image::RgbImage::from_raw(width, height, out)
}

/// NV12 (Y plane, then interleaved UV at half resolution) to RGB.
#[cfg_attr(not(feature = "camera"), allow(dead_code))]
pub fn nv12_to_rgb(data: &[u8], width: u32, height: u32) -> Option<image::RgbImage> {
    let (w, h) = (width as usize, height as usize);
    if data.len() < w * h * 3 / 2 || w % 2 != 0 || h % 2 != 0 {
        return None;
    }
    let (luma, chroma) = data.split_at(w * h);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let uv = (y / 2) * w + (x / 2) * 2;
            out.extend_from_slice(&yuv_to_rgb(luma[y * w + x], chroma[uv], chroma[uv + 1]));
        }
    }
    image::RgbImage::from_raw(width, height, out)
}

/// Grab one frame from camera `index` as RGB.
#[cfg(feature = "camera")]
pub fn grab(index: u32) -> Result<image::RgbImage, String> {
    use nokhwa::Camera;
    use nokhwa::utils::{CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType};
    let format = RequestedFormat::new::<nokhwa::pixel_format::RgbFormat>(RequestedFormatType::AbsoluteHighestResolution);
    let err = |e: nokhwa::NokhwaError| format!("camera {index}: {e}");
    let mut cam = Camera::new(CameraIndex::Index(index), format).map_err(err)?;
    cam.open_stream().map_err(err)?;
    // The first frames of many UVC cameras are dark while the exposure settles.
    let mut frame = cam.frame().map_err(err)?;
    for _ in 0..4 {
        frame = cam.frame().map_err(err)?;
    }
    let _ = cam.stop_stream();
    let (w, h) = (frame.resolution().width(), frame.resolution().height());
    let data = frame.buffer();
    let img = match frame.source_frame_format() {
        FrameFormat::MJPEG => crate::camera::decode_frame(data).ok(),
        FrameFormat::YUYV => yuyv_to_rgb(data, w, h),
        FrameFormat::NV12 => nv12_to_rgb(data, w, h),
        FrameFormat::GRAY => data
            .get(..(w * h) as usize)
            .and_then(|d| image::GrayImage::from_raw(w, h, d.to_vec()))
            .map(|g| image::DynamicImage::ImageLuma8(g).to_rgb8()),
        FrameFormat::RAWRGB => data.get(..(w * h * 3) as usize).and_then(|d| image::RgbImage::from_raw(w, h, d.to_vec())),
        other => return Err(format!("camera {index}: frame format {other:?} is not supported")),
    };
    img.ok_or_else(|| format!("camera {index}: the frame could not be decoded"))
}

/// Without the `camera` feature there is no local capture.
#[cfg(not(feature = "camera"))]
pub fn grab(_index: u32) -> Result<image::RgbImage, String> {
    Err("this engine was built without camera capture (feature camera); upload frames with POST /api/camera/frame".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_conversions() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        let yuyv = [235u8, 128, 16, 128];
        let img = yuyv_to_rgb(&yuyv, 2, 1).unwrap();
        assert_eq!((img.get_pixel(0, 0).0, img.get_pixel(1, 0).0), ([255, 255, 255], [0, 0, 0]));
        assert!(yuyv_to_rgb(&yuyv, 4, 1).is_none(), "too short");
        let nv12 = [16u8, 235, 16, 235, 128, 128];
        let img = nv12_to_rgb(&nv12, 2, 2).unwrap();
        assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255]);
    }
}
