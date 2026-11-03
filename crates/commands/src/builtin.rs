//! Built-in document commands. Ids are stable API. All lengths are mm and all angles degrees (counter-clockwise), in
//! the field frame unless a field says otherwise. Every edit returns JSON with `created` and `affected` ids and the
//! resulting field-frame `bounds` (`{min, max, size, center}`, or `null`); lookups by id never guess and fail with
//! [`CommandError::NotFound`](crate::CommandError::NotFound) listing close matches.

mod arrange;
// Art Library items (the engine keeps the libraries).
mod art_library;
mod array;
pub(crate) use array::sync_virtual_arrays;
mod document;
#[cfg(not(target_arch = "wasm32"))]
mod dxf;
mod file;
mod group;
mod image;
// Jigs and placement: Fit to Stock, jig pockets, pre-flight checks, re-mark region (short list #2, #8, #9, #15).
mod jigs;
mod job_settings;
mod layer;
mod object;
mod place;
mod region;
mod shape;
mod stock;
mod sub_layer;
mod svg_import;
// Tabs placed by hand, dogbones, slot resizing and tracing several images (LightBurn's Tools menu extras).
mod joinery;
mod tabs;
mod trace_many;
// Text, variable text and barcodes (crates/text).
pub(crate) mod text;
// Production slots of variable text (the serial ledger, empty pockets, Stop at End).
pub mod slots;
mod util;
// Geometry commands (crates/geom): booleans, offsets, path tools, nodes, generators, arrays, arrange.
mod arrays;
mod boolean;
// Fill Region (paint bucket): a closed shape for the area around a point.
mod fill_region;
mod generate;
mod node;
mod offset;
mod order;
mod path_tools;
mod pathutil;
// Libraries and generators: material library presets, test generators, start points, registration targets.
mod preset;
mod targets;
pub mod camera_markers;
mod testgen;
pub mod testgen_presets;
// Files and image tools (crate fileio): exports, further imports, trace, masks, pages, notes.
pub(crate) mod files;
mod rd_import;
mod image_tools;
mod pages;

// Design aids and constructions: shape properties, selection queries, page positions, guides, curve tolerance,
// extend, warp, tangent circles, rubber band, toolpath to path.
mod construct;
mod design;
#[cfg(test)]
mod design_review_tests;
#[cfg(test)]
mod design_tests;
#[cfg(test)]
mod fill_region_tests;
#[cfg(test)]
mod geom_tests;
#[cfg(test)]
mod tests;

pub use art_library::{ART_FORMAT, ArtFragment, ArtLayer, art_fragment};
pub use file::{ORIGIN_KEY, WorkspaceOrigin, lbrn2_device_name};

use crate::Registry;

pub(crate) fn register_all(r: &mut Registry) {
    r.register::<document::Inspect>();
    r.register::<document::Rename>();
    r.register::<file::DocNew>();
    r.register::<file::DocLoad>();
    r.register::<file::DocSave>();
    r.register::<file::ImportLbrn2>();
    r.register::<file::ExportLbrn2>();
    r.register::<stock::StockDefine>();
    r.register::<stock::StockClear>();
    r.register::<layer::LayerAdd>();
    r.register::<layer::LayerUpdate>();
    r.register::<layer::LayerDelete>();
    r.register::<layer::LayerMove>();
    r.register::<layer::LayerTool>();
    r.register::<sub_layer::SubAdd>();
    r.register::<sub_layer::SubUpdate>();
    r.register::<sub_layer::SubDelete>();
    r.register::<sub_layer::SubMove>();
    r.register::<sub_layer::SubDuplicate>();
    r.register::<shape::ShapeRect>();
    r.register::<shape::ShapeEllipse>();
    r.register::<shape::ShapePath>();
    r.register::<svg_import::ImportSvg>();
    r.register::<art_library::ArtInsert>();
    r.register::<object::Transform>();
    r.register::<object::SetLayer>();
    r.register::<object::Rename>();
    r.register::<object::Delete>();
    r.register::<object::Duplicate>();
    r.register::<place::Place>();
    r.register::<array::Array>();
    r.register::<array::ArrayFlatten>();
    r.register::<array::ArrayAdd>();
    r.register::<array::ArrayRemove>();
    r.register::<array::ArrayDuplicate>();
    r.register::<stock::StockMove>();
    r.register::<arrange::Move>();
    r.register::<arrange::Align>();
    r.register::<arrange::Distribute>();
    r.register::<group::GroupCreate>();
    r.register::<group::GroupUngroup>();
    r.register::<group::GroupRename>();
    r.register::<image::ImportImage>();
    r.register::<image::AdjustImage>();
    r.register::<job_settings::SetOptimization>();
    r.register::<job_settings::SetCutProperties>();
    r.register::<job_settings::SetRotary>();
    #[cfg(not(target_arch = "wasm32"))]
    r.register::<dxf::ImportDxf>();
    register_geometry(r);
    register_design(r);
    register_text(r);
    register_libs(r);
    register_files(r);
    register_tools(r);
    jigs::register(r);
}

/// Tabs, dogbones, Resize Slots and Multi-File Trace (`tabs`, `joinery`, `trace_many`).
fn register_tools(r: &mut Registry) {
    r.register::<tabs::AddTabs>();
    r.register::<tabs::MoveTab>();
    r.register::<tabs::RemoveTabs>();
    r.register::<tabs::ClearTabs>();
    r.register::<tabs::ListTabs>();
    r.register::<joinery::Dogbone>();
    r.register::<joinery::ResizeSlots>();
    r.register::<trace_many::TraceMany>();
    r.register::<trace_many::TraceFiles>();
}

