//! Job settings that are not laser parameters: LightBurn's per-layer cut options ([`CutOptions`], on every
//! [`crate::CutSettings`]), the Optimization Settings of a project ([`Optimization`]), per-shape cut properties
//! ([`ObjectOptions`]: cut order priority and power scale) and the galvo rotary setup saved with a project
//! ([`Rotary`]). All are optional in files and left out of JSON while they are the defaults, so older files load and
//! new ones stay small. `plan` reads them; `commands` edits them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

/// How a fill layer groups shapes into one hatch (LightBurn's Fill Grouping, `scanOpt`).
///
/// New settings ([`CutOptions::default`], so every new layer) fill `all_at_once`, LightBurn's default. In JSON the
/// field is left out while `individually`, and a missing field reads as `individually` (this type's `Default`), which
/// is what every document saved before new layers changed to `all_at_once` meant by leaving it out; so those load
/// unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FillGrouping {
    /// Each object is hatched on its own (even-odd within the object), shape after shape. What a missing field means
    /// (older files).
    #[default]
    Individually,
    /// Every shape of the layer in one hatch (even-odd across shapes: overlaps cancel, as in LightBurn): one sweep of
    /// scanlines across the layer (shapes with different power scales are hatched apart). LightBurn's default and
    /// that of new settings.
    AllAtOnce,
    /// The shapes of each top-level group in one hatch; ungrouped objects on their own.
    ByGroup,
}

impl FillGrouping {
    fn is_default(&self) -> bool {
        *self == FillGrouping::default()
    }
}

/// Dot mode (LightBurn's Dot Mode for galvo line layers): instead of marking lines, fire a dot every `spacing_mm`
/// along them for `dot_time_ms`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DotMode {
    /// How long each dot fires, ms (0.01..=1000).
    pub dot_time_ms: f64,
    /// Distance between dots, mm (> 0).
    pub dot_spacing_mm: f64,
}

/// Wobble (LightBurn's galvo Wobble): the beam circles while it moves along a line, widening it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wobble {
    /// Distance travelled along the line per circle, mm (> 0).
    pub step_mm: f64,
    /// Circle diameter, mm (> 0).
    pub size_mm: f64,
}

/// Perforation mode: mark `cut_mm`, skip `skip_mm`, repeating along each line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Perforation {
    pub cut_mm: f64,
    pub skip_mm: f64,
}

fn yes() -> bool {
    true
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// How a layer's tabs are made (LightBurn's Tab Generation).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TabGeneration {
    /// Spread evenly on every closed outline (by `count` or `spacing_mm`); a shape with tabs placed by hand
    /// ([`ObjectOptions::tabs`]) uses those instead. Default (what OpenEtch's tabs always did).
    #[default]
    Auto,
    /// Only the tabs placed by hand.
    Manual,
}

impl TabGeneration {
    fn is_auto(&self) -> bool {
        *self == TabGeneration::Auto
    }
}

/// Tabs (bridges) left unmarked on closed shapes so cut parts stay in place (LightBurn's Tabs / Bridges).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tabs {
    /// The layer's Tabs / Bridges switch: off keeps these settings and the hand-placed tabs but cuts through
    /// (default on).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// auto (default) or manual (hand-placed tabs only).
    #[serde(default, skip_serializing_if = "TabGeneration::is_auto")]
    pub generation: TabGeneration,
    /// Tabs per shape, spread evenly (used when `spacing_mm` is 0).
    #[serde(default)]
    pub count: u32,
    /// Distance between tabs along the outline, mm; 0 uses `count`.
    #[serde(default)]
    pub spacing_mm: f64,
    /// With `spacing_mm`: at most this many tabs per outline (LightBurn's Limit Max Tabs); 0 is no limit.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub max_tabs: u32,
    /// Length of each tab (the gap left), mm.
    pub size_mm: f64,
    /// No automatic tabs on closed outlines inside another closed outline of the same pass (LightBurn's Skip Inner
    /// Shapes; hand-placed tabs are kept).
    #[serde(default, skip_serializing_if = "is_false")]
    pub skip_inner: bool,
    /// Mark the tabs at this percentage of the pass's power instead of leaving them uncut (LightBurn's Tab Cut
    /// Power), 0..=100; 0 (default) leaves a gap.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cut_power_pct: f64,
}

