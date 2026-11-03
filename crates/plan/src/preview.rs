//! Previews: what a job will do, as field-frame segments. [`simulate`] reads encoded packets back (what the board
//! would run); [`preview_plan`] draws a plan directly (cheap, for the live UI) and [`preview_document`] adds a
//! summary per layer (`GET /api/preview`).

use kurbo::{Affine, Point as KPoint};
use lmc::units::units_to_speed;
use lmc::{LASER_ENABLE_BIT, ListCommand, Packet, TraceKind, TraceState, decode, trace_from};
use serde::{Deserialize, Serialize};

use doc::{Cleanup, Document, FillMode, LayerId};

use crate::geom::{Point, dist};
use crate::job::{ImageRaster, JobPlan, PassPlan, Stats, pass_stats};
use crate::{PlanError, Target};

/// What the galvo does along a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// Laser off.
    Jump,
    /// Laser on (a zero-length mark is a dwell).
    Mark,
}

/// One straight motion in field-frame mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewSegment {
    pub kind: SegmentKind,
    pub from: Point,
    pub to: Point,
    /// The layer it belongs to; `None` from [`simulate`], which only sees packets.
    pub layer: Option<String>,
    /// Index of its pass in [`JobPlan::passes`] (burn order); `None` from [`simulate`]. A jump belongs to the pass
    /// whose polyline it travels to.
    #[serde(default)]
    pub pass: Option<usize>,
    /// A mark that fires: the master oscillator is open and the laser-enable port bit is set. A mark with
    /// `emits == false` moves the galvo at mark speed without light (a list bug if it ever shows up).
    pub emits: bool,
    /// The mark's own power in %, when it differs from its pass's (grayscale image runs, power scale); `None` from
    /// [`simulate`] and for marks at the pass's power.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_pct: Option<f64>,
    /// Estimated time from the start of the job to the end of this segment, s (the plan's time model: motion at the
    /// set speeds plus delays, pauses and dots; 0 from [`simulate`]). For a time slider and "start here".
    #[serde(default)]
    pub t_s: f64,
}

/// A previewed job (`GET /api/preview`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Preview {
    pub segments: Vec<PreviewSegment>,
    /// One entry per job pass in burn order, so [`PreviewSegment::pass`] indexes it; empty from [`simulate`].
    #[serde(default)]
    pub passes: Vec<PassPreview>,
    pub stats: Stats,
    /// One entry per document layer, in document order (burn order); empty from [`preview_plan`] and [`simulate`].
    #[serde(default)]
    pub layers: Vec<LayerPreview>,
    /// What the plan left out (e.g. images), from [`JobPlan::warnings`].
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The image passes' scan runs as rasters on their dot grids, for clients that draw them as textures instead of
    /// one segment per run (`GET /api/preview.bin`, the browser's preview). Not in the JSON: its segments hold every
    /// run. Empty from [`simulate`].
    #[serde(skip)]
    pub rasters: Vec<PreviewRaster>,
}

/// One image's scan runs in one pass, as a raster: segments `first..end` of the preview (its runs and the jumps
/// between them; the jump into the first run comes before) drawn as the dots they mark.
///
/// A dot is marked when a run covers its centre (a run that covers no centre, a sliver left by a mask, marks the dot
/// under its middle), at that run's power: so the raster shows exactly the plan's runs, dot for dot, at the plan's
/// power per dot, within the bounds of the dots the runs lie on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PreviewRaster {
    /// Index into [`Preview::passes`].
    pub pass: usize,
    pub first: usize,
    pub end: usize,
    /// Dot (column c, row r) covers `c..c + 1` x `r..r + 1` under this affine (grid units to field mm, kurbo's
    /// coefficients `[a, b, c, d, e, f]`: x = a·c + c·r + e, y = b·c + d·r + f). Cropped to the dots the runs mark.
    pub grid: [f64; 6],
    pub cols: usize,
    pub rows: usize,
    /// Per dot, rows from row 0 (burned first), `cols` per row: 0 unmarked, `k` marked at `powers[k - 1]`.
    pub levels: Vec<u8>,
    /// The power of each level, % (the runs' own powers: grey levels, power scale).
    pub powers: Vec<f64>,
    /// The pass's power, %.
    pub pass_power_pct: f64,
    /// Per row, in burn order: the first and one-past-the-last segment of its runs (a row without runs: an empty
    /// range where the previous row ended). Rows burn one after the other, so both columns never decrease.
    pub row_segments: Vec<[u32; 2]>,
    /// Per row: the column (grid units, continuous) where its first run starts and where its last run ends, in burn
    /// direction (a bidirectional job's every other row runs backwards); `[0, 0]` for a row without runs.
    pub row_span: Vec<[f32; 2]>,
}

