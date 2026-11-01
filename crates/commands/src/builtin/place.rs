//! `art.place`: position (and optionally scale) a selection relative to the stock.
//!
//! All placement maths happens in the stock's own frame (origin at the stock centre, axes along its edges). The
//! selection's bounding box in that frame is scaled about its centre and moved; the resulting stock-frame affine is
//! conjugated back into the field frame and pre-multiplied onto every selected object. Placement is idempotent: running
//! the same `art.place` twice gives the same result (except for `rotation_deg`, which adds rotation every time).

use doc::{Document, ObjectId, Stock, StockShape};
use kurbo::{Affine, BezPath, Point, Rect, Shape, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::apply_affine;
use super::region::{self, EPS};
use super::util::{self, invalid, r4, transact};
use crate::{Command, CommandError, Result, Risk};

/// Where on the stock the selection's bounding box is anchored, in the stock's own (possibly rotated) frame.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Center,
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    /// Direction from the centre: -1, 0 or 1 per axis, +Y being "top".
    fn dir(self) -> (f64, f64) {
        match self {
            Anchor::Center => (0.0, 0.0),
            Anchor::TopLeft => (-1.0, 1.0),
            Anchor::Top => (0.0, 1.0),
            Anchor::TopRight => (1.0, 1.0),
            Anchor::Left => (-1.0, 0.0),
            Anchor::Right => (1.0, 0.0),
            Anchor::BottomLeft => (-1.0, -1.0),
            Anchor::Bottom => (0.0, -1.0),
            Anchor::BottomRight => (1.0, -1.0),
        }
    }
}

/// How to size the selection.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fit {
    /// Keep the current size.
    None,
    /// Scale uniformly to the largest size that keeps the selection's bounding box inside the stock, at least
    /// `margin_mm` from its edge (for a circle, the box corners stay inside the circle shrunk by the margin).
    Contain {
        /// Clearance to the stock edge in mm. Default 0.
        #[serde(default)]
        margin_mm: f64,
    },
    /// Scale uniformly so the bounding box is this wide (along the stock's X axis), in mm.
    Width {
        /// Target width in mm.
        width_mm: f64,
    },
    /// Scale uniformly so the bounding box is this tall (along the stock's Y axis), in mm.
    Height {
        /// Target height in mm.
        height_mm: f64,
    },
    /// Scale to this exact bounding-box size in mm.
    Exact {
        /// Target width in mm.
        width_mm: f64,
        /// Target height in mm.
        height_mm: f64,
        /// When true, scale uniformly to the largest size that fits within width x height instead of stretching.
        /// Default false.
        #[serde(default)]
        keep_aspect: bool,
    },
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaceInput {
    /// Exact object or group ids (a group stands for all its members), placed together as one selection.
    pub ids: Vec<ObjectId>,
    /// Which point of the stock the group's bounding box is aligned to, in the stock's own frame: "center" (default),
    /// "top_left", "top", "top_right", "left", "right", "bottom_left", "bottom", "bottom_right". "top" is the stock's
    /// +Y side. On curved edges (circle, rounded corners) the box is pushed toward the anchor until it touches the edge.
    pub anchor: Option<Anchor>,
    /// Extra offset [dx, dy] in mm along the stock's own axes, applied after anchoring. Default [0, 0].
    pub offset_mm: Option<[f64; 2]>,
    /// Sizing: {"mode": "none"} (default), {"mode": "contain", "margin_mm"}, {"mode": "width", "width_mm"},
    /// {"mode": "height", "height_mm"} or {"mode": "exact", "width_mm", "height_mm", "keep_aspect"?}.
    pub fit: Option<Fit>,
    /// Rotate the group counter-clockwise by this many degrees about its centre before fitting. Default 0.
    pub rotation_deg: Option<f64>,
}

/// `art.place`.
pub struct Place;

