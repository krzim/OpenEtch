//! Arrays and nesting: `art.circular_array`, `art.along_path`, `art.nest`.

use doc::{Document, ObjectId, StockShape};
use geom::measure::Walker;
use geom::nest::{Part, shelf_pack};
use kurbo::{Affine, Point, Rect, Shape, Vec2};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::object::{apply_affine, copy_objects};
use super::pathutil::{self, geom_err};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Most objects one array may create.
const MAX_OBJECTS: usize = 50_000;
/// Most items (original included) in one array.
const MAX_ITEMS: u32 = 10_000;

fn check_size(cmd: &str, copies: usize, members: usize) -> Result<()> {
    if copies.saturating_mul(members) > MAX_OBJECTS {
        return Err(invalid(cmd, format!("that would create more than {MAX_OBJECTS} objects")));
    }
    Ok(())
}

fn yes() -> bool {
    true
}

fn full_turn() -> f64 {
    360.0
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CircularArrayInput {
    /// Exact object or group ids (a group stands for all its members), copied together.
    pub ids: Vec<ObjectId>,
    /// Number of items in the finished array, the original included (2..=10000).
    pub count: u32,
    /// Centre of rotation [x, y] in field mm.
    pub center: [f64; 2],
    /// Total sweep in degrees, counter-clockwise (negative: clockwise), default 360. A full turn spaces the items
    /// evenly around it; a smaller sweep puts the last item at its end.
    #[serde(default = "full_turn")]
    pub angle_deg: f64,
    /// Turn each copy with the circle (default true, like stopwatch ticks); false keeps the original rotation and
    /// only moves the copies round (like clock numerals).
    #[serde(default = "yes")]
    pub rotate_copies: bool,
}

/// `art.circular_array`.
pub struct CircularArray;

impl Command for CircularArray {
    type Input = CircularArrayInput;
    const ID: &'static str = "art.circular_array";
    const TITLE: &'static str = "Circular array";
    const DESCRIPTION: &'static str = "Copy objects around a centre (LightBurn's Circular Array). count is the number \
        of items including the original; angle_deg the total sweep (360 default: evenly round the circle; less: the \
        last copy lands at the end). rotate_copies false keeps each copy upright, moving its bounds centre round. \
        Returns `copies` (the ids of each copy in order), created ids and the bounds of everything.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CircularArrayInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            if !(2..=MAX_ITEMS).contains(&i.count) {
                return Err(invalid(cmd, format!("count must be 2..={MAX_ITEMS}")));
            }
            check_size(cmd, i.count as usize - 1, ids.len())?;
            let [cx, cy] = util::finite2(cmd, "center", i.center)?;
            let sweep = util::finite(cmd, "angle_deg", i.angle_deg)?;
            if sweep == 0.0 || sweep.abs() > 360.0 {
                return Err(invalid(cmd, "angle_deg must be non-zero and within ±360"));
            }
            let c = Point::new(cx, cy);
            let step = if sweep.abs() >= 360.0 - 1e-9 { sweep / f64::from(i.count) } else { sweep / f64::from(i.count - 1) };
            let bc = doc.bounds_of(&ids).map_or(c, |b| b.center());
            let mut copies = Vec::new();
            let mut created = Vec::new();
            for k in 1..i.count {
                let a = (step * f64::from(k)).to_radians();
                let t = if i.rotate_copies {
                    Affine::rotate_about(a, c)
                } else {
                    Affine::translate(Affine::rotate_about(a, c) * bc - bc)
                };
                let new = copy_objects(doc, &ids, Vec2::ZERO, &format!(" {}", k + 1))?;
                apply_affine(doc, &new, t);
                created.extend(new.iter().cloned());
                copies.push(new);
            }
            let mut r = util::edit_result(doc, &created, &ids);
            r["copies"] = json!(copies);
            r["step_deg"] = json!(util::r4(step));
            Ok((r, format!("circular array of {} x{} about [{cx}, {cy}]", ids.join(", "), i.count)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlongPathInput {
    /// Exact object or group ids to copy (not the path), copied together.
    pub ids: Vec<ObjectId>,
    /// Exact id of the object whose outline the copies follow (its longest subpath); it is not changed.
    pub path_id: ObjectId,
    /// Number of items including the original (2..=10000), spread evenly. Give this or spacing_mm.
    pub count: Option<u32>,
    /// Distance in mm along the path between items. Give this or count.
    pub spacing_mm: Option<f64>,
    /// Turn copies with the path's direction (default true); false keeps the original rotation.
    #[serde(default = "yes")]
    pub rotate_to_path: bool,
}

/// `art.along_path`.
pub struct AlongPath;

impl Command for AlongPath {
    type Input = AlongPathInput;
    const ID: &'static str = "art.along_path";
    const TITLE: &'static str = "Copy along path";
    const DESCRIPTION: &'static str = "Copy objects along another object's outline (LightBurn's Copy Along Path). The \
        original is item 0 at the path's start (on a closed path: the point nearest the selection's centre); the \
        selection keeps its offset from that point, turning with the path when rotate_to_path. count spreads items \
        evenly (an open path's last item at its end); spacing_mm places one every so many mm. Returns `copies` per \
        item, created ids and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AlongPathInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let path_obj = pathutil::single_object(cmd, doc, &i.path_id, false)?;
            if ids.contains(&path_obj.id) {
                return Err(invalid(cmd, "the path object cannot also be one of the copied objects"));
            }
            let w = Walker::longest(&pathutil::vector_path(cmd, &path_obj)?).map_err(geom_err(cmd))?;
            let len = w.length();
            let bc = doc.bounds_of(&ids).map_or(Point::ORIGIN, |b| b.center());
            let s0 = if w.closed { w.nearest(bc) } else { 0.0 };
            let positions: Vec<f64> = match (i.count, i.spacing_mm) {
                (Some(n), None) => {
                    if !(2..=MAX_ITEMS).contains(&n) {
                        return Err(invalid(cmd, format!("count must be 2..={MAX_ITEMS}")));
                    }
                    let div = if w.closed { f64::from(n) } else { f64::from(n - 1) };
                    (1..n).map(|k| s0 + len * f64::from(k) / div).collect()
                }
                (None, Some(sp)) => {
                    let sp = util::positive(cmd, "spacing_mm", sp)?;
                    let n = (len / sp).floor();
                    if n > f64::from(MAX_ITEMS) {
                        return Err(invalid(cmd, format!("that spacing makes more than {MAX_ITEMS} items")));
                    }
                    let n = n as u32;
                    (1..=n).map(|k| f64::from(k) * sp).filter(|s| if w.closed { *s < len - 1e-9 } else { *s <= len + 1e-9 }).map(|s| s0 + s).collect()
                }
                _ => return Err(invalid(cmd, "give exactly one of count and spacing_mm")),
            };
            if positions.is_empty() {
                return Err(invalid(cmd, format!("the path ({:.3} mm) is shorter than one spacing", len)));
            }
            check_size(cmd, positions.len(), ids.len())?;
            let (p0, a0) = w.at(s0);
            let mut copies = Vec::new();
            let mut created = Vec::new();
            for (k, s) in positions.iter().enumerate() {
                let (p, a) = w.at(*s);
                let t = if i.rotate_to_path {
                    Affine::translate(p.to_vec2()) * Affine::rotate(a - a0) * Affine::translate(-p0.to_vec2())
                } else {
                    Affine::translate(p - p0)
                };
                let new = copy_objects(doc, &ids, Vec2::ZERO, &format!(" {}", k + 2))?;
                apply_affine(doc, &new, t);
                created.extend(new.iter().cloned());
                copies.push(new);
            }
            let mut r = util::edit_result(doc, &created, &ids);
            r["copies"] = json!(copies);
            r["path_length_mm"] = json!(util::r4(len));
            Ok((r, format!("copied {} {} times along {}", ids.join(", "), copies.len(), path_obj.id)))
        })
    }
}

/// Where `art.nest` packs.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NestRegion {
    /// "stock" (the stock's box; a round stock uses the square inside it) or "field" (the whole field or bed).
    Named(NamedRegion),
    /// A rectangle in field mm.
    Rect {
        /// Lower-left corner [x, y].
        min: [f64; 2],
        /// Upper-right corner [x, y].
        max: [f64; 2],
    },
    /// The bounding box of another object (LightBurn: a target rectangle), which is not moved.
    Object {
        /// Exact object id.
        object: ObjectId,
    },
}

