//! Jig commands: a saved fixture with numbered pockets, a datum, the material height and keep-out walls
//! (`doc::jig`), made as a grid (`jig.create`), from drawn or imported shapes (`jig.from_objects`, `jig.import` for a
//! DXF or SVG drawing of the jig), edited (`jig.edit`, `jig.set_pockets`, `jig.add_keep_out`, `jig.remove_keep_out`),
//! moved as a whole (`jig.move`), deleted (`jig.delete`) and listed (`jig.list`).
//!
//! `jig.place` puts a design into every enabled pocket as a virtual array (linked clones, `array.rs`): the originals
//! sit in the first enabled pocket and every other enabled pocket gets a clone that follows each edit of them. A
//! pocket switched off has no clone and no serial number (the variable-text offsets count the enabled pockets only),
//! so empty pockets drop out of the job and of the serial sequence. Moving the jig moves the designs placed in it.

use std::collections::{BTreeSet, HashSet};

use doc::jig::{JIG_POCKET_KEY, Jig, KeepOut, MAX_JIGS, MAX_KEEP_OUTS, MAX_POCKETS, Pocket, PocketShape};
use doc::{Document, ObjectId, VirtualArray};
use kurbo::{Affine, BezPath, Point, Rect, Shape};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use super::super::array::Placed;
use super::super::object::apply_affine;
use super::super::pathutil::{delete_objects, geom_err};
use super::super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// The settings of a virtual array that is a jig placement (`Document::virtual_arrays`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Placement {
    /// The jig's id.
    jig: String,
    /// The pocket the originals sit in (the first enabled one).
    source_pocket: String,
    /// Variable text (serial numbers, CSV rows) counts on by this much per enabled pocket.
    #[serde(default)]
    auto_increment: i64,
}

fn placement_of(a: &VirtualArray) -> Option<Placement> {
    a.settings.get("jig")?;
    serde_json::from_value(a.settings.clone()).ok()
}

/// Refuse the grid-array commands (Edit Array, Duplicate Array) on a jig placement.
pub(crate) fn refuse_placement(cmd: &str, a: &VirtualArray) -> Result<()> {
    match placement_of(a) {
        Some(p) => Err(invalid(cmd, format!("{} places a design in the pockets of jig {}; change the jig instead (jig.set_pockets, jig.move)", a.id, p.jig))),
        None => Ok(()),
    }
}

/// The clones of a jig placement, pocket by pocket (`None` for a grid array): every enabled pocket but the
/// originals' own, in number order. A jig that no longer exists places nothing.
pub(crate) fn placement_cells(doc: &Document, a: &VirtualArray) -> Result<Option<Vec<Placed>>> {
    if a.settings.get("jig").is_none() {
        return Ok(None);
    }
    let p: Placement = serde_json::from_value(a.settings.clone())
        .map_err(|e| CommandError::Failed(format!("array.sync: jig placement {}: bad settings ({e})", a.id)))?;
    let Some(jig) = doc.jigs.iter().find(|j| j.id == p.jig) else { return Ok(Some(Vec::new())) };
    let Some(src) = jig.pocket(&p.source_pocket) else { return Ok(Some(Vec::new())) };
    let from = jig.pocket_transform(src).inverse();
    let order = jig.enabled_pockets();
    let src_order = order.iter().position(|q| q.id == src.id).unwrap_or(0) as i64;
    let mut out = Vec::new();
    for (n, q) in order.iter().enumerate() {
        if q.id == src.id {
            continue;
        }
        out.push(Placed {
            label: format!("[P{}]", q.number),
            delta: (n as i64 - src_order).saturating_mul(p.auto_increment),
            t: jig.pocket_transform(q) * from,
            tag: Some(format!("{}/{}", jig.id, q.id)),
        });
    }
    Ok(Some(out))
}

fn jig_index(cmd: &str, doc: &Document, id: &str) -> Result<usize> {
    doc.jigs.iter().position(|j| j.id == id || j.name == id).ok_or_else(|| CommandError::NotFound {
        message: format!("{cmd}: no jig {id}; jig.list lists them"),
        candidates: doc.jigs.iter().map(|j| j.id.clone()).collect(),
    })
}

/// Virtual arrays placing designs in jig `jig`, by index.
fn placements(doc: &Document, jig: &str) -> Vec<(usize, Placement)> {
    doc.virtual_arrays.iter().enumerate().filter_map(|(k, a)| placement_of(a).filter(|p| p.jig == jig).map(|p| (k, p))).collect()
}

/// Keep every placement's originals in the jig's first enabled pocket: when that pocket changed (switched off, or an
/// earlier one switched on), the originals move along with the clones' layout. Locked originals refuse.
fn settle_sources(cmd: &str, doc: &mut Document, jig_idx: usize) -> Result<()> {
    let jig = doc.jigs[jig_idx].clone();
    let Some(first) = jig.enabled_pockets().first().map(|p| (*p).clone()) else {
        return Err(invalid(cmd, "at least one pocket must stay on"));
    };
    for (k, p) in placements(doc, &jig.id) {
        if p.source_pocket == first.id {
            continue;
        }
        let old = jig.pocket(&p.source_pocket).cloned().unwrap_or_else(|| first.clone());
        let m = jig.pocket_transform(&first) * jig.pocket_transform(&old).inverse();
        let sources = doc.virtual_arrays[k].sources.clone();
        util::check_unlocked(cmd, doc, &sources)?;
        apply_affine(doc, &sources, m);
        let settings = Placement { source_pocket: first.id.clone(), ..p };
        doc.virtual_arrays[k].settings = serde_json::to_value(&settings).map_err(|e| CommandError::Failed(e.to_string()))?;
    }
    Ok(())
}

/// How pockets are numbered (the burn and serial order).
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Numbering {
    /// Row by row from the far row (+Y) toward the operator, each left to right (reading order; default).
    #[default]
    Rows,
    /// Column by column from the left, each from the far end toward the operator.
    Columns,
    /// Row by row, alternating direction (left to right, then right to left), the shortest travel.
    Serpentine,
}

/// Number `pockets` 1..=n in `mode` order (jig frame); ids stay.
fn renumber(pockets: &mut [Pocket], mode: Numbering) {
    if pockets.is_empty() {
        return;
    }
    let size = |p: &Pocket| {
        let b = p.shape.local_path().bounding_box();
        b.width().min(b.height())
    };
    let tol = pockets.iter().map(size).fold(f64::INFINITY, f64::min).max(1e-6) / 2.0;
    let mut idx: Vec<usize> = (0..pockets.len()).collect();
    type Key = fn(&Pocket) -> f64;
    let (major, minor): (Key, Key) = match mode {
        Numbering::Columns => (|p| p.center[0], |p| -p.center[1]),
        Numbering::Rows | Numbering::Serpentine => (|p| -p.center[1], |p| p.center[0]),
    };
    idx.sort_by(|&a, &b| major(&pockets[a]).total_cmp(&major(&pockets[b])));
    // Group into rows (or columns) whose major coordinate is within half the smallest pocket.
    let mut lines: Vec<Vec<usize>> = Vec::new();
    for i in idx {
        match lines.last_mut() {
            Some(l) if (major(&pockets[i]) - major(&pockets[l[0]])).abs() <= tol => l.push(i),
            _ => lines.push(vec![i]),
        }
    }
    let mut n = 1;
    for (r, mut l) in lines.into_iter().enumerate() {
        l.sort_by(|&a, &b| minor(&pockets[a]).total_cmp(&minor(&pockets[b])));
        if mode == Numbering::Serpentine && r % 2 == 1 {
            l.reverse();
        }
        for i in l {
            pockets[i].number = n;
            n += 1;
        }
    }
    pockets.sort_by_key(|p| p.number);
}

