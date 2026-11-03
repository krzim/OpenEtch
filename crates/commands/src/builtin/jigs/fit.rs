//! `object.fit_to_stock`: scale (aspect locked) and centre the selection so it fills the stock as much as it can
//! within a margin. On round stock the design's real outline (its convex hull, through its smallest enclosing circle)
//! is fitted, not its bounding box, so a round or hexagonal design fills a coin edge to edge. On rectangles the
//! selection can also be turned by 90 degrees, or by the angle that lets it grow the most. One undo step.

use doc::{Document, ObjectId, StockShape};
use kurbo::{Affine, Point};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::object::apply_affine;
use super::super::pathutil::geom_err;
use super::super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

fn default_margin() -> f64 {
    1.0
}

/// How the selection may turn while it is fitted into a rectangular stock.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitRotate {
    /// Keep the selection's orientation (default).
    #[default]
    None,
    /// Also try it turned by 90 degrees, and keep whichever is larger.
    Ninety,
    /// Turn it by whatever angle makes it largest.
    Best,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitInput {
    /// Exact object or group ids (a group stands for its members); they are scaled and moved together.
    pub ids: Vec<ObjectId>,
    /// Space kept free between the design and the stock's edge, mm (default 1).
    #[serde(default = "default_margin")]
    pub margin_mm: f64,
    /// Rectangular stock only: "none" (default), "ninety" or "best" (see the command description).
    #[serde(default)]
    pub rotate: FitRotate,
}

/// `object.fit_to_stock`.
pub struct FitToStock;

impl Command for FitToStock {
    type Input = FitInput;
    const ID: &'static str = "object.fit_to_stock";
    const TITLE: &'static str = "Fit to stock";
    const DESCRIPTION: &'static str = "Scale the selection (aspect ratio kept) and centre it on the stock so it is as \
        large as it can be with margin_mm (default 1) free to the stock's edge. Round stock: the design's real \
        outline (convex hull) is fitted through its smallest enclosing circle, not its bounding box. Rectangular \
        stock (rounded corners respected): rotate \"none\" keeps the orientation, \"ninety\" also tries a quarter \
        turn, \"best\" searches every angle. Needs a stock (stock.define). Returns affected, scale, rotation_deg, \
        fill (the share of the usable stock the fitted outline's bounds or circle take, 0..1) and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FitInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let stock = doc
                .stock
                .clone()
                .ok_or_else(|| CommandError::Failed(format!("{cmd}: no stock is defined; define it first (stock.define)")))?;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let margin = util::non_negative(cmd, "margin_mm", i.margin_mm)?;
            let hull = hull_of(cmd, doc, &ids)?;
            let fit = match stock.shape {
                StockShape::Circle { diameter_mm } => {
                    let r = diameter_mm / 2.0 - margin;
                    if r <= 0.0 {
                        return Err(invalid(cmd, format!("margin_mm {margin} leaves no room on a {diameter_mm} mm disc")));
                    }
                    fit_circle(cmd, &hull, Point::new(stock.center[0], stock.center[1]), r)?
                }
                StockShape::Rect { width_mm, height_mm, corner_radius_mm } => {
                    let (w, h) = (width_mm - 2.0 * margin, height_mm - 2.0 * margin);
                    if w <= 0.0 || h <= 0.0 {
                        return Err(invalid(cmd, format!("margin_mm {margin} leaves no room on the {width_mm} x {height_mm} mm stock")));
                    }
                    let room = Room { w, h, r: (corner_radius_mm - margin).max(0.0) };
                    fit_rect(cmd, &hull, stock.transform(), room, i.rotate)?
                }
            };
            apply_affine(doc, &ids, fit.affine);
            let mut r = util::edit_result(doc, &[], &ids);
            r["scale"] = json!(util::r4(fit.scale));
            r["rotation_deg"] = json!(util::r4(fit.rotation_deg));
            r["fill"] = json!(util::r4(fit.fill));
            let turned = if fit.rotation_deg.abs() > 1e-9 { format!(", turned {}°", util::r4(fit.rotation_deg)) } else { String::new() };
            Ok((r, format!("fitted {} to the stock (x{}{turned})", ids.join(", "), util::r4(fit.scale))))
        })
    }
}

