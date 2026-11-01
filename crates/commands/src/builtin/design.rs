//! Design aids: `shape.update` (LightBurn's Shape Properties for rectangles, ellipses and polygons), the selection
//! queries of LightBurn's Edit menu (`select.open_shapes`, `select.open_on_fill`, `select.contained`),
//! `object.move_to_page_position` (Move Selected Objects), manual guides saved with the project (`guide.add`,
//! `guide.move`, `guide.delete`) and the output curve tolerance (`doc.set_curve_tolerance`).

use doc::{CURVE_TOLERANCE_RANGE_MM, Document, FillMode, Geometry, Guide, GuideAxis, MAX_GUIDE_MM, MAX_GUIDES, ObjectId};
use geom::flatten::{Polyline, point_in_ring};
use kurbo::{Affine, Point, Rect};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::object::apply_affine;
use super::place::Anchor;
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// `null` → `Some(None)`, a value → `Some(Some(v))`, absent → `None` (with `#[serde(default)]`).
fn double_option<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<Option<f64>>, D::Error> {
    Ok(Some(Option::deserialize(d)?))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShapeUpdateInput {
    /// Exact id of a rectangle, ellipse or polygon object.
    pub id: ObjectId,
    /// Rectangle: width in mm (its own X axis).
    pub width_mm: Option<f64>,
    /// Rectangle: height in mm.
    pub height_mm: Option<f64>,
    /// Rectangle (at most half the shorter side) or polygon (corners where it fits): corner radius in mm.
    pub corner_radius_mm: Option<f64>,
    /// Ellipse: radius along its own X axis, mm.
    pub rx_mm: Option<f64>,
    /// Ellipse: radius along its own Y axis, mm.
    pub ry_mm: Option<f64>,
    /// Polygon: number of sides (or star points), 3..=1000.
    pub sides: Option<u32>,
    /// Polygon: centre-to-corner radius, mm.
    pub radius_mm: Option<f64>,
    /// Polygon: make it a star with inner corners at this fraction of the radius (0 < r < 1); null makes it a plain
    /// polygon again.
    #[serde(default, deserialize_with = "double_option")]
    #[schemars(with = "Option<f64>")]
    pub star_ratio: Option<Option<f64>>,
}

/// `shape.update`.
pub struct ShapeUpdate;

impl Command for ShapeUpdate {
    type Input = ShapeUpdateInput;
    const ID: &'static str = "shape.update";
    const TITLE: &'static str = "Shape properties";
    const DESCRIPTION: &'static str = "Edit the parameters of a primitive shape in place (LightBurn's Shape \
        Properties): a rectangle's width_mm, height_mm and corner_radius_mm; an ellipse's rx_mm and ry_mm; a \
        polygon's sides, radius_mm, star_ratio (null for a plain polygon) and corner_radius_mm. Its centre, rotation \
        and layer stay. Fields that do not apply to the shape are refused; paths and text cannot be edited this way \
        (text: text.update). Returns the affected id, the new `geometry` and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ShapeUpdateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let o = super::pathutil::single_object(cmd, doc, &i.id, true)?;
            let given = |name: &str, v: bool| if v { Some(name.to_string()) } else { None };
            let fields: Vec<String> = [
                given("width_mm", i.width_mm.is_some()),
                given("height_mm", i.height_mm.is_some()),
                given("corner_radius_mm", i.corner_radius_mm.is_some()),
                given("rx_mm", i.rx_mm.is_some()),
                given("ry_mm", i.ry_mm.is_some()),
                given("sides", i.sides.is_some()),
                given("radius_mm", i.radius_mm.is_some()),
                given("star_ratio", i.star_ratio.is_some()),
            ]
            .into_iter()
            .flatten()
            .collect();
            if fields.is_empty() {
                return Err(invalid(cmd, "nothing to change: give at least one shape parameter"));
            }
            let refuse = |allowed: &[&str], kind: &str| -> Result<()> {
                match fields.iter().find(|f| !allowed.contains(&f.as_str())) {
                    Some(f) => Err(invalid(cmd, format!("{f} does not apply to {kind} {}", o.id))),
                    None => Ok(()),
                }
            };
            let geometry = match o.geometry {
                Geometry::Rect { width_mm, height_mm, corner_radius_mm } => {
                    refuse(&["width_mm", "height_mm", "corner_radius_mm"], "rectangle")?;
                    let w = util::positive(cmd, "width_mm", i.width_mm.unwrap_or(width_mm))?;
                    let h = util::positive(cmd, "height_mm", i.height_mm.unwrap_or(height_mm))?;
                    let r =
                        util::non_negative(cmd, "corner_radius_mm", i.corner_radius_mm.unwrap_or(corner_radius_mm))?;
                    if w > 2e6 || h > 2e6 {
                        return Err(invalid(cmd, "the rectangle is too large"));
                    }
                    // An explicit radius must fit; a kept one shrinks with the rectangle.
                    let max = w.min(h) / 2.0;
                    let r = if i.corner_radius_mm.is_some() {
                        if r > max + 1e-12 {
                            return Err(invalid(cmd, "corner_radius_mm must be at most half the shorter side"));
                        }
                        r
                    } else {
                        r.min(max)
                    };
                    Geometry::Rect { width_mm: w, height_mm: h, corner_radius_mm: r }
                }
                Geometry::Ellipse { rx_mm, ry_mm } => {
                    refuse(&["rx_mm", "ry_mm"], "ellipse")?;
                    let rx = util::positive(cmd, "rx_mm", i.rx_mm.unwrap_or(rx_mm))?;
                    let ry = util::positive(cmd, "ry_mm", i.ry_mm.unwrap_or(ry_mm))?;
                    if rx > 1e6 || ry > 1e6 {
                        return Err(invalid(cmd, "the ellipse is too large"));
                    }
                    Geometry::Ellipse { rx_mm: rx, ry_mm: ry }
                }
                Geometry::Polygon { sides, radius_mm, star_ratio, corner_radius_mm } => {
                    refuse(&["sides", "radius_mm", "star_ratio", "corner_radius_mm"], "polygon")?;
                    let (s, r) = (i.sides.unwrap_or(sides), i.radius_mm.unwrap_or(radius_mm));
                    let star = i.star_ratio.unwrap_or(star_ratio);
                    let cr = i.corner_radius_mm.unwrap_or(corner_radius_mm);
                    if let Some(p) = doc::polygon_problem(s, r, star, cr) {
                        return Err(invalid(cmd, p));
                    }
                    Geometry::Polygon { sides: s, radius_mm: r, star_ratio: star, corner_radius_mm: cr }
                }
                _ => {
                    return Err(invalid(
                        cmd,
                        format!(
                            "{} (\"{}\") is a {}; only rectangles, ellipses and polygons have shape parameters",
                            o.id,
                            o.name,
                            o.geometry.kind_name()
                        ),
                    ));
                }
            };
            let json_geometry = serde_json::to_value(&geometry).map_err(|e| CommandError::Failed(e.to_string()))?;
            doc.object_mut(&o.id).ok_or_else(|| CommandError::Failed(format!("object {} vanished", o.id)))?.geometry =
                geometry;
            let mut r = util::edit_result(doc, &[], std::slice::from_ref(&o.id));
            r["geometry"] = json_geometry;
            Ok((r, format!("set {} of {}", fields.join(", "), o.id)))
        })
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectOpenInput {
    /// Only among these object or group ids. Default: every object on the active page.
    pub ids: Option<Vec<ObjectId>>,
}