/// A new jig's pocket ids follow their first numbering ("p1" is pocket 1); later renumberings keep the ids.
fn ids_by_number(pockets: &mut [Pocket]) {
    for p in pockets {
        p.id = format!("p{}", p.number);
    }
}

/// The next unused "p<n>" / "k<n>" id in a jig.
fn next_id(prefix: &str, used: impl Iterator<Item = String>) -> String {
    let n = used.filter_map(|id| id.strip_prefix(prefix).and_then(|s| s.parse::<u64>().ok())).max().unwrap_or(0);
    format!("{prefix}{}", n + 1)
}

/// The shape of a pocket outline: a rectangle or circle when it is one (jig frame axes), else its path.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PocketShapeInput {
    /// A rectangular pocket, optionally with rounded corners.
    Rect {
        width_mm: f64,
        height_mm: f64,
        #[serde(default)]
        corner_radius_mm: f64,
    },
    /// A round pocket (a coin).
    Circle { diameter_mm: f64 },
}

fn pocket_shape(cmd: &str, s: &PocketShapeInput) -> Result<PocketShape> {
    Ok(match *s {
        PocketShapeInput::Rect { width_mm, height_mm, corner_radius_mm } => {
            util::positive(cmd, "pocket.width_mm", width_mm)?;
            util::positive(cmd, "pocket.height_mm", height_mm)?;
            util::non_negative(cmd, "pocket.corner_radius_mm", corner_radius_mm)?;
            if corner_radius_mm > width_mm.min(height_mm) / 2.0 {
                return Err(invalid(cmd, "pocket.corner_radius_mm must be at most half the shorter side"));
            }
            PocketShape::Rect { width_mm, height_mm, corner_radius_mm }
        }
        PocketShapeInput::Circle { diameter_mm } => PocketShape::Circle { diameter_mm: util::positive(cmd, "pocket.diameter_mm", diameter_mm)? },
    })
}

fn check_jig(cmd: &str, jig: &Jig) -> Result<()> {
    match jig.problem() {
        Some(p) => Err(invalid(cmd, p)),
        None => Ok(()),
    }
}

/// Add `jig` to the document and answer with its summary.
fn add_jig(cmd: &str, doc: &mut Document, jig: Jig) -> Result<Value> {
    if doc.jigs.len() >= MAX_JIGS {
        return Err(invalid(cmd, format!("a document holds at most {MAX_JIGS} jigs")));
    }
    check_jig(cmd, &jig)?;
    let v = jig_json(doc, &jig);
    doc.jigs.push(jig);
    Ok(v)
}

fn default_name(doc: &Document) -> String {
    format!("Jig {}", doc.jigs.len() + 1)
}

fn yes() -> bool {
    true
}

fn one() -> i64 {
    1
}

// ---------------------------------------------------------------------------------------------------------------
// jig.create
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// Display name (default "Jig n").
    pub name: Option<String>,
    /// Pocket rows (along field Y), >= 1.
    pub rows: u32,
    /// Pocket columns (along field X), >= 1.
    pub cols: u32,
    /// Centre-to-centre distance between columns, mm (> 0 when cols > 1).
    #[serde(default)]
    pub pitch_x_mm: f64,
    /// Centre-to-centre distance between rows, mm (> 0 when rows > 1).
    #[serde(default)]
    pub pitch_y_mm: f64,
    /// Every pocket's outline: {"kind": "circle", "diameter_mm"} or {"kind": "rect", "width_mm", "height_mm",
    /// "corner_radius_mm"?}.
    pub pocket: PocketShapeInput,
    /// Where the grid's centre (the jig's datum) goes, field mm. Default: the stock's centre, else [0, 0].
    pub center: Option<[f64; 2]>,
    /// Height of the blanks' top surface, mm (recorded for the operator).
    pub material_height_mm: Option<f64>,
    /// Pocket numbering: "rows" (default), "columns" or "serpentine".
    #[serde(default)]
    pub numbering: Numbering,
}

/// `jig.create`.
pub struct JigCreate;

impl Command for JigCreate {
    type Input = CreateInput;
    const ID: &'static str = "jig.create";
    const TITLE: &'static str = "New jig";
    const DESCRIPTION: &'static str = "Add a jig of rows x cols identical pockets (circle or rect) at pitch_x/y_mm \
        centre to centre, its datum at the grid's centre (center, default the stock's centre or the field's), with \
        an optional material height. Pockets are numbered by `numbering` (rows from the far side, left to right, by \
        default): the burn and serial-number order. Returns the jig (id, pockets with ids and numbers, bounds).";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: CreateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if i.rows == 0 || i.cols == 0 {
                return Err(invalid(cmd, "rows and cols must be >= 1"));
            }
            if (i.rows as usize).saturating_mul(i.cols as usize) > MAX_POCKETS {
                return Err(invalid(cmd, format!("a jig holds at most {MAX_POCKETS} pockets")));
            }
            let shape = pocket_shape(cmd, &i.pocket)?;
            let px = if i.cols > 1 { util::positive(cmd, "pitch_x_mm", i.pitch_x_mm)? } else { 0.0 };
            let py = if i.rows > 1 { util::positive(cmd, "pitch_y_mm", i.pitch_y_mm)? } else { 0.0 };
            let size = shape.local_path().bounding_box();
            if (i.cols > 1 && px < size.width() - 1e-9) || (i.rows > 1 && py < size.height() - 1e-9) {
                return Err(invalid(cmd, "the pitch is smaller than a pocket: pockets would overlap"));
            }
            let center = match i.center {
                Some(c) => util::finite2(cmd, "center", c)?,
                None => doc.stock.as_ref().map_or([0.0, 0.0], |s| s.center),
            };
            if let Some(h) = i.material_height_mm {
                util::non_negative(cmd, "material_height_mm", h)?;
            }
            let mut pockets = Vec::new();
            let (w, h) = (f64::from(i.cols - 1) * px, f64::from(i.rows - 1) * py);
            for r in 0..i.rows {
                for c in 0..i.cols {
                    let n = pockets.len() + 1;
                    pockets.push(Pocket {
                        id: format!("p{n}"),
                        number: n as u32,
                        center: [f64::from(c) * px - w / 2.0, f64::from(r) * py - h / 2.0],
                        rotation_deg: 0.0,
                        shape: shape.clone(),
                        enabled: true,
                    });
                }
            }
            renumber(&mut pockets, i.numbering);
            ids_by_number(&mut pockets);
            let name = match &i.name {
                Some(n) => util::name(cmd, "name", n)?,
                None => default_name(doc),
            };
            let id = doc.alloc_id("jig")?;
            let jig = Jig { id: id.clone(), name, datum: center, rotation_deg: 0.0, material_height_mm: i.material_height_mm, pockets, keep_out: vec![], outline: None };
            let n = jig.pockets.len();
            let v = add_jig(cmd, doc, jig)?;
            Ok((json!({ "created": [], "affected": [], "jig": v, "bounds": v["bounds"].clone() }), format!("added jig {id} ({n} pockets)")))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// jig.from_objects / jig.import
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FromObjectsInput {
    /// Exact object or group ids whose closed outlines become pockets. The largest outline that holds every other
    /// one becomes the jig plate's outline instead (unless plate is false).
    pub ids: Vec<ObjectId>,
    /// Objects whose closed outlines become keep-out zones (walls, clamps, pins). Default none.
    #[serde(default)]
    pub keep_out_ids: Vec<ObjectId>,
    /// Display name (default "Jig n").
    pub name: Option<String>,
    /// Detect the plate outline (default true).
    #[serde(default = "yes")]
    pub plate: bool,
    /// Delete the source objects once they are part of the jig (default true).
    #[serde(default = "yes")]
    pub delete_sources: bool,
    /// Pocket numbering: "rows" (default), "columns" or "serpentine".
    #[serde(default)]
    pub numbering: Numbering,
    /// Height of the blanks' top surface, mm.
    pub material_height_mm: Option<f64>,
}