/// The convex hull of the objects' outlines in the field frame.
fn hull_of(cmd: &str, doc: &Document, ids: &[ObjectId]) -> Result<Vec<Point>> {
    let mut pts = Vec::new();
    for id in ids {
        let o = util::object(doc, id)?;
        for line in geom::flatten(&o.field_path(), geom::TOLERANCE).map_err(geom_err(cmd))? {
            pts.extend(line.points);
        }
    }
    let hull = geom::construct::convex_hull(&pts);
    if hull.is_empty() {
        return Err(invalid(cmd, "the selection has no outline to fit"));
    }
    Ok(hull)
}

/// A fit: the field-frame affine to pre-multiply, its scale and turn, and how much of the room it fills.
struct Fit {
    affine: Affine,
    scale: f64,
    rotation_deg: f64,
    fill: f64,
}

fn fit_circle(cmd: &str, hull: &[Point], centre: Point, room: f64) -> Result<Fit> {
    let (m, r) = geom::enclose::min_enclosing_circle(hull).ok_or_else(|| invalid(cmd, "the selection has no outline to fit"))?;
    if r < 1e-9 {
        return Err(invalid(cmd, "the selection is a single point; there is nothing to scale"));
    }
    let s = room / r;
    let affine = Affine::translate(centre.to_vec2()) * Affine::scale(s) * Affine::translate(-m.to_vec2());
    Ok(Fit { affine, scale: s, rotation_deg: 0.0, fill: 1.0 })
}

/// The usable rectangle of the stock (its size less the margins) and its usable corner radius.
#[derive(Clone, Copy)]
struct Room {
    w: f64,
    h: f64,
    r: f64,
}

/// Bounds of `pts` turned by `a` radians: (min, max).
fn turned_bounds(pts: &[Point], a: f64) -> (Point, Point) {
    let (s, c) = a.sin_cos();
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in pts {
        let q = Point::new(c * p.x - s * p.y, s * p.x + c * p.y);
        lo = Point::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = Point::new(hi.x.max(q.x), hi.y.max(q.y));
    }
    (lo, hi)
}

/// The largest scale of `pts` turned by `a` that fits a `w x h` box (a zero extent does not limit).
fn box_scale(pts: &[Point], a: f64, room: Room) -> f64 {
    let (lo, hi) = turned_bounds(pts, a);
    let (bw, bh) = (hi.x - lo.x, hi.y - lo.y);
    let sx = if bw > 1e-12 { room.w / bw } else { f64::INFINITY };
    let sy = if bh > 1e-12 { room.h / bh } else { f64::INFINITY };
    sx.min(sy)
}

/// Whether `q` (stock frame, centred) lies inside the room's rounded rectangle.
fn in_room(q: Point, room: Room) -> bool {
    let (hx, hy) = (room.w / 2.0, room.h / 2.0);
    let tol = 1e-7 * (1.0 + hx.max(hy));
    if q.x.abs() > hx + tol || q.y.abs() > hy + tol {
        return false;
    }
    if room.r <= 0.0 {
        return true;
    }
    let (cx, cy) = (hx - room.r, hy - room.r);
    let (dx, dy) = (q.x.abs() - cx, q.y.abs() - cy);
    dx <= 0.0 || dy <= 0.0 || dx.hypot(dy) <= room.r + tol
}