/// Exports, imports, image tools, pages and notes (`files`, `image_tools`, `pages`).
fn register_files(r: &mut Registry) {
    r.register::<files::DocExport>();
    r.register::<files::DocPrint>();
    #[cfg(not(target_arch = "wasm32"))]
    r.register::<files::ImportPdf>();
    r.register::<files::ImportAi>();
    r.register::<files::ImportEps>();
    r.register::<files::ImportEzd>();
    r.register::<files::ImportHpgl>();
    r.register::<files::ImportGcode>();
    r.register::<rd_import::ImportRd>();
    r.register::<files::ImportGerber>();
    r.register::<files::ImportModel>();
    r.register::<files::EditModel>();
    r.register::<image_tools::TraceImage>();
    r.register::<image_tools::TracePreview>();
    r.register::<image_tools::ConvertToBitmap>();
    r.register::<image_tools::ApplyMask>();
    r.register::<image_tools::RemoveMask>();
    r.register::<image_tools::FlattenMask>();
    r.register::<image_tools::ReplaceImage>();
    r.register::<image_tools::ExportImage>();
    r.register::<image_tools::ProcessedImage>();
    r.register::<pages::PageList>();
    r.register::<pages::PageAdd>();
    r.register::<pages::PageDuplicate>();
    r.register::<pages::PageDelete>();
    r.register::<pages::PageRename>();
    r.register::<pages::PageActivate>();
    r.register::<pages::PageMove>();
    r.register::<pages::PageSendObjects>();
    r.register::<pages::SetNotes>();
}

/// Library presets, test generators, start points and registration targets.
fn register_libs(r: &mut Registry) {
    r.register::<preset::ApplyPreset>();
    r.register::<preset::UnlinkPreset>();
    r.register::<testgen::MaterialTest>();
    r.register::<testgen::IntervalTest>();
    r.register::<testgen::FocusTest>();
    r.register::<camera_markers::CameraMarkers>();
    r.register::<targets::SetStartPoint>();
    r.register::<targets::ClearStartPoint>();
    r.register::<targets::AlignTargets>();
}

/// The geometry commands (booleans, offsets, path and node tools, generators, arrays, arrange), after the rest.
fn register_geometry(r: &mut Registry) {
    r.register::<boolean::PathBoolean>();
    r.register::<boolean::PathWeld>();
    r.register::<boolean::PathCutShapes>();
    r.register::<offset::PathOffset>();
    r.register::<path_tools::BreakApart>();
    r.register::<path_tools::AutoJoin>();
    r.register::<path_tools::ClosePath>();
    r.register::<path_tools::DeleteDuplicates>();
    r.register::<path_tools::ConvertToPath>();
    r.register::<path_tools::Optimize>();
    r.register::<path_tools::Fillet>();
    r.register::<path_tools::Unfillet>();
    r.register::<path_tools::Trim>();
    r.register::<path_tools::Reverse>();
    r.register::<fill_region::FillRegion>();
    r.register::<fill_region::FillRegionPreview>();
    r.register::<node::NodeList>();
    r.register::<node::NodeMove>();
    r.register::<node::NodeInsert>();
    r.register::<node::NodeDelete>();
    r.register::<node::NodeBreak>();
    r.register::<node::NodeJoin>();
    r.register::<node::NodeSetType>();
    r.register::<node::NodeSegmentType>();
    r.register::<generate::ShapePolygon>();
    r.register::<generate::ShapeLine>();
    r.register::<generate::ShapeCircle>();
    r.register::<arrays::CircularArray>();
    r.register::<arrays::AlongPath>();
    r.register::<arrays::Nest>();
    r.register::<order::Order>();
    r.register::<order::Lock>();
    r.register::<order::MakeSame>();
    r.register::<order::RotateTo>();
    r.register::<order::ScaleTo>();
}

/// Design aids and constructions (`design`, `construct`).
fn register_design(r: &mut Registry) {
    r.register::<design::ShapeUpdate>();
    r.register::<design::SelectOpen>();
    r.register::<design::SelectOpenOnFill>();
    r.register::<design::SelectContained>();
    r.register::<design::MoveToPagePosition>();
    r.register::<design::GuideAdd>();
    r.register::<design::GuideMove>();
    r.register::<design::GuideDelete>();
    r.register::<design::SetCurveTolerance>();
    r.register::<construct::Extend>();
    r.register::<construct::PathWarp>();
    r.register::<construct::TangentCandidates>();
    r.register::<construct::TangentCircle>();
    r.register::<construct::RubberBand>();
    r.register::<construct::ToolpathToPath>();
}

/// Text, variable text, barcodes and fonts (`text` module).
fn register_text(r: &mut Registry) {
    r.register::<text::ShapeText>();
    r.register::<text::TextUpdate>();
    r.register::<text::ApplyPath>();
    r.register::<text::DetachPath>();
    r.register::<text::ConvertToPath>();
    r.register::<text::Refresh>();
    r.register::<text::SetSerial>();
    r.register::<text::Advance>();
    r.register::<text::LoadCsv>();
    r.register::<text::ClearCsv>();
    r.register::<text::PreviewRow>();
    r.register::<slots::Slots>();
    r.register::<text::ShapeBarcode>();
    r.register::<text::FontList>();
}