impl Command for Place {
    type Input = PlaceInput;
    const ID: &'static str = "art.place";
    const TITLE: &'static str = "Place on stock";
    const DESCRIPTION: &'static str = "Move (and optionally rotate and scale) objects as a group relative to the \
        stock: anchor the group's bounding box to the stock centre, an edge or a corner (in the stock's own rotated \
        frame), optionally fit it (contain with a margin, width, height or exact size in mm), then offset it. Returns \
        the final field-frame bounds and the margins in mm from the group's box to each stock edge, plus the minimum \
        clearance of the actual geometry to the stock outline (negative = outside). Fails if no stock is defined.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PlaceInput) -> Result<(Value, String)> {
        transact(doc, |doc| place(doc, i))
    }
}

fn place(doc: &mut Document, i: PlaceInput) -> Result<(Value, String)> {
    let id = Place::ID;
    let stock =
        doc.stock.clone().ok_or_else(|| CommandError::Failed("no stock is defined; run stock.define first".into()))?;
    let ids = util::object_ids(id, doc, &i.ids)?;
    util::check_unlocked(id, doc, &ids)?;
    let anchor = i.anchor.unwrap_or_default();
    let fit = i.fit.unwrap_or(Fit::None);
    let [ox, oy] = util::finite2(id, "offset_mm", i.offset_mm.unwrap_or([0.0, 0.0]))?;

    // 1. Optional extra rotation about the group's field-frame centre.
    if let Some(r) = i.rotation_deg {
        util::finite(id, "rotation_deg", r)?;
        let c = doc.bounds_of(&ids).map(|b| b.center()).unwrap_or(Point::ORIGIN);
        apply_affine(doc, &ids, Affine::rotate_about(r.to_radians(), c));
    }

    // 2. Bounding box of the group in the stock frame.
    let to_stock = stock.transform().inverse();
    let local = group_path(doc, &ids, to_stock);
    let b = local.bounding_box();
    let (w, h) = (b.width(), b.height());
    if w <= 1e-9 && h <= 1e-9 {
        return Err(CommandError::Failed("the selection has zero size".into()));
    }

    // 3. Scale factors and the margin the box must keep.
    let (sx, sy, margin) = scale_for(&stock, fit, w, h)?;

    // 4. Position: anchor in the stock frame, then offset.
    let half = Vec2::new(sx * w / 2.0, sy * h / 2.0);
    let center = anchor_center(&stock, anchor, half, margin) + Vec2::new(ox, oy);
    let local_t = Affine::translate(center.to_vec2())
        * Affine::scale_non_uniform(sx, sy)
        * Affine::translate(-b.center().to_vec2());
    apply_affine(doc, &ids, stock.transform() * local_t * to_stock);

    // 5. Report.
    let local_after = group_path(doc, &ids, to_stock);
    let lb = local_after.bounding_box();
    let (hw, hh) = region::half_size(&stock);
    let clearance = region::max_sdf(&stock, &local_after).map(|d| r4(-d));
    let mut r = util::edit_result(doc, &[], &ids);
    r["stock_frame_bounds"] = util::rect_json(lb);
    r["margins_mm"] = json!({
        "left": r4(lb.x0 + hw),
        "right": r4(hw - lb.x1),
        "bottom": r4(lb.y0 + hh),
        "top": r4(hh - lb.y1),
    });
    r["clearance_mm"] = json!(clearance);
    r["bbox_inside_stock"] = json!(region::box_inside(&stock, lb, 0.0));
    r["geometry_inside_stock"] = json!(clearance.is_some_and(|c| c >= -1e-4));
    r["scale"] = json!([sx, sy]);
    let summary = format!("placed {} at stock {:?}", ids.join(", "), anchor).to_lowercase();
    Ok((r, summary))
}

/// The union of the selected objects' paths, mapped by `t`.
fn group_path(doc: &Document, ids: &[ObjectId], t: Affine) -> BezPath {
    let mut all = BezPath::new();
    for o in doc.objects.iter().filter(|o| ids.contains(&o.id)) {
        all.extend(t * o.field_path());
    }
    all
}