/// One closed outline (field frame) of an object, as a path.
fn closed_outlines(cmd: &str, doc: &Document, ids: &[ObjectId]) -> Result<Vec<BezPath>> {
    let mut out = Vec::new();
    for id in ids {
        let o = util::object(doc, id)?;
        if matches!(o.geometry, doc::Geometry::Image { .. }) {
            return Err(invalid(cmd, format!("{id} is an image; draw or import the jig as vector outlines")));
        }
        for mut sp in geom::split(&o.field_path()) {
            // An outline that ends where it starts is closed even without a ClosePath (kurbo's ellipses).
            if !sp.closed && sp.segs.len() > 1 && sp.start().distance(sp.end()) <= 1e-6 {
                sp.closed = true;
            }
            if sp.closed {
                out.push(geom::join(std::slice::from_ref(&sp)));
            }
        }
    }
    Ok(out)
}

/// A pocket's shape and centre from a field-frame outline: a circle or an axis-aligned rectangle when it is one,
/// else the outline itself around its bounding box's centre.
fn pocket_from_outline(cmd: &str, path: &BezPath) -> Result<(Point, PocketShape)> {
    let b = path.bounding_box();
    let c = b.center();
    let ring: Vec<Point> = geom::flatten(path, 0.005).map_err(geom_err(cmd))?.into_iter().flat_map(|l| l.points).collect();
    if ring.len() >= 8 {
        let d: Vec<f64> = ring.iter().map(|p| p.distance(c)).collect();
        let mean = d.iter().sum::<f64>() / d.len() as f64;
        if mean > 0.0 && d.iter().all(|r| (r - mean).abs() <= mean * 0.005) && (b.width() - b.height()).abs() <= mean * 0.01 {
            return Ok((c, PocketShape::Circle { diameter_mm: b.width().max(b.height()) }));
        }
    }
    let on_box = |p: &Point| (p.x - b.x0).abs() < 1e-6 || (p.x - b.x1).abs() < 1e-6 || (p.y - b.y0).abs() < 1e-6 || (p.y - b.y1).abs() < 1e-6;
    let plain_lines = path.elements().iter().all(|e| !matches!(e, kurbo::PathEl::QuadTo(..) | kurbo::PathEl::CurveTo(..)));
    if plain_lines && ring.len() == 4 && ring.iter().all(on_box) {
        return Ok((c, PocketShape::Rect { width_mm: b.width(), height_mm: b.height(), corner_radius_mm: 0.0 }));
    }
    Ok((c, PocketShape::Path { path: Affine::translate(-c.to_vec2()) * path.clone() }))
}

/// Build a jig from field-frame outlines: pockets (one plate outline among them, when asked) and keep-outs. The
/// datum is the centre of the pockets' bounds.
/// How [`jig_from_outlines`] builds a jig.
struct Build<'a> {
    /// Detect the plate outline.
    plate: bool,
    name: Option<&'a str>,
    numbering: Numbering,
    height: Option<f64>,
}

fn jig_from_outlines(cmd: &str, doc: &mut Document, pockets_in: Vec<BezPath>, keep: Vec<BezPath>, b: Build) -> Result<Jig> {
    let Build { plate, name, numbering, height } = b;
    let mut outlines = pockets_in;
    let mut plate_path = None;
    if plate && outlines.len() > 2 {
        // The largest outline that holds the centre of every other one.
        let (k, _) = outlines
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.bounding_box().area().total_cmp(&b.1.bounding_box().area()))
            .expect("non-empty");
        let big = outlines[k].bounding_box();
        if outlines.iter().enumerate().all(|(j, o)| j == k || big.contains(o.bounding_box().center()) && o.bounding_box().area() < big.area()) {
            plate_path = Some(outlines.remove(k));
        }
    }
    if outlines.is_empty() {
        return Err(invalid(cmd, "no closed outline to make a pocket from (pockets must be closed shapes)"));
    }
    if outlines.len() > MAX_POCKETS {
        return Err(invalid(cmd, format!("{} pockets: a jig holds at most {MAX_POCKETS}", outlines.len())));
    }
    if keep.len() > MAX_KEEP_OUTS {
        return Err(invalid(cmd, format!("{} keep-out zones: a jig holds at most {MAX_KEEP_OUTS}", keep.len())));
    }
    let mut placed = Vec::new();
    for o in &outlines {
        placed.push(pocket_from_outline(cmd, o)?);
    }
    let datum = placed.iter().map(|(c, s)| (Affine::translate(c.to_vec2()) * s.local_path()).bounding_box()).reduce(|a, b| a.union(b)).expect("non-empty").center();
    let to_jig = Affine::translate(-datum.to_vec2());
    let mut pockets: Vec<Pocket> = placed
        .into_iter()
        .enumerate()
        .map(|(n, (c, shape))| Pocket { id: format!("p{}", n + 1), number: n as u32 + 1, center: [c.x - datum.x, c.y - datum.y], rotation_deg: 0.0, shape, enabled: true })
        .collect();
    renumber(&mut pockets, numbering);
    ids_by_number(&mut pockets);
    let keep_out = keep
        .into_iter()
        .enumerate()
        .map(|(n, p)| KeepOut { id: format!("k{}", n + 1), name: format!("Wall {}", n + 1), path: to_jig * p, enabled: true })
        .collect();
    if let Some(h) = height {
        util::non_negative(cmd, "material_height_mm", h)?;
    }
    let name = match name {
        Some(n) => util::name(cmd, "name", n)?,
        None => default_name(doc),
    };
    Ok(Jig {
        id: doc.alloc_id("jig")?,
        name,
        datum: [datum.x, datum.y],
        rotation_deg: 0.0,
        material_height_mm: height,
        pockets,
        keep_out,
        outline: plate_path.map(|p| to_jig * p),
    })
}

/// `jig.from_objects`.
pub struct JigFromObjects;

