//! Text like LightBurn: fonts, layout, variable text and barcodes, producing the outlines that
//! [`doc::Text::outline`] caches (so planning never needs fonts).
//!
//! - Fonts ([`registry`]): installed TrueType/OpenType fonts (feature `system`, via fontdb; shaped with harfrust,
//!   outlines from skrifa), the bundled single-line Hershey fonts ([`hershey`]), AutoCAD `.shx` shape fonts
//!   ([`shx`]) and EzCad `.jsf` single-line fonts ([`jsf`]) from font folders. LightBurn's own single-line font
//!   format is not read: it is undocumented and no sample was available.
//! - Layout ([`render_text`]): lines, height, HSpace/VSpace, Align X/Y, bold/italic faces, upper case, max width and
//!   squeeze, bend (distorted or rigid glyphs), text on a path, welding (see the `layout` module for the rules and
//!   how they were matched to LightBurn's saved outlines).
//! - Variable text ([`vars`]): serial numbers, date/time, CSV merge and cut-setting codes, as LightBurn formats them.
//! - Barcodes ([`barcode`]).
//!
//! Everything but font discovery and the clock is pure and builds for wasm32.

pub mod barcode;
pub mod hershey;
pub mod jsf;
mod layout;
pub mod outline;
pub mod registry;
pub mod shx;
pub mod stroke;
pub mod vars;

use doc::{Document, Geometry, Object, Text};
use kurbo::BezPath;

pub use registry::{FamilyInfo, Font, Resolved, families, resolve};
pub use vars::Moment;

/// A rendered text.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The outline in the text's own frame.
    pub outline: BezPath,
    /// The text it shows (after variable text, before upper case).
    pub shown: String,
    /// What could not be honoured (missing glyphs, faces), for the caller to report.
    pub warnings: Vec<String>,
}

/// Lay `t` out showing `shown` (its text, or the evaluated variable text) with its font. `path` is the outline to
/// follow (in the text's own frame) when the text is on a path.
pub fn render_text(t: &Text, shown: &str, path: Option<&BezPath>) -> Result<Rendered, String> {
    if let Some(p) = t.problem() {
        return Err(p);
    }
    let r = registry::resolve(&t.font, t.bold, t.italic)?;
    render_with(t, shown, path, &r)
}

/// [`render_text`] with an already resolved font.
pub fn render_with(t: &Text, shown: &str, path: Option<&BezPath>, r: &Resolved) -> Result<Rendered, String> {
    let mut lines = layout::lines(shown, t.upper);
    if t.ignore_empty_vars && t.variable.is_some() {
        lines.retain(|l| !l.trim().is_empty());
    }
    let laid = layout::lay_out(t, &lines, r);
    let outline = layout::finish(t, &laid, path)?;
    if !geom::is_finite(&outline) {
        return Err("the text's outline is not finite (check its height and bend)".into());
    }
    Ok(Rendered { outline, shown: shown.to_string(), warnings: laid.warnings })
}

/// What a text object shows: its text, or for variable text the evaluated format (`moment` for date/time; `laser`
/// the device name for the cut-setting `L` code).
pub fn shown_text(doc: &Document, obj: &Object, t: &Text, moment: Option<&Moment>, laser: &str) -> String {
    match &t.variable {
        Some(v) => vars::evaluate(doc, &obj.layer, &t.text, v, moment, laser),
        None => t.text.clone(),
    }
}

/// The outline a text on a path follows, in the text object's own frame: the path object's outline.
pub fn path_for(doc: &Document, obj: &Object, t: &Text) -> Result<Option<BezPath>, String> {
    let Some(tp) = &t.path else { return Ok(None) };
    let other = doc
        .object(&tp.object)
        .ok_or_else(|| format!("the path object {} of text {} no longer exists", tp.object, obj.id))?;
    if matches!(other.geometry, Geometry::Image { .. } | Geometry::Text(_)) {
        return Err(format!("text {} can only follow a vector shape, not {}", obj.id, other.id));
    }
    let det = obj.transform.determinant();
    if !(det.is_finite() && det != 0.0) {
        return Err(format!("text {} has a transform that cannot be inverted", obj.id));
    }
    Ok(Some(obj.transform.inverse() * other.field_path()))
}

/// Render text object `obj` of `doc` as it should show now. Fails for an object that is not text, a missing font or
/// path object.
pub fn render_object(doc: &Document, obj: &Object, moment: Option<&Moment>, laser: &str) -> Result<Rendered, String> {
    let Geometry::Text(t) = &obj.geometry else {
        return Err(format!("{} is not a text object", obj.id));
    };
    let shown = shown_text(doc, obj, t, moment, laser);
    let path = path_for(doc, obj, t)?;
    render_text(t, &shown, path.as_ref())
}

#[cfg(test)]
mod tests;
