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
mod layer;
mod object;
mod place;
mod region;
mod shape;
mod stock;
mod svg_import;
mod util;

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
    #[cfg(not(target_arch = "wasm32"))]
    r.register::<dxf::ImportDxf>();
}