impl Command for JigFromObjects {
    type Input = FromObjectsInput;
    const ID: &'static str = "jig.from_objects";
    const TITLE: &'static str = "Make jig from shapes";
    const DESCRIPTION: &'static str = "Turn drawn or imported shapes into a jig: every closed outline of `ids` becomes \
        a pocket (circles and axis-aligned rectangles are recognised, anything else keeps its outline), the largest \
        outline holding all the others becomes the plate (plate: false keeps it a pocket), keep_out_ids become \
        keep-out zones. The datum is the pockets' centre; numbering as jig.create. The source objects are deleted \
        unless delete_sources is false. Returns the jig and deleted ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: FromObjectsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let keep_ids = if i.keep_out_ids.is_empty() { vec![] } else { util::object_ids(cmd, doc, &i.keep_out_ids)? };
            if let Some(x) = keep_ids.iter().find(|k| ids.contains(k)) {
                return Err(invalid(cmd, format!("{x} is listed both as a pocket and as a keep-out zone")));
            }
            let outlines = closed_outlines(cmd, doc, &ids)?;
            let keep = closed_outlines(cmd, doc, &keep_ids)?;
            let jig = jig_from_outlines(cmd, doc, outlines, keep, Build { plate: i.plate, name: i.name.as_deref(), numbering: i.numbering, height: i.material_height_mm })?;
            let mut deleted: Vec<ObjectId> = Vec::new();
            if i.delete_sources {
                deleted = ids.iter().chain(&keep_ids).cloned().collect();
                util::check_unlocked(cmd, doc, &deleted)?;
                if let Some(x) = deleted.iter().find(|d| doc.virtual_arrays.iter().any(|a| a.sources.contains(*d) || a.clones.contains(*d))) {
                    return Err(invalid(cmd, format!("{x} belongs to a virtual array; flatten it first or keep the sources")));
                }
                delete_objects(doc, &deleted);
            }
            let (id, n) = (jig.id.clone(), jig.pockets.len());
            let v = add_jig(cmd, doc, jig)?;
            Ok((json!({ "created": [], "affected": [], "deleted": deleted, "jig": v, "bounds": v["bounds"].clone() }), format!("made jig {id} ({n} pockets) from shapes")))
        })
    }
}

/// File formats `jig.import` reads.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JigFormat {
    Svg,
    /// Native builds only.
    Dxf,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportInput {
    /// The file's contents as text.
    pub content: String,
    /// "svg" or "dxf".
    pub format: JigFormat,
    /// Display name (default "Jig n").
    pub name: Option<String>,
    /// Where the jig's datum (the pockets' centre) goes, field mm. Default: where the drawing puts it (a DXF's own
    /// coordinates with keep_position, else centred on the field).
    pub center: Option<[f64; 2]>,
    /// DXF: keep the drawing's coordinates (its origin on the field centre). Default false.
    #[serde(default)]
    pub keep_position: bool,
    /// Detect the plate outline (default true).
    #[serde(default = "yes")]
    pub plate: bool,
    /// Pocket numbering: "rows" (default), "columns" or "serpentine".
    #[serde(default)]
    pub numbering: Numbering,
    /// Height of the blanks' top surface, mm.
    pub material_height_mm: Option<f64>,
}

/// `jig.import`.
pub struct JigImport;

impl Command for JigImport {
    type Input = ImportInput;
    const ID: &'static str = "jig.import";
    const TITLE: &'static str = "Import jig";
    const DESCRIPTION: &'static str = "Make a jig from a DXF or SVG drawing of it (content as text): every closed \
        outline becomes a pocket, the largest one holding the rest the plate (as jig.from_objects); nothing is added \
        to the design. center moves the jig's datum (the pockets' centre); DXF keep_position keeps the drawing's \
        coordinates. Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ImportInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let r = match i.format {
                JigFormat::Svg => super::super::svg_import::ImportSvg::apply(doc, from_json(cmd, json!({ "svg": i.content, "split": true, "name": "Jig drawing" }))?)?,
                JigFormat::Dxf => super::super::dxf::ImportDxf::apply(
                    doc,
                    from_json(cmd, json!({ "content": i.content, "split": true, "keep_position": i.keep_position, "name": "Jig drawing" }))?,
                )?,
            };
            let created: Vec<ObjectId> = r.0["created"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
            if created.is_empty() {
                return Err(invalid(cmd, "the drawing has no shapes"));
            }
            let outlines = closed_outlines(cmd, doc, &created)?;
            delete_objects(doc, &created);
            let mut jig = jig_from_outlines(cmd, doc, outlines, vec![], Build { plate: i.plate, name: i.name.as_deref(), numbering: i.numbering, height: i.material_height_mm })?;
            if let Some(c) = i.center {
                jig.datum = util::finite2(cmd, "center", c)?;
            }
            let (id, n) = (jig.id.clone(), jig.pockets.len());
            let v = add_jig(cmd, doc, jig)?;
            Ok((json!({ "created": [], "affected": [], "jig": v, "bounds": v["bounds"].clone() }), format!("imported jig {id} ({n} pockets)")))
        })
    }
}

fn from_json<T: serde::de::DeserializeOwned>(cmd: &str, v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| invalid(cmd, e.to_string()))
}

// ---------------------------------------------------------------------------------------------------------------
// jig.edit / jig.move / jig.set_pockets / keep-outs / jig.delete
// ---------------------------------------------------------------------------------------------------------------

/// `Some(None)` for an explicit JSON null, `Some(Some(v))` for a value, `None` when the field is absent.
fn double_option<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Option<Option<f64>>, D::Error> {
    Option::<f64>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// A new display name.
    pub name: Option<String>,
    /// Height of the blanks' top surface, mm; null clears it.
    #[serde(default, deserialize_with = "double_option")]
    pub material_height_mm: Option<Option<f64>>,
    /// Number the pockets again: "rows", "columns" or "serpentine" (pocket ids stay).
    pub numbering: Option<Numbering>,
}

/// `jig.edit`.
pub struct JigEdit;

impl Command for JigEdit {
    type Input = EditInput;
    const ID: &'static str = "jig.edit";
    const TITLE: &'static str = "Edit jig";
    const DESCRIPTION: &'static str = "Rename a jig, set or clear (null) its material height, or number its pockets \
        again (numbering rows | columns | serpentine; pocket ids stay, so per-slot records keep pointing at the same \
        pocket). Designs placed in it follow the new order. Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: EditInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let mut parts = Vec::new();
            if let Some(n) = &i.name {
                doc.jigs[k].name = util::name(cmd, "name", n)?;
                parts.push("renamed");
            }
            if let Some(h) = i.material_height_mm {
                if let Some(h) = h {
                    util::non_negative(cmd, "material_height_mm", h)?;
                }
                doc.jigs[k].material_height_mm = h;
                parts.push("material height");
            }
            if let Some(m) = i.numbering {
                renumber(&mut doc.jigs[k].pockets, m);
                settle_sources(cmd, doc, k)?;
                parts.push("renumbered");
            }
            if parts.is_empty() {
                return Err(invalid(cmd, "nothing to do: give name, material_height_mm or numbering"));
            }
            check_jig(cmd, &doc.jigs[k])?;
            let v = jig_json(doc, &doc.jigs[k]);
            Ok((json!({ "created": [], "affected": [], "jig": v, "bounds": v["bounds"].clone() }), format!("jig {}: {}", doc.jigs[k].id, parts.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// Put the datum here, field mm. Not with translate.
    pub datum: Option<[f64; 2]>,
    /// Move by [dx, dy] mm. Not with datum.
    pub translate: Option<[f64; 2]>,
    /// Turn to this angle about the datum, degrees counter-clockwise. Not with rotate_by_deg.
    pub rotation_deg: Option<f64>,
    /// Turn by this many degrees about the datum. Not with rotation_deg.
    pub rotate_by_deg: Option<f64>,
    /// Move the designs placed in the jig along (default true).
    #[serde(default = "yes")]
    pub move_designs: bool,
}

/// `jig.move`.
pub struct JigMove;

impl Command for JigMove {
    type Input = MoveInput;
    const ID: &'static str = "jig.move";
    const TITLE: &'static str = "Move jig";
    const DESCRIPTION: &'static str = "Move the whole jig (pockets, walls, plate) to where it sits on the machine: \
        datum [x, y] or translate [dx, dy] in field mm, rotation_deg (absolute) or rotate_by_deg, about the datum. \
        Designs placed in its pockets move with it (move_designs false leaves them). Locked placed designs refuse. \
        Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: MoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            if i.datum.is_some() && i.translate.is_some() {
                return Err(invalid(cmd, "give datum or translate, not both"));
            }
            if i.rotation_deg.is_some() && i.rotate_by_deg.is_some() {
                return Err(invalid(cmd, "give rotation_deg or rotate_by_deg, not both"));
            }
            let before = doc.jigs[k].transform();
            let j = &mut doc.jigs[k];
            if let Some(d) = i.datum {
                j.datum = util::finite2(cmd, "datum", d)?;
            }
            if let Some(t) = i.translate {
                let [dx, dy] = util::finite2(cmd, "translate", t)?;
                j.datum = [j.datum[0] + dx, j.datum[1] + dy];
            }
            if let Some(r) = i.rotation_deg {
                j.rotation_deg = util::finite(cmd, "rotation_deg", r)? % 360.0;
            }
            if let Some(r) = i.rotate_by_deg {
                j.rotation_deg = (j.rotation_deg + util::finite(cmd, "rotate_by_deg", r)?) % 360.0;
            }
            if i.datum.is_none() && i.translate.is_none() && i.rotation_deg.is_none() && i.rotate_by_deg.is_none() {
                return Err(invalid(cmd, "nothing to do: give datum, translate, rotation_deg or rotate_by_deg"));
            }
            check_jig(cmd, j)?;
            let m = j.transform() * before.inverse();
            let id = j.id.clone();
            let mut moved: Vec<ObjectId> = Vec::new();
            if i.move_designs {
                for (a, _) in placements(doc, &id) {
                    moved.extend(doc.virtual_arrays[a].sources.iter().cloned());
                }
                util::check_unlocked(cmd, doc, &moved)?;
                apply_affine(doc, &moved, m);
            }
            let v = jig_json(doc, &doc.jigs[k]);
            Ok((json!({ "created": [], "affected": moved, "jig": v, "bounds": v["bounds"].clone() }), format!("moved jig {id}")))
        })
    }
}

