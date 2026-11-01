//! `art.array`: rectangular grid of copies (LightBurn's Grid Array), and LightBurn's virtual arrays: linked clones
//! kept in [`Document::virtual_arrays`] and laid out again after every edit ([`sync_virtual_arrays`]), with
//! `array.flatten`, `array.add`, `array.remove` and `array.duplicate` (the virtual array's context menu).

use std::collections::{BTreeSet, HashSet};

use doc::{Document, Object, ObjectId, VIRTUAL_ARRAY_KEY, VirtualArray};
use kurbo::{Affine, Rect, Vec2};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::object::{Mirror, apply_affine, copy_objects};
use super::util::{self, invalid, transact};
use crate::{Command, CommandError, Result, Risk};

/// Upper bound on cells, to keep a typo from creating millions of objects.
const MAX_CELLS: u32 = 10_000;
/// Most objects one array (or one virtual array) may create.
const MAX_OBJECTS: usize = 50_000;

fn one() -> u32 {
    1
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayInput {
    /// Exact object or group ids (a group stands for all its members), copied together per cell. May be empty with
    /// `edit_array` (the array's originals).
    #[serde(default)]
    pub ids: Vec<ObjectId>,
    /// Number of rows (along field Y), >= 1 (default 1). total_height_mm overrides it.
    #[serde(default = "one")]
    pub rows: u32,
    /// Number of columns (along field X), >= 1 (default 1). total_width_mm overrides it.
    #[serde(default = "one")]
    pub cols: u32,
    /// Distance between columns in mm along field +X, centre to centre (negative builds toward -X); with spacing_x
    /// "gap", the gap between neighbouring copies' bounding boxes.
    pub pitch_x_mm: f64,
    /// Distance between rows in mm along field +Y, away from the operator (negative builds toward the operator);
    /// with spacing_y "gap", the gap between neighbouring rows' bounding boxes.
    pub pitch_y_mm: f64,
    /// How pitch_x_mm is measured: "center" (centre to centre, default) or "gap" (LightBurn's Padding Between
    /// Edges: the selection's width plus the pitch).
    #[serde(default)]
    pub spacing_x: Spacing,
    /// How pitch_y_mm is measured: "center" (default) or "gap".
    #[serde(default)]
    pub spacing_y: Spacing,
    /// Build columns toward -X instead of +X (LightBurn's Reverse Direction). Default false.
    #[serde(default)]
    pub reverse_x: bool,
    /// Build rows toward -Y instead of +Y. Default false.
    #[serde(default)]
    pub reverse_y: bool,
    /// Shift every other row (rows 1, 3, ...) along X by this many mm, or "half" for half the selection's width
    /// (LightBurn's Row Shift / Shift by half: a brick or honeycomb layout).
    pub row_shift: Option<Shift>,
    /// Shift every other column (columns 1, 3, ...) along Y by this many mm, or "half" for half its height.
    pub col_shift: Option<Shift>,
    /// Flip the copies in every other column about their own centre: "x" left-right, "y" top-bottom (LightBurn's
    /// Mirror Alternate Columns).
    pub mirror_alternate_cols: Option<Mirror>,
    /// Flip the copies in every other row about their own centre: "x" or "y" (Mirror Alternate Rows).
    pub mirror_alternate_rows: Option<Mirror>,
    /// LightBurn's Total Width: as many columns as fit in this many mm without going over (the selection's width,
    /// the steps and an alternate row shift counted). Overrides cols.
    pub total_width_mm: Option<f64>,
    /// LightBurn's Total Height: as many rows as fit in this many mm. Overrides rows.
    pub total_height_mm: Option<f64>,
    /// LightBurn's Random Orientation with this seed: every copy is turned about its own centre by a random angle
    /// (0-360°); the same seed gives the same angles. The originals keep theirs.
    pub random_orientation_seed: Option<u64>,
    /// LightBurn's Auto-Increment Variable Text: the n-th copy (row by row, the originals are 0) of a serial-number
    /// or CSV text or barcode gets its Offset raised by n times this. Default 0 (off).
    #[serde(default)]
    pub auto_increment: i64,
    /// LightBurn's Group Results: group the originals and every copy into one new group. Not with virtual.
    #[serde(default)]
    pub group_results: bool,
    /// LightBurn's Create Virtual Array: linked clones that follow every later edit of the originals instead of
    /// independent copies (see `array.flatten`).
    #[serde(default, rename = "virtual")]
    pub virtual_array: bool,
    /// LightBurn's Edit Array: lay out this existing virtual array (its id) again with these settings.
    pub edit_array: Option<String>,
}

/// How a grid pitch is measured.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Spacing {
    /// Centre to centre.
    #[default]
    Center,
    /// Between neighbouring bounding boxes (the selection's size is added).
    Gap,
}

/// An alternate row or column shift.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Shift {
    /// This many mm.
    Mm(f64),
    /// "half": half the selection's width (rows) or height (columns).
    Half(HalfWord),
}

/// The word "half".
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HalfWord {
    Half,
}

/// The settings a grid is laid out from (what a virtual array keeps): [`ArrayInput`] without the ids and one-off
/// options, its counts resolved.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grid {
    rows: u32,
    cols: u32,
    pitch_x_mm: f64,
    pitch_y_mm: f64,
    spacing_x: Spacing,
    spacing_y: Spacing,
    reverse_x: bool,
    reverse_y: bool,
    row_shift: Option<Shift>,
    col_shift: Option<Shift>,
    mirror_alternate_cols: Option<Mirror>,
    mirror_alternate_rows: Option<Mirror>,
    #[serde(default)]
    random_orientation_seed: Option<u64>,
    #[serde(default)]
    auto_increment: i64,
}

/// The steps between cells and the alternate shifts, in mm, for a selection of bounds `b`.
struct Steps {
    sx: f64,
    sy: f64,
    row_shift: f64,
    col_shift: f64,
}