/// The ids to search: `ids` expanded (group ids stand for their members), or every object.
fn search_ids(cmd: &str, doc: &Document, ids: Option<&[ObjectId]>) -> Result<Vec<ObjectId>> {
    match ids {
        Some(ids) => util::object_ids(cmd, doc, ids),
        None => Ok(doc.objects.iter().map(|o| o.id.clone()).collect()),
    }
}

/// Whether the object's outline has an open subpath (images and empty paths have none).
fn is_open(o: &doc::Object) -> bool {
    !matches!(o.geometry, Geometry::Image { .. })
        && geom::flatten(&o.field_path(), geom::TOLERANCE).is_ok_and(|ls| ls.iter().any(|l| !l.closed))
}

fn on_fill_layer(doc: &Document, o: &doc::Object) -> bool {
    doc.layer(&o.layer).is_some_and(|l| {
        std::iter::once(&l.settings)
            .chain(&l.sub_layers)
            .any(|s| matches!(s.mode, FillMode::Fill | FillMode::FillThenLine | FillMode::OffsetFill))
    })
}

fn ids_result(ids: Vec<ObjectId>) -> (Value, String) {
    let summary = format!("{} shape{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
    (json!({ "ids": ids, "count": ids.len() }), summary)
}

/// `select.open_shapes`.
pub struct SelectOpen;

impl Command for SelectOpen {
    type Input = SelectOpenInput;
    const ID: &'static str = "select.open_shapes";
    const TITLE: &'static str = "Select open shapes";
    const DESCRIPTION: &'static str = "The ids of every vector shape (optionally only among `ids`) with an open \
        subpath, one that does not form a closed loop (LightBurn's Edit > Select Open Shapes). Reads only. Returns \
        `ids` in drawing order and `count`.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: SelectOpenInput) -> Result<(Value, String)> {
        let ids = search_ids(Self::ID, doc, i.ids.as_deref())?;
        let out = ids.into_iter().filter(|id| doc.object(id).is_some_and(is_open)).collect();
        Ok(ids_result(out))
    }
}