/// A pocket named by its id ("p3") or its number (3).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PocketRef {
    Number(u32),
    Id(String),
}

fn pocket_ids(cmd: &str, jig: &Jig, refs: &[PocketRef]) -> Result<Vec<String>> {
    refs.iter()
        .map(|r| {
            let p = match r {
                PocketRef::Number(n) => jig.pockets.iter().find(|p| p.number == *n),
                PocketRef::Id(s) => jig.pockets.iter().find(|p| p.id == *s || s.trim_start_matches(['P', 'p', '#']).parse::<u32>().ok() == Some(p.number)),
            };
            p.map(|p| p.id.clone()).ok_or_else(|| invalid(cmd, format!("jig {} has no pocket {}", jig.id, match r {
                PocketRef::Number(n) => n.to_string(),
                PocketRef::Id(s) => s.clone(),
            })))
        })
        .collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPocketsInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// Pockets by number (3) or id ("p3"). Leave out with all.
    #[serde(default)]
    pub pockets: Vec<PocketRef>,
    /// Every pocket of the jig.
    #[serde(default)]
    pub all: bool,
    /// On (marked) or off (skipped: empty or damaged; no copy of the design, no serial number).
    pub enabled: bool,
}

/// `jig.set_pockets`.
pub struct JigSetPockets;

impl Command for JigSetPockets {
    type Input = SetPocketsInput;
    const ID: &'static str = "jig.set_pockets";
    const TITLE: &'static str = "Switch pockets on or off";
    const DESCRIPTION: &'static str = "Switch jig pockets on or off (by number or id, or all). An off pocket gets no \
        copy of the placed designs and no serial number: the enabled pockets count serials consecutively in number \
        order. At least one pocket stays on; the designs' originals move to the first enabled pocket when theirs \
        goes off. Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: SetPocketsInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let ids: BTreeSet<String> = if i.all {
                if !i.pockets.is_empty() {
                    return Err(invalid(cmd, "give pockets or all, not both"));
                }
                doc.jigs[k].pockets.iter().map(|p| p.id.clone()).collect()
            } else {
                if i.pockets.is_empty() {
                    return Err(invalid(cmd, "pockets must name at least one pocket (or give all: true)"));
                }
                pocket_ids(cmd, &doc.jigs[k], &i.pockets)?.into_iter().collect()
            };
            for p in doc.jigs[k].pockets.iter_mut().filter(|p| ids.contains(&p.id)) {
                p.enabled = i.enabled;
            }
            settle_sources(cmd, doc, k)?;
            let j = &doc.jigs[k];
            let on = j.pockets.iter().filter(|p| p.enabled).count();
            let v = jig_json(doc, j);
            let what = if i.enabled { "on" } else { "off" };
            Ok((json!({ "created": [], "affected": [], "jig": v, "bounds": v["bounds"].clone() }), format!("jig {}: {} pockets {what} ({on} of {} on)", j.id, ids.len(), j.pockets.len())))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddKeepOutInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// Objects whose closed outlines become keep-out zones. Not with rect.
    #[serde(default)]
    pub ids: Vec<ObjectId>,
    /// A rectangle [x0, y0, x1, y1] in field mm. Not with ids.
    pub rect: Option<[f64; 4]>,
    /// Display name (default "Wall n").
    pub name: Option<String>,
    /// Delete the source objects (default true).
    #[serde(default = "yes")]
    pub delete_sources: bool,
}

/// `jig.add_keep_out`.
pub struct JigAddKeepOut;