impl Default for Tabs {
    fn default() -> Self {
        Tabs {
            enabled: true,
            generation: TabGeneration::Auto,
            count: 4,
            spacing_mm: 0.0,
            max_tabs: 0,
            size_mm: 0.5,
            skip_inner: false,
            cut_power_pct: 0.0,
        }
    }
}

/// Most tabs placed by hand on one shape.
pub const MAX_MANUAL_TABS: usize = 1000;

/// The shape of a lead-in or lead-out (LightBurn's Lead-In / Lead-Out style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeadStyle {
    /// A straight line.
    #[default]
    Line,
    /// A short arc that meets the cut along its direction.
    Arc,
}

/// A lead-in or lead-out (LightBurn's Line-mode Lead-In / Lead-Out, GRBL gantries only): a short cut added before the
/// start (or after the end) of every path, so the mark a cut leaves where it starts and stops lands off the shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lead {
    #[serde(default)]
    pub style: LeadStyle,
    /// Length of the lead, mm (straight-line distance from the path's start or end to the lead's far end), > 0 and at
    /// most [`MAX_GANTRY_EXTRA_MM`].
    pub length_mm: f64,
    /// Angle between the lead and the path's own direction, degrees, -90..=90: positive puts the lead outside a closed
    /// shape (for a hole, an outline inside another one of the same pass, inside the hole: the waste side) and above
    /// an open path (to the left of its direction), negative on the other side; 90 meets the path square, 0 runs
    /// along it.
    pub angle_deg: f64,
}

/// Per-pass cut options (LightBurn's layer settings beyond the laser parameters). The defaults change nothing; they
/// fill all at once ([`FillGrouping`]), as LightBurn does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CutOptions {
    /// These settings mark (LightBurn's per-sub-layer output toggle). A layer's own output is [`crate::Layer::output`];
    /// this switches off one sub-layer (or the main settings while its sub-layers still run).
    #[serde(skip_serializing_if = "is_true")]
    pub output: bool,
    /// Wait before each pass of these settings, ms (LightBurn's Pause Before).
    #[serde(skip_serializing_if = "is_zero")]
    pub pause_before_ms: f64,
    /// Wait after each pass, ms (Pause After).
    #[serde(skip_serializing_if = "is_zero")]
    pub pause_after_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dot_mode: Option<DotMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wobble: Option<Wobble>,
    /// Fill: shorten both ends of every hatch line by half this width, mm (LightBurn's Dot Width Adjust for fills).
    #[serde(skip_serializing_if = "is_zero")]
    pub fill_dot_width_mm: f64,
    /// Fill grouping. `all_at_once` in new settings; missing in JSON reads as `individually` (see [`FillGrouping`]).
    #[serde(default, skip_serializing_if = "FillGrouping::is_default")]
    pub fill_grouping: FillGrouping,
    /// Fill: turn the hatch by `hatch.angle_increment_deg` on every run of the job instead of every pass
    /// (LightBurn's Auto-rotate).
    #[serde(skip_serializing_if = "is_false")]
    pub auto_rotate: bool,
    /// Fill: hatch each connected region on its own instead of whole scan lines across gaps (LightBurn's Flood
    /// Fill), so travel stays inside a region.
    #[serde(skip_serializing_if = "is_false")]
    pub flood_fill: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perforation: Option<Perforation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Tabs>,
    /// Z position of the first pass, mm (needs a board-driven Z axis; else reported and ignored).
    #[serde(skip_serializing_if = "is_zero")]
    pub z_offset_mm: f64,
    /// Z change per pass, mm (LightBurn's Z step per pass).
    #[serde(skip_serializing_if = "is_zero")]
    pub z_step_mm: f64,
    /// Line passes: offset closed outlines outward (positive) or inward (negative) by this much, mm (LightBurn's
    /// Kerf Offset, to make up for the beam width when cutting parts). Open paths are not offset.
    #[serde(skip_serializing_if = "is_zero")]
    pub kerf_offset_mm: f64,
    /// GRBL gantries only (LightBurn's GRBL "Constant Power Mode"): these passes use constant power, `M3`, even when
    /// the device's default is dynamic power `M4` (power then does not follow speed, so it suits fills at an even
    /// speed). Ignored by galvo jobs.
    #[serde(skip_serializing_if = "is_false")]
    pub constant_power: bool,
    /// GRBL gantries only (LightBurn's per-layer Air Assist): air assist (`M8`, coolant flood) is on while these
    /// passes mark and off (`M9`) for passes without it and at the end of the job. Ignored by galvo jobs.
    #[serde(skip_serializing_if = "is_false")]
    pub air_assist: bool,
    /// GRBL gantries only (LightBurn's Overscanning for fills and images): every straight scan line (hatch line or
    /// image run) is approached from this far before its start and left this far past its end with the laser off
    /// (`S0`), so the head is at speed while it marks. Shortened at the bed's edge. 0..=50 mm; 0 is off. Ignored by
    /// galvo jobs (a galvo uses its delays instead).
    #[serde(skip_serializing_if = "is_zero")]
    pub overscan_mm: f64,
    /// GRBL gantries only (LightBurn's Overcut for line cuts): a closed outline is cut on past its start point by this
    /// much (at most once around), so the start and end of the cut overlap. 0..=50 mm; 0 is off. Ignored by galvo
    /// jobs.
    #[serde(skip_serializing_if = "is_zero")]
    pub overcut_mm: f64,
    /// GRBL gantries only (LightBurn's Fill-mode Ramp Length, for stamps): the power of a fill rises from
    /// `ramp_min_power_pct` at the edge of the design to the layer's power this far into the fill, in steps, so the
    /// engraving's sides slope. Only the edges inside the fill (holes, the design's own outlines) ramp unless
    /// `ramp_outer_edge`. 0..=50 mm; 0 is off. Ignored by galvo jobs and by non-fill passes.
    #[serde(skip_serializing_if = "is_zero")]
    pub ramp_length_mm: f64,
    /// GRBL ramp: also ramp the outermost edges of the fill (LightBurn's Ramp Outer Edge).
    #[serde(skip_serializing_if = "is_false")]
    pub ramp_outer_edge: bool,
    /// GRBL ramp: the power at the edge, % (LightBurn ramps from Min Power to Max Power); at most the layer's power.
    #[serde(skip_serializing_if = "is_zero")]
    pub ramp_min_power_pct: f64,
    /// GRBL gantries only, line passes: a lead-in cut before each path's start (LightBurn's Lead-In).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_in: Option<Lead>,
    /// GRBL gantries only, line passes: a lead-out cut after each path's end (LightBurn's Lead-Out).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_out: Option<Lead>,
    /// GRBL gantries only, line passes: a dwell with the laser off before each path is cut (after the travel to it),
    /// ms (LightBurn's Start Pause Time without Cut Through). 0..=60,000 ms.
    #[serde(skip_serializing_if = "is_zero")]
    pub start_pause_ms: f64,
    /// GRBL line passes: a dwell with the laser off after each path, ms (LightBurn's End Pause Time).
    #[serde(skip_serializing_if = "is_zero")]
    pub end_pause_ms: f64,
}