fn steps(cmd: &str, g: &Grid, b: Rect) -> Result<Steps> {
    let px = util::finite(cmd, "pitch_x_mm", g.pitch_x_mm)?;
    let py = util::finite(cmd, "pitch_y_mm", g.pitch_y_mm)?;
    let step = |pitch: f64, spacing: Spacing, size: f64, reverse: bool| {
        let s = if spacing == Spacing::Gap { size + pitch } else { pitch };
        if reverse { -s } else { s }
    };
    let shift = |s: Option<Shift>, field: &str, size: f64| -> Result<f64> {
        match s {
            None => Ok(0.0),
            Some(Shift::Half(_)) => Ok(size / 2.0),
            Some(Shift::Mm(v)) => util::finite(cmd, field, v),
        }
    };
    Ok(Steps {
        sx: step(px, g.spacing_x, b.width(), g.reverse_x),
        sy: step(py, g.spacing_y, b.height(), g.reverse_y),
        row_shift: shift(g.row_shift, "row_shift", b.width())?,
        col_shift: shift(g.col_shift, "col_shift", b.height())?,
    })
}

/// One copy of the grid: its row, column, place in creation order (row by row; the originals are 0) and the
/// transform taking the originals onto it.
struct Cell {
    row: u32,
    col: u32,
    index: u32,
    t: Affine,
    mirrored: bool,
    rotation_deg: Option<f64>,
}

/// SplitMix64: a small deterministic generator (the same seed gives the same orientations on every build).
struct SplitMix(u64);

impl SplitMix {
    fn next_unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Every copy's cell (the originals' cell left out) for a selection of bounds `b`.
fn layout(cmd: &str, g: &Grid, b: Rect) -> Result<Vec<Cell>> {
    if g.rows == 0 || g.cols == 0 {
        return Err(invalid(cmd, "rows and cols must be >= 1"));
    }
    if g.rows.saturating_mul(g.cols) > MAX_CELLS {
        return Err(invalid(cmd, format!("rows x cols must be at most {MAX_CELLS}")));
    }
    let s = steps(cmd, g, b)?;
    if (g.cols > 1 && s.sx.abs() < 1e-9) || (g.rows > 1 && s.sy.abs() < 1e-9) {
        return Err(invalid(cmd, "a zero step would stack copies on top of each other"));
    }
    let flip = |m: Mirror| match m {
        Mirror::X => Affine::FLIP_X,
        Mirror::Y => Affine::FLIP_Y,
    };
    let mut rng = g.random_orientation_seed.map(SplitMix);
    let mut cells = Vec::with_capacity((g.rows * g.cols) as usize);
    for r in 0..g.rows {
        for c in 0..g.cols {
            if r == 0 && c == 0 {
                continue;
            }
            let dx = f64::from(c) * s.sx + if r % 2 == 1 { s.row_shift } else { 0.0 };
            let dy = f64::from(r) * s.sy + if c % 2 == 1 { s.col_shift } else { 0.0 };
            let mut m = Affine::IDENTITY;
            if c % 2 == 1
                && let Some(f) = g.mirror_alternate_cols
            {
                m = flip(f) * m;
            }
            if r % 2 == 1
                && let Some(f) = g.mirror_alternate_rows
            {
                m = flip(f) * m;
            }
            let mirrored = m != Affine::IDENTITY;
            let rotation_deg = rng.as_mut().map(|g| (g.next_unit() * 360.0 * 100.0).round() / 100.0);
            if let Some(a) = rotation_deg {
                m = Affine::rotate(a.to_radians()) * m;
            }
            // About the copy's own centre.
            let ctr = b.center().to_vec2() + Vec2::new(dx, dy);
            let t = Affine::translate(ctr) * m * Affine::translate(-ctr) * Affine::translate((dx, dy));
            cells.push(Cell { row: r, col: c, index: r * g.cols + c, t, mirrored, rotation_deg });
        }
    }
    Ok(cells)
}

/// How many cells fit in `total` mm without going over: the selection's `size`, `step` between cells and `shift`
/// (an alternate shift that widens the grid).
fn fit_count(cmd: &str, field: &str, total: f64, size: f64, step: f64, shift: f64) -> Result<u32> {
    let total = util::positive(cmd, field, total)?;
    let room = total - size - shift.abs();
    if room < -1e-9 {
        return Err(invalid(cmd, format!("{field} ({total} mm) is smaller than the selection and its shift")));
    }
    if step.abs() < 1e-9 {
        return Ok(1);
    }
    let n = (room.max(0.0) / step.abs() + 1e-9).floor() + 1.0;
    Ok(n.min(f64::from(MAX_CELLS)) as u32)
}

/// The grid an input asks for around a selection of bounds `b` (Total Width / Height resolved into counts).
fn grid_of(cmd: &str, i: &ArrayInput, b: Rect) -> Result<Grid> {
    let mut g = Grid {
        rows: i.rows,
        cols: i.cols,
        pitch_x_mm: i.pitch_x_mm,
        pitch_y_mm: i.pitch_y_mm,
        spacing_x: i.spacing_x,
        spacing_y: i.spacing_y,
        reverse_x: i.reverse_x,
        reverse_y: i.reverse_y,
        row_shift: i.row_shift,
        col_shift: i.col_shift,
        mirror_alternate_cols: i.mirror_alternate_cols,
        mirror_alternate_rows: i.mirror_alternate_rows,
        random_orientation_seed: i.random_orientation_seed,
        auto_increment: i.auto_increment,
    };
    if i.total_width_mm.is_some() || i.total_height_mm.is_some() {
        let s = steps(cmd, &g, b)?;
        // A shift widens the grid only when there is a second row (or column); two rounds settle both counts.
        for _ in 0..2 {
            if let Some(w) = i.total_width_mm {
                let shift = if g.rows > 1 { s.row_shift } else { 0.0 };
                g.cols = fit_count(cmd, "total_width_mm", w, b.width(), s.sx, shift)?;
            }
            if let Some(h) = i.total_height_mm {
                let shift = if g.cols > 1 { s.col_shift } else { 0.0 };
                g.rows = fit_count(cmd, "total_height_mm", h, b.height(), s.sy, shift)?;
            }
        }
    }
    Ok(g)
}