impl Command for JigAddKeepOut {
    type Input = AddKeepOutInput;
    const ID: &'static str = "jig.add_keep_out";
    const TITLE: &'static str = "Add keep-out zone";
    const DESCRIPTION: &'static str = "Add keep-out zones to a jig (walls, clamps, pins the laser must never mark): \
        the closed outlines of ids, or a field-frame rect [x0, y0, x1, y1]. They move with the jig, and a job that \
        marks inside an enabled one is refused when it is requested or started. Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: AddKeepOutInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let mut paths = Vec::new();
            let mut sources = Vec::new();
            match (i.ids.is_empty(), i.rect) {
                (false, None) => {
                    sources = util::object_ids(cmd, doc, &i.ids)?;
                    paths = closed_outlines(cmd, doc, &sources)?;
                }
                (true, Some([x0, y0, x1, y1])) => {
                    for v in [x0, y0, x1, y1] {
                        util::finite(cmd, "rect", v)?;
                    }
                    let r = Rect::new(x0, y0, x1, y1).abs();
                    if r.width() <= 0.0 || r.height() <= 0.0 {
                        return Err(invalid(cmd, "rect has no area"));
                    }
                    paths.push(r.to_path(0.01));
                }
                _ => return Err(invalid(cmd, "give ids or rect (one of them)")),
            }
            if paths.is_empty() {
                return Err(invalid(cmd, "no closed outline to make a keep-out zone from"));
            }
            let to_jig = doc.jigs[k].transform().inverse();
            let base = match &i.name {
                Some(n) => Some(util::name(cmd, "name", n)?),
                None => None,
            };
            let several = paths.len() > 1;
            for (n, p) in paths.into_iter().enumerate() {
                let j = &mut doc.jigs[k];
                let id = next_id("k", j.keep_out.iter().map(|z| z.id.clone()));
                let name = match &base {
                    Some(b) if several => format!("{b} {}", n + 1),
                    Some(b) => b.clone(),
                    None => format!("Wall {}", j.keep_out.len() + 1),
                };
                j.keep_out.push(KeepOut { id, name, path: to_jig * p, enabled: true });
            }
            check_jig(cmd, &doc.jigs[k])?;
            if i.delete_sources && !sources.is_empty() {
                util::check_unlocked(cmd, doc, &sources)?;
                delete_objects(doc, &sources);
            }
            let v = jig_json(doc, &doc.jigs[k]);
            Ok((json!({ "created": [], "affected": [], "deleted": if i.delete_sources { sources } else { vec![] }, "jig": v, "bounds": v["bounds"].clone() }), format!("added keep-out zones to jig {}", doc.jigs[k].id)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeepOutRefInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// The keep-out zone's id ("k2") or name.
    pub keep_out: String,
    /// Absent: delete the zone. true / false: keep it and switch it on or off.
    pub enabled: Option<bool>,
}

/// `jig.remove_keep_out`.
pub struct JigRemoveKeepOut;

impl Command for JigRemoveKeepOut {
    type Input = KeepOutRefInput;
    const ID: &'static str = "jig.remove_keep_out";
    const TITLE: &'static str = "Remove keep-out zone";
    const DESCRIPTION: &'static str = "Delete a jig's keep-out zone, or with enabled true / false switch it on or \
        off (an off zone is kept but not enforced). Returns the jig.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: KeepOutRefInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let j = &mut doc.jigs[k];
            let z = j.keep_out.iter().position(|z| z.id == i.keep_out || z.name == i.keep_out).ok_or_else(|| CommandError::NotFound {
                message: format!("{cmd}: jig {} has no keep-out zone {}", j.id, i.keep_out),
                candidates: j.keep_out.iter().map(|z| z.id.clone()).collect(),
            })?;
            let summary = match i.enabled {
                None => {
                    let z = j.keep_out.remove(z);
                    format!("removed keep-out zone {} from jig {}", z.name, j.id)
                }
                Some(on) => {
                    j.keep_out[z].enabled = on;
                    format!("keep-out zone {} of jig {} {}", j.keep_out[z].name, j.id, if on { "on" } else { "off" })
                }
            };
            let v = jig_json(doc, &doc.jigs[k]);
            Ok((json!({ "created": [], "affected": [], "jig": v, "bounds": v["bounds"].clone() }), summary))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JigRefInput {
    /// The jig's id (or its name).
    pub jig: String,
}

/// `jig.delete`.
pub struct JigDelete;

impl Command for JigDelete {
    type Input = JigRefInput;
    const ID: &'static str = "jig.delete";
    const TITLE: &'static str = "Delete jig";
    const DESCRIPTION: &'static str = "Delete a jig. Designs placed in it keep their originals (in the first pocket); \
        the copies in the other pockets are deleted. Returns deleted ids.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: JigRefInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let id = doc.jigs[k].id.clone();
            let arrays: HashSet<usize> = placements(doc, &id).into_iter().map(|(a, _)| a).collect();
            let mut deleted: Vec<ObjectId> = Vec::new();
            for a in &arrays {
                deleted.extend(doc.virtual_arrays[*a].clones.iter().cloned());
            }
            let mut n = 0;
            doc.virtual_arrays.retain(|_| {
                let keep = !arrays.contains(&n);
                n += 1;
                keep
            });
            delete_objects(doc, &deleted);
            for c in &deleted {
                doc.object_options.remove(c);
            }
            doc.jigs.remove(k);
            Ok((json!({ "created": [], "affected": [], "deleted": deleted, "bounds": Value::Null }), format!("deleted jig {id}")))
        })
    }
}

// ---------------------------------------------------------------------------------------------------------------
// jig.place
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaceInput {
    /// The jig's id (or its name).
    pub jig: String,
    /// Exact object or group ids: the design to put in every pocket (not already in a virtual array).
    pub ids: Vec<ObjectId>,
    /// Variable text (serial numbers, CSV rows) counts on by this much per enabled pocket (default 1; 0 gives every
    /// pocket the same value).
    #[serde(default = "one")]
    pub auto_increment: i64,
    /// Centre the design on the first pocket (default true); false keeps where it is relative to that pocket.
    #[serde(default = "yes")]
    pub center: bool,
}

/// `jig.place`.
pub struct JigPlace;

impl Command for JigPlace {
    type Input = PlaceInput;
    const ID: &'static str = "jig.place";
    const TITLE: &'static str = "Place in every pocket";
    const DESCRIPTION: &'static str = "Put a design in every enabled pocket of a jig: it is centred on (and turned \
        with) the first enabled pocket and linked copies follow it into every other enabled pocket (a virtual \
        array: edit the original, the copies follow; array.flatten makes them independent, array.remove takes the \
        design out). Variable text counts on by auto_increment per pocket (default 1) in pocket order, so each \
        pocket gets its own serial number. Returns array, created (the copies), pockets ([{pocket, number, ids}]) \
        and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: PlaceInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = jig_index(cmd, doc, &i.jig)?;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            util::check_unlocked(cmd, doc, &ids)?;
            if let Some(x) = ids.iter().find(|id| doc.virtual_arrays.iter().any(|a| a.sources.contains(*id) || a.clones.contains(*id))) {
                return Err(invalid(cmd, format!("{x} already belongs to a virtual array (or a jig placement)")));
            }
            let jig = doc.jigs[k].clone();
            let first = (*jig.enabled_pockets().first().ok_or_else(|| invalid(cmd, "every pocket is off"))?).clone();
            let pt = jig.pocket_transform(&first);
            if i.center {
                let b = doc.bounds_of(&ids).ok_or_else(|| invalid(cmd, "nothing to place"))?;
                // The design's centre onto the pocket's, turned with the pocket.
                let t = pt * Affine::translate(-b.center().to_vec2());
                apply_affine(doc, &ids, t);
            }
            let aid = doc.alloc_id("array")?;
            let settings = Placement { jig: jig.id.clone(), source_pocket: first.id.clone(), auto_increment: i.auto_increment };
            let settings = serde_json::to_value(&settings).map_err(|e| CommandError::Failed(e.to_string()))?;
            doc.virtual_arrays.push(VirtualArray { id: aid.clone(), sources: ids.clone(), clones: vec![], settings });
            super::super::array::sync_virtual_arrays(doc)?;
            let a = doc.virtual_arrays.iter().find(|a| a.id == aid).expect("just added").clone();
            let mut r = util::edit_result(doc, &a.clones, &ids);
            r["array"] = json!(aid);
            r["pockets"] = json!(pocket_contents(doc, &jig));
            r["results"] = json!(ids);
            Ok((r, format!("placed {} in {} pockets of jig {}", ids.join(", "), jig.enabled_pockets().len(), jig.id)))
        })
    }
}