impl Default for CutOptions {
    fn default() -> Self {
        CutOptions {
            output: true,
            pause_before_ms: 0.0,
            pause_after_ms: 0.0,
            dot_mode: None,
            wobble: None,
            fill_dot_width_mm: 0.0,
            fill_grouping: FillGrouping::AllAtOnce,
            auto_rotate: false,
            flood_fill: false,
            perforation: None,
            tabs: None,
            z_offset_mm: 0.0,
            z_step_mm: 0.0,
            kerf_offset_mm: 0.0,
            constant_power: false,
            air_assist: false,
            overscan_mm: 0.0,
            overcut_mm: 0.0,
            ramp_length_mm: 0.0,
            ramp_outer_edge: false,
            ramp_min_power_pct: 0.0,
            lead_in: None,
            lead_out: None,
            start_pause_ms: 0.0,
            end_pause_ms: 0.0,
        }
    }
}

/// Longest pause before or after a pass, ms.
pub const MAX_PAUSE_MS: f64 = 600_000.0;
/// Longest overscan and overcut, mm ([`CutOptions::overscan_mm`], [`CutOptions::overcut_mm`]).
pub const MAX_GANTRY_EXTRA_MM: f64 = 50.0;
/// Longest start or end pause of a GRBL path, ms ([`CutOptions::start_pause_ms`]).
pub const MAX_PATH_PAUSE_MS: f64 = 60_000.0;

impl CutOptions {
    pub fn is_default(&self) -> bool {
        *self == CutOptions::default()
    }

