//! LightBurn's layer colours by index. A layer's colour in LightBurn is fixed by its index, so these are
//! interoperability facts: an imported layer shows the colour LightBurn shows, and an exported layer whose colour is
//! one of these keeps it.
//!
//! Values only, from openlaser-rs `crates/doc/src/palette.rs` (https://github.com/luizbueno3d/openlaser-rs @ 9d81a15,
//! MIT OR Apache-2.0, Copyright (c) the openlaser-rs authors), which transcribed them from LightBurn's public
//! "Color Palette" documentation page (provenance in that repository's `crates/doc/PROVENANCE.md`).

/// Number of colour layers, C00..C29.
pub const COLOR_LAYERS: u32 = 30;
/// Layer index of the tool layer T1 (T2 is `T1 + 1`). Tool layers never output.
pub const T1: u32 = 30;

/// Display colours of C00..C29.
pub const PALETTE: [&str; 30] = [
    "#000000", "#0000ff", "#ff0000", "#00e000", "#d0d000", "#ff8000", "#00e0e0", "#ff00ff", "#b4b4b4", "#0000a0",
    "#a00000", "#00a000", "#a0a000", "#c08000", "#00a0ff", "#a000a0", "#808080", "#7d87b9", "#bb7784", "#4a6fe3",
    "#d33f6a", "#8cd78c", "#f0b98d", "#f6c4e1", "#fa9ed4", "#500a78", "#b45a00", "#004754", "#86fa88", "#ffdb66",
];

/// Display colours of T1 and T2.
pub const TOOL_COLORS: [&str; 2] = ["#f36926", "#0c96d9"];

/// The colour LightBurn shows for a layer index (grey for indices it does not define).
pub fn color(index: u32) -> &'static str {
    match index {
        i if i < COLOR_LAYERS => PALETTE[i as usize],
        T1 => TOOL_COLORS[0],
        i if i == T1 + 1 => TOOL_COLORS[1],
        _ => "#808080",
    }
}

/// LightBurn's default layer name for an index: `C00`..`C29`, `T1`, `T2`.
pub fn default_name(index: u32) -> String {
    match index {
        i if i < COLOR_LAYERS => format!("C{i:02}"),
        i if i == T1 || i == T1 + 1 => format!("T{}", i - T1 + 1),
        i => format!("C{i}"),
    }
}

/// The colour-layer index whose colour is exactly `css` (case-insensitive `#rrggbb`).
pub fn index_of_color(css: &str) -> Option<u32> {
    let c = css.trim().to_ascii_lowercase();
    PALETTE.iter().position(|p| *p == c).map(|i| i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_colours_and_lookup() {
        assert_eq!(default_name(0), "C00");
        assert_eq!(default_name(29), "C29");
        assert_eq!(default_name(30), "T1");
        assert_eq!(default_name(31), "T2");
        assert_eq!(color(1), "#0000ff");
        assert_eq!(color(31), "#0c96d9");
        assert_eq!(index_of_color("#FF0000"), Some(2));
        assert_eq!(index_of_color("#123456"), None);
    }
}