/// Most dots one [`PreviewRaster`] may have (16 M); a preview holds at most four times that in all. Images past that
/// are left as segments.
pub const MAX_RASTER_DOTS: usize = 1 << 24;
const MAX_PREVIEW_RASTER_DOTS: usize = 1 << 26;

/// A [`PreviewRaster`] without its arrays, as the clients' formats carry it (the browser worker's meta JSON,
/// `GET /api/preview.bin`'s header): its arrays are in three shared ones ([`PackedRasters`]), from dot `levels_at`
/// and row `rows_at` on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RasterHeader {
    pub pass: usize,
    pub first: usize,
    pub end: usize,
    pub grid: [f64; 6],
    pub cols: usize,
    pub rows: usize,
    pub powers: Vec<f64>,
    pub pass_power_pct: f64,
    pub levels_at: usize,
    pub rows_at: usize,
}

/// A preview's rasters packed for a client: their headers and every raster's arrays one after the other.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PackedRasters {
    pub headers: Vec<RasterHeader>,
    /// [`PreviewRaster::levels`], concatenated.
    pub levels: Vec<u8>,
    /// [`PreviewRaster::row_segments`], two per row, concatenated.
    pub row_segments: Vec<u32>,
    /// [`PreviewRaster::row_span`], two per row, concatenated.
    pub row_span: Vec<f32>,
}

impl PackedRasters {
    pub fn new(rasters: &[PreviewRaster]) -> PackedRasters {
        let mut out = PackedRasters {
            headers: Vec::with_capacity(rasters.len()),
            levels: Vec::with_capacity(rasters.iter().map(|r| r.levels.len()).sum()),
            ..PackedRasters::default()
        };
        for r in rasters {
            out.headers.push(RasterHeader {
                pass: r.pass,
                first: r.first,
                end: r.end,
                grid: r.grid,
                cols: r.cols,
                rows: r.rows,
                powers: r.powers.clone(),
                pass_power_pct: r.pass_power_pct,
                levels_at: out.levels.len(),
                rows_at: out.row_segments.len() / 2,
            });
            out.levels.extend_from_slice(&r.levels);
            out.row_segments.extend(r.row_segments.iter().flatten());
            out.row_span.extend(r.row_span.iter().flatten());
        }
        out
    }

    /// The rasters back from their packed form (`None` when an array is too short for a header).
    pub fn unpack(&self) -> Option<Vec<PreviewRaster>> {
        self.headers
            .iter()
            .map(|h| {
                let dots = h.cols.checked_mul(h.rows)?;
                let rows = 2 * h.rows_at..2 * (h.rows_at + h.rows);
                Some(PreviewRaster {
                    pass: h.pass,
                    first: h.first,
                    end: h.end,
                    grid: h.grid,
                    cols: h.cols,
                    rows: h.rows,
                    levels: self.levels.get(h.levels_at..h.levels_at + dots)?.to_vec(),
                    powers: h.powers.clone(),
                    pass_power_pct: h.pass_power_pct,
                    row_segments: self.row_segments.get(rows.clone())?.chunks(2).map(|c| [c[0], c[1]]).collect(),
                    row_span: self.row_span.get(rows)?.chunks(2).map(|c| [c[0], c[1]]).collect(),
                })
            })
            .collect()
    }
}

/// One pass of a previewed job ([`crate::PassPlan`] without its polylines, plus its share of the stats).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PassPreview {
    pub layer_id: LayerId,
    /// 0 for the layer's main settings, `n` for `sub_layers[n - 1]`.
    pub sub_layer: usize,
    /// Repetition of those settings.
    pub repeat: u32,
    /// Straight mark segments.
    pub segments: usize,
    pub mark_length_mm: f64,
    /// Travel into and within the pass.
    pub jump_length_mm: f64,
    /// Motion and delays of the pass, as [`Stats::estimated_time_s`].
    pub estimated_time_s: f64,
}

/// One set of settings of a layer (its main settings or one sub-layer, as LightBurn's sub-layer tabs show them) and
/// what its passes contribute to the job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubLayerPreview {
    /// 0 for the main settings, `n` for `sub_layers[n - 1]` (as [`PassPreview::sub_layer`]).
    pub sub_layer: usize,
    /// The sub-layer's name, if it has one.
    pub name: Option<String>,
    /// Display label: "Main", the name, "Cleanup" for an unnamed cleanup, else "Sub-layer n".
    pub label: String,
    pub cleanup: Option<Cleanup>,
    pub mode: FillMode,
    pub power_pct: f64,
    pub speed_mm_s: f64,
    pub frequency_khz: f64,
    pub pulse_width_ns: Option<f64>,
    /// Repetitions asked for (per run, for a cleanup).
    pub passes: u32,
    /// Job passes planned with these settings (0 when they have nothing to mark or the layer no output).
    pub planned_passes: usize,
    pub segments: usize,
    pub mark_length_mm: f64,
    pub jump_length_mm: f64,
    pub estimated_time_s: f64,
}