/// The top-level items (objects or outermost new groups) among `objects`, where `old_groups` existed before.
fn top_items(doc: &Document, objects: &[ObjectId], old_groups: &BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in objects {
        let chain = doc.object(id).and_then(|o| o.group.as_deref()).map(|g| doc.group_chain(g)).unwrap_or_default();
        let item = chain.iter().rev().find(|g| !old_groups.contains(&g.id)).map_or(id.clone(), |g| g.id.clone());
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// `art.array`.
pub struct Array;

impl Command for Array {
    type Input = ArrayInput;
    const ID: &'static str = "art.array";
    const TITLE: &'static str = "Grid array";
    const DESCRIPTION: &'static str = "Copy objects into a rows x cols grid (LightBurn's Grid Array); the originals \
        are cell (0, 0). Copy (r, c) is offset by [c * step_x, r * step_y] (field mm), a step being the pitch \
        (spacing center) or the selection's size plus it (gap), negated by reverse_x/y; odd rows/columns can shift \
        and mirror. total_width/height_mm set the counts from a size; random_orientation_seed turns copies; \
        auto_increment raises variable-text Offsets per copy; group_results groups everything; virtual makes linked \
        clones (edit_array lays one out again). Returns cells, rows, cols, count, size_mm, results, bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArrayInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            if let Some(aid) = &i.edit_array {
                return edit_virtual(doc, aid, &i);
            }
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            let b = doc.bounds_of(&ids).ok_or_else(|| invalid(cmd, "nothing to array"))?;
            let g = grid_of(cmd, &i, b)?;
            let cells = layout(cmd, &g, b)?;
            if cells.len().saturating_mul(ids.len()) > MAX_OBJECTS {
                return Err(invalid(cmd, format!("that would create more than {MAX_OBJECTS} objects")));
            }
            if i.virtual_array {
                if i.group_results {
                    return Err(invalid(cmd, "group_results does not apply to a virtual array (its clones follow the originals)"));
                }
                return create_virtual(doc, ids, g);
            }
            let old_groups: BTreeSet<String> = doc.groups.iter().map(|g| g.id.clone()).collect();
            let mut created = Vec::new();
            let mut out = Vec::new();
            for cell in &cells {
                let copies = copy_objects(doc, &ids, Vec2::ZERO, &format!(" [{},{}]", cell.row, cell.col))?;
                apply_affine(doc, &copies, cell.t);
                let delta = i64::from(cell.index).saturating_mul(g.auto_increment);
                for id in &copies {
                    let mut o = util::object(doc, id)?.clone();
                    if super::text::bump_copy(cmd, doc, &mut o, delta, None)? {
                        *doc.object_mut(id).expect("just copied") = o;
                    }
                }
                out.push(json!({ "row": cell.row, "col": cell.col, "ids": copies, "mirrored": cell.mirrored, "rotation_deg": cell.rotation_deg }));
                created.extend(copies);
            }
            let mut items = top_items(doc, &ids, &BTreeSet::new());
            items.extend(top_items(doc, &created, &old_groups));
            let mut group = Value::Null;
            if i.group_results && items.len() > 1 {
                let (r, _) = super::group::GroupCreate::apply(doc, super::group::CreateInput { ids: items.clone(), name: None })?;
                group = r["group"].clone();
                items = vec![group.as_str().unwrap_or_default().to_string()];
            }
            let mut res = util::edit_result(doc, &created, &ids);
            res["cells"] = json!(out);
            res["group"] = group;
            finish(doc, &mut res, &g, &ids, &created, &items);
            Ok((res, format!("arrayed {} into {}x{}", ids.join(", "), g.rows, g.cols)))
        })
    }
}

/// The parts of an array result every variant shares: the counts, the total size and what to select.
fn finish(doc: &Document, res: &mut Value, g: &Grid, ids: &[ObjectId], created: &[ObjectId], items: &[String]) {
    let all: Vec<ObjectId> = ids.iter().chain(created).cloned().collect();
    let size = doc.bounds_of(&all).map(|b| json!([util::r4(b.width()), util::r4(b.height())]));
    res["rows"] = json!(g.rows);
    res["cols"] = json!(g.cols);
    res["count"] = json!(g.rows * g.cols);
    res["size_mm"] = size.unwrap_or(Value::Null);
    // LightBurn's Select Results: the items a client selects afterwards.
    res["results"] = json!(items);
}

// ---------------------------------------------------------------------------------------------------------------
// Virtual arrays
// ---------------------------------------------------------------------------------------------------------------

fn array_index(cmd: &str, doc: &Document, id: &str) -> Result<usize> {
    doc.virtual_arrays.iter().position(|a| a.id == id).ok_or_else(|| CommandError::NotFound {
        message: format!("{cmd}: no virtual array {id}"),
        candidates: doc.virtual_arrays.iter().map(|a| a.id.clone()).collect(),
    })
}

/// The virtual array an object belongs to, as an original or a clone.
fn array_of<'a>(doc: &'a Document, object: &str) -> Option<&'a VirtualArray> {
    doc.virtual_arrays.iter().find(|a| a.sources.iter().chain(&a.clones).any(|x| x == object))
}

/// Refuse objects that already belong to a virtual array.
fn check_free(cmd: &str, doc: &Document, ids: &[ObjectId]) -> Result<()> {
    match ids.iter().find_map(|id| array_of(doc, id).map(|a| (id, a))) {
        Some((id, a)) => Err(invalid(cmd, format!("{id} already belongs to virtual array {}", a.id))),
        None => Ok(()),
    }
}