    /// The options of settings saved without any (`{}` or no `options` at all): the defaults, but filling
    /// shapes individually as OpenEtch did before new settings filled all at once.
    pub fn absent() -> Self {
        CutOptions { fill_grouping: FillGrouping::Individually, ..CutOptions::default() }
    }

    /// Why these options cannot be planned, if they cannot (the field and the rule).
    pub fn problem(&self) -> Option<String> {
        let range = |name: &str, v: f64, lo: f64, hi: f64| {
            (!(v.is_finite() && v >= lo && v <= hi)).then(|| format!("options.{name} must be within {lo}..={hi} (got {v})"))
        };
        let positive = |name: &str, v: f64| (!(v.is_finite() && v > 0.0)).then(|| format!("options.{name} must be > 0 (got {v})"));
        range("pause_before_ms", self.pause_before_ms, 0.0, MAX_PAUSE_MS)
            .or_else(|| range("pause_after_ms", self.pause_after_ms, 0.0, MAX_PAUSE_MS))
            .or_else(|| range("fill_dot_width_mm", self.fill_dot_width_mm, 0.0, 10.0))
            .or_else(|| range("z_offset_mm", self.z_offset_mm, -1000.0, 1000.0))
            .or_else(|| range("z_step_mm", self.z_step_mm, -100.0, 100.0))
            .or_else(|| range("kerf_offset_mm", self.kerf_offset_mm, -50.0, 50.0))
            .or_else(|| range("overscan_mm", self.overscan_mm, 0.0, MAX_GANTRY_EXTRA_MM))
            .or_else(|| range("overcut_mm", self.overcut_mm, 0.0, MAX_GANTRY_EXTRA_MM))
            .or_else(|| range("ramp_length_mm", self.ramp_length_mm, 0.0, MAX_GANTRY_EXTRA_MM))
            .or_else(|| range("ramp_min_power_pct", self.ramp_min_power_pct, 0.0, 100.0))
            .or_else(|| range("start_pause_ms", self.start_pause_ms, 0.0, MAX_PATH_PAUSE_MS))
            .or_else(|| range("end_pause_ms", self.end_pause_ms, 0.0, MAX_PATH_PAUSE_MS))
            .or_else(|| {
                [("lead_in", &self.lead_in), ("lead_out", &self.lead_out)].into_iter().find_map(|(name, l)| {
                    l.as_ref().and_then(|l| {
                        (!(l.length_mm.is_finite() && l.length_mm > 0.0 && l.length_mm <= MAX_GANTRY_EXTRA_MM))
                            .then(|| format!("options.{name}.length_mm must be within 0..={MAX_GANTRY_EXTRA_MM} and > 0 (got {})", l.length_mm))
                            .or_else(|| range(&format!("{name}.angle_deg"), l.angle_deg, -90.0, 90.0))
                    })
                })
            })
            .or_else(|| {
                self.dot_mode.as_ref().and_then(|d| {
                    range("dot_mode.dot_time_ms", d.dot_time_ms, 0.01, 1000.0)
                        .or_else(|| range("dot_mode.dot_spacing_mm", d.dot_spacing_mm, 0.001, 1000.0))
                })
            })
            .or_else(|| {
                self.wobble.as_ref().and_then(|w| {
                    range("wobble.step_mm", w.step_mm, 0.001, 100.0).or_else(|| range("wobble.size_mm", w.size_mm, 0.001, 100.0))
                })
            })
            .or_else(|| {
                self.perforation.as_ref().and_then(|p| positive("perforation.cut_mm", p.cut_mm).or_else(|| positive("perforation.skip_mm", p.skip_mm)))
            })
            .or_else(|| {
                self.tabs.as_ref().and_then(|t| {
                    positive("tabs.size_mm", t.size_mm)
                        .or_else(|| range("tabs.spacing_mm", t.spacing_mm, 0.0, 1e6))
                        .or_else(|| range("tabs.cut_power_pct", t.cut_power_pct, 0.0, 100.0))
                        .or_else(|| {
                            (t.generation == TabGeneration::Auto && t.count == 0 && t.spacing_mm == 0.0)
                                .then(|| "options.tabs needs a count or a spacing (or generation \"manual\")".to_owned())
                        })
                        .or_else(|| (t.count > 1000).then(|| format!("options.tabs.count must be at most 1000 (got {})", t.count)))
                        .or_else(|| (t.max_tabs > 1000).then(|| format!("options.tabs.max_tabs must be at most 1000 (got {})", t.max_tabs)))
                })
            })
    }
}

