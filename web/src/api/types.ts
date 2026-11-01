// Wire types for the engine API (see ARCHITECTURE.md). They mirror the serde output of crates/doc and crates/commands.
// All lengths are millimetres and all angles degrees, in the field frame (origin at the galvo field centre, +Y away
// from the operator).

/** kurbo `Affine` coefficients `[a, b, c, d, e, f]`: x' = a*x + c*y + e, y' = b*x + d*y + f (same as SVG matrix()). */
export type Affine = [number, number, number, number, number, number];

export type Vec2 = [number, number];

export type FillMode = "line" | "fill" | "fill_then_line";

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
  sub_layers: CutSettings[];
  /** Settings from other software that OpenEtch does not model (e.g. `lbrn2.*`), kept so exports round-trip. */
  extras?: Record<string, string>;
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
  /** A bitmap centred on the local origin. Not engraved yet (the planner skips it with a warning). */
  | { kind: "image"; width_mm: number; height_mm: number; mime: string; data_base64: string };

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
  /** Groups (absent on older engines). */
  groups?: Group[];
  extras?: Record<string, string>;
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
  /** Pass index within the layer (newer engines). */
  pass?: number;
  from: Vec2;
  to: Vec2;
}

/** Per-layer preview data (newer engines). */
export interface PreviewLayer {
  layer_id: string;
  name?: string;
  color?: string;
  mode?: FillMode | string;
  segments?: number;
  mark_length_mm?: number;
  estimated_time_s?: number;
}

export interface Preview {
  segments: PreviewSegment[];
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
  [key: string]: unknown;
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

/** Server-to-client WebSocket messages on /api/events. */
export type EngineEvent =
  | { type: "document_changed"; revision: number; /** Who edited; absent for undo/redo. */ origin?: Origin | null }
  | { type: "machine"; [key: string]: unknown }
  | { type: "run_pending"; [key: string]: unknown }
  | ApprovalLockedEvent
  | RunFinishedEvent
  | ProjectSavedEvent
  | { type: "history_changed"; [key: string]: unknown };