/// `select.open_on_fill`.
pub struct SelectOpenOnFill;

impl Command for SelectOpenOnFill {
    type Input = SelectOpenInput;
    const ID: &'static str = "select.open_on_fill";
    const TITLE: &'static str = "Select open shapes set to fill";
    const DESCRIPTION: &'static str = "The ids of open vector shapes on layers that fill (Fill, Fill+Line or Offset \
        Fill on the layer or any sub-layer): they hatch nothing, a common cause of missing fills (LightBurn's Edit > \
        Select Open Shapes Set to Fill). Reads only. Returns `ids` and `count`.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: SelectOpenInput) -> Result<(Value, String)> {
        let ids = search_ids(Self::ID, doc, i.ids.as_deref())?;
        let out =
            ids.into_iter().filter(|id| doc.object(id).is_some_and(|o| is_open(o) && on_fill_layer(doc, o))).collect();
        Ok(ids_result(out))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectContainedInput {
    /// The selected (containing) object or group ids.
    pub ids: Vec<ObjectId>,
}

/// `select.contained`.
pub struct SelectContained;

impl Command for SelectContained {
    type Input = SelectContainedInput;
    const ID: &'static str = "select.contained";
    const TITLE: &'static str = "Select contained shapes";
    const DESCRIPTION: &'static str = "The selected shapes plus every other shape that lies entirely inside the \
        filled area of one of them (closed outlines, even-odd, as jobs fill them; checked on outlines flattened at \
        0.01 mm, vertices and edge midpoints) (LightBurn's Edit > Select Contained Shapes). Reads only. Returns `ids` \
        (the selection first) and `count`.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: SelectContainedInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let sel = util::object_ids(cmd, doc, &i.ids)?;
        let mut containers: Vec<(Rect, Vec<Vec<Point>>)> = Vec::new();
        for id in &sel {
            let o = util::object(doc, id)?;
            if matches!(o.geometry, Geometry::Image { .. }) {
                continue;
            }
            let rings: Vec<Vec<Point>> = geom::flatten(&o.field_path(), geom::TOLERANCE)
                .map_err(super::pathutil::geom_err(cmd))?
                .into_iter()
                .filter(|l| l.closed && l.points.len() >= 3)
                .map(|l| l.points)
                .collect();
            if !rings.is_empty() {
                containers.push((o.bounds(), rings));
            }
        }
        let inside = |p: Point, rings: &[Vec<Point>]| rings.iter().filter(|r| point_in_ring(p, r)).count() % 2 == 1;
        let mut out = sel.clone();
        for o in doc.objects.iter().filter(|o| !sel.contains(&o.id)) {
            let b = o.bounds();
            let Ok(lines) = geom::flatten(&o.field_path(), geom::TOLERANCE) else { continue };
            let pts: Vec<Point> = lines
                .iter()
                .flat_map(|l: &Polyline| {
                    let d = l.drawn();
                    let mids: Vec<Point> = d.windows(2).map(|w| w[0].midpoint(w[1])).collect();
                    d.into_iter().chain(mids)
                })
                .collect();
            if pts.is_empty() {
                continue;
            }
            let held = containers.iter().any(|(cb, rings)| {
                cb.contains(Point::new(b.x0, b.y0))
                    && cb.contains(Point::new(b.x1, b.y1))
                    && pts.iter().all(|p| inside(*p, rings))
            });
            if held {
                out.push(o.id.clone());
            }
        }
        Ok(ids_result(out))
    }
}

/// What `object.move_to_page_position` aligns to.
#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PageReference {
    /// The workspace: the galvo field (or a gantry's bed), centred on the field origin. Default.
    #[default]
    Workspace,
    /// The stock's field-frame bounding box.
    Stock,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PagePositionInput {
    /// Exact object or group ids, moved together as one selection.
    pub ids: Vec<ObjectId>,
    /// Where the selection's bounding box goes: "center", "top_left", "top", "top_right", "left", "right",
    /// "bottom_left", "bottom", "bottom_right" ("top" is +Y, away from the operator).
    pub anchor: Anchor,
    /// Keep this far in from the edges, mm (default 0).
    #[serde(default)]
    pub margin_mm: f64,
    /// "workspace" (default) or "stock".
    #[serde(default)]
    pub reference: PageReference,
}