/// What the Cut Planner orders by before optimizing (LightBurn's Order By list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderBy {
    /// Each layer's position in the Cuts / Layers list (galvo jobs always run layer by layer: every layer has its
    /// own settings and passes).
    Layer,
    /// All objects of a group together before the next group.
    Group,
    /// Each object's cut order priority ([`ObjectOptions::priority`], 0 first; objects without one last).
    Priority,
}

/// Direction for "Cut in direction order".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CutDirection {
    TopToBottom,
    BottomToTop,
    LeftToRight,
    RightToLeft,
}

/// LightBurn's Optimization Settings, saved with the project. The defaults are OpenEtch's behaviour before these
/// existed (order by layer, reduce travel, best start, best direction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Optimization {
    /// "Enable Optimization and Output Re-ordering": off outputs graphics in the order they were drawn.
    pub enabled: bool,
    /// Order By, applied in list order.
    pub order_by: Vec<OrderBy>,
    /// Cut an enclosed closed shape before the shape around it.
    pub inner_first: bool,
    /// Cut in direction order (instead of nearest-first).
    pub direction_order: Option<CutDirection>,
    /// Reduce travel moves (nearest-first ordering).
    pub reduce_travel: bool,
    /// Prefer cuts that keep going the same way.
    pub reduce_direction_changes: bool,
    /// A closed shape may start at any of its points.
    pub best_start: bool,
    /// Start closed shapes at sharp corners when they have any.
    pub corners: bool,
    /// Open lines and hatches may run in either direction.
    pub best_direction: bool,
    /// Remove line segments that overlap others of the same pass.
    pub remove_overlaps: bool,
    /// How close lines must be to count as overlapping, mm.
    pub overlap_distance_mm: f64,
    /// Run line-mode (cut) layers after every fill and image layer.
    pub cuts_last: bool,
}

impl Default for Optimization {
    fn default() -> Self {
        Optimization {
            enabled: true,
            order_by: vec![OrderBy::Layer],
            inner_first: false,
            direction_order: None,
            reduce_travel: true,
            reduce_direction_changes: false,
            best_start: true,
            corners: false,
            best_direction: true,
            remove_overlaps: false,
            overlap_distance_mm: 0.025,
            cuts_last: false,
        }
    }
}

impl Optimization {
    pub fn is_default(&self) -> bool {
        *self == Optimization::default()
    }

    pub fn problem(&self) -> Option<String> {
        if !(self.overlap_distance_mm.is_finite() && (0.0..=10.0).contains(&self.overlap_distance_mm)) {
            return Some(format!("overlap_distance_mm must be within 0..=10 (got {})", self.overlap_distance_mm));
        }
        let mut seen = Vec::new();
        for o in &self.order_by {
            if seen.contains(o) {
                return Some(format!("order_by lists {o:?} twice"));
            }
            seen.push(*o);
        }
        None
    }
}

/// Per-shape cut properties (LightBurn's Shape Properties).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ObjectOptions {
    /// Cut order priority (0 first) used by "Order by Priority".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u32>,
    /// Power Scale: this shape marks at this percentage of its layer's power, 0..=100.
    pub power_scale_pct: f64,
    /// Where (and in which direction) the shape's closed outlines start (LightBurn's Set Start Point). Used when
    /// the optimization's best start point (and, for the direction, best direction) is off, as in LightBurn, where
    /// those options may override it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<StartPoint>,
    /// Tabs placed by hand (LightBurn's Add Tabs tool), each a point in the object's own (local) coordinates, mm:
    /// the tab is centred on the nearest point of the shape's closed outlines. Used while the layer's tabs are on
    /// ([`Tabs`]), in both generations; at most [`MAX_MANUAL_TABS`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<[f64; 2]>,
}

/// A user-defined start point of a shape (LightBurn's Set Start Point).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartPoint {
    /// The node the start point was set on (numbered across subpaths, as `node.list` numbers them).
    pub node: usize,
    /// That node's position in the object's own (local) coordinates, mm: the burn starts at the nearest point of
    /// the outline through it.
    pub local: [f64; 2],
    /// Run the outline the other way (LightBurn: Shift+click).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reverse: bool,
}