fn create_virtual(doc: &mut Document, ids: Vec<ObjectId>, g: Grid) -> Result<(Value, String)> {
    let cmd = Array::ID;
    check_free(cmd, doc, &ids)?;
    let id = doc.alloc_id("array")?;
    let settings = serde_json::to_value(&g).map_err(|e| CommandError::Failed(format!("{cmd}: {e}")))?;
    doc.virtual_arrays.push(VirtualArray { id: id.clone(), sources: ids.clone(), clones: vec![], settings });
    let k = doc.virtual_arrays.len() - 1;
    relayout(doc, k)?;
    let clones = doc.virtual_arrays[k].clones.clone();
    let mut res = util::edit_result(doc, &clones, &ids);
    res["array"] = json!(id);
    res["group"] = Value::Null;
    let items = top_items(doc, &ids, &BTreeSet::new());
    finish(doc, &mut res, &g, &ids, &clones, &items);
    Ok((res, format!("made virtual array {id} of {} ({}x{})", ids.join(", "), g.rows, g.cols)))
}

fn edit_virtual(doc: &mut Document, aid: &str, i: &ArrayInput) -> Result<(Value, String)> {
    let cmd = Array::ID;
    let k = array_index(cmd, doc, aid)?;
    if !i.ids.is_empty() {
        let ids = util::object_ids(cmd, doc, &i.ids)?;
        let mut have = doc.virtual_arrays[k].sources.clone();
        let mut want = ids;
        have.sort();
        want.sort();
        if have != want {
            return Err(invalid(cmd, "with edit_array, ids must be empty or the array's originals (array.add / array.remove change them)"));
        }
    }
    if i.group_results {
        return Err(invalid(cmd, "group_results does not apply to a virtual array"));
    }
    let ids = doc.virtual_arrays[k].sources.clone();
    let b = doc.bounds_of(&ids).ok_or_else(|| invalid(cmd, "the array's originals are not on this page"))?;
    let g = grid_of(cmd, i, b)?;
    layout(cmd, &g, b)?;
    doc.virtual_arrays[k].settings = serde_json::to_value(&g).map_err(|e| CommandError::Failed(format!("{cmd}: {e}")))?;
    relayout(doc, k)?;
    let clones = doc.virtual_arrays[k].clones.clone();
    let mut res = util::edit_result(doc, &[], &clones);
    res["array"] = json!(aid);
    res["group"] = Value::Null;
    let items = top_items(doc, &ids, &BTreeSet::new());
    finish(doc, &mut res, &g, &ids, &clones, &items);
    Ok((res, format!("laid out virtual array {aid} again ({}x{})", g.rows, g.cols)))
}

/// Lay virtual array `k` out again from its originals and settings, keeping its clones' ids (new ones are
/// allocated when it grew). The clones replace the old ones in drawing order, after the originals when there were
/// none. Unchanged auto-incremented texts and barcodes keep their drawing (no font is loaded).
fn relayout(doc: &mut Document, k: usize) -> Result<()> {
    let cmd = "array.sync";
    let a = doc.virtual_arrays[k].clone();
    let g: Grid = serde_json::from_value(a.settings.clone())
        .map_err(|e| CommandError::Failed(format!("{cmd}: virtual array {}: bad settings ({e})", a.id)))?;
    let Some(b) = doc.bounds_of(&a.sources) else { return Ok(()) };
    let cells = layout(cmd, &g, b)?;
    if cells.len().saturating_mul(a.sources.len()) > MAX_OBJECTS {
        return Err(invalid(cmd, format!("virtual array {} would hold more than {MAX_OBJECTS} clones", a.id)));
    }
    let sources: Vec<Object> = a.sources.iter().filter_map(|s| doc.object(s).cloned()).collect();
    let old: HashSet<&ObjectId> = a.clones.iter().collect();
    let prev: Vec<Option<Object>> = a.clones.iter().map(|c| doc.object(c).cloned()).collect();
    let mut ids = a.clones.clone().into_iter();
    let mut made = Vec::with_capacity(cells.len() * sources.len());
    let mut j = 0;
    for cell in &cells {
        let delta = i64::from(cell.index).saturating_mul(g.auto_increment);
        for src in &sources {
            let id = match ids.next() {
                Some(id) => id,
                None => doc.alloc_id("obj")?,
            };
            let mut o = src.clone();
            o.id = id;
            o.name = format!("{} [{},{}]", src.name, cell.row, cell.col);
            o.group = None;
            o.transform = cell.t * src.transform;
            o.extras.insert(VIRTUAL_ARRAY_KEY.into(), a.id.clone());
            super::text::bump_copy(cmd, doc, &mut o, delta, prev.get(j).and_then(Option::as_ref))?;
            made.push(o);
            j += 1;
        }
    }
    let new_ids: Vec<ObjectId> = made.iter().map(|o| o.id.clone()).collect();
    let at = doc
        .objects
        .iter()
        .position(|o| old.contains(&o.id))
        .or_else(|| doc.objects.iter().rposition(|o| a.sources.contains(&o.id)).map(|p| p + 1))
        .unwrap_or(doc.objects.len());
    let before = doc.objects[..at.min(doc.objects.len())].iter().filter(|o| old.contains(&o.id)).count();
    doc.objects.retain(|o| !old.contains(&o.id));
    let at = (at - before).min(doc.objects.len());
    doc.objects.splice(at..at, made);
    // Cut properties follow the originals; dropped clones leave nothing behind.
    for c in &a.clones {
        doc.object_options.remove(c);
        doc.locked.remove(c);
    }
    let per = sources.len().max(1);
    for (n, id) in new_ids.iter().enumerate() {
        if let Some(opt) = doc.object_options.get(&sources[n % per].id).cloned() {
            doc.object_options.insert(id.clone(), opt);
        }
    }
    doc.virtual_arrays[k].sources = sources.iter().map(|o| o.id.clone()).collect();
    doc.virtual_arrays[k].clones = new_ids;
    Ok(())
}

/// Whether object `id` is on a page other than the active one.
fn on_other_page(doc: &Document, id: &str) -> bool {
    doc.pages.iter().any(|p| p.objects.iter().any(|o| o.id == id))
}