/// A named nesting region.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NamedRegion {
    Stock,
    Field,
}

fn default_spacing() -> f64 {
    1.0
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NestInput {
    /// Exact object or group ids; each object or group is one part (a group keeps its layout).
    pub ids: Vec<ObjectId>,
    /// "stock", "field", {"min": [x,y], "max": [x,y]} or {"object": id}.
    pub region: NestRegion,
    /// Gap between parts in mm (default 1).
    #[serde(default = "default_spacing")]
    pub spacing_mm: f64,
    /// Gap kept from the region's edge in mm (default 0).
    #[serde(default)]
    pub edge_mm: f64,
    /// Rotations (degrees) each part may take, e.g. [0, 90]; at most 8. Default [0].
    pub rotations: Option<Vec<f64>>,
}

/// `art.nest`.
pub struct Nest;

impl Command for Nest {
    type Input = NestInput;
    const ID: &'static str = "art.nest";
    const TITLE: &'static str = "Nest";
    const DESCRIPTION: &'static str = "Pack parts into a region by their bounding boxes (like LightBurn's Quick Nest; \
        greedy shelves from the lower left, tallest first, each part in its listed rotation that fits best). Boxes \
        never interlock, so irregular parts pack loosely; a round stock packs into its inscribed square. Parts that \
        do not fit stay where they are and are listed in `unplaced`. Returns affected ids, `placed` \
        ({id, rotation_deg}) and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: NestInput) -> Result<(Value, String)> {
        transact(doc, |doc| nest(doc, i))
    }
}