/// Scale factors for a `w` x `h` box (stock frame) and the margin anchoring must respect.
fn scale_for(stock: &Stock, fit: Fit, w: f64, h: f64) -> Result<(f64, f64, f64)> {
    let id = Place::ID;
    let uniform_to = |target: f64, size: f64, axis: &str| -> Result<(f64, f64, f64)> {
        if size <= 1e-9 {
            return Err(CommandError::Failed(format!("cannot fit by {axis}: the selection has zero {axis}")));
        }
        let s = target / size;
        Ok((s, s, 0.0))
    };
    match fit {
        Fit::None => Ok((1.0, 1.0, 0.0)),
        Fit::Width { width_mm } => uniform_to(util::positive(id, "fit.width_mm", width_mm)?, w, "width"),
        Fit::Height { height_mm } => uniform_to(util::positive(id, "fit.height_mm", height_mm)?, h, "height"),
        Fit::Exact { width_mm, height_mm, keep_aspect } => {
            util::positive(id, "fit.width_mm", width_mm)?;
            util::positive(id, "fit.height_mm", height_mm)?;
            let sx = if w > 1e-9 { width_mm / w } else { f64::INFINITY };
            let sy = if h > 1e-9 { height_mm / h } else { f64::INFINITY };
            if keep_aspect {
                let s = sx.min(sy);
                Ok((s, s, 0.0))
            } else if sx.is_infinite() || sy.is_infinite() {
                Err(CommandError::Failed(
                    "cannot stretch a selection with zero width or height; use keep_aspect".into(),
                ))
            } else {
                Ok((sx, sy, 0.0))
            }
        }
        Fit::Contain { margin_mm } => {
            let m = util::non_negative(id, "fit.margin_mm", margin_mm)?;
            let s = contain_scale(stock, w, h, m);
            if s <= 0.0 {
                return Err(invalid(id, format!("fit.margin_mm {m} leaves no room on the stock")));
            }
            Ok((s, s, m))
        }
    }
}

/// Largest uniform scale at which a centred `w` x `h` box stays `m` inside the stock. Both stock shapes are centrally
/// symmetric and convex, so the centred position admits the largest box; anchoring only uses leftover room.
fn contain_scale(stock: &Stock, w: f64, h: f64, m: f64) -> f64 {
    let (hw, hh) = region::half_size(stock);
    let (aw, ah) = (hw - m, hh - m);
    if aw <= 0.0 || ah <= 0.0 {
        return 0.0;
    }
    let bound = |avail: f64, size: f64| if size > 1e-9 { 2.0 * avail / size } else { f64::INFINITY };
    let upper = bound(aw, w).min(bound(ah, h));
    let fits = |s: f64| region::box_inside(stock, Rect::from_center_size(Point::ORIGIN, (s * w, s * h)), m);
    match stock.shape {
        // Closed forms: the circle touches all four corners; the sharp rect touches two sides.
        StockShape::Circle { diameter_mm } => 2.0 * (diameter_mm / 2.0 - m) / w.hypot(h),
        StockShape::Rect { .. } if fits(upper) => upper,
        StockShape::Rect { .. } => bisect(0.0, upper, fits),
    }
}

/// Centre of a box with half-size `half` anchored to the stock (stock frame), at least `margin` inside when it fits.
fn anchor_center(stock: &Stock, anchor: Anchor, half: Vec2, margin: f64) -> Point {
    let (dx, dy) = anchor.dir();
    let (hw, hh) = region::half_size(stock);
    // Edge-aligned target: the box's anchor point on the inset bounding rectangle's anchor point.
    let target = Point::new(dx * (hw - margin - half.x), dy * (hh - margin - half.y));
    let fits = |c: Point| region::box_inside(stock, Rect::from_center_size(c, (2.0 * half.x, 2.0 * half.y)), margin);
    if !region::is_curved(stock) || fits(target) || !fits(Point::ORIGIN) {
        return target;
    }
    // Slide from the centre toward the target until the box touches the curved edge.
    let t = bisect(0.0, 1.0, |t| fits(Point::ORIGIN.lerp(target, t)));
    Point::ORIGIN.lerp(target, t)
}

