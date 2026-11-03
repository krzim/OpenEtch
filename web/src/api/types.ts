// Wire types for the engine API (see ARCHITECTURE.md). They mirror the serde output of crates/doc and crates/commands.
// All lengths are millimetres and all angles degrees, in the field frame (origin at the galvo field centre, +Y away
// from the operator).

import type { PackedSegments } from "./packedPreview.ts";

/** kurbo `Affine` coefficients `[a, b, c, d, e, f]`: x' = a*x + c*y + e, y' = b*x + d*y + f (same as SVG matrix()). */
export type Affine = [number, number, number, number, number, number];

export type Vec2 = [number, number];

/** `offset_fill`: LightBurn's Offset Fill (concentric inward outlines at the hatch interval). */
export type FillMode = "line" | "fill" | "fill_then_line" | "offset_fill";

export interface Hatch {
  interval_mm: number;
  angle_deg: number;
  angle_increment_deg: number;
  cross_hatch: boolean;
  bidirectional: boolean;
}

export interface CutSettings {
  mode: FillMode;
  /** 0..=100. */
  power_pct: number;
  speed_mm_s: number;
  frequency_khz: number;
  /** MOPA pulse width; null leaves the source at its current setting. */
  pulse_width_ns: number | null;
  passes: number;
  hatch: Hatch;
  /** Galvo timing override; null uses the machine profile. The UI shows but does not edit it. */
  timing: unknown | null;
  /** LightBurn's sub-layer name (`subname`); absent when unnamed. */
  name?: string | null;
  /** Makes this a cleanup sub-layer (LightBurn `isCleanup` / `cleanupPass`); absent for an ordinary pass. */
  cleanup?: Cleanup | null;
}

/**
 * A cleanup sub-layer (LightBurn's "Enable cleanup pass" / "Clean after # of passes"): it runs after main passes n, 2n,
 * ... and after the last main pass (so with n at least the layer's passes, once after all of them), each time doing
 * its own `passes`, before the layer's other sub-layers.
 */
export interface Cleanup {
  /** 1..=1000. */
  every_n_passes: number;
}

export interface Layer {
  id: string;
  name: string;
  /** CSS colour. */
  color: string;
  visible: boolean;
  /** Excluded from jobs when false. */
  output: boolean;
  settings: CutSettings;
  /** Further passes over the same objects (LightBurn sub-layers), each run after the main settings, in order. */
  sub_layers: CutSettings[];
  /** Settings from other software that OpenEtch does not model (e.g. `lbrn2.*`), kept so exports round-trip. */
  extras?: Record<string, string>;
  /** LightBurn's tool layer T1 / T2 (never outputs, framed with the job); absent for an ordinary layer. */
  tool?: "T1" | "T2" | null;
}

/** One kurbo `PathEl` as serde derives it, e.g. `{"MoveTo":{"x":0,"y":0}}` or `"ClosePath"`. */
export type KurboPathEl =
  | { MoveTo: { x: number; y: number } }
  | { LineTo: { x: number; y: number } }
  | { QuadTo: [{ x: number; y: number }, { x: number; y: number }] }
  | { CurveTo: [{ x: number; y: number }, { x: number; y: number }, { x: number; y: number }] }
  | "ClosePath";

/**
 * A kurbo `BezPath`. The contract says it is an SVG path string; kurbo 0.13's derived serde emits an element array.
 * The UI accepts either.
 */
export type BezPathJson = string | KurboPathEl[];

export type Geometry =
  | { kind: "rect"; width_mm: number; height_mm: number; corner_radius_mm: number }
  | { kind: "ellipse"; rx_mm: number; ry_mm: number }
  | { kind: "path"; path: BezPathJson }
  /**
   * A regular polygon or star (LightBurn's Polygon tool), kept parametric so Shape Properties can edit it: `sides`
   * corners (star points) on a circle of `radius_mm`, the first one up (+Y); a star's inner corners at
   * `star_ratio` of the radius; corners rounded by `corner_radius_mm` where it fits (crates/doc Geometry::Polygon).
   * LightBurn's Polygon / Star shapes add `ry_mm` (vertical radius, when it differs) and `half_step` (corners turned
   * by 180° / sides, so an edge or a star's inner corner is centred on +Y).
   */
  | {
      kind: "polygon";
      sides: number;
      radius_mm: number;
      star_ratio?: number | null;
      corner_radius_mm?: number;
      ry_mm?: number | null;
      half_step?: boolean;
    }
  /** A bitmap centred on the local origin. Not engraved yet (the planner skips it with a warning). */
  | { kind: "image"; width_mm: number; height_mm: number; mime: string; data_base64: string }
  /** Live text (`doc::Text`): its settings plus the rendered `outline`, which is what is drawn and burned. */
  | TextGeometry;