impl Default for ObjectOptions {
    fn default() -> Self {
        ObjectOptions { priority: None, power_scale_pct: 100.0, start: None, tabs: Vec::new() }
    }
}

impl ObjectOptions {
    pub fn is_default(&self) -> bool {
        *self == ObjectOptions::default()
    }
}

/// How the rotary turns the work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RotaryKind {
    /// The work is held by the rotary's chuck: one rotary turn is one turn of the work.
    #[default]
    Chuck,
    /// The work rests on rollers: one rotary turn turns the work by roller diameter / object diameter.
    Roller,
}

/// A tapered object (tumbler): the diameter changes linearly along field X, from `object_diameter_mm` at
/// `x = -length_mm / 2` to `end_diameter_mm` at `x = +length_mm / 2` (constant beyond).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Taper {
    pub end_diameter_mm: f64,
    pub length_mm: f64,
}

/// Galvo rotary marking (LightBurn's galvo Rotary Setup), saved with the project. The work's axis lies along field
/// X; the design's Y is the unwrapped surface at `object_diameter_mm`. The job is split into strips of
/// `split_width_mm` along Y: each strip is marked around the top of the work (field Y = 0), then the axis turns the
/// work to the next strip. The first strip is the design's lowest (−Y) one, marked at the rotation the work has
/// when the job starts; the axis returns there at the end. Needs a board-driven extension axis (unverified on
/// hardware).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Rotary {
    pub enabled: bool,
    pub kind: RotaryKind,
    /// Motor steps for one turn of the rotary (chuck or roller).
    pub steps_per_rotation: u32,
    /// Diameter of the work, mm.
    pub object_diameter_mm: f64,
    /// Roller diameter, mm (roller rotaries).
    pub roller_diameter_mm: f64,
    /// Strip width along the circumference, mm (LightBurn's split size).
    pub split_width_mm: f64,
    /// Project each strip onto the curved surface (y = r·sin(arc / r)) so it lands where the flat design says.
    pub cylinder_correction: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taper: Option<Taper>,
}

impl Default for Rotary {
    fn default() -> Self {
        Rotary {
            enabled: false,
            kind: RotaryKind::Chuck,
            steps_per_rotation: 3200,
            object_diameter_mm: 80.0,
            roller_diameter_mm: 30.0,
            split_width_mm: 10.0,
            cylinder_correction: true,
            taper: None,
        }
    }
}

impl Rotary {
    /// Motor steps per turn of the work.
    pub fn steps_per_object_turn(&self) -> f64 {
        match self.kind {
            RotaryKind::Chuck => f64::from(self.steps_per_rotation),
            RotaryKind::Roller => f64::from(self.steps_per_rotation) * self.object_diameter_mm / self.roller_diameter_mm,
        }
    }

    /// Diameter of the work at field X `x` (the taper, else the object diameter).
    pub fn diameter_at(&self, x: f64) -> f64 {
        match &self.taper {
            None => self.object_diameter_mm,
            Some(t) => {
                let f = ((x + t.length_mm / 2.0) / t.length_mm).clamp(0.0, 1.0);
                self.object_diameter_mm + (t.end_diameter_mm - self.object_diameter_mm) * f
            }
        }
    }