/// Largest `x` in `[lo, hi]` with `ok(x)`, given `ok(lo)` holds and `ok` is monotone.
fn bisect(mut lo: f64, mut hi: f64, ok: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < EPS * 1e-3 {
            break;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::shape::ShapeRect;

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    fn stock(shape: StockShape, rotation_deg: f64, center: [f64; 2]) -> Stock {
        Stock { name: "s".into(), shape, thickness_mm: 1.0, material: None, center, rotation_deg }
    }

    /// Document with the given stock and a 20 x 10 rect at [30, 30] (obj-2).
    fn doc(s: Stock) -> Document {
        let mut d = Document { stock: Some(s), ..Document::default() };
        run::<ShapeRect>(&mut d, json!({"width_mm": 20, "height_mm": 10, "center": [30, 30]})).unwrap();
        d
    }

    fn f(v: &Value) -> f64 {
        v.as_f64().unwrap()
    }

    #[test]
    fn requires_stock() {
        let mut d = doc(stock(StockShape::Circle { diameter_mm: 10.0 }, 0.0, [0.0, 0.0]));
        d.stock = None;
        assert!(run::<Place>(&mut d, json!({"ids": ["obj-2"]})).is_err());
    }

    #[test]
    fn center_on_offset_stock() {
        let mut d =
            doc(stock(StockShape::Rect { width_mm: 100.0, height_mm: 50.0, corner_radius_mm: 0.0 }, 0.0, [10.0, -5.0]));
        let (r, _) = run::<Place>(&mut d, json!({"ids": ["obj-2"]})).unwrap();
        assert_eq!(r["bounds"]["center"], json!([10.0, -5.0]));
        assert_eq!(r["margins_mm"], json!({"left": 40.0, "right": 40.0, "bottom": 20.0, "top": 20.0}));
        assert_eq!(r["bbox_inside_stock"], json!(true));
    }

    #[test]
    fn contain_on_rect_with_anchor() {
        let mut d =
            doc(stock(StockShape::Rect { width_mm: 100.0, height_mm: 50.0, corner_radius_mm: 0.0 }, 0.0, [0.0, 0.0]));
        let (r, _) = run::<Place>(
            &mut d,
            json!({"ids": ["obj-2"], "anchor": "top_left", "fit": {"mode": "contain", "margin_mm": 5}}),
        )
        .unwrap();
        // 20 x 10 into 90 x 40: height limits, scale 4 -> 80 x 40, flush with left and top margins.
        assert_eq!(r["bounds"]["size"], json!([80.0, 40.0]));
        assert_eq!(r["margins_mm"], json!({"left": 5.0, "right": 15.0, "bottom": 5.0, "top": 5.0}));
        // Idempotent.
        let (r2, _) = run::<Place>(
            &mut d,
            json!({"ids": ["obj-2"], "anchor": "top_left", "fit": {"mode": "contain", "margin_mm": 5}}),
        )
        .unwrap();
        assert_eq!(r["bounds"], r2["bounds"]);
    }

    #[test]
    fn contain_on_circle_keeps_corners_inside() {
        let mut d = doc(stock(StockShape::Circle { diameter_mm: 50.0 }, 0.0, [5.0, 5.0]));
        let (r, _) =
            run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "contain", "margin_mm": 2}})).unwrap();
        // Box diagonal = 50 - 2*2 = 46; 20:10 aspect -> w = 46 * 2 / sqrt(5).
        let w = 46.0 * 2.0 / 5f64.sqrt();
        let size = &r["bounds"]["size"];
        assert!((f(&size[0]) - w).abs() < 1e-3 && (f(&size[1]) - w / 2.0).abs() < 1e-3, "{size}");
        assert_eq!(r["bounds"]["center"], json!([5.0, 5.0]));
        assert!((f(&r["clearance_mm"]) - 2.0).abs() < 1e-3, "{}", r["clearance_mm"]);
        assert_eq!(r["bbox_inside_stock"], json!(true));
    }

    #[test]
    fn top_anchor_on_circle_touches_edge() {
        let mut d = doc(stock(StockShape::Circle { diameter_mm: 50.0 }, 0.0, [0.0, 0.0]));
        let (r, _) = run::<Place>(&mut d, json!({"ids": ["obj-2"], "anchor": "top"})).unwrap();
        // Box 20 x 10 pushed up: top corners at (+-10, y) on the circle r = 25 -> y = sqrt(625 - 100).
        let top = f(&r["bounds"]["max"][1]);
        assert!((top - 525f64.sqrt()).abs() < 1e-3, "{top}");
        assert!(f(&r["clearance_mm"]).abs() < 1e-3);
    }

    #[test]
    fn rotated_stock_anchor_is_in_stock_frame() {
        // 100 x 50 stock rotated 90°: its own +Y ("top") points along field -X.
        let mut d =
            doc(stock(StockShape::Rect { width_mm: 100.0, height_mm: 50.0, corner_radius_mm: 0.0 }, 90.0, [0.0, 0.0]));
        let (r, _) = run::<Place>(&mut d, json!({"ids": ["obj-2"], "anchor": "top"})).unwrap();
        // The box (20 x 10 in the field, so 10 x 20 in the stock frame) sits flush at stock top = field x -25.
        assert_eq!(r["bounds"]["min"][0], json!(-25.0));
        assert_eq!(r["margins_mm"]["top"], json!(0.0));
        assert_eq!(r["stock_frame_bounds"]["size"], json!([10.0, 20.0]));
    }

    #[test]
    fn fit_modes() {
        let rect = StockShape::Rect { width_mm: 100.0, height_mm: 100.0, corner_radius_mm: 0.0 };
        let mut d = doc(stock(rect.clone(), 0.0, [0.0, 0.0]));
        let (r, _) = run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "width", "width_mm": 50}})).unwrap();
        assert_eq!(r["bounds"]["size"], json!([50.0, 25.0]));
        let (r, _) =
            run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "height", "height_mm": 10}})).unwrap();
        assert_eq!(r["bounds"]["size"], json!([20.0, 10.0]));
        let (r, _) =
            run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "exact", "width_mm": 30, "height_mm": 30}}))
                .unwrap();
        assert_eq!(r["bounds"]["size"], json!([30.0, 30.0]));
        let (r, _) = run::<Place>(
            &mut d,
            json!({"ids": ["obj-2"], "rotation_deg": 90, "offset_mm": [1, 2], "fit": {"mode": "none"}}),
        )
        .unwrap();
        assert_eq!(r["bounds"]["size"], json!([30.0, 30.0]));
        assert_eq!(r["bounds"]["center"], json!([1.0, 2.0]));
    }

    #[test]
    fn contain_on_rounded_rect_avoids_corners() {
        let rr = StockShape::Rect { width_mm: 40.0, height_mm: 40.0, corner_radius_mm: 10.0 };
        let mut d = doc(stock(rr, 0.0, [0.0, 0.0]));
        let (r, _) =
            run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "exact", "width_mm": 1, "height_mm": 1}}))
                .unwrap();
        assert_eq!(r["bbox_inside_stock"], json!(true));
        let (r, _) =
            run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "contain", "margin_mm": 0}})).unwrap();
        assert_eq!(r["bbox_inside_stock"], json!(true));
        // A square's corner touches the r = 10 arc: a = 10 + 10 / sqrt(2) per half side.
        let side = 2.0 * (10.0 + 10.0 / 2f64.sqrt());
        assert!((f(&r["bounds"]["size"][0]) - side).abs() < 1e-3, "{}", r["bounds"]);
        assert!(f(&r["clearance_mm"]).abs() < 1e-3);
    }

    #[test]
    fn contain_refuses_oversized_margin() {
        let mut d = doc(stock(StockShape::Circle { diameter_mm: 10.0 }, 0.0, [0.0, 0.0]));
        assert!(run::<Place>(&mut d, json!({"ids": ["obj-2"], "fit": {"mode": "contain", "margin_mm": 6}})).is_err());
    }
}