fn nest(doc: &mut Document, i: NestInput) -> Result<(Value, String)> {
    let cmd = Nest::ID;
    let spacing = util::non_negative(cmd, "spacing_mm", i.spacing_mm)?;
    let edge = util::non_negative(cmd, "edge_mm", i.edge_mm)?;
    let rotations = i.rotations.unwrap_or_else(|| vec![0.0]);
    if rotations.is_empty() || rotations.len() > 8 {
        return Err(invalid(cmd, "rotations must list 1 to 8 angles"));
    }
    for r in &rotations {
        util::finite(cmd, "rotations", *r)?;
    }
    let (region, exclude): (Rect, Option<ObjectId>) = match &i.region {
        NestRegion::Named(NamedRegion::Field) => {
            let [w, h] = doc.workspace().ok_or_else(|| CommandError::Failed(format!("{cmd}: the document has no field size")))?;
            (Rect::from_center_size((0.0, 0.0), (w, h)), None)
        }
        NestRegion::Named(NamedRegion::Stock) => {
            let s = doc.stock.as_ref().ok_or_else(|| CommandError::Failed(format!("{cmd}: no stock is defined; run stock.define first")))?;
            let r = match s.shape {
                StockShape::Circle { diameter_mm } => {
                    let side = diameter_mm / 2f64.sqrt();
                    Rect::from_center_size((s.center[0], s.center[1]), (side, side))
                }
                StockShape::Rect { .. } => {
                    let q = s.rotation_deg.rem_euclid(90.0);
                    if q > 1e-6 && q < 90.0 - 1e-6 {
                        return Err(invalid(cmd, "the stock is rotated; nest into a rect region instead"));
                    }
                    s.bounds()
                }
            };
            (r, None)
        }
        NestRegion::Rect { min, max } => {
            let [x0, y0] = util::finite2(cmd, "region.min", *min)?;
            let [x1, y1] = util::finite2(cmd, "region.max", *max)?;
            if x1 <= x0 || y1 <= y0 {
                return Err(invalid(cmd, "region max must be above and right of min"));
            }
            (Rect::new(x0, y0, x1, y1), None)
        }
        NestRegion::Object { object } => {
            let o = util::object(doc, object)?;
            (o.bounds(), Some(o.id.clone()))
        }
    };
    let region = region.inset(-edge);
    if region.width() <= 0.0 || region.height() <= 0.0 {
        return Err(invalid(cmd, "the region is smaller than its edge gap"));
    }
    let units: Vec<util::Unit> = util::units(cmd, doc, &i.ids)?
        .into_iter()
        .filter(|u| exclude.as_ref().is_none_or(|x| !u.members.contains(x)))
        .collect();
    if units.is_empty() {
        return Err(invalid(cmd, "nothing to nest"));
    }
    let all: Vec<ObjectId> = units.iter().flat_map(|u| u.members.iter().cloned()).collect();
    util::check_unlocked(cmd, doc, &all)?;
    // Per unit and rotation: the rotated box.
    let mut boxes: Vec<Vec<Rect>> = Vec::new();
    for u in &units {
        let mut per = Vec::new();
        for r in &rotations {
            let rot = Affine::rotate(r.to_radians());
            let b = u
                .members
                .iter()
                .filter_map(|m| doc.object(m))
                .map(|o| (rot * o.field_path()).bounding_box())
                .reduce(|a, b| a.union(b))
                .unwrap_or_default();
            per.push(b);
        }
        boxes.push(per);
    }
    let parts: Vec<Part> = boxes.iter().map(|bs| Part { sizes: bs.iter().map(|b| (b.width(), b.height())).collect() }).collect();
    let placed = shelf_pack(&parts, region, spacing);
    let mut affected = Vec::new();
    let mut report = Vec::new();
    let mut unplaced = Vec::new();
    for ((u, p), bs) in units.iter().zip(&placed).zip(&boxes) {
        match p {
            Some(p) => {
                let b = bs[p.orientation];
                let t = Affine::translate(p.min - b.origin()) * Affine::rotate(rotations[p.orientation].to_radians());
                apply_affine(doc, &u.members, t);
                affected.extend(u.members.iter().cloned());
                report.push(json!({ "id": u.id, "rotation_deg": rotations[p.orientation] }));
            }
            None => unplaced.push(u.id.clone()),
        }
    }
    if affected.is_empty() {
        return Err(CommandError::Failed(format!("{cmd}: no part fits in the region")));
    }
    let mut r = util::edit_result(doc, &[], &affected);
    r["placed"] = json!(report);
    r["unplaced"] = json!(unplaced);
    Ok((r, format!("nested {} parts ({} did not fit)", report.len(), unplaced.len())))
}