    pub fn problem(&self) -> Option<String> {
        let positive = |name: &str, v: f64| (!(v.is_finite() && v > 0.0 && v <= 10_000.0)).then(|| format!("rotary.{name} must be within 0..=10000 mm (got {v})"));
        if !(1..=1_000_000).contains(&self.steps_per_rotation) {
            return Some(format!("rotary.steps_per_rotation must be within 1..=1000000 (got {})", self.steps_per_rotation));
        }
        positive("object_diameter_mm", self.object_diameter_mm)
            .or_else(|| positive("roller_diameter_mm", self.roller_diameter_mm))
            .or_else(|| positive("split_width_mm", self.split_width_mm))
            .or_else(|| {
                (self.split_width_mm > self.object_diameter_mm).then(|| "rotary.split_width_mm must not exceed the object diameter".to_owned())
            })
            .or_else(|| {
                self.taper.as_ref().and_then(|t| positive("taper.end_diameter_mm", t.end_diameter_mm).or_else(|| positive("taper.length_mm", t.length_mm)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_serialise_to_nothing_and_round_trip() {
        // New settings fill all at once (LightBurn's default); options saved empty keep filling shapes individually.
        assert_eq!(serde_json::to_value(CutOptions::default()).unwrap(), serde_json::json!({"fill_grouping": "all_at_once"}));
        assert_eq!(serde_json::to_value(CutOptions::absent()).unwrap(), serde_json::json!({}));
        assert_eq!(serde_json::from_value::<CutOptions>(serde_json::json!({})).unwrap(), CutOptions::absent());
        let back: CutOptions = serde_json::from_value(serde_json::to_value(CutOptions::default()).unwrap()).unwrap();
        assert_eq!(back, CutOptions::default());
        let o = CutOptions {
            output: false,
            pause_after_ms: 50.0,
            dot_mode: Some(DotMode { dot_time_ms: 1.0, dot_spacing_mm: 0.5 }),
            fill_grouping: FillGrouping::AllAtOnce,
            tabs: Some(Tabs { generation: TabGeneration::Manual, skip_inner: true, cut_power_pct: 20.0, ..Tabs::default() }),
            ..CutOptions::default()
        };
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(serde_json::from_value::<CutOptions>(v).unwrap(), o);
        assert!(serde_json::from_value::<CutOptions>(serde_json::json!({"wobbel": 1})).is_err());
        assert!(Optimization::default().is_default() && ObjectOptions::default().is_default());
    }

    #[test]
    fn problems_are_named() {
        assert!(CutOptions::default().problem().is_none());
        let bad = CutOptions { pause_before_ms: -1.0, ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("pause_before_ms"));
        let bad = CutOptions { tabs: Some(Tabs { count: 0, spacing_mm: 0.0, size_mm: 1.0, ..Tabs::default() }), ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("tabs"));
        let manual = Tabs { count: 0, generation: TabGeneration::Manual, ..Tabs::default() };
        assert!(CutOptions { tabs: Some(manual), ..CutOptions::default() }.problem().is_none(), "manual tabs need no count");
        let bad = CutOptions { tabs: Some(Tabs { cut_power_pct: 101.0, ..Tabs::default() }), ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("cut_power_pct"));
        // Tabs saved before the switch and the generation existed read as on and automatic, and write back the same.
        let old: Tabs = serde_json::from_value(serde_json::json!({"count": 3, "spacing_mm": 0.0, "size_mm": 1.0})).unwrap();
        assert_eq!(old, Tabs { count: 3, size_mm: 1.0, ..Tabs::default() });
        assert_eq!(serde_json::to_value(&old).unwrap(), serde_json::json!({"count": 3, "spacing_mm": 0.0, "size_mm": 1.0}));
        let bad = CutOptions { wobble: Some(Wobble { step_mm: 0.0, size_mm: 1.0 }), ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("wobble.step_mm"));
        let bad = CutOptions { overscan_mm: 51.0, ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("overscan_mm"));
        let bad = CutOptions { overcut_mm: -1.0, ..CutOptions::default() };
        assert!(bad.problem().unwrap().contains("overcut_mm"));
        let gantry = CutOptions { constant_power: true, air_assist: true, overscan_mm: 2.5, overcut_mm: 1.0, ..CutOptions::default() };
        assert!(gantry.problem().is_none());
        let v = serde_json::to_value(&gantry).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"fill_grouping": "all_at_once", "constant_power": true, "air_assist": true, "overscan_mm": 2.5, "overcut_mm": 1.0})
        );
        assert_eq!(serde_json::from_value::<CutOptions>(v).unwrap(), gantry);
        let o = Optimization { order_by: vec![OrderBy::Group, OrderBy::Group], ..Optimization::default() };
        assert!(o.problem().is_some());
        assert!(Rotary::default().problem().is_none());
        assert!(Rotary { split_width_mm: 200.0, ..Rotary::default() }.problem().is_some());
    }

    #[test]
    fn rotary_geometry() {
        let r = Rotary { kind: RotaryKind::Roller, steps_per_rotation: 1000, object_diameter_mm: 90.0, roller_diameter_mm: 30.0, ..Rotary::default() };
        assert_eq!(r.steps_per_object_turn(), 3000.0);
        let t = Rotary { taper: Some(Taper { end_diameter_mm: 60.0, length_mm: 100.0 }), object_diameter_mm: 80.0, ..Rotary::default() };
        assert_eq!((t.diameter_at(-50.0), t.diameter_at(0.0), t.diameter_at(80.0)), (80.0, 70.0, 60.0));
    }
}