export type FontKind = "system" | "stroke" | "shx" | "jsf";
export type TextAlignH = "left" | "middle" | "right";
export type TextAlignV = "top" | "middle" | "bottom";
export type VarMode = "date_time" | "serial" | "csv" | "cut_setting";

/** A text object's geometry (crates/doc/src/text.rs). Lengths in mm, spacing in % (LightBurn's HSpace/VSpace). */
export interface TextGeometry {
  kind: "text";
  text: string;
  font: { family: string; kind: FontKind };
  height_mm: number;
  h_spacing: number;
  v_spacing: number;
  align_h: TextAlignH;
  align_v: TextAlignV;
  bold: boolean;
  italic: boolean;
  upper: boolean;
  welded: boolean;
  bend_deg: number;
  distort: boolean;
  path?: { object: string; offset_mm: number } | null;
  max_width_mm: number;
  squeeze: boolean;
  ignore_empty_vars: boolean;
  variable?: { mode: VarMode; offset: number } | null;
  outline: BezPathJson;
  /** What the outline shows (the evaluated variable text) when it differs from `text`. */
  shown?: string | null;
}

/** The document's serial number / CSV row counter (LightBurn's Variable Text window). */
export interface VariableState {
  start: number;
  end: number;
  current: number;
  advance_by: number;
  auto_advance: boolean;
  csv?: { name: string; rows: string[][] } | null;
}

export interface DocObject {
  id: string;
  name: string;
  layer: string;
  /** Object-local geometry. */
  geometry: Geometry;
  /** Local to field frame. */
  transform: Affine;
  /** The group this object belongs to directly, if any. */
  group?: string | null;
  /** Foreign data kept for round trips (e.g. LightBurn text attributes). */
  extras?: Record<string, string>;
}

/** A group of objects; groups nest through `parent`. */
export interface Group {
  id: string;
  name: string;
  parent?: string | null;
}

export type StockShape =
  | { kind: "rect"; width_mm: number; height_mm: number; corner_radius_mm: number }
  | { kind: "circle"; diameter_mm: number };

export interface Stock {
  name: string;
  shape: StockShape;
  thickness_mm: number;
  material: string | null;
  /** Centre in the field frame. */
  center: Vec2;
  rotation_deg: number;
}

export interface GDocument {
  revision: number;
  name: string;
  stock: Stock | null;
  layers: Layer[];
  objects: DocObject[];
  next_id: number;
  /** Galvo field size the document was laid out for; the engine fills it from the machine profile. */
  field_mm?: number | null;
  /** Workspace (W x H mm) of a non-square (gantry) device the document was laid out for; absent for galvo fields. */
  workspace_mm?: [number, number] | null;
  /** Groups (absent on older engines). */
  groups?: Group[];
  extras?: Record<string, string>;
  /** Variable text state; absent while it is the default (start 0, end 999, current 0, advance by 1). */
  variables?: VariableState | null;
  /** Manual guidelines saved with the project (`guide.add` / `guide.move` / `guide.delete`); absent while none. */
  guides?: DocGuide[];
  /** Output curve tolerance in mm (`doc.set_curve_tolerance`); absent or null: the device's, else 0.01 mm. */
  curve_tolerance_mm?: number | null;
  /** LightBurn's virtual arrays (`art.array` with virtual): linked clones laid out again after every edit; absent while none. */
  virtual_arrays?: VirtualArray[];
}

/** A virtual array (doc::VirtualArray): the originals, their clones cell by cell, and the grid settings. */
export interface VirtualArray {
  id: string;
  sources: string[];
  clones: string[];
  /** The `art.array` grid settings (rows, cols, pitch_x_mm, ..., counts resolved). */
  settings: Record<string, unknown>;
}

/** A manual guide in the document: "x" is a vertical line at x = position_mm, "y" a horizontal one. */
export interface DocGuide {
  id: string;
  axis: "x" | "y";
  position_mm: number;
}

/**
 * Who issued a command. The engine serializes `commands::Origin` as `{"kind":"ui"}`, `{"kind":"agent","name":"..."}`
 * or `{"kind":"script"}`; plain strings are accepted too.
 */
export type Origin = string | { kind: "ui" | "script" } | { kind: "agent"; name: string };

/** One row of an older engine's GET /api/history (an append-only log). */
export interface HistoryEntry {
  revision: number;
  command: string;
  origin: Origin;
  summary: string;
}

/** Where a timeline step is: applied, after the marker (rolled back), or failed on replay. */
export type StepState = "active" | "rolled_back" | "failed";

