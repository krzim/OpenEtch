//! Preview rendering: the SVG of the field, stock, objects and toolpath and its PNG (moved to crates/appcore, shared
//! with the browser core).

pub use appcore::render::{MAX_PNG_PX, RenderOptions, preview_svg, safe_color};

use crate::EngineError;

/// Rasterise an SVG to a square-ish PNG `width_px` wide (clamped to 16..=[`MAX_PNG_PX`]).
pub fn svg_to_png(svg: &str, width_px: u32) -> Result<Vec<u8>, EngineError> {
    Ok(appcore::render::svg_to_png(svg, width_px)?)
}