fn fit_rect(cmd: &str, hull: &[Point], stock: Affine, room: Room, rotate: FitRotate) -> Result<Fit> {
    // The hull in the stock's own frame (stock centre at the origin, its sides along the axes).
    let local: Vec<Point> = hull.iter().map(|p| stock.inverse() * *p).collect();
    let mut angles: Vec<f64> = vec![0.0];
    match rotate {
        FitRotate::None => {}
        FitRotate::Ninety => angles.push(90.0),
        FitRotate::Best => {
            // Every whole degree, plus every angle that lays a hull edge along a side (where the optimum often is).
            angles.extend((1..180).map(f64::from));
            let n = local.len();
            for k in 0..n {
                let d = local[(k + 1) % n] - local[k];
                if d.hypot() > 1e-9 {
                    let phi = d.y.atan2(d.x).to_degrees();
                    angles.extend([-phi, 90.0 - phi]);
                }
            }
        }
    }
    let norm = |a: f64| {
        let r = a.rem_euclid(180.0);
        if r > 90.0 { r - 180.0 } else { r }
    };
    let score = |deg: f64| box_scale(&local, deg.to_radians(), room);
    // The largest scale; on a tie (within 1e-9 relative) the smaller turn wins, so "best" keeps the design upright
    // when turning gains nothing.
    let mut best = (0.0f64, score(0.0));
    for a in angles.into_iter().map(norm) {
        let s = score(a);
        if s > best.1 * (1.0 + 1e-9) || ((s - best.1).abs() <= best.1 * 1e-9 && a.abs() < best.0.abs()) {
            best = (a, s);
        }
    }
    if rotate == FitRotate::Best {
        // Golden-section refinement within a degree either side.
        let (mut lo, mut hi) = (best.0 - 1.0, best.0 + 1.0);
        let g = (5f64.sqrt() - 1.0) / 2.0;
        for _ in 0..40 {
            let (a, b) = (hi - g * (hi - lo), lo + g * (hi - lo));
            if score(a) > score(b) { hi = b } else { lo = a }
        }
        let a = norm((lo + hi) / 2.0);
        if score(a) > best.1 * (1.0 + 1e-9) {
            best = (a, score(a));
        }
    }
    let (deg, mut s) = best;
    if !s.is_finite() || s <= 0.0 {
        return Err(invalid(cmd, "the selection is a single point; there is nothing to scale"));
    }
    let turn = deg.to_radians();
    let (lo, hi) = turned_bounds(&local, turn);
    let mid = lo.midpoint(hi).to_vec2();
    let place = |s: f64| Affine::scale(s) * Affine::translate(-mid) * Affine::rotate(turn);
    if room.r > 0.0 {
        // Rounded corners: shrink until every hull point is inside (bisection on the scale, centre kept).
        let fits = |s: f64| local.iter().all(|p| in_room(place(s) * *p, room));
        if !fits(s) {
            let (mut a, mut b) = (0.0, s);
            for _ in 0..60 {
                let m = (a + b) / 2.0;
                if fits(m) { a = m } else { b = m }
            }
            s = a;
        }
    }
    // Field frame: into the stock frame, place, back out.
    let affine = stock * place(s) * stock.inverse();
    let (bw, bh) = (hi.x - lo.x, hi.y - lo.y);
    let fill = ((bw * s / room.w).max(0.0)).max((bh * s / room.h).max(0.0)).min(1.0);
    Ok(Fit { affine, scale: s, rotation_deg: deg, fill })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};

    fn exec(s: &mut Session, id: &str, params: Value) -> Value {
        match s.execute(Invocation { id: id.into(), params }, Origin::Agent("test".into())) {
            Ok(o) => o.result,
            Err(e) => panic!("{id} failed: {e}"),
        }
    }

    fn fails(s: &mut Session, id: &str, params: Value) -> String {
        match s.execute(Invocation { id: id.into(), params: params.clone() }, Origin::Agent("test".into())) {
            Ok(o) => panic!("{id} {params} should fail, got {}", o.result),
            Err(e) => e.to_string(),
        }
    }

    fn session() -> Session {
        Session::new(Document { field_mm: Some(150.0), ..Document::default() })
    }

    fn id(v: &Value) -> String {
        v["created"][0].as_str().unwrap().to_string()
    }

    #[test]
    fn round_stock_fits_the_outline_not_the_box() {
        let mut s = session();
        exec(&mut s, "stock.define", json!({"shape": {"kind": "circle", "diameter_mm": 40}, "thickness_mm": 2, "center": [10, 5]}));
        // A hexagon of circumradius 3 somewhere else: its bounding box's diagonal is larger than its true circle.
        let hex = id(&exec(&mut s, "shape.polygon", json!({"sides": 6, "radius_mm": 3, "center": [-30, 20]})));
        let r = exec(&mut s, "object.fit_to_stock", json!({"ids": [hex], "margin_mm": 2}));
        // Room radius 18 over circumradius 3.
        assert!((r["scale"].as_f64().unwrap() - 6.0).abs() < 1e-3, "{r}");
        let o = s.document().object(&hex).unwrap();
        let pts: Vec<Point> = geom::flatten(&o.field_path(), 0.01).unwrap().into_iter().flat_map(|l| l.points).collect();
        let far = pts.iter().map(|p| p.distance(Point::new(10.0, 5.0))).fold(0.0, f64::max);
        assert!((far - 18.0).abs() < 1e-3, "{far}");
        // One undo step brings it back.
        s.undo().unwrap();
        assert!((s.document().object(&hex).unwrap().bounds().center().x + 30.0).abs() < 1e-9);
    }

    #[test]
    fn rect_stock_keeps_aspect_and_can_turn() {
        let mut s = session();
        exec(&mut s, "stock.define", json!({"shape": {"kind": "rect", "width_mm": 60, "height_mm": 20}, "thickness_mm": 1}));
        // A tall 2 x 10 bar: upright it can grow to 18 mm tall (x1.8); turned a quarter it grows to 58 mm (x5.8).
        let bar = id(&exec(&mut s, "shape.rect", json!({"width_mm": 2, "height_mm": 10, "center": [30, 30]})));
        let r = exec(&mut s, "object.fit_to_stock", json!({"ids": [bar]}));
        assert!((r["scale"].as_f64().unwrap() - 1.8).abs() < 1e-9 && r["rotation_deg"] == json!(0.0), "{r}");
        let b = s.document().object(&bar).unwrap().bounds();
        assert!(b.center().distance(Point::ZERO) < 1e-9 && (b.height() - 18.0).abs() < 1e-9, "{b:?}");
        s.undo().unwrap();
        let r = exec(&mut s, "object.fit_to_stock", json!({"ids": [bar], "rotate": "ninety"}));
        assert!((r["scale"].as_f64().unwrap() - 5.8).abs() < 1e-9 && r["rotation_deg"].as_f64().unwrap().abs() == 90.0, "{r}");
        let b = s.document().object(&bar).unwrap().bounds();
        assert!((b.width() - 58.0).abs() < 1e-6 && (b.height() - 11.6).abs() < 1e-6, "{b:?}");
        s.undo().unwrap();
        let r = exec(&mut s, "object.fit_to_stock", json!({"ids": [bar], "rotate": "best"}));
        assert!(r["scale"].as_f64().unwrap() >= 5.8 - 1e-9, "{r}");
    }

    #[test]
    fn best_rotation_beats_the_box_for_a_diagonal_bar() {
        let mut s = session();
        exec(&mut s, "stock.define", json!({"shape": {"kind": "rect", "width_mm": 50, "height_mm": 10}, "thickness_mm": 1, "rotation_deg": 30}));
        // A thin bar at 45 degrees in the field: best lays it along the stock's long side.
        let bar = id(&exec(&mut s, "shape.path", json!({"svg_path_data": "M 0 0 L 10 10 L 10.5 9.5 L 0.5 -0.5 Z"})));
        let none = exec(&mut s, "object.fit_to_stock", json!({"ids": [bar], "margin_mm": 0}))["scale"].as_f64().unwrap();
        s.undo().unwrap();
        let best = exec(&mut s, "object.fit_to_stock", json!({"ids": [bar], "margin_mm": 0, "rotate": "best"}));
        assert!(best["scale"].as_f64().unwrap() > none * 1.5, "{best} vs {none}");
        // Everything is inside the turned stock.
        let d = s.document();
        let st = d.stock.as_ref().unwrap().transform();
        for p in geom::flatten(&d.object(&bar).unwrap().field_path(), 0.01).unwrap().into_iter().flat_map(|l| l.points) {
            let q = st.inverse() * p;
            assert!(q.x.abs() <= 25.0 + 1e-6 && q.y.abs() <= 5.0 + 1e-6, "{q:?}");
        }
    }

    #[test]
    fn rounded_corners_and_refusals() {
        let mut s = session();
        let sq = id(&exec(&mut s, "shape.rect", json!({"width_mm": 10, "height_mm": 10})));
        assert!(fails(&mut s, "object.fit_to_stock", json!({"ids": [sq]})).contains("no stock"));
        exec(&mut s, "stock.define", json!({"shape": {"kind": "rect", "width_mm": 20, "height_mm": 20, "corner_radius_mm": 6}, "thickness_mm": 1}));
        let r = exec(&mut s, "object.fit_to_stock", json!({"ids": [sq], "margin_mm": 0}));
        // A square in a rounded square: its corners must stay inside the 6 mm radius, so it cannot reach 20 mm.
        let sc = r["scale"].as_f64().unwrap();
        assert!(sc < 2.0 && sc > 1.5, "{r}");
        let d = s.document();
        for p in geom::flatten(&d.object(&sq).unwrap().field_path(), 0.01).unwrap().into_iter().flat_map(|l| l.points) {
            assert!(in_room(p, Room { w: 20.0, h: 20.0, r: 6.0 }), "{p:?}");
        }
        assert!(fails(&mut s, "object.fit_to_stock", json!({"ids": [sq], "margin_mm": 11})).contains("no room"));
        assert!(fails(&mut s, "object.fit_to_stock", json!({"ids": [sq], "margin_mm": -1})).contains("margin_mm"));
        exec(&mut s, "object.lock", json!({"ids": [sq], "locked": true}));
        assert!(fails(&mut s, "object.fit_to_stock", json!({"ids": [sq]})).contains("locked"));
    }
}