/// `object.move_to_page_position`.
pub struct MoveToPagePosition;

impl Command for MoveToPagePosition {
    type Input = PagePositionInput;
    const ID: &'static str = "object.move_to_page_position";
    const TITLE: &'static str = "Move to page position";
    const DESCRIPTION: &'static str = "Move the selection so its bounding box sits at a corner, an edge centre or \
        the centre of the workspace (the field or bed; LightBurn's Move Selected Objects window) or of the stock's \
        bounding box, margin_mm in from the edges. Size and rotation are kept. Returns affected ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PagePositionInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            let margin = util::non_negative(cmd, "margin_mm", i.margin_mm)?;
            let page = match i.reference {
                PageReference::Workspace => {
                    let [w, h] = doc
                        .workspace()
                        .ok_or_else(|| CommandError::Failed(format!("{cmd}: the document has no field or bed size")))?;
                    Rect::from_center_size((0.0, 0.0), (w, h))
                }
                PageReference::Stock => doc
                    .stock
                    .as_ref()
                    .ok_or_else(|| CommandError::Failed(format!("{cmd}: no stock is defined")))?
                    .bounds(),
            };
            let page = page.inset(-margin);
            if page.width() < 0.0 || page.height() < 0.0 {
                return Err(invalid(cmd, "the margin is larger than half the page"));
            }
            let b = doc.bounds_of(&ids).ok_or_else(|| CommandError::Failed(format!("{cmd}: nothing to move")))?;
            let (dx, dy) = i.anchor.dir();
            let pick = |d: f64, lo: f64, hi: f64, blo: f64, bhi: f64| match d {
                d if d < 0.0 => lo - blo,
                d if d > 0.0 => hi - bhi,
                _ => (lo + hi) / 2.0 - (blo + bhi) / 2.0,
            };
            let mx = pick(dx, page.x0, page.x1, b.x0, b.x1);
            let my = pick(dy, page.y0, page.y1, b.y0, b.y1);
            apply_affine(doc, &ids, Affine::translate((mx, my)));
            let r = util::edit_result(doc, &[], &ids);
            Ok((r, format!("moved {} by [{}, {}]", ids.join(", "), util::r4(mx), util::r4(my))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuideAddInput {
    /// "x": a vertical guide at x = position_mm; "y": a horizontal guide at y = position_mm.
    pub axis: GuideAxis,
    /// Field-frame position in mm.
    pub position_mm: f64,
}

/// `guide.add`.
pub struct GuideAdd;

impl Command for GuideAdd {
    type Input = GuideAddInput;
    const ID: &'static str = "guide.add";
    const TITLE: &'static str = "Add guide";
    const DESCRIPTION: &'static str = "Add a manual guideline saved with the project (LightBurn's guides dragged out \
        of the rulers, here also placed numerically): axis \"x\" is a vertical line at x = position_mm, \"y\" a \
        horizontal one, in field mm. Guides are drawing and snapping aids only; they never burn or export. Returns \
        the new guide `id` and `guides`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: GuideAddInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let p = guide_position(cmd, i.position_mm)?;
            if doc.guides.len() >= MAX_GUIDES {
                return Err(invalid(cmd, format!("a document keeps at most {MAX_GUIDES} guides")));
            }
            let id = doc.alloc_id("guide")?;
            doc.guides.push(Guide { id: id.clone(), axis: i.axis, position_mm: p });
            let r = json!({ "id": id, "guides": doc.guides });
            Ok((r, format!("added guide {id} at {} = {p}", axis_name(i.axis))))
        })
    }
}

fn axis_name(a: GuideAxis) -> &'static str {
    match a {
        GuideAxis::X => "x",
        GuideAxis::Y => "y",
    }
}

fn guide_position(cmd: &str, v: f64) -> Result<f64> {
    let p = util::finite(cmd, "position_mm", v)?;
    if p.abs() > MAX_GUIDE_MM {
        return Err(invalid(cmd, format!("position_mm must be within ±{MAX_GUIDE_MM}")));
    }
    Ok(p)
}

