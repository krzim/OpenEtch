//! Built-in document commands. Ids are stable API. All lengths are mm and all angles degrees (counter-clockwise), in
//! the field frame unless a field says otherwise. Every edit returns JSON with `created` and `affected` ids and the
//! resulting field-frame `bounds` (`{min, max, size, center}`, or `null`); lookups by id never guess and fail with
//! [`CommandError::NotFound`](crate::CommandError::NotFound) listing close matches.

mod array;
mod document;
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

use crate::Registry;

pub(crate) fn register_all(r: &mut Registry) {
    r.register::<document::Inspect>();
    r.register::<document::Rename>();
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
}