/// What one layer contributes to a previewed job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerPreview {
    pub layer_id: LayerId,
    pub name: String,
    /// Display colour (CSS).
    pub color: String,
    /// The layer's own mode (sub-layers may differ).
    pub mode: FillMode,
    /// Whether the layer is in the job at all.
    pub output: bool,
    pub visible: bool,
    /// Passes planned for it (repeats and sub-layers; 0 when it has nothing to mark or no output).
    pub passes: usize,
    /// Straight mark segments.
    pub segments: usize,
    pub mark_length_mm: f64,
    /// Travel into and within its passes.
    pub jump_length_mm: f64,
    /// Time of its passes (motion and delays, as [`Stats::estimated_time_s`], without the job's start-up delay).
    pub estimated_time_s: f64,
    /// The main settings and every sub-layer, in list order, with their share of the totals above.
    #[serde(default)]
    pub sub_layers: Vec<SubLayerPreview>,
}

/// Draw `plan` without encoding: a jump from the field centre (or the previous polyline's end) to each polyline's
/// start, then its marks (a dot is a zero-length mark). Zero-length jumps are left out. Every segment carries the
/// estimated time at its end ([`PreviewSegment::t_s`]), in the same model as the plan's stats.
pub fn preview_plan(plan: &JobPlan) -> Preview {
    let mut rasters = Vec::new();
    let mut raster_dots = 0usize;
    let mut segments = Vec::new();
    let mut passes = Vec::with_capacity(plan.passes.len());
    let mut pos: Point = [0.0, 0.0];
    // The time before the first pass (the open-MO waits): what the stats have beyond the passes' own time.
    let pass_total: f64 = {
        let mut p: Point = [0.0, 0.0];
        plan.passes
            .iter()
            .map(|pass| {
                let (_, t, end) = pass_stats(pass, p);
                p = end;
                t
            })
            .sum()
    };
    let mut t_us = (plan.stats.estimated_time_s * 1e6 - pass_total).max(0.0);
    for (index, pass) in plan.passes.iter().enumerate() {
        let (s, _, _) = pass_stats(pass, pos);
        passes.push(PassPreview {
            layer_id: pass.layer_id.clone(),
            sub_layer: pass.sub_layer,
            repeat: pass.repeat,
            segments: s.segment_count,
            mark_length_mm: s.mark_length_mm,
            jump_length_mm: s.jump_length_mm,
            estimated_time_s: s.estimated_time_s,
        });
        let layer = Some(pass.layer_id.clone());
        let at = |kind, from, to, emits, power_pct, t_us: f64| PreviewSegment {
            kind,
            from,
            to,
            layer: layer.clone(),
            pass: Some(index),
            emits,
            power_pct,
            t_s: t_us / 1e6,
        };
        let tm = &pass.settings.timing;
        t_us += pass.pause_before_us;
        // This pass's image rasters (in polyline order), the one being drawn and its runs' mark segments.
        let images: Vec<&ImageRaster> = plan.rasters.iter().filter(|r| r.pass == index).collect();
        let mut image = 0usize;
        let mut image_first = 0usize;
        let mut marks: Vec<u32> = Vec::new();
        for (i, poly) in pass.polylines.iter().enumerate() {
            let Some(&first) = poly.first() else { continue };
            let jump = dist(pos, first);
            if jump > 0.0 {
                t_us += pass.settings.jump_us(jump) + crate::encode::jump_delay_us(tm, jump).max(0.0);
            }
            if first != pos {
                segments.push(at(SegmentKind::Jump, pos, first, false, None, t_us));
            }
            while images.get(image).is_some_and(|r| r.polylines.end <= i) {
                image += 1;
            }
            let run_of = images.get(image).filter(|r| r.polylines.contains(&i) && poly.len() == 2);
            if let Some(r) = run_of {
                if i == r.polylines.start {
                    image_first = segments.len();
                    marks.clear();
                }
                marks.push(u32::try_from(segments.len()).unwrap_or(u32::MAX));
            }
            let power = Some(pass.power_of(i)).filter(|p| *p != pass.settings.power_pct);
            t_us += tm.laser_on_delay_us.max(0.0);
            if poly.len() == 1 {
                t_us += pass.dot_time_us.unwrap_or(0.0);
                segments.push(at(SegmentKind::Mark, first, first, true, power, t_us));
            }
            // A gantry's start pause and overscan run-in come before the first mark, the rest after the last.
            let (before, after) = crate::gcode::extra_us_of(pass, i);
            t_us += before;
            let polygon = (tm.polygon_delay_us.max(0.0) / 10.0).trunc() * 10.0;
            for (k, w) in poly.windows(2).enumerate() {
                if k > 0 {
                    t_us += polygon;
                }
                t_us += pass.settings.mark_us(dist(w[0], w[1]));
                segments.push(at(SegmentKind::Mark, w[0], w[1], true, power, t_us));
            }
            t_us += after;
            t_us += tm.laser_off_delay_us.max(0.0) + tm.end_delay_us.max(0.0);
            if let Some(last) = segments.last_mut().filter(|s| s.kind == SegmentKind::Mark) {
                last.t_s = t_us / 1e6;
            }
            pos = poly.last().copied().unwrap_or(first);
            if let Some(r) = run_of.filter(|r| i + 1 == r.polylines.end) {
                let room = MAX_PREVIEW_RASTER_DOTS - raster_dots;
                if marks.len() == r.polylines.len()
                    && let Some(raster) = preview_raster(r, pass, &marks, image_first, segments.len(), room)
                {
                    raster_dots += raster.levels.len();
                    rasters.push(raster);
                }
            }
        }
        t_us += pass.pause_after_us;
    }
    Preview { segments, passes, stats: plan.stats, layers: Vec::new(), warnings: plan.warnings.clone(), rasters }
}