/// Bring every virtual array up to date after an edit (the session runs this after every command that changed the
/// document): originals that no longer exist leave their array, an array with none left is removed with its clones
/// (unless its originals are on another page), and the clones are laid out again from the originals. Objects marked
/// as clones that no array lists lose the mark.
pub fn sync_virtual_arrays(doc: &mut Document) -> Result<()> {
    let listed: HashSet<ObjectId> = doc.virtual_arrays.iter().flat_map(|a| a.clones.iter().cloned()).collect();
    for o in doc.objects.iter_mut() {
        if o.extras.contains_key(VIRTUAL_ARRAY_KEY) && !listed.contains(&o.id) {
            o.extras.remove(VIRTUAL_ARRAY_KEY);
        }
    }
    let mut k = 0;
    while k < doc.virtual_arrays.len() {
        let (sources, clones) = (doc.virtual_arrays[k].sources.clone(), doc.virtual_arrays[k].clones.clone());
        let here: Vec<ObjectId> = sources.iter().filter(|s| doc.object(s).is_some()).cloned().collect();
        if here.is_empty() {
            if sources.iter().any(|s| on_other_page(doc, s)) {
                k += 1;
                continue;
            }
            let clones: HashSet<ObjectId> = clones.into_iter().collect();
            doc.objects.retain(|o| !clones.contains(&o.id));
            for c in &clones {
                doc.object_options.remove(c);
                doc.locked.remove(c);
            }
            doc.virtual_arrays.remove(k);
            continue;
        }
        doc.virtual_arrays[k].sources = here;
        relayout(doc, k)?;
        k += 1;
    }
    Ok(())
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArraysInput {
    /// Virtual array ids (`virtual_arrays` in the document), or objects of them (an original or a clone).
    pub arrays: Vec<String>,
}

/// The arrays named by `names` (array ids or member objects), deduplicated.
fn arrays_named(cmd: &str, doc: &Document, names: &[String]) -> Result<Vec<String>> {
    if names.is_empty() {
        return Err(invalid(cmd, "arrays must list at least one virtual array"));
    }
    let mut out: Vec<String> = Vec::new();
    for n in names {
        let id = match doc.virtual_arrays.iter().find(|a| &a.id == n) {
            Some(a) => a.id.clone(),
            None => match array_of(doc, n) {
                Some(a) => a.id.clone(),
                None => doc.virtual_arrays[array_index(cmd, doc, n)?].id.clone(),
            },
        };
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// `array.flatten`.
pub struct ArrayFlatten;

impl Command for ArrayFlatten {
    type Input = ArraysInput;
    const ID: &'static str = "array.flatten";
    const TITLE: &'static str = "Flatten array";
    const DESCRIPTION: &'static str = "LightBurn's Flatten Array: the clones of virtual arrays become ordinary, \
        independent objects (same ids, no longer following the originals) and the arrays end. Returns created \
        (none), affected (the former clones) and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArraysInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let ids = arrays_named(Self::ID, doc, &i.arrays)?;
            let mut freed = Vec::new();
            for id in &ids {
                let k = array_index(Self::ID, doc, id)?;
                let a = doc.virtual_arrays.remove(k);
                for c in &a.clones {
                    if let Some(o) = doc.object_mut(c) {
                        o.extras.remove(VIRTUAL_ARRAY_KEY);
                        freed.push(c.clone());
                    }
                }
            }
            let mut r = util::edit_result(doc, &[], &freed);
            r["arrays"] = json!(ids);
            Ok((r, format!("flattened {}", ids.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayAddInput {
    /// The virtual array id.
    pub array: String,
    /// Exact object or group ids to add as originals (not already in a virtual array).
    pub ids: Vec<ObjectId>,
}

/// `array.add`.
pub struct ArrayAdd;

impl Command for ArrayAdd {
    type Input = ArrayAddInput;
    const ID: &'static str = "array.add";
    const TITLE: &'static str = "Add to array";
    const DESCRIPTION: &'static str = "LightBurn's Add to Array: objects become originals of a virtual array too and \
        get clones in every cell (laid out again from the larger selection). Returns created (new clones), \
        affected and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArrayAddInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let k = array_index(cmd, doc, &i.array)?;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            check_free(cmd, doc, &ids)?;
            let before: HashSet<ObjectId> = doc.virtual_arrays[k].clones.iter().cloned().collect();
            doc.virtual_arrays[k].sources.extend(ids.iter().cloned());
            relayout(doc, k)?;
            let a = &doc.virtual_arrays[k];
            let created: Vec<ObjectId> = a.clones.iter().filter(|c| !before.contains(*c)).cloned().collect();
            let affected: Vec<ObjectId> = a.clones.iter().filter(|c| before.contains(*c)).cloned().collect();
            let r = util::edit_result(doc, &created, &affected);
            Ok((r, format!("added {} to virtual array {}", ids.join(", "), i.array)))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayRemoveInput {
    /// Exact object or group ids: originals of virtual arrays.
    pub ids: Vec<ObjectId>,
}

/// `array.remove`.
pub struct ArrayRemove;

impl Command for ArrayRemove {
    type Input = ArrayRemoveInput;
    const ID: &'static str = "array.remove";
    const TITLE: &'static str = "Remove from array";
    const DESCRIPTION: &'static str = "LightBurn's Remove from Array: originals leave their virtual array (they stay \
        as ordinary objects) and their clones are deleted; an array left without originals ends. Returns affected, \
        deleted and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArrayRemoveInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let ids = util::object_ids(cmd, doc, &i.ids)?;
            if let Some(id) = ids.iter().find(|id| !doc.virtual_arrays.iter().any(|a| a.sources.contains(id))) {
                return Err(invalid(cmd, format!("{id} is not an original of a virtual array")));
            }
            let before: HashSet<ObjectId> = doc.virtual_arrays.iter().flat_map(|a| a.clones.iter().cloned()).collect();
            for a in doc.virtual_arrays.iter_mut() {
                a.sources.retain(|s| !ids.contains(s));
            }
            // Arrays left without originals end with their clones (sync_virtual_arrays sees no originals anywhere).
            sync_virtual_arrays(doc)?;
            let after: HashSet<ObjectId> = doc.virtual_arrays.iter().flat_map(|a| a.clones.iter().cloned()).collect();
            let mut deleted: Vec<&ObjectId> = before.difference(&after).collect();
            deleted.sort();
            let mut r = util::edit_result(doc, &[], &ids);
            r["deleted"] = json!(deleted);
            Ok((r, format!("removed {} from their virtual array", ids.join(", "))))
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayDuplicateInput {
    /// The virtual array id (or one of its objects).
    pub array: String,
    /// Offset [dx, dy] in field mm for the copy (default [0, 0], in place like LightBurn's Duplicate).
    #[serde(default)]
    pub offset: [f64; 2],
}

/// `array.duplicate`.
pub struct ArrayDuplicate;

impl Command for ArrayDuplicate {
    type Input = ArrayDuplicateInput;
    const ID: &'static str = "array.duplicate";
    const TITLE: &'static str = "Duplicate array";
    const DESCRIPTION: &'static str = "LightBurn's Duplicate Array: copy a virtual array's originals (moved by \
        offset) into a new virtual array with the same settings. Returns array (the new id), created (originals and \
        clones) and bounds.";
    const RISK: Risk = Risk::Edit;

    fn apply(doc: &mut Document, i: ArrayDuplicateInput) -> Result<(Value, String)> {
        transact(doc, |doc| {
            let cmd = Self::ID;
            let id = arrays_named(cmd, doc, std::slice::from_ref(&i.array))?.remove(0);
            let [dx, dy] = util::finite2(cmd, "offset", i.offset)?;
            let a = doc.virtual_arrays[array_index(cmd, doc, &id)?].clone();
            let sources = copy_objects(doc, &a.sources, Vec2::new(dx, dy), "")?;
            let new = doc.alloc_id("array")?;
            doc.virtual_arrays.push(VirtualArray { id: new.clone(), sources: sources.clone(), clones: vec![], settings: a.settings });
            let k = doc.virtual_arrays.len() - 1;
            relayout(doc, k)?;
            let created: Vec<ObjectId> = sources.iter().chain(&doc.virtual_arrays[k].clones).cloned().collect();
            let mut r = util::edit_result(doc, &created, &[]);
            r["array"] = json!(new);
            Ok((r, format!("duplicated virtual array {id} as {new}")))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::shape::ShapeEllipse;

    fn run<C: Command>(d: &mut Document, v: Value) -> Result<(Value, String)> {
        C::apply(d, serde_json::from_value(v).map_err(|e| invalid("t", e.to_string()))?)
    }

    #[test]
    fn grid_of_copies() {
        let mut d = Document::default();
        run::<ShapeEllipse>(&mut d, json!({"rx_mm": 1, "ry_mm": 1})).unwrap();
        let (r, _) =
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 2, "cols": 3, "pitch_x_mm": 5, "pitch_y_mm": -4}))
                .unwrap();
        assert_eq!(r["created"].as_array().unwrap().len(), 5);
        assert_eq!(d.objects.len(), 6);
        assert_eq!(r["bounds"]["min"], json!([-1.0, -5.0]));
        assert_eq!(r["bounds"]["max"], json!([11.0, 1.0]));
        assert_eq!(r["cells"][4], json!({"row": 1, "col": 2, "ids": ["obj-7"], "mirrored": false, "rotation_deg": null}));
    }

    #[test]
    fn gap_spacing_shifts_reverse_and_alternate_mirrors() {
        use crate::builtin::shape::ShapeRect;
        let mut d = Document::default();
        // A 4 x 2 rectangle centred on the origin.
        run::<ShapeRect>(&mut d, json!({"width_mm": 4, "height_mm": 2})).unwrap();
        let (r, _) = run::<Array>(
            &mut d,
            json!({"ids": ["obj-2"], "rows": 2, "cols": 3, "pitch_x_mm": 1, "pitch_y_mm": 0.5,
                   "spacing_x": "gap", "spacing_y": "gap", "row_shift": "half", "reverse_y": true}),
        )
        .unwrap();
        let centre = |id: &str| d.object(id).unwrap().bounds().center();
        // Columns 5 mm apart (4 + 1 gap), rows 2.5 mm apart toward -Y, row 1 shifted by half the width (2 mm).
        let cell = |k: usize| r["cells"][k]["ids"][0].as_str().unwrap().to_string();
        assert_eq!(centre(&cell(0)), kurbo::Point::new(5.0, 0.0));
        assert_eq!(centre(&cell(1)), kurbo::Point::new(10.0, 0.0));
        assert_eq!(centre(&cell(2)), kurbo::Point::new(2.0, -2.5));
        assert_eq!(centre(&cell(4)), kurbo::Point::new(12.0, -2.5));
        // Alternate columns mirrored left-right about their own centre; odd columns shifted up by 1 mm.
        let mut d = Document::default();
        let tri = "M 0 0 L 4 0 L 0 2 Z";
        run::<crate::builtin::shape::ShapePath>(&mut d, json!({"svg_path_data": tri})).unwrap();
        let (r, _) = run::<Array>(
            &mut d,
            json!({"ids": ["obj-2"], "rows": 1, "cols": 2, "pitch_x_mm": 10, "pitch_y_mm": 0,
                   "col_shift": 1, "mirror_alternate_cols": "x"}),
        )
        .unwrap();
        let copy = r["cells"][0]["ids"][0].as_str().unwrap();
        assert_eq!(r["cells"][0]["mirrored"], json!(true));
        let o = d.object("obj-2").unwrap().bounds();
        let c = d.object(copy).unwrap().bounds();
        assert!((c.x0 - o.x0 - 10.0).abs() < 1e-9 && (c.y0 - o.y0 - 1.0).abs() < 1e-9, "{o:?} {c:?}");
        // The right angle moved from the left side to the right side.
        let pts = d.object(copy).unwrap().field_path();
        let corner = pts.elements().iter().filter_map(|e| e.end_point()).find(|p| (p.y - c.y1).abs() < 1e-9).unwrap();
        assert!((corner.x - c.x1).abs() < 1e-9, "{corner:?} {c:?}");
        // A gap of minus the width would stack copies.
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 1, "cols": 2, "pitch_x_mm": -4, "pitch_y_mm": 0, "spacing_x": "gap"}))
                .is_err()
        );
    }

    #[test]
    fn refuses_degenerate_grids() {
        let mut d = Document::default();
        run::<ShapeEllipse>(&mut d, json!({"rx_mm": 1, "ry_mm": 1})).unwrap();
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 0, "cols": 3, "pitch_x_mm": 5, "pitch_y_mm": 5}))
                .is_err()
        );
        assert!(
            run::<Array>(&mut d, json!({"ids": ["obj-2"], "rows": 1, "cols": 3, "pitch_x_mm": 0, "pitch_y_mm": 0}))
                .is_err()
        );
        assert!(
            run::<Array>(
                &mut d,
                json!({"ids": ["obj-2"], "rows": 1000, "cols": 1000, "pitch_x_mm": 1, "pitch_y_mm": 1})
            )
            .is_err()
        );
        assert_eq!(d.objects.len(), 1);
    }

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

    fn new_session() -> (Session, String) {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let id = exec(&mut s, "shape.rect", json!({"width_mm": 4, "height_mm": 2}))["created"][0].as_str().unwrap().to_string();
        (s, id)
    }

    #[test]
    fn total_size_sets_the_counts_without_going_over() {
        let (mut s, id) = new_session();
        // 4 mm wide, 6 mm centre to centre: 4 + 6 (n - 1) <= 30 gives n = 5; rows 2 + 3 (n - 1) <= 10 gives 3.
        let r = exec(&mut s, "art.array", json!({"ids": [id], "pitch_x_mm": 6, "pitch_y_mm": 3, "total_width_mm": 30, "total_height_mm": 10}));
        assert_eq!((r["cols"].as_u64(), r["rows"].as_u64(), r["count"].as_u64()), (Some(5), Some(3), Some(15)));
        assert_eq!(r["size_mm"], json!([28.0, 8.0]));
        s.undo().unwrap();
        // Gap spacing and a half-row shift count too: step 4 + 2 = 6, shift 2: 4 + 2 + 6 (n - 1) <= 30 gives 5.
        let r = exec(&mut s, "art.array", json!({"ids": [id], "rows": 2, "pitch_x_mm": 2, "pitch_y_mm": 1, "spacing_x": "gap", "row_shift": "half", "total_width_mm": 30}));
        assert_eq!(r["cols"], json!(5));
        assert!(r["size_mm"][0].as_f64().unwrap() <= 30.0);
        s.undo().unwrap();
        assert!(fails(&mut s, "art.array", json!({"ids": [id], "pitch_x_mm": 6, "pitch_y_mm": 3, "total_width_mm": 3})).contains("smaller than the selection"));
        assert!(fails(&mut s, "art.array", json!({"ids": [id], "pitch_x_mm": 6, "pitch_y_mm": 3, "total_width_mm": -1})).contains("total_width_mm"));
    }

    #[test]
    fn random_orientation_is_seeded_and_group_and_select_results() {
        let (mut s, id) = new_session();
        let p = json!({"ids": [id], "rows": 2, "cols": 3, "pitch_x_mm": 10, "pitch_y_mm": 10, "random_orientation_seed": 42, "group_results": true});
        let a = exec(&mut s, "art.array", p.clone());
        let angles: Vec<f64> = a["cells"].as_array().unwrap().iter().map(|c| c["rotation_deg"].as_f64().unwrap()).collect();
        assert_eq!(angles.len(), 5);
        assert!(angles.iter().all(|a| (0.0..=360.0).contains(a)));
        assert!(angles.windows(2).any(|w| w[0] != w[1]));
        // Each copy turned about its own centre, which stays on the grid.
        let copy = a["cells"][0]["ids"][0].as_str().unwrap().to_string();
        let c = s.document().object(&copy).unwrap().bounds().center();
        assert!((c.x - 10.0).abs() < 1e-9 && c.y.abs() < 1e-9, "{c:?}");
        // One group holding everything, and that group is what to select.
        let g = a["group"].as_str().unwrap().to_string();
        assert_eq!(a["results"], json!([g]));
        assert_eq!(s.document().group_members(&g).len(), 6);
        s.undo().unwrap();
        let b = exec(&mut s, "art.array", p);
        assert_eq!(angles, b["cells"].as_array().unwrap().iter().map(|c| c["rotation_deg"].as_f64().unwrap()).collect::<Vec<_>>());
        s.undo().unwrap();
        let other = exec(&mut s, "art.array", json!({"ids": [id], "rows": 2, "cols": 3, "pitch_x_mm": 10, "pitch_y_mm": 10, "random_orientation_seed": 7}));
        assert_ne!(other["cells"][0]["rotation_deg"], json!(angles[0]));
        assert_eq!(other["results"].as_array().unwrap().len(), 6);
        assert!(other["group"].is_null());
    }

    #[test]
    fn auto_increment_raises_serial_offsets_per_copy() {
        let mut s = Session::new(Document { field_mm: Some(150.0), ..Document::default() });
        let font = json!({ "family": "Hershey Sans", "kind": "stroke" });
        let t = exec(&mut s, "shape.text", json!({ "text": "0ddd", "font": font, "variable": { "mode": "serial", "offset": 1 } }))["created"][0]
            .as_str()
            .unwrap()
            .to_string();
        let qr = exec(&mut s, "shape.barcode", json!({ "kind": "qr", "data": "d", "variable": {"mode": "serial"}, "center": [0, 20] }))["created"][0]
            .as_str()
            .unwrap()
            .to_string();
        exec(&mut s, "var.set_serial", json!({ "start": 1, "end": 999, "current": 5, "advance_by": 1 }));
        let r = exec(&mut s, "art.array", json!({"ids": [t, qr], "rows": 2, "cols": 2, "pitch_x_mm": 40, "pitch_y_mm": 40, "auto_increment": 3}));
        let d = s.document();
        let shown = |id: &str| match &d.object(id).unwrap().geometry {
            doc::Geometry::Text(t) => (t.variable.as_ref().unwrap().offset, t.shown.clone().unwrap()),
            _ => unreachable!(),
        };
        // Copies 1, 2, 3 (row by row) get offsets 1 + 3n: 4, 7, 10, showing 5 + offset.
        for (k, want) in [(0, 4), (1, 7), (2, 10)] {
            let id = r["cells"][k]["ids"][0].as_str().unwrap();
            assert_eq!(shown(id), (want, format!("{:03}", 5 + want)));
            let b = r["cells"][k]["ids"][1].as_str().unwrap();
            let params: Value = serde_json::from_str(&d.object(b).unwrap().extras[crate::BARCODE_KEY]).unwrap();
            assert_eq!(params["variable"]["offset"], json!(want - 1));
        }
        // The original is unchanged.
        assert_eq!(shown(&t), (1, "006".into()));
    }

    #[test]
    fn virtual_array_follows_its_originals() {
        let (mut s, id) = new_session();
        let r = exec(&mut s, "art.array", json!({"ids": [id], "rows": 1, "cols": 3, "pitch_x_mm": 10, "pitch_y_mm": 0, "virtual": true}));
        let arr = r["array"].as_str().unwrap().to_string();
        let clones: Vec<String> = r["created"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(clones.len(), 2);
        let d = s.document();
        assert_eq!(d.virtual_arrays[0].clones, clones);
        assert_eq!(d.object(&clones[1]).unwrap().extras[VIRTUAL_ARRAY_KEY], arr);
        assert!((d.object(&clones[1]).unwrap().bounds().center().x - 20.0).abs() < 1e-9);
        // Editing the original reshapes and moves every clone, in the same undo step.
        exec(&mut s, "object.transform", json!({"ids": [id], "scale": [2, 1], "translate": [0, 5]}));
        let b = s.document().object(&clones[1]).unwrap().bounds();
        assert!((b.width() - 8.0).abs() < 1e-9 && (b.center().y - 5.0).abs() < 1e-9, "{b:?}");
        s.undo().unwrap();
        assert!((s.document().object(&clones[1]).unwrap().bounds().width() - 4.0).abs() < 1e-9);
        // Edit Array: more cells keep the old clone ids first.
        let r = exec(&mut s, "art.array", json!({"edit_array": arr, "rows": 2, "cols": 3, "pitch_x_mm": 10, "pitch_y_mm": 10}));
        assert_eq!(r["count"], json!(6));
        let now = s.document().virtual_arrays[0].clones.clone();
        assert_eq!(now.len(), 5);
        assert_eq!(&now[..2], &clones[..]);
        // Add to Array and Duplicate Array.
        let e = exec(&mut s, "shape.ellipse", json!({"rx_mm": 1, "ry_mm": 1, "center": [0, -10]}))["created"][0].as_str().unwrap().to_string();
        let r = exec(&mut s, "array.add", json!({"array": arr, "ids": [e]}));
        assert_eq!(r["created"].as_array().unwrap().len(), 5);
        assert_eq!(s.document().virtual_arrays[0].clones.len(), 10);
        assert!(fails(&mut s, "array.add", json!({"array": arr, "ids": [e]})).contains("already belongs"));
        let r = exec(&mut s, "array.duplicate", json!({"array": arr, "offset": [0, 50]}));
        assert_eq!(r["created"].as_array().unwrap().len(), 12);
        assert_eq!(s.document().virtual_arrays.len(), 2);
        s.undo().unwrap();
        // Remove from Array: the ellipse stays, its clones go.
        let r = exec(&mut s, "array.remove", json!({"ids": [e]}));
        assert_eq!(r["deleted"].as_array().unwrap().len(), 5);
        assert!(s.document().object(&e).is_some());
        assert!(fails(&mut s, "array.remove", json!({"ids": [e]})).contains("not an original"));
        // Deleting the last original ends the array with its clones; undo brings both back.
        let n = s.document().objects.len();
        exec(&mut s, "object.delete", json!({"ids": [id]}));
        assert!(s.document().virtual_arrays.is_empty());
        assert_eq!(s.document().objects.len(), n - 6);
        s.undo().unwrap();
        // Flatten: the clones stay as ordinary objects that no longer follow.
        let r = exec(&mut s, "array.flatten", json!({"arrays": [arr]}));
        assert_eq!(r["affected"].as_array().unwrap().len(), 5);
        assert!(s.document().virtual_arrays.is_empty());
        assert!(!s.document().object(&now[0]).unwrap().extras.contains_key(VIRTUAL_ARRAY_KEY));
        exec(&mut s, "object.transform", json!({"ids": [id], "translate": [0, 7]}));
        assert!(s.document().object(&now[0]).unwrap().bounds().center().y.abs() < 1e-9);
        // Bad input.
        assert!(fails(&mut s, "array.flatten", json!({"arrays": ["array-404"]})).contains("no virtual array"));
        assert!(fails(&mut s, "art.array", json!({"ids": [id], "rows": 1, "cols": 2, "pitch_x_mm": 10, "pitch_y_mm": 0, "virtual": true, "group_results": true})).contains("group_results"));
    }

    #[test]
    fn virtual_array_on_another_page_is_kept() {
        let (mut s, id) = new_session();
        exec(&mut s, "art.array", json!({"ids": [id], "rows": 1, "cols": 2, "pitch_x_mm": 10, "pitch_y_mm": 0, "virtual": true}));
        exec(&mut s, "page.add", json!({}));
        assert_eq!(s.document().virtual_arrays.len(), 1);
        exec(&mut s, "shape.rect", json!({"width_mm": 1, "height_mm": 1}));
        assert_eq!(s.document().virtual_arrays.len(), 1);
        exec(&mut s, "page.activate", json!({"page": "Page 1"}));
        exec(&mut s, "object.transform", json!({"ids": [id], "translate": [0, 3]}));
        let c = s.document().virtual_arrays[0].clones[0].clone();
        assert!((s.document().object(&c).unwrap().bounds().center().y - 3.0).abs() < 1e-9);
    }
}
