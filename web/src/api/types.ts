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
  | { kind: "path"; path: BezPathJson };

export interface DocObject {
  id: string;
  name: string;
  layer: string;
  /** Object-local geometry. */
  geometry: Geometry;
  /** Local to field frame. */
  transform: Affine;
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
}

/**
 * Who issued a command. The engine serializes `commands::Origin` as `{"kind":"ui"}`, `{"kind":"agent","name":"..."}`
 * or `{"kind":"script"}`; plain strings are accepted too.
 */
export type Origin = string | { kind: "ui" | "script" } | { kind: "agent"; name: string };

export interface HistoryEntry {
  revision: number;
  command: string;
  origin: Origin;
  summary: string;
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
  from: Vec2;
  to: Vec2;
}

export interface Preview {
  segments: PreviewSegment[];
  /** Free-form job statistics (time estimate, mark/jump length...). Shown as key/value pairs. */
  stats?: Record<string, unknown>;
}

/** A run requested by `job.request_run` / POST /api/runs that waits for UI approval. Fields beyond `id` are optional. */
export interface PendingRun {
  id: string;
  state?: string;
  origin?: Origin;
  summary?: string;
  created_at?: string;
  [key: string]: unknown;
}

/** GET /api/machine. Only `connected` is relied on; everything else is displayed when present. */
export interface MachineStatus {
  connected: boolean;
  sim?: boolean;
  busy?: boolean;
  /** Galvo field side length in mm. */
  field_mm?: number;
  pending_runs?: PendingRun[];
  [key: string]: unknown;
}

/** Server-to-client WebSocket messages on /api/events. */
export type EngineEvent =
  | { type: "document_changed"; revision: number }
  | { type: "machine"; [key: string]: unknown }
  | { type: "run_pending"; [key: string]: unknown };