/** One step of the Fusion-style timeline (GET /api/history `steps`). */
export interface HistoryStep {
  index: number;
  invocations?: { id: string; params?: unknown }[];
  origin: Origin;
  summary: string;
  state: StepState | string;
  /** Why a replayed step failed. */
  error?: string | null;
  revision?: number;
}

/** GET /api/history on engines with the timeline: `marker` is the index of the last applied step. */
export interface HistoryTimeline {
  marker: number;
  steps: HistoryStep[];
  /** What last moved the marker: after "undo" the next edit discards the `undone_steps` steps after the marker. */
  marker_moved_by?: "edit" | "undo" | "timeline";
  /** How many steps right after the marker were undone with undo/redo; the next edit discards them. */
  undone_steps?: number;
}

export type Risk = "query" | "edit" | "pointer" | "emit";

export interface CommandSpec {
  id: string;
  title: string;
  description: string;
  risk: Risk;
  input_schema: unknown;
}

export type ExecuteResponse = { ok: true; result: unknown; revision: number } | { ok: false; error: string };

export interface PreviewSegment {
  kind: "jump" | "mark";
  /** Layer id for marks; may be null for jumps. */
  layer?: string | null;
  /** Index of the job pass it belongs to, in burn order (`plan::JobPlan::passes`); a jump belongs to the pass it travels into. */
  pass?: number;
  /** 0 for the layer's main settings, n for `sub_layers[n - 1]` (engines that report it). */
  sub_layer?: number;
  from: Vec2;
  to: Vec2;
  /** Estimated time from the job's start to the end of this segment, s (engines with the preview time slider). */
  t_s?: number;
}

/** Per-layer preview data (newer engines). */
export interface PreviewLayer {
  layer_id: string;
  name?: string;
  color?: string;
  mode?: FillMode | string;
  /** Job passes planned for the layer (repeats and sub-layers). */
  passes?: number;
  segments?: number;
  mark_length_mm?: number;
  estimated_time_s?: number;
}

/**
 * One job pass (engines that report them), indexed by `PreviewSegment.pass`. Mirrors `plan::PassPlan` without its
 * polylines; the stats are optional.
 */
export interface PreviewPass {
  layer_id: string;
  /** 0 for the layer's main settings, n for `sub_layers[n - 1]`. */
  sub_layer: number;
  /** Repetition of those settings, `0..passes`. */
  repeat?: number;
  segments?: number;
  mark_length_mm?: number;
  jump_length_mm?: number;
  estimated_time_s?: number;
}

export interface Preview {
  /** Empty when `packed` holds the segments (a preview planned in the browser, src/wasm/). */
  segments: PreviewSegment[];
  /** The segments as flat arrays (api/packedPreview.ts), from the browser's preview worker. */
  packed?: PackedSegments;
  /** The document revision it was planned from, when known (Start Here runs only that revision). */
  revision?: number;
  /** The job's passes in burn order (engines that report them). */
  passes?: PreviewPass[];
  /** Free-form job statistics (time estimate, mark/jump length...). */
  stats?: Record<string, unknown>;
  layers?: PreviewLayer[];
  warnings?: string[];
}

/** Where a run is in its life (ARCHITECTURE.md, shared run states). Unknown strings from a newer engine are kept. */
export type RunState =
  | "pending"
  | "approved"
  | "awaiting_confirm"
  | "running"
  | "finished"
  | "failed"
  | "aborted"
  | "cancelled";

/** `plan::Stats` for a run: what the approved job will do. Fields are optional so an older engine still renders. */
export interface RunStats {
  mark_length_mm?: number;
  jump_length_mm?: number;
  /** Motion plus delays; acceleration is not modelled. */
  estimated_time_s?: number;
  segment_count?: number;
  polyline_count?: number;
  [key: string]: unknown;
}

/** A run requested by `job.request_run` / POST /api/runs. Only `id` is guaranteed; the rest is shown when present. */
export interface PendingRun {
  id: string;
  state?: RunState | string;
  origin?: Origin;
  /** Document revision the job was planned from. */
  revision?: number;
  /** The document changed since the run was planned; the engine refuses to approve it. */
  stale?: boolean;
  summary?: string;
  stats?: RunStats;
  packet_count?: number;
  created_unix_s?: number;
  error?: string | null;
  [key: string]: unknown;
}

/** Optional physical confirmation: an approved run waits for this board input before it streams. */
export interface PhysicalConfirm {
  input_bit: number;
  timeout_s: number;
}