fn guide_index(cmd: &str, doc: &Document, id: &str) -> Result<usize> {
    doc.guides.iter().position(|g| g.id == id).ok_or_else(|| CommandError::NotFound {
        message: format!(
            "{cmd}: no guide with id '{id}'; guides: {}",
            if doc.guides.is_empty() {
                "none".to_string()
            } else {
                doc.guides.iter().map(|g| g.id.as_str()).collect::<Vec<_>>().join(", ")
            }
        ),
        candidates: doc.guides.iter().map(|g| g.id.clone()).collect(),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuideMoveInput {
    /// Exact guide id.
    pub id: String,
    /// The new position in field mm (along the guide's axis).
    pub position_mm: f64,
    /// Turn it into a vertical ("x") or horizontal ("y") guide. Default: unchanged.
    pub axis: Option<GuideAxis>,
}

/// `guide.move`.
pub struct GuideMove;

impl Command for GuideMove {
    type Input = GuideMoveInput;
    const ID: &'static str = "guide.move";
    const TITLE: &'static str = "Move guide";
    const DESCRIPTION: &'static str = "Set a guide's position (field mm) and optionally its axis. Returns `guides`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: GuideMoveInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let k = guide_index(cmd, doc, &i.id)?;
        let p = guide_position(cmd, i.position_mm)?;
        let g = &mut doc.guides[k];
        g.position_mm = p;
        if let Some(a) = i.axis {
            g.axis = a;
        }
        let summary = format!("moved guide {} to {} = {p}", g.id, axis_name(g.axis));
        Ok((json!({ "guides": doc.guides }), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GuideDeleteInput {
    /// Exact guide ids to delete. Default: every guide.
    pub ids: Option<Vec<String>>,
}

/// `guide.delete`.
pub struct GuideDelete;

impl Command for GuideDelete {
    type Input = GuideDeleteInput;
    const ID: &'static str = "guide.delete";
    const TITLE: &'static str = "Delete guides";
    const DESCRIPTION: &'static str = "Delete guides by id, or every guide when ids is left out. Fails when there is \
        nothing to delete or an id is unknown. Returns `deleted` and the remaining `guides`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: GuideDeleteInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        let ids: Vec<String> = match i.ids {
            Some(ids) => {
                if ids.is_empty() {
                    return Err(invalid(cmd, "ids must list at least one guide id (or be left out for all)"));
                }
                for id in &ids {
                    guide_index(cmd, doc, id)?;
                }
                ids
            }
            None => doc.guides.iter().map(|g| g.id.clone()).collect(),
        };
        if ids.is_empty() {
            return Err(CommandError::NothingTo("delete: the document has no guides"));
        }
        doc.guides.retain(|g| !ids.contains(&g.id));
        let summary = format!("deleted guide{} {}", if ids.len() == 1 { "" } else { "s" }, ids.join(", "));
        Ok((json!({ "deleted": ids, "guides": doc.guides }), summary))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CurveToleranceInput {
    /// Largest distance in mm between a curve and the polyline a job marks for it, 0.001..=1; null goes back to
    /// the device's setting (default 0.01 mm).
    pub tolerance_mm: Option<f64>,
}

/// `doc.set_curve_tolerance`.
pub struct SetCurveTolerance;

impl Command for SetCurveTolerance {
    type Input = CurveToleranceInput;
    const ID: &'static str = "doc.set_curve_tolerance";
    const TITLE: &'static str = "Curve tolerance";
    const DESCRIPTION: &'static str = "Set the document's output curve tolerance (LightBurn's Curve Tolerance): how \
        finely jobs flatten curves, 0.001..=1 mm, saved with the project and used instead of the device's setting; \
        null clears it (the device's setting, else 0.01 mm). Coarser marks faster with fewer points; finer follows \
        curves more closely. Returns `curve_tolerance_mm`.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CurveToleranceInput) -> Result<(Value, String)> {
        let cmd = Self::ID;
        if let Some(t) = i.tolerance_mm {
            util::finite(cmd, "tolerance_mm", t)?;
            if !CURVE_TOLERANCE_RANGE_MM.contains(&t) {
                return Err(invalid(cmd, format!("tolerance_mm must be within 0.001..=1 (got {t})")));
            }
        }
        doc.curve_tolerance_mm = i.tolerance_mm;
        let summary = match i.tolerance_mm {
            Some(t) => format!("curve tolerance {t} mm"),
            None => "curve tolerance from the device".to_string(),
        };
        Ok((json!({ "curve_tolerance_mm": i.tolerance_mm }), summary))
    }
}
