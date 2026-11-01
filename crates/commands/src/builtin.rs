//! Built-in document commands. Ids are stable API. All lengths are mm and all angles degrees (counter-clockwise), in
//! the field frame unless a field says otherwise. Every edit returns JSON with `created` and `affected` ids and the
//! resulting field-frame `bounds` (`{min, max, size, center}`, or `null`); lookups by id never guess and fail with
//! [`CommandError::NotFound`](crate::CommandError::NotFound) listing close matches.

mod arrange;
mod array;
mod document;
#[cfg(not(target_arch = "wasm32"))]
mod dxf;
mod file;
mod group;
mod image;
mod job_settings;
mod layer;
mod object;
mod place;
mod region;
mod shape;
mod stock;
mod sub_layer;
mod svg_import;
// Text, variable text and barcodes (crates/text).
pub(crate) mod text;
mod util;
// Geometry commands (crates/geom): booleans, offsets, path tools, nodes, generators, arrays, arrange.
mod arrays;
mod boolean;
mod generate;
mod node;
mod offset;
mod order;
mod path_tools;
mod pathutil;

#[cfg(test)]
mod geom_tests;
#[cfg(test)]
mod tests;

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
    r.register::<sub_layer::SubAdd>();
    r.register::<sub_layer::SubUpdate>();
    r.register::<sub_layer::SubDelete>();
    r.register::<sub_layer::SubMove>();
    r.register::<sub_layer::SubDuplicate>();
    r.register::<shape::ShapeRect>();
    r.register::<shape::ShapeEllipse>();
    r.register::<shape::ShapePath>();
    r.register::<svg_import::ImportSvg>();
    r.register::<object::Transform>();
    r.register::<object::SetLayer>();
    r.register::<object::Rename>();
    r.register::<object::Delete>();
    r.register::<object::Duplicate>();
    r.register::<place::Place>();
    r.register::<array::Array>();
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
    register_text(r);
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
    r.register::<path_tools::Trim>();
    r.register::<path_tools::Reverse>();
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
    r.register::<text::ShapeBarcode>();
    r.register::<text::FontList>();
}