/** GET /api/machine `approval`: how runs are approved on this engine. */
export interface ApprovalInfo {
  /** False until someone runs `openetch set-pin` on the engine machine. */
  pin_set: boolean;
  physical_confirm: PhysicalConfirm | null;
  /**
   * The PIN file changed while the engine was running (`openetch set-pin`). The engine keeps the PIN it loaded at
   * startup and refuses approvals (code "pin_changed") until it is restarted.
   */
  pin_changed?: boolean;
  /** After repeated wrong PINs: when approvals unlock (engine clock, Unix seconds), or null when unlocked. */
  locked_until_unix_s?: number | null;
  /**
   * How runs are approved (`--approval`, fixed when the engine starts): "pin" (the operator PIN; also what an older
   * engine without this field does) or "click" (no PIN: Start in this app, or the foot switch).
   */
  mode?: "pin" | "click";
  /** In the "click" mode: the input (the profile's Start Marking input) that drives the foot switch cycle. */
  foot_switch?: FootSwitch | null;
}

/** The foot switch of the no-PIN mode (`approval.foot_switch`): press to frame, press again to burn what was framed. */
export interface FootSwitch {
  input_bit: number;
  active_low: boolean;
}

/** GET /api/machine. Only `connected` is relied on; everything else is displayed when present. */
export interface MachineStatus {
  connected: boolean;
  sim?: boolean;
  busy?: boolean;
  /** Galvo field side length in mm. */
  field_mm?: number;
  pending_runs?: PendingRun[];
  /** Pending and recent runs in every state. */
  runs?: PendingRun[];
  approval?: ApprovalInfo;
  /** The foot switch cycle (no-PIN mode): press to frame the job, press again to burn what was framed. */
  foot_cycle?: FootCycle;
  /** The engine is shutting down (SIGTERM / SIGINT): new runs, Start and frames are refused; a running job finishes. */
  shutting_down?: boolean;
  shutdown?: ShutdownInfo | null;
  [key: string]: unknown;
}

/** GET /api/machine `shutdown`: a graceful shutdown in progress. */
export interface ShutdownInfo {
  signal: string;
  since_unix_s: number;
  /** How long the engine waits for a running job before aborting it. */
  grace_s: number;
  abort_at_unix_s: number;
  /** A second signal or the grace limit is aborting the job. */
  aborting: boolean;
  /** The run the engine waits for, or null. */
  waiting_for_run: string | null;
}

/** GET /api/machine `foot_cycle`. */
export interface FootCycle {
  /** The engine runs without a PIN and the device has a foot switch (Start Marking input). */
  available: boolean;
  phase: "off" | "framing" | "burning";
  /** The document revision being framed or burned. */
  revision: number | null;
  run_id: string | null;
  since_unix_s: number | null;
  /** Framing (and the cycle) ends after this long without a press. */
  idle_timeout_s: number;
  /** The last notice (why a press did not burn, why the cycle ended). */
  notice: string | null;
}

/** `approval_locked`: too many wrong PINs; approvals are refused until `until_unix_s` (engine clock). */
export interface ApprovalLockedEvent {
  type: "approval_locked";
  until_unix_s: number;
}

/** `run_finished`: an approved run ended (`state` is "finished", "failed", "aborted" or "cancelled"). */
export interface RunFinishedEvent {
  type: "run_finished";
  /** Contract field; older engines sent `run_id`. */
  id?: string;
  run_id?: string;
  ok: boolean;
  state?: RunState | string;
  error?: string | null;
}

/** GET /api/project: the engine's current project file and whether the document changed since it was saved. */
export interface ProjectInfo {
  /** Project name (file `<projects-dir>/<name>.openetch`), or null when the document was never saved or opened. */
  name: string | null;
  /**
   * Name of the file a replacing upload or LightBurn import came from (without its extension), while no project holds
   * the document; null otherwise (absent on older engines).
   */
  display_name?: string | null;
  /** Document revision at the last save or open, or null. */
  saved_revision: number | null;
  revision: number;
  dirty: boolean;
}

/** One row of GET /api/projects. */
export interface ProjectEntry {
  name: string;
  modified_unix_s?: number;
  /** File size in bytes. */
  size?: number;
}

/** `project_saved`: the document was saved as project `name` at `revision` (by any client). */
export interface ProjectSavedEvent {
  type: "project_saved";
  name: string;
  revision: number;
}

/** `device_changed`: another device became active (refetch the devices, the machine and the document). */
export interface DeviceChangedEvent {
  type: "device_changed";
  id?: string;
}

/** Server-to-client WebSocket messages on /api/events. */
export type EngineEvent =
  | { type: "document_changed"; revision: number; /** Who edited; absent for undo/redo. */ origin?: Origin | null }
  | { type: "machine"; [key: string]: unknown }
  | { type: "run_pending"; [key: string]: unknown }
  | ApprovalLockedEvent
  | RunFinishedEvent
  | ProjectSavedEvent
  | { type: "history_changed"; [key: string]: unknown }
  | DeviceChangedEvent;
