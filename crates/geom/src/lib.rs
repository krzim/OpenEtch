//! Pure 2D geometry for OpenEtch documents, in millimetres (usually the field frame, but nothing here depends on the
//! frame). Paths come in and go out as [`kurbo::BezPath`]. No I/O, no allocation-unbounded loops, wasm-compatible.
//!
//! What keeps curves and what does not:
//!
//! - **Curve-preserving** (work on [`subpath::SubPath`] segments, so lines stay lines and Béziers stay Béziers):
//!   break apart, auto-join, close, reverse, delete duplicate segments, optimize (it only ever turns near-straight
//!   curves into lines), fillet (adds circular arcs as cubics), trim (cuts curves with `subsegment`), node editing,
//!   generators.
//! - **Polygonal results** ([`region`], [`offset`]): booleans, weld, cut shapes and offsets run on outlines flattened
//!   at [`TOLERANCE`] (0.01 mm) through `i_overlay`, so their results are polylines whose vertices lie within
//!   0.01 mm of the true curves (an offset circle comes back as a polygon with many short sides, not as a circle;
//!   round offset arcs above ~81 mm radius are coarser, see [`offset`]).
//!
//! Every entry point refuses or skips non-finite input instead of panicking; calls into `i_overlay` are additionally
//! wrapped in [`guard`], so an internal failure there becomes a [`GeomError`] on native builds.

pub mod construct;
pub mod dogbone;
pub mod extend;
pub mod fillet;
pub mod fit;
pub mod flatten;
pub mod measure;
pub mod nest;
pub mod nodes;
pub mod offset;
pub mod ops;
pub mod region;
pub mod shapes;
pub mod slots;
pub mod subpath;
pub mod unfillet;
pub mod warp;

pub use flatten::{Polyline, flatten, polylines_to_path};
pub use kurbo;
pub use region::{BoolOp, Fill, Region};
pub use subpath::{SubPath, join, split};

/// Flattening tolerance in mm: the largest distance between a curve and the polyline that replaces it.
pub const TOLERANCE: f64 = 0.01;

/// Points closer than this (mm) are the same point.
pub const EPS: f64 = 1e-7;

/// Upper bound on flattened points one operation will process, so a crafted path cannot exhaust memory.
pub const MAX_POINTS: usize = 2_000_000;

/// Largest coordinate magnitude (mm) the polygon engine is given. `i_overlay` maps coordinates onto an integer grid
/// sized to their extent and panics past about 1e150, so a finite but absurd coordinate (a node dragged to 1e308)
/// is refused instead; 1 km keeps that grid finer than 0.002 mm.
pub const MAX_COORD: f64 = 1e6;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GeomError {
    /// The input cannot be used (with the reason, written for the caller).
    #[error("{0}")]
    Invalid(String),
    /// The polygon engine failed on this input.
    #[error("the geometry engine failed on this input ({0}); try simplifying the shapes")]
    Engine(String),
}

pub type Result<T, E = GeomError> = std::result::Result<T, E>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(GeomError::Invalid(msg.into()))
}

/// Run `f`, turning a panic inside it into [`GeomError::Engine`] (native builds; on wasm a panic aborts).
pub fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| GeomError::Engine(what.to_string()))
}

/// Whether every coordinate of `path` is finite.
pub fn is_finite(path: &kurbo::BezPath) -> bool {
    use kurbo::PathEl;
    let ok = |p: kurbo::Point| p.x.is_finite() && p.y.is_finite();
    path.elements().iter().all(|el| match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => ok(p),
        PathEl::QuadTo(a, p) => ok(a) && ok(p),
        PathEl::CurveTo(a, b, p) => ok(a) && ok(b) && ok(p),
        PathEl::ClosePath => true,
    })
}

/// Bounding box of a path, `None` when it has no points.
pub fn bounds(path: &kurbo::BezPath) -> Option<kurbo::Rect> {
    use kurbo::Shape;
    (!path.elements().is_empty()).then(|| path.bounding_box())
}