/// A run on a raster's grid: its row, the first and last dot it marks, and its start and end column (grid units).
type RunDots = (i64, i64, i64, f64, f64);

/// Where run `from`-`to` lies on the grid whose inverse is `inv` ([`PreviewRaster`]'s rule: the dots whose centres
/// it covers, else the dot under its middle); `None` when it does not run along one row's centre line.
fn run_dots(inv: &Affine, from: Point, to: Point) -> Option<RunDots> {
    let a = *inv * KPoint::new(from[0], from[1]);
    let b = *inv * KPoint::new(to[0], to[1]);
    let y = (a.y + b.y) / 2.0 - 0.5;
    let row = y.round();
    let on_row = (y - row).abs() <= 1e-3 && (a.y - b.y).abs() <= 1e-3;
    if !on_row || !a.x.is_finite() || !b.x.is_finite() {
        return None;
    }
    let (lo, hi) = (a.x.min(b.x), a.x.max(b.x));
    let (mut d0, mut d1) = ((lo - 0.5).ceil(), (hi - 0.5).floor());
    if d1 < d0 {
        d0 = ((lo + hi) / 2.0).floor();
        d1 = d0;
    }
    #[allow(clippy::cast_possible_truncation)]
    Some((row as i64, d0 as i64, d1 as i64, a.x, b.x))
}