/// Every pocket's placed objects (originals in the source pocket, clones in the rest), pocket by pocket.
fn pocket_contents(doc: &Document, jig: &Jig) -> Vec<Value> {
    let mut by_pocket: std::collections::BTreeMap<String, Vec<ObjectId>> = Default::default();
    for (k, p) in placements(doc, &jig.id) {
        by_pocket.entry(p.source_pocket.clone()).or_default().extend(doc.virtual_arrays[k].sources.iter().cloned());
        for c in &doc.virtual_arrays[k].clones {
            if let Some(tag) = doc.object(c).and_then(|o| o.extras.get(JIG_POCKET_KEY))
                && let Some((_, pid)) = tag.split_once('/')
            {
                by_pocket.entry(pid.to_string()).or_default().push(c.clone());
            }
        }
    }
    let order = jig.enabled_pockets();
    let mut pockets: Vec<&Pocket> = jig.pockets.iter().collect();
    pockets.sort_by_key(|p| p.number);
    pockets
        .into_iter()
        .map(|p| {
            json!({
                "pocket": p.id,
                "number": p.number,
                "enabled": p.enabled,
                "order": order.iter().position(|q| q.id == p.id),
                "ids": by_pocket.get(&p.id).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

/// A jig as JSON for results and `jig.list`: the stored fields, each pocket's field-frame centre, its order among
/// the enabled pockets (the burn and serial order; null when off), the objects placed in it, and the bounds.
fn jig_json(doc: &Document, jig: &Jig) -> Value {
    let contents = pocket_contents(doc, jig);
    let pockets: Vec<Value> = {
        let mut ps: Vec<&Pocket> = jig.pockets.iter().collect();
        ps.sort_by_key(|p| p.number);
        ps.into_iter()
            .zip(contents)
            .map(|(p, c)| {
                let fc = jig.pocket_center(p);
                let b = jig.pocket_outline(p).bounding_box();
                json!({
                    "id": p.id,
                    "number": p.number,
                    "enabled": p.enabled,
                    "order": c["order"],
                    "center": util::pt_json(fc),
                    "size": [util::r4(b.width()), util::r4(b.height())],
                    "shape": serde_json::to_value(&p.shape).ok().map(|mut s| { if let Some(o) = s.as_object_mut() { o.remove("path"); } s }),
                    "objects": c["ids"],
                })
            })
            .collect()
    };
    let arrays: Vec<&str> = placements(doc, &jig.id).into_iter().map(|(k, _)| doc.virtual_arrays[k].id.as_str()).collect();
    json!({
        "id": jig.id,
        "name": jig.name,
        "datum": util::pt_json(Point::new(jig.datum[0], jig.datum[1])),
        "rotation_deg": util::r4(jig.rotation_deg),
        "material_height_mm": jig.material_height_mm,
        "pockets": pockets,
        "enabled_pockets": jig.pockets.iter().filter(|p| p.enabled).count(),
        "keep_out": jig.keep_out.iter().map(|z| json!({"id": z.id, "name": z.name, "enabled": z.enabled, "bounds": util::rect_json((jig.transform() * z.path.clone()).bounding_box())})).collect::<Vec<_>>(),
        "placements": arrays,
        "bounds": jig.bounds().map(util::rect_json).unwrap_or(Value::Null),
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    /// One jig's id (or name); default every jig.
    pub jig: Option<String>,
}

/// `jig.list`.
pub struct JigList;

impl Command for JigList {
    type Input = ListInput;
    const ID: &'static str = "jig.list";
    const TITLE: &'static str = "List jigs";
    const DESCRIPTION: &'static str = "The document's jigs: id, name, datum, rotation, material height, pockets ({id \
        (stable, for per-slot records), number, enabled, order (position among the enabled pockets: the burn and \
        serial order, the variable-text offset of the copy there; null when off), field-frame center, size, shape, \
        objects placed there}), keep-out zones, placements (virtual array ids) and bounds. Read-only.";
    const RISK: Risk = Risk::Query;

    fn apply(doc: &mut Document, i: ListInput) -> Result<(Value, String)> {
        let jigs: Vec<Value> = match &i.jig {
            Some(j) => vec![jig_json(doc, &doc.jigs[jig_index(Self::ID, doc, j)?])],
            None => doc.jigs.iter().map(|j| jig_json(doc, j)).collect(),
        };
        let n = jigs.len();
        Ok((json!({ "jigs": jigs }), format!("listed {n} jigs")))
    }
}

/// The objects a clone tag or placement puts in pocket `pocket` of `jig` (for framing one pocket).
#[cfg(test)]
fn objects_in(doc: &Document, jig: &str, pocket: &str) -> Vec<ObjectId> {
    let j = doc.jigs.iter().find(|x| x.id == jig).unwrap();
    pocket_contents(doc, j).into_iter().find(|v| v["pocket"] == json!(pocket)).map(|v| serde_json::from_value(v["ids"].clone()).unwrap()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Invocation, Origin, Session};
    use doc::VIRTUAL_ARRAY_KEY;

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

    fn coin_jig(s: &mut Session) -> String {
        let r = exec(s, "jig.create", json!({"rows": 2, "cols": 3, "pitch_x_mm": 30, "pitch_y_mm": 30, "pocket": {"kind": "circle", "diameter_mm": 25}, "center": [0, 0], "material_height_mm": 3}));
        r["jig"]["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn grid_jig_numbers_rows_from_the_far_side() {
        let mut s = session();
        let j = coin_jig(&mut s);
        let d = s.document();
        let jig = &d.jigs[0];
        assert_eq!(jig.id, j);
        // Pocket 1 is far left (row at +Y), pocket 4 near left.
        let c = |n: u32| jig.pocket_center(jig.pockets.iter().find(|p| p.number == n).unwrap());
        assert_eq!(c(1), Point::new(-30.0, 15.0));
        assert_eq!(c(3), Point::new(30.0, 15.0));
        assert_eq!(c(4), Point::new(-30.0, -15.0));
        exec(&mut s, "jig.edit", json!({"jig": j, "numbering": "serpentine"}));
        let jig = &s.document().jigs[0];
        let c = |n: u32| jig.pocket_center(jig.pockets.iter().find(|p| p.number == n).unwrap());
        assert_eq!(c(4), Point::new(30.0, -15.0));
        assert!(fails(&mut s, "jig.create", json!({"rows": 1, "cols": 2, "pitch_x_mm": 10, "pocket": {"kind": "circle", "diameter_mm": 25}})).contains("overlap"));
        assert!(fails(&mut s, "jig.create", json!({"rows": 0, "cols": 2, "pitch_x_mm": 30, "pocket": {"kind": "circle", "diameter_mm": 25}})).contains("rows"));
    }

    #[test]
    fn place_follows_pockets_serials_and_jig_moves() {
        let mut s = session();
        let j = coin_jig(&mut s);
        let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
        let t = exec(&mut s, "shape.text", json!({ "text": "0ddd", "font": font, "variable": { "mode": "serial" }, "center": [50, 50] }))["created"][0]
            .as_str()
            .unwrap()
            .to_string();
        let r = exec(&mut s, "jig.place", json!({"jig": j, "ids": [t]}));
        assert_eq!(r["created"].as_array().unwrap().len(), 5);
        let d = s.document();
        // The original moved onto pocket 1; the copy in pocket 2 sits 30 mm to the right.
        let oc = d.object(&t).unwrap().bounds().center();
        assert!(oc.distance(Point::new(-30.0, 15.0)) < 1e-6, "{oc:?}");
        let in2 = objects_in(d, &j, "p2");
        assert_eq!(in2.len(), 1);
        let c2 = d.object(&in2[0]).unwrap();
        assert!((c2.bounds().center().x - 0.0).abs() < 1e-6);
        assert_eq!(c2.extras[JIG_POCKET_KEY], format!("{j}/p2"));
        let offset = |d: &Document, id: &str| match &d.object(id).unwrap().geometry {
            doc::Geometry::Text(t) => t.variable.as_ref().unwrap().offset,
            _ => unreachable!(),
        };
        assert_eq!(offset(d, &in2[0]), 1);
        assert_eq!(offset(d, &objects_in(d, &j, "p6")[0]), 5);
        // Pocket 2 off: no copy there, the later pockets count on without it.
        exec(&mut s, "jig.set_pockets", json!({"jig": j, "pockets": [2], "enabled": false}));
        let d = s.document();
        assert!(objects_in(d, &j, "p2").is_empty());
        assert_eq!(offset(d, &objects_in(d, &j, "p3")[0]), 1);
        assert_eq!(offset(d, &objects_in(d, &j, "p6")[0]), 4);
        assert_eq!(d.virtual_arrays[0].clones.len(), 4);
        // Pocket 1 off: the original moves into pocket 3 (the first enabled one).
        exec(&mut s, "jig.set_pockets", json!({"jig": j, "pockets": ["p1"], "enabled": false}));
        let d = s.document();
        assert!(d.object(&t).unwrap().bounds().center().distance(Point::new(30.0, 15.0)) < 1e-6);
        assert_eq!(offset(d, &objects_in(d, &j, "p4")[0]), 1);
        assert!(fails(&mut s, "jig.set_pockets", json!({"jig": j, "all": true, "enabled": false})).contains("at least one pocket"));
        exec(&mut s, "jig.set_pockets", json!({"jig": j, "all": true, "enabled": true}));
        assert!(s.document().object(&t).unwrap().bounds().center().distance(Point::new(-30.0, 15.0)) < 1e-6);
        // Move the jig: everything follows, in one step.
        exec(&mut s, "jig.move", json!({"jig": j, "translate": [5, -2], "rotate_by_deg": 90}));
        let d = s.document();
        // Pocket 1 (-30, 15) turned 90 degrees about the datum (0, 0) then moved: (-15, -30) + (5, -2).
        assert!(d.object(&t).unwrap().bounds().center().distance(Point::new(-10.0, -32.0)) < 1e-6);
        let p2 = d.object(&objects_in(d, &j, "p2")[0]).unwrap().bounds().center();
        assert!(p2.distance(Point::new(-10.0, -2.0)) < 1e-6, "{p2:?}");
        s.undo().unwrap();
        // The grid commands refuse a placement; flatten works.
        let arr = s.document().virtual_arrays[0].id.clone();
        assert!(fails(&mut s, "array.duplicate", json!({"array": arr})).contains("pockets of jig"));
        assert!(fails(&mut s, "art.array", json!({"edit_array": arr, "rows": 1, "cols": 2, "pitch_x_mm": 1, "pitch_y_mm": 1})).contains("pockets of jig"));
        // jig.list reports pocket ids, order and contents.
        let l = exec(&mut s, "jig.list", json!({}));
        assert_eq!(l["jigs"][0]["pockets"][1]["id"], json!("p2"));
        assert_eq!(l["jigs"][0]["pockets"][1]["order"], json!(1));
        assert_eq!(l["jigs"][0]["placements"], json!([arr]));
        // Deleting the jig keeps the original and drops the copies.
        exec(&mut s, "jig.delete", json!({"jig": j}));
        let d = s.document();
        assert!(d.jigs.is_empty() && d.virtual_arrays.is_empty());
        assert_eq!(d.objects.len(), 1);
        assert!(!d.objects[0].extras.contains_key(VIRTUAL_ARRAY_KEY));
    }

    #[test]
    fn jig_from_shapes_finds_plate_pockets_and_walls() {
        let mut s = session();
        let plate = exec(&mut s, "shape.rect", json!({"width_mm": 100, "height_mm": 60}))["created"][0].as_str().unwrap().to_string();
        let a = exec(&mut s, "shape.ellipse", json!({"rx_mm": 10, "ry_mm": 10, "center": [-25, 0]}))["created"][0].as_str().unwrap().to_string();
        let b = exec(&mut s, "shape.rect", json!({"width_mm": 20, "height_mm": 10, "center": [25, 0]}))["created"][0].as_str().unwrap().to_string();
        let tri = exec(&mut s, "shape.path", json!({"svg_path_data": "M -5 20 L 5 20 L 0 28 Z"}))["created"][0].as_str().unwrap().to_string();
        let wall = exec(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 4, "center": [0, -20]}))["created"][0].as_str().unwrap().to_string();
        let r = exec(&mut s, "jig.from_objects", json!({"ids": [plate, a, b, tri], "keep_out_ids": [wall]}));
        assert_eq!(r["deleted"].as_array().unwrap().len(), 5);
        let d = s.document();
        assert!(d.objects.is_empty());
        let j = &d.jigs[0];
        assert!(j.outline.is_some());
        assert_eq!(j.pockets.len(), 3, "{:?}", j.pockets);
        // The triangle is on the far row: pocket 1; then the circle and the rectangle left to right.
        let by = |n: u32| j.pockets.iter().find(|p| p.number == n).unwrap();
        assert!(matches!(by(1).shape, PocketShape::Path { .. }));
        assert!(matches!(by(2).shape, PocketShape::Circle { diameter_mm } if (diameter_mm - 20.0).abs() < 1e-6));
        assert!(matches!(by(3).shape, PocketShape::Rect { width_mm, .. } if (width_mm - 20.0).abs() < 1e-9));
        assert_eq!(doc::jig::document_keep_outs(d).len(), 1);
        // Keep-outs: add a rect, switch it off, remove it.
        let id = j.id.clone();
        exec(&mut s, "jig.add_keep_out", json!({"jig": id, "rect": [40, 20, 50, 30], "name": "Clamp"}));
        assert_eq!(doc::jig::document_keep_outs(s.document()).len(), 2);
        exec(&mut s, "jig.remove_keep_out", json!({"jig": id, "keep_out": "Clamp", "enabled": false}));
        assert_eq!(doc::jig::document_keep_outs(s.document()).len(), 1);
        exec(&mut s, "jig.remove_keep_out", json!({"jig": id, "keep_out": "k2"}));
        assert_eq!(s.document().jigs[0].keep_out.len(), 1);
    }

    #[test]
    fn import_svg_jig() {
        let mut s = session();
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="80mm" height="40mm" viewBox="0 0 80 40">
            <rect x="0" y="0" width="80" height="40" fill="none" stroke="black"/>
            <circle cx="20" cy="20" r="10" fill="none" stroke="black"/>
            <circle cx="60" cy="20" r="10" fill="none" stroke="black"/></svg>"#;
        let r = exec(&mut s, "jig.import", json!({"content": svg, "format": "svg", "center": [10, 0], "name": "Two coins"}));
        assert_eq!(r["jig"]["pockets"].as_array().unwrap().len(), 2);
        let d = s.document();
        assert!(d.objects.is_empty(), "nothing is added to the design");
        let j = &d.jigs[0];
        assert_eq!(j.name, "Two coins");
        assert!(j.outline.is_some());
        let c1 = j.pocket_center(&j.pockets[0]);
        assert!(c1.distance(Point::new(-10.0, 0.0)) < 1e-6, "{c1:?}");
        assert!(matches!(j.pockets[0].shape, PocketShape::Circle { diameter_mm } if (diameter_mm - 20.0).abs() < 1e-3));
        // The file format keeps the jig.
        let saved = exec(&mut s, "doc.save", json!({}));
        let text = saved["content"].as_str().unwrap();
        assert!(text.contains("\"jigs\""));
        let back = doc::file::from_openetch_json(text).unwrap();
        assert_eq!(back.jigs, s.document().jigs);
    }
}
