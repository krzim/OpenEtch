//! LightBurn `.lbrn2` project files: [`parse`] text into a typed [`LbrnProject`], [`write`] one back, and convert
//! to and from the OpenEtch document ([`to_document`], [`from_document`]). Pure (no I/O), wasm-ok.
//!
//! # The format, as established for galvo projects
//!
//! Evidence: the owner's own LightBurn 1.3.01, 1.4.00 and 2.1.02 projects made for this machine (a 25-swatch MOPA
//! colour test, a grouped vector drawing, a two-line text engraving, a photo bitmap) and the owner's 1.x material
//! library (`.clb`); cross-checked with openlaser-rs `docs/LBRN2.md` and the community notes of
//! jlucaso1/lbrn2-to-svg and tscircuit/lbrnts. Nothing from LightBurn itself was used beyond reading these files.
//!
//! **Root.** `<?xml version="1.0" encoding="UTF-8"?>` then `<LightBurnProject AppVersion="1.4.00" FormatVersion="1"
//! MaterialHeight="0" MirrorX="False" MirrorY="False">` (2.1 adds `DeviceName="JCZFiber (Unknown)"` and
//! `AskForSendName`). Children in order: `Thumbnail Source=<base64 PNG>` (cropped to the art, so it says nothing
//! about the workspace), `VariableText` (`Start/End/Current/Increment/AutoAdvance Value=`), `UIPrefs`
//! (`Optimize_*`), the `CutSetting`s, the `Shape`s, `Notes ShowOnLoad="0" Notes=""`. 1.x files use CRLF, 2.1 LF.
//!
//! **Coordinates.** Millimetres, **y up**, origin at the **lower-left corner of the galvo field**, so the field
//! centre is `(field/2, field/2)` and OpenEtch's field frame is `lightburn - field/2` on both axes. Evidence: (1) every
//! shape in every sample lies at positive coordinates (overall x 31.0..89.0, y 18.2..92.5 mm), which a centre origin
//! would not produce for centred art; (2) the art that is visibly centred sits exactly on a half-field point: the
//! photo's XForm translation is x = 55.000 (110 mm field, LightBurn's default galvo lens), the colour-test grid is
//! centred on (58.000, 58.000), the drawing's frame rectangle on x = 75.000 (150 mm, the size in the owner's library
//! file name); (3) y is up: the two-line text has XForm `0 -1 1 0`, i.e. reading direction → -y and its first line
//! → +x, and the thumbnail shows it reading top-to-bottom with the first line on the right; the drawing's chest star
//! has a lower y than its head, and the thumbnail shows the star below. **The file does not record the field size**,
//! and the samples were evidently laid out in different ones, so import and export take `field_mm` from the caller
//! (the document's, normally the machine profile's). A wrong `field_mm` shifts everything by half the difference;
//! [`to_document`] warns when objects land outside the field.
//!
//! **XForm.** `a b c d e f`: `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (kurbo's [`kurbo::Affine::new`] order).
//! Every Shape has one (written with ~6 significant digits by LightBurn, e.g. `1 -3.12639e-13 3.12639e-13 1 43.7174
//! 71.417`). A Group's XForm applies to its children: child world = parent · child (the drawing nests groups
//! two deep with identity inner XForms under a `0.42206` scale).
//!
//! **Rect / Ellipse.** `Type="Rect" W H Cr` (corner radius), `Type="Ellipse" Rx Ry`, both centred on the local
//! origin, so the XForm translation is the centre (the colour test's 4.76 mm swatches have translations on a
//! 7.14 mm pitch; the drawing's 28 x 39 mm frame rectangle encloses all its paths only when read as centred).
//!
//! **Path.** `Type="Path" CutIndex VertID PrimID` with `<XForm>`, `<VertList>`, `<PrimList>`. `VertList` is a
//! concatenation of `V x y` records, each followed by control components `c0x n c0y n c1x n c1y n` with no
//! separators. A full pair is an absolute control point in the path's own coordinates; the bare flag `c0x1` / `c1x1`
//! (no `y`) means "no control point" (in all samples, all 1248 `B` primitives use full pairs, and the bare form is
//! always `...x1` on vertices only lines touch). `PrimList` concatenates `L i j` (line) and `B i j` (cubic from
//! vertex i to j using i's `c0` and j's `c1`), or is the word `LineClosed` / `LineOpen` (all vertices joined by lines,
//! closed or not; 4 such paths in the drawing). A primitive that does not start where the previous ended starts a
//! new subpath; one ending on its subpath's first vertex closes it (text glyphs: `L0 1..L7 0B8 9..`). `VertID` and
//! `PrimID` share lists: the first Path with an id and a list defines it, later Paths may give only the id (the
//! drawing has 84 paths, 79 VertLists, 49 PrimLists). Some 2.2 files repeat a full list under an id already defined.
//! The parser keeps each path's ids and which lists it carried ([`Encoded`]); the writer writes them back so, and
//! gives new paths fresh ids above every kept one with their lists inline.
//!
//! **Group.** `Type="Group"` (usually no CutIndex) with `<XForm>` and `<Children>` holding Shapes, nested up to two
//! deep in the samples.
//!
//! **Text.** `Type="Text" CutIndex Font="Arial,-1,100,5,50,0,0,0,0,0" Str H LS LnS Ah Av Weld HasBackupPath="1"`
//! (+ `VariableOffset` for numbered text) containing `<BackupPath Type="Path" CutIndex>` (a Path with its own XForm,
//! VertList, PrimList: the rendered outline in the text's local coordinates) and then the Text's `<XForm>`. In all
//! 26 Text shapes of the samples the two XForms are identical; openlaser-rs found the backup's copy can be stale after
//! a move, so the importer applies the Text's own XForm to the backup outline. Text without a backup cannot be drawn
//! and is only kept for export.
//!
//! **Bitmap.** `Type="Bitmap" W H` (mm before the XForm, centred: `W` is the pixel width at the image's own
//! resolution, the XForm scales and may mirror it) plus `Gamma Contrast Brightness EnhanceAmount EnhanceRadius
//! EnhanceDenoise File SourceHash Data=<base64 PNG>`; its settings are a `<CutSetting_Img type="Image">` (with
//! `dotWidth useDotCorrection halftoneAngle negative ditherMode dpi cleanupPass`). A layer index can have both a
//! `CutSetting` and a `CutSetting_Img` (`.lbt` templates have both for every index).
//!
//! **Root and layout.** Besides the root attributes (`AppVersion`, `DeviceName`, `AskForSendName`, 2.2's
//! `CurrentPage`/`Page0Name`, ...), the files hold `Thumbnail`, `VariableText`, `UIPrefs` and `Notes`, which OpenEtch
//! keeps verbatim with the root's child order. LightBurn writes 4-space indentation and `<tag Value="..."/>`;
//! script-made files use other declarations, `" />"` endings and zero-padded numbers (`15.8750`), all of which are
//! kept ([`Format`], and each number's source spelling while its value is unchanged). `.lbt` templates have the same
//! layout as projects.
//!
//! **CutSetting.** `<CutSetting type="Cut|Scan|Scan+Cut|...">` with `<tag Value="..."/>` children. Types seen:
//! `Scan` (fill, 32 project layers + all 37 library settings) and `Cut` (line, 1); `Scan+Cut` and `Tool` are from
//! openlaser-rs / lbrnts. Children seen in projects and the library: `index name LinkPath minPower maxPower
//! minPower2 maxPower2 speed frequency PPI QPulseWidth JumpSpeed wobbleEnable perforate numPasses anglePerPass
//! perfLen perfSkip dotTime dotSpacing bidir floodFill autoRotate crossHatch overscan overscanPercent interval angle
//! subname priority doOutput hide tabsEnabled tabCount tabCountMax tabSpacing tabSize`. Units: power %, `speed`
//! mm/s, `frequency` **Hz** (100000 = 100 kHz), `QPulseWidth` ns, `interval` mm, angles degrees, booleans `1`/`0`.
//! LightBurn omits values at its defaults (no `bidir` means bidirectional; `bidir 0` is written when off), so a
//! missing value takes OpenEtch's default (with a warning for power and speed). `priority` is the run
//! order; `doOutput 0` / `hide 1` are the output and visibility switches. `<SubLayer type="Scan" index="1">` children
//! (seen in the library) hold the same tags for an extra pass, typically `maxPower speed frequency numPasses
//! crossHatch interval angle subname`; a 3D sliced image layer's cleanup sub-layer adds `isCleanup 1` (its
//! `numPasses` is then "Clean after # of passes") and its parent `cleanupPass 1`.
//!
//! **Layers.** `CutIndex` 0..=29 are the colour layers C00..C29 with fixed colours ([`palette`]); 30 and 31 are the
//! tool layers T1 and T2 (`type="Tool"`, never output; from lbrnts' notes: "tool layers have no cut parameters and
//! never output", `CutIndex="30"`).
//!
//! # Mapping (import → OpenEtch → export)
//!
//! | LightBurn | OpenEtch |
//! |---|---|
//! | CutSetting `index` | layer (one per index used or defined, ordered by `priority`); index kept in `extras["lbrn2.index"]`; the source tags in order and spelling in `extras["lbrn2._src"]`; a second `CutSetting_Img` of the index in `extras["lbrn2._img"]` |
//! | `name` (else `subname`, else `C05`) | `Layer.name`; colour from the palette by index |
//! | `type` Cut / Scan / Scan+Cut | `mode` line / fill / fill_then_line; any other type kept in `extras["lbrn2.type"]` |
//! | `maxPower` % | `power_pct` (clamped to 0..=100 with a warning) |
//! | `speed` mm/s | `speed_mm_s` |
//! | `frequency` Hz | `frequency_khz` (÷ 1000); when absent, 30 kHz and `extras["lbrn2.frequency_absent"]`, so export leaves it out until `layer.update` sets a frequency |
//! | `QPulseWidth` ns | `pulse_width_ns` |
//! | `numPasses` | `passes` (clamped to 1..=1000) |
//! | `interval`, `angle`, `anglePerPass`, `crossHatch`, `bidir` | `hatch.interval_mm`, `angle_deg`, `angle_increment_deg`, `cross_hatch`, `bidirectional` |
//! | `doOutput`, `hide` | `output`, `visible` |
//! | `priority` | layer order; written back as read while every layer came from LightBurn and the order still agrees, else the position |
//! | `SubLayer`s | `sub_layers` (their other values in `extras["lbrn2.subN.<tag>"]`, N the 1-based position) |
//! | sub-layer `subname` | `sub_layers[i].name` (the main settings' `subname` stays in `extras["lbrn2.subname"]`) |
//! | sub-layer `isCleanup 1` + `numPasses` | `sub_layers[i].cleanup = {every_n_passes: numPasses}`, `passes` 1 (LightBurn's "Clean after # of passes"; see [`doc::Cleanup`]); the parent's `cleanupPass` ("Enable cleanup pass") stays in `extras` and is written as `1` while a cleanup sub-layer exists and dropped when none does |
//! | every other child (`minPower`, `PPI`, `tabCount`, ...) | `extras["lbrn2.<tag>"]`, written back verbatim (only tags matching `[A-Za-z_][A-Za-z0-9_.-]*`; others are dropped with a warning) |
//! | Rect / Ellipse | `Geometry::Rect` / `Ellipse`, transform = shift · groups · XForm |
//! | Path | `Geometry::Path` (exact lines and cubics), transform = shift · groups · XForm; ids and the lists it carried in `extras` |
//! | Group | `doc::Group` (nested through `parent`; members point at their innermost group), its XForm and attributes in its `extras` |
//! | Text | an object drawn as its BackupPath outline, named after the text; `Str`, `Font`, `H`, ... in `extras["lbrn2.<attr>"]`, exported as `Type="Text"` with a BackupPath of the current outline |
//! | Bitmap | `Geometry::Image` (`W`x`H`, `Data` byte for byte), the other attributes in `extras` |
//! | other types, Text without outline, unusable shapes | kept as XML in the extras of the object before them (`lbrn2._after.N`), exported again after it |
//! | every object | its attribute order (`lbrn2._attrs`), XForm text (`lbrn2._xform`) and unknown children (`lbrn2._child.N`) |
//! | root attributes and elements, layout | `Document::extras` (`lbrn2._root_attrs`, `lbrn2._root`, `lbrn2._el.N`, `lbrn2._decl`, `lbrn2._format`) |
//! | T1 tool shape (export) | `Document::stock` outline (import: an object on a non-output layer) |
//!
//! **Gantry (bed) devices** ([`Frame::Bed`], [`to_document_in`], [`from_document_in`]): coordinates are mm from the
//! device's origin corner, which the root's `MirrorX` / `MirrorY` record (front-left = neither, rear-left =
//! `MirrorY`, rear-right = both, front-right = `MirrorX`). Import uses the file's flags and the device's `W x H`:
//! field = S · (LightBurn - [W/2, H/2]) with S negating the mirrored axes; export applies the inverse for the target
//! device and writes its flags. Evidence: public LightBurn 1.2-2.0 projects saved for gantry devices, comparing the
//! embedded thumbnail (LightBurn's top-down view, front at the bottom) with the outlines: five rear-right (both flags,
//! Ruida) files from four authors are turned 180° when drawn raw and upright with the rule; a rear-left (`MirrorY`)
//! file's text is upside down raw and reads correctly with it; a front-right (`MirrorX`) file matches only with x
//! mirrored; eight front-left files match raw. That confirms the orientation. Mirroring about the bed (W - x) rather
//! than about 0 is likely but not shown (thumbnails are cropped to the art): four of the five rear-right files have
//! only positive raw coordinates, i.e. distances from the origin corner, and LightBurn moves and flips a design when
//! the device origin changes and reorients files from another origin on open since 1.3. `tests/frames.rs` encodes
//! the rule as a regression test; it is not itself evidence.
//!
//! shift = translate(-field/2, -field/2) on import and its inverse on export. Export writes what the import kept;
//! a transform, number or path list that did not change since keeps its exact source text, so an unchanged import
//! exports byte for byte (all 27 sample projects and templates do, `tests/real_samples.rs`).
//!
//! Limits: shared `VertList`/`PrimList`s are resolved into every path that uses them, so vertices and segments are
//! counted per use ([`MAX_SEGMENTS`]) and [`to_document`] caps the path elements it creates. Shapes whose placement
//! overflows or cannot be inverted, and layer indices above 31, are reported as warnings. The writer drops characters
//! XML 1.0 cannot hold. Not kept: XML comments; groups are written where their first member is in the drawing order.
//! Old-format `.lbrn` files are not read: no sample exists to establish their format.

mod convert;
mod model;
pub mod palette;
mod parse;
mod path;
mod write;

pub use convert::{
    EXTRA_PREFIX, FREQUENCY_ABSENT, FREQUENCY_ABSENT_KEY, Frame, Imported, MAX_BED_MM, MAX_PASSES, cut_setting_to_layer,
    from_document, from_document_in, keys, to_document, to_document_in,
};
pub use model::{
    Backup, CutParams, CutSetting, Element, Encoded, Format, LbrnProject, PathData, Prim, Prims, RootItem, Shape,
    ShapeKind, SubLayer, Vertex,
};
pub use parse::{LibraryEntry, MAX_BYTES, MAX_SEGMENTS, parse, parse_library};
pub use write::write;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LbrnError {
    #[error("not valid XML: {0}")]
    Xml(String),
    #[error("not a LightBurn project: {0}")]
    NotProject(String),
    #[error("malformed LightBurn project: {0}")]
    Malformed(String),
    #[error("LightBurn project too large: {0}")]
    TooLarge(String),
    #[error("{0}")]
    Invalid(String),
}