/// Why raster `r` does not draw exactly `segments[r.first..r.end]` (a preview's segments), or `None` when it does:
/// every mark of the range lies on one row of the grid inside the raster (within half a dot), in burn order (rows and their segment
/// ranges never go back), its dots hold its power (its own, else `r.pass_power_pct`), no other dot is marked, and
/// every other segment of the range is a jump of the same pass. For tests of the clients' rasters.
pub fn raster_mismatch(segments: &[PreviewSegment], r: &PreviewRaster) -> Option<String> {
    let seg = segments.get(r.first..r.end)?;
    let grid = Affine::new(r.grid);
    if r.levels.len() != r.cols * r.rows || r.row_segments.len() != r.rows || r.row_span.len() != r.rows {
        return Some("array sizes do not match cols x rows".into());
    }
    let inv = grid.inverse();
    let mut covered = vec![false; r.levels.len()];
    let mut last_row = 0i64;
    for (k, s) in seg.iter().enumerate() {
        let i = r.first + k;
        if s.pass != Some(r.pass) {
            return Some(format!("segment {i} is not of pass {}", r.pass));
        }
        if s.kind == SegmentKind::Jump {
            continue;
        }
        let Some((row, d0, d1, ax, bx)) = run_dots(&inv, s.from, s.to) else {
            return Some(format!("segment {i} does not run along a row"));
        };
        let power = s.power_pct.unwrap_or(r.pass_power_pct);
        if row < last_row || row >= r.rows as i64 || d0 < 0 || d1 >= r.cols as i64 {
            return Some(format!("segment {i} (row {row}, dots {d0}..={d1}) is outside the raster or out of order"));
        }
        last_row = row;
        // A run may end part-way into a dot it does not mark (dot width correction, a mask): at most half a dot out.
        let tol = 0.5 + 1e-6;
        if ax.min(bx) < -tol || ax.max(bx) > r.cols as f64 + tol {
            return Some(format!("segment {i} reaches outside the raster's bounds"));
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let rr = row as usize;
        let [a, b] = r.row_segments[rr];
        if !(a as usize <= i && i < b as usize) {
            return Some(format!("segment {i} is not in its row's range {a}..{b}"));
        }
        for d in d0..=d1 {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let at = rr * r.cols + d as usize;
            covered[at] = true;
            let level = r.levels[at];
            if level == 0 || r.powers.get(usize::from(level) - 1) != Some(&power) {
                return Some(format!("dot {d} of row {row} has level {level}, not segment {i}'s power {power}"));
            }
        }
    }
    if let Some(at) = r.levels.iter().zip(&covered).position(|(l, c)| *l != 0 && !c) {
        return Some(format!("dot {at} is marked but no run covers it"));
    }
    if r.row_segments.windows(2).any(|w| w[1][0] < w[0][1] || w[1][1] < w[1][0]) {
        return Some("row ranges go back".into());
    }
    None
}

/// The raster of image runs `r` of `pass`, whose runs are the marks `marks` of segments `first..end`; `None` when
/// they do not lie on the grid's rows in burn order, would take more than `room` (or [`MAX_RASTER_DOTS`]) dots, or
/// have more than 255 powers: those runs stay segments to the clients.
fn preview_raster(r: &ImageRaster, pass: &PassPlan, marks: &[u32], first: usize, end: usize, room: usize) -> Option<PreviewRaster> {
    let grid = Affine::new(r.grid);
    let det = grid.determinant();
    if det.is_nan() || det.abs() <= 1e-18 {
        return None;
    }
    let inv = grid.inverse();
    let mut spans: Vec<RunDots> = Vec::with_capacity(marks.len());
    let (mut c0, mut c1, mut r0, mut r1) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
    for poly in &pass.polylines[r.polylines.clone()] {
        let s = run_dots(&inv, poly[0], poly[1])?;
        if spans.last().is_some_and(|p| p.0 > s.0) {
            return None;
        }
        (c0, c1, r0, r1) = (c0.min(s.1), c1.max(s.2), r0.min(s.0), r1.max(s.0));
        spans.push(s);
    }
    if spans.is_empty() {
        return None;
    }
    let (cols, rows) = (usize::try_from(c1 - c0 + 1).ok()?, usize::try_from(r1 - r0 + 1).ok()?);
    let dots = cols.checked_mul(rows)?;
    if dots > MAX_RASTER_DOTS.min(room) {
        return None;
    }
    let mut levels = vec![0u8; dots];
    let mut powers: Vec<f64> = Vec::new();
    let mut row_segments = vec![[u32::MAX; 2]; rows];
    let mut row_span = vec![[0f32; 2]; rows];
    #[allow(clippy::cast_precision_loss)]
    let (fc0, fr0) = (c0 as f64, r0 as f64);
    for (k, &(row, d0, d1, ax, bx)) in spans.iter().enumerate() {
        let power = pass.power_of(r.polylines.start + k);
        let level = match powers.iter().position(|p| p.to_bits() == power.to_bits()) {
            Some(at) => at + 1,
            None => {
                powers.push(power);
                powers.len()
            }
        };
        let level = u8::try_from(level).ok()?;
        let rr = usize::try_from(row - r0).ok()?;
        let line = &mut levels[rr * cols..(rr + 1) * cols];
        for d in d0..=d1 {
            line[usize::try_from(d - c0).ok()?] = level;
        }
        let m = marks[k];
        if row_segments[rr][0] == u32::MAX {
            row_segments[rr] = [m, m + 1];
            row_span[rr] = [(ax - fc0) as f32, (bx - fc0) as f32];
        } else {
            row_segments[rr][1] = m + 1;
            row_span[rr][1] = (bx - fc0) as f32;
        }
    }
    // Rows without runs: an empty range where the previous row ended (row 0 always has runs).
    for rr in 1..rows {
        if row_segments[rr][0] == u32::MAX {
            let at = row_segments[rr - 1][1];
            row_segments[rr] = [at, at];
        }
    }
    Some(PreviewRaster {
        pass: r.pass,
        first,
        end,
        grid: (grid * Affine::translate((fc0, fr0))).as_coeffs(),
        cols,
        rows,
        levels,
        powers,
        pass_power_pct: pass.settings.power_pct,
        row_segments,
        row_span,
    })
}

/// Multiply every estimated time of `preview` by `factor` (the stats, each pass, layer and sub-layer, and every
/// segment's `t_s`), so a learned correction (`engine::marktime`) keeps the totals and the timeline in step.
/// A factor that is not finite and positive changes nothing.
pub fn scale_times(preview: &mut Preview, factor: f64) {
    if !(factor.is_finite() && factor > 0.0) || factor == 1.0 {
        return;
    }
    preview.stats.estimated_time_s *= factor;
    preview.passes.iter_mut().for_each(|p| p.estimated_time_s *= factor);
    for l in &mut preview.layers {
        l.estimated_time_s *= factor;
        l.sub_layers.iter_mut().for_each(|s| s.estimated_time_s *= factor);
    }
    preview.segments.iter_mut().for_each(|s| s.t_s *= factor);
}

/// [`preview_plan`] plus a [`LayerPreview`] for every layer of `doc` (the document `plan` was planned from).
pub fn preview_document(doc: &Document, plan: &JobPlan) -> Preview {
    let mut preview = preview_plan(plan);
    let mut layers: Vec<LayerPreview> = doc
        .layers
        .iter()
        .map(|l| LayerPreview {
            layer_id: l.id.clone(),
            name: l.name.clone(),
            color: l.color.clone(),
            mode: l.settings.mode.clone(),
            output: l.output,
            visible: l.visible,
            passes: 0,
            segments: 0,
            mark_length_mm: 0.0,
            jump_length_mm: 0.0,
            estimated_time_s: 0.0,
            sub_layers: std::iter::once(&l.settings)
                .chain(&l.sub_layers)
                .enumerate()
                .map(|(n, s)| SubLayerPreview {
                    sub_layer: n,
                    name: if n == 0 { None } else { s.name.clone() },
                    label: sub_layer_label(n, s),
                    cleanup: if n == 0 { None } else { s.cleanup.clone() },
                    mode: s.mode.clone(),
                    power_pct: s.power_pct,
                    speed_mm_s: s.speed_mm_s,
                    frequency_khz: s.frequency_khz,
                    pulse_width_ns: s.pulse_width_ns,
                    passes: s.passes,
                    planned_passes: 0,
                    segments: 0,
                    mark_length_mm: 0.0,
                    jump_length_mm: 0.0,
                    estimated_time_s: 0.0,
                })
                .collect(),
        })
        .collect();
    for pass in &preview.passes {
        if let Some(l) = layers.iter_mut().find(|l| l.layer_id == pass.layer_id) {
            l.passes += 1;
            l.segments += pass.segments;
            l.mark_length_mm += pass.mark_length_mm;
            l.jump_length_mm += pass.jump_length_mm;
            l.estimated_time_s += pass.estimated_time_s;
            if let Some(s) = l.sub_layers.get_mut(pass.sub_layer) {
                s.planned_passes += 1;
                s.segments += pass.segments;
                s.mark_length_mm += pass.mark_length_mm;
                s.jump_length_mm += pass.jump_length_mm;
                s.estimated_time_s += pass.estimated_time_s;
            }
        }
    }
    preview.layers = layers;
    preview
}

/// "Main" for the main settings (`n` 0); a sub-layer's name, "Cleanup" for an unnamed cleanup, else "Sub-layer n".
fn sub_layer_label(n: usize, s: &doc::CutSettings) -> String {
    match (n, &s.name, &s.cleanup) {
        (0, _, _) => "Main".into(),
        (_, Some(name), _) => name.clone(),
        (_, None, Some(_)) => "Cleanup".into(),
        (_, None, None) => format!("Sub-layer {n}"),
    }
}

/// The rest of `plan` from preview segment `segment` on ([`preview_plan`]'s numbering: LightBurn's "Start Here" on
/// the preview slider). A jump starts the job at the polyline it travels to; a mark starts it at that mark's start
/// point, on the same polyline. Earlier passes and polylines are dropped; the stats are recomputed for `target`.
/// Refused for a multi-part plan (rotary or Z) and for an index past the end.
pub fn start_at(plan: &JobPlan, segment: usize, target: &Target) -> Result<JobPlan, PlanError> {
    if plan.parts.len() > 1 {
        return Err(PlanError::Target("start here is not available for rotary or Z-stepped jobs".into()));
    }
    let mut n = 0usize;
    let mut pos: Point = [0.0, 0.0];
    for (pi, pass) in plan.passes.iter().enumerate() {
        for (li, poly) in pass.polylines.iter().enumerate() {
            let Some(&first) = poly.first() else { continue };
            // (first point index of the remainder) if `segment` falls in this polyline.
            let mut cut: Option<usize> = None;
            if first != pos {
                if n == segment {
                    cut = Some(0);
                }
                n += 1;
            }
            let marks = poly.len().saturating_sub(1).max(usize::from(poly.len() == 1));
            if cut.is_none() && segment < n + marks {
                cut = Some(if poly.len() == 1 { 0 } else { segment - n });
            }
            if let Some(k) = cut {
                let mut out = JobPlan { passes: plan.passes[pi..].to_vec(), stats: Stats::default(), warnings: plan.warnings.clone(), parts: plan.parts.clone(), rasters: Vec::new() };
                let p = &mut out.passes[0];
                p.polylines.drain(..li);
                if !p.powers.is_empty() {
                    p.powers.drain(..li);
                }
                p.polylines[0].drain(..k);
                p.pause_before_us = 0.0;
                p.gantry.image_runs = p.gantry.image_runs.saturating_sub(li);
                if k > 0 && !p.gantry.baked && (p.gantry.lead_in.is_some() || p.gantry.lead_out.is_some()) {
                    // A GRBL lead at a path resumed part-way would be measured from the resume point, and for a
                    // closed outline (now open) on the wrong side, into the part: that polyline is cut on its own,
                    // without leads.
                    let mut head = p.clone();
                    head.polylines.truncate(1);
                    head.powers.truncate(1);
                    head.gantry.lead_in = None;
                    head.gantry.lead_out = None;
                    p.polylines.remove(0);
                    if !p.powers.is_empty() {
                        p.powers.remove(0);
                    }
                    p.gantry.image_runs = p.gantry.image_runs.saturating_sub(1);
                    if p.polylines.is_empty() {
                        out.passes[0] = head;
                    } else {
                        head.pause_after_us = 0.0;
                        out.passes.insert(0, head);
                    }
                }
                out.stats = crate::job::plan_stats(&out, target);
                return Ok(out);
            }
            n += marks;
            pos = poly.last().copied().unwrap_or(first);
        }
    }
    Err(PlanError::Target(format!("segment {segment} is past the end of the job ({n} segments)")))
}

impl Preview {
    /// Keep only the segments and layer entries of `layers` (stats stay those of the whole job, and `passes` stays
    /// whole so the segments' pass indices still point into it).
    pub fn retain_layers(&mut self, layers: &[&str]) {
        self.segments.retain(|s| s.layer.as_deref().is_some_and(|l| layers.contains(&l)));
        self.layers.retain(|l| layers.contains(&l.layer_id.as_str()));
    }
}

/// Read packets back into field-frame segments with `lmc::trace_from`, starting from the state `machine`'s run
/// prelude leaves the board in: galvo at the field centre, MO open (`Fiber_SetMo(1)`) and the port at the
/// laser-enable bit. The list itself never opens the MO (see the `encode` module), so a mark emits while the list
/// keeps the laser-enable bit set.
pub fn simulate(packets: &[Packet], target: &Target) -> Preview {
    let start = TraceState { mo_open: true, port: LASER_ENABLE_BIT, ..TraceState::default() };
    let (trace, _) = trace_from(packets, start);
    let segments: Vec<PreviewSegment> = trace
        .iter()
        .map(|s| PreviewSegment {
            kind: match s.kind {
                TraceKind::Jump => SegmentKind::Jump,
                TraceKind::Mark => SegmentKind::Mark,
            },
            from: target.from_galvo(s.from),
            to: target.from_galvo(s.to),
            layer: None,
            pass: None,
            emits: s.emits(),
            power_pct: None,
            t_s: 0.0,
        })
        .collect();
    let mut stats = Stats::default();
    let mut in_run = false;
    for s in &segments {
        let len = dist(s.from, s.to);
        match s.kind {
            SegmentKind::Jump => {
                stats.jump_length_mm += len;
                in_run = false;
            }
            SegmentKind::Mark => {
                stats.mark_length_mm += len;
                stats.segment_count += 1;
                if !in_run {
                    stats.polyline_count += 1;
                }
                in_run = true;
            }
        }
    }
    stats.estimated_time_s = list_time_us(&decode(packets), target) / 1e6;
    Preview { segments, passes: Vec::new(), stats, layers: Vec::new(), warnings: Vec::new(), rasters: Vec::new() }
}

/// Estimated run time of a decoded list in µs, with the same model as the plan's estimate: motion at the speeds the
/// words give (times [`Target::motion_time_scale`]), jump delays, laser on/off delays per mark run, polygon delays
/// between consecutive marks, and explicit waits.
fn list_time_us(cmds: &[ListCommand], target: &Target) -> f64 {
    let scale = target.motion_time_scale;
    let unit = target.galvo_unit_mm();
    let speed = |units: u16| units_to_speed(units, target.transform().speed_field_mm());
    let (mut jump_speed, mut mark_speed) = (0.0, 0.0);
    let (mut on, mut off, mut polygon, mut jump_delay) = (0.0, 0.0, 0.0, 0.0);
    let mut pos = (lmc::CENTER, lmc::CENTER);
    let mut prev_mark = false;
    let mut t = 0.0;
    let galvo_dist = |a: (u16, u16), b: (u16, u16)| {
        (f64::from(a.0) - f64::from(b.0)).hypot(f64::from(a.1) - f64::from(b.1)) * unit
    };
    for c in cmds {
        let mut is_mark = false;
        match *c {
            ListCommand::JumpSpeed { units } => jump_speed = speed(units),
            ListCommand::MarkSpeed { units } => mark_speed = speed(units),
            ListCommand::LaserOnDelay(d) => on = d.micros().max(0.0),
            ListCommand::LaserOffDelay(d) => off = d.micros().max(0.0),
            ListCommand::PolygonDelay { time_10us } => polygon = f64::from(time_10us) * 10.0,
            ListCommand::JumpDelay(d) => jump_delay = d.micros().max(0.0),
            ListCommand::DelayTime { time_10us } | ListCommand::LaserOnPoint { time_10us } => t += f64::from(time_10us) * 10.0,
            ListCommand::JumpTo { x, y, .. } => {
                if jump_speed > 0.0 {
                    t += galvo_dist(pos, (x, y)) / jump_speed * 1e6 * scale;
                }
                t += jump_delay;
                pos = (x, y);
            }
            ListCommand::MarkTo { x, y, .. } => {
                if mark_speed > 0.0 {
                    t += galvo_dist(pos, (x, y)) / mark_speed * 1e6 * scale;
                }
                t += if prev_mark { polygon } else { on + off };
                is_mark = true;
                pos = (x, y);
            }
            _ => {}
        }
        prev_mark = is_mark;
    }
    t
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_job;
    use crate::target::tests::target;
    use doc::{Geometry, Layer, Object};
    use kurbo::Affine;

    fn rect(id: &str, layer: &str, at: (f64, f64)) -> Object {
        let g = Geometry::Rect { width_mm: 10.0, height_mm: 10.0, corner_radius_mm: 0.0 };
        Object::new(id.into(), id, layer.into(), g, Affine::translate(at))
    }

    #[test]
    fn per_layer_totals_add_up_and_segments_carry_their_pass() {
        let mut d = Document::default();
        let mut engrave = Layer::new("layer-7".into(), "Engrave", "#ff0000");
        engrave.settings.mode = FillMode::Fill;
        engrave.settings.passes = 2;
        d.layers.push(engrave);
        d.layers.push(Layer { output: false, ..Layer::new("layer-8".into(), "Off", "#0000ff") });
        d.objects = vec![rect("obj-a", "layer-1", (-20.0, 0.0)), rect("obj-b", "layer-7", (20.0, 0.0)), rect("obj-c", "layer-8", (0.0, 30.0))];
        let plan = plan_job(&d, &target()).unwrap();
        let p = preview_document(&d, &plan);
        assert_eq!(p.layers.iter().map(|l| l.layer_id.as_str()).collect::<Vec<_>>(), ["layer-1", "layer-7", "layer-8"]);
        let (line, fill, off) = (&p.layers[0], &p.layers[1], &p.layers[2]);
        assert_eq!((line.passes, fill.passes, off.passes), (1, 2, 0));
        assert!(!off.output && off.segments == 0 && off.estimated_time_s == 0.0);
        assert_eq!(line.segments + fill.segments, p.stats.segment_count);
        assert!((line.mark_length_mm + fill.mark_length_mm - p.stats.mark_length_mm).abs() < 1e-9);
        assert!((line.jump_length_mm + fill.jump_length_mm - p.stats.jump_length_mm).abs() < 1e-9);
        let total = line.estimated_time_s + fill.estimated_time_s;
        assert!(total > 0.0 && total <= p.stats.estimated_time_s, "{total} vs {}", p.stats.estimated_time_s);
        // Every segment names its layer and pass, and the pass index points at that layer's pass.
        for s in &p.segments {
            let pass = s.pass.expect("planned segments carry their pass");
            assert_eq!(s.layer.as_deref(), Some(plan.passes[pass].layer_id.as_str()));
        }
        assert!(p.segments.iter().any(|s| s.pass == Some(2)), "the second fill pass is there");
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["layers"][1]["mode"], "fill");
        assert!(v["segments"][0]["pass"].is_u64());

        let mut only = p.clone();
        only.retain_layers(&["layer-7"]);
        assert!(only.segments.iter().all(|s| s.layer.as_deref() == Some("layer-7")));
        assert_eq!(only.segments.len(), p.segments.iter().filter(|s| s.layer.as_deref() == Some("layer-7")).count());
        assert_eq!(only.layers.len(), 1);
        assert_eq!(only.stats, p.stats, "stats stay the whole job's");
    }
}
