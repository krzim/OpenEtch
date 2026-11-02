// The Fill Region tool (a paint bucket), without React: the engine invocations (`path.fill_region`,
// `path.fill_region_preview`, crates/commands/src/builtin/fill_region.rs), the preview answer parsed into faces, a
// per-revision cache of faces found so far (a pointer inside a face already seen needs no new query), and the tool's
// state machine:
//
// - mouse / pen: hovering shows the area under the pointer; a click fills it (with every Shift-collected area, in one
//   command and one undo step); Shift+click adds the area to the collection instead; Enter fills the collection; Esc
//   drops the collection, or leaves the tool when there is none.
// - touch: a tap shows the area (it is "armed"); a second tap inside the same area, or the bar's Add button, fills it
//   (again with the collection). A tap elsewhere arms that area instead. Shift (the touch modifier bar) adds the
//   tapped area to the collection.
//
// `npm test` covers it (fillRegion.test.ts).

import type { Invocation } from "../api/client.ts";
import type { Vec2 } from "../api/types.ts";

export const FILL_CMD = {
  fill: "path.fill_region",
  preview: "path.fill_region_preview",
} as const;

/** Options shared by the preview and the fill (only what differs from the engine's defaults is sent). */
export interface FillOpts {
  /** Close openings narrower than this, mm (0: lines must meet). */
  gapMm: number;
  /** Islands inside the area stay holes (the engine's default). */
  holes: boolean;
}

export const DEFAULT_FILL_OPTS: FillOpts = { gapMm: 0, holes: true };

/** Largest gap the engine accepts, mm. */
export const MAX_GAP_MM = 10;

function optParams(o: FillOpts): Record<string, unknown> {
  const p: Record<string, unknown> = {};
  if (o.gapMm > 0) p.gap_mm = Math.min(MAX_GAP_MM, o.gapMm);
  if (!o.holes) p.holes = false;
  return p;
}

const round = (v: number) => Math.round(v * 1e4) / 1e4;
const pt = (p: Vec2): Vec2 => [round(p[0]), round(p[1])];

/** Create the areas around `points` on `layer` (one undo step). */
export function fillRegion(points: readonly Vec2[], layer: string | null, o: FillOpts = DEFAULT_FILL_OPTS): Invocation {
  const params: Record<string, unknown> = { points: points.map(pt), ...optParams(o) };
  if (layer) params.layer = layer;
  return { id: FILL_CMD.fill, params };
}

/** The outlines those points would give, without creating anything. */
export function fillPreview(points: readonly Vec2[], o: FillOpts = DEFAULT_FILL_OPTS): Invocation {
  return { id: FILL_CMD.preview, params: { points: points.map(pt), ...optParams(o) } };
}

/** One area: its contours in field mm (the first is the outer one, the rest holes) and its area. */
export interface Face {
  contours: Vec2[][];
  areaMm2: number;
}

/** What a preview said about one point: its face, or why there is none. */
export type PointAnswer = { face: Face } | { error: string; code: string };

function isVec2(v: unknown): v is Vec2 {
  return Array.isArray(v) && v.length === 2 && typeof v[0] === "number" && typeof v[1] === "number" && Number.isFinite(v[0]) && Number.isFinite(v[1]);
}

function parseFace(v: unknown): Face | null {
  const f = v as { contours?: unknown; area_mm2?: unknown } | null;
  if (!f || !Array.isArray(f.contours)) return null;
  const contours = f.contours.filter((c): c is unknown[] => Array.isArray(c)).map((c) => c.filter(isVec2));
  if (contours.length === 0 || contours[0].length < 3) return null;
  return { contours, areaMm2: typeof f.area_mm2 === "number" ? f.area_mm2 : 0 };
}

/** The engine's preview answer as one answer per point (malformed parts become errors). */
export function parsePreview(result: unknown, n: number): PointAnswer[] {
  const r = result as { faces?: unknown; points?: unknown } | null;
  const faces = Array.isArray(r?.faces) ? r.faces.map(parseFace) : [];
  const pts = Array.isArray(r?.points) ? r.points : [];
  const out: PointAnswer[] = [];
  for (let i = 0; i < n; i++) {
    const p = pts[i] as { face?: unknown; error?: unknown; code?: unknown } | undefined;
    const face = typeof p?.face === "number" ? faces[p.face] : null;
    if (face) out.push({ face });
    else out.push({ error: typeof p?.error === "string" ? p.error : "no answer", code: typeof p?.code === "string" ? p.code : "error" });
  }
  return out;
}

/** Even-odd point in ring. */
function inRing(p: Vec2, ring: readonly Vec2[]): boolean {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const a = ring[i];
    const b = ring[j];
    if (a[1] > p[1] !== b[1] > p[1] && p[0] < ((b[0] - a[0]) * (p[1] - a[1])) / (b[1] - a[1]) + a[0]) inside = !inside;
  }
  return inside;
}

/** Whether `p` lies in the face (inside its outer contour, outside every hole). */
export function faceContains(f: Face, p: Vec2): boolean {
  const [outer, ...holes] = f.contours;
  return !!outer && inRing(p, outer) && !holes.some((h) => inRing(p, h));
}

/** SVG path data of a face (field mm; draw with fill-rule evenodd). */
export function facePathData(f: Face): string {
  return f.contours
    .filter((c) => c.length >= 3)
    .map((c) => `M${c.map((q) => `${round(q[0])},${round(q[1])}`).join("L")}Z`)
    .join("");
}

/** Faces found for one document revision and one set of options; anything else empties it. */
export class FaceCache {
  private key = "";
  private faces: Face[] = [];

  /** Point the cache at `revision` and `o`; returns whether that dropped what it held. */
  sync(revision: number, o: FillOpts): boolean {
    const k = `${revision}|${o.gapMm}|${o.holes}`;
    if (k === this.key) return false;
    this.key = k;
    const had = this.faces.length > 0;
    this.faces = [];
    return had;
  }

  /** A known face holding `p`. */
  find(p: Vec2): Face | null {
    return this.faces.find((f) => faceContains(f, p)) ?? null;
  }

  add(f: Face): void {
    if (!this.faces.includes(f)) this.faces.push(f);
    // A long session of hovering over a huge document stays bounded.
    if (this.faces.length > 200) this.faces.shift();
  }

  get size(): number {
    return this.faces.length;
  }
}

/**
 * Hover previews one at a time. The engine runs commands in turn, so on a heavy document a slow answer must not leave
 * a question queued behind it for every pause of the mouse: while one is out, only the latest point waits.
 */
export class HoverGate {
  private out = false;
  private waiting: Vec2 | null = null;

  /** Whether to ask about `at` now; false when an answer is still out (then `at` waits, replacing an older point). */
  request(at: Vec2): boolean {
    if (this.out) {
      this.waiting = at;
      return false;
    }
    this.out = true;
    return true;
  }

  /** The answer came back: the gate is free, and the point that waited meanwhile (if any) is returned. */
  done(): Vec2 | null {
    const next = this.waiting;
    this.out = false;
    this.waiting = null;
    return next;
  }

  /** Forget the waiting point (the tool was left); an answer still out frees the gate when it comes. */
  clear(): void {
    this.waiting = null;
  }
}

// ---- the tool's state machine ----------------------------------------------------------------------------------------

/** One area the tool shows: where it was picked and its face (null while the preview is on its way). */
export interface Pick {
  at: Vec2;
  face: Face | null;
}

export interface FillState {
  /** The area under the mouse (desktop hover). */
  hover: Pick | null;
  /** Touch: the tapped area waiting for a second tap or Add. */
  armed: Pick | null;
  /** Areas collected with Shift, filled together. */
  pending: Pick[];
  /** The last problem, shown in the bar (no enclosed area, a failed fill). */
  error: string | null;
  /** A fill is on its way to the engine. */
  busy: boolean;
}

export const IDLE: FillState = { hover: null, armed: null, pending: [], error: null, busy: false };

/** What the hook does next after an event. */
export type FillEffect =
  | { kind: "none" }
  /** Ask the engine for the face at `at` (debounced for hover). */
  | { kind: "preview"; at: Vec2; why: "hover" | "arm" | "add" }
  /** Fill these points now. */
  | { kind: "commit"; points: Vec2[] }
  /** Leave the tool. */
  | { kind: "exit" };

/** Whether a face already collected holds `p`. */
function collected(s: FillState, p: Vec2): boolean {
  return s.pending.some((k) => k.face && faceContains(k.face, p));
}

/** Points to fill: the collection plus `extra` (when it is not inside an area already collected). */
function commitPoints(s: FillState, extra: Vec2 | null): Vec2[] {
  const pts = s.pending.map((k) => k.at);
  if (extra && !collected(s, extra)) pts.push(extra);
  return pts;
}

export interface ClickInput {
  at: Vec2;
  shift: boolean;
  touch: boolean;
  /** A face already known to hold `at` (the cache), if any. */
  known: Face | null;
  /** Touch: a second tap this close to the first (mm) counts as the same area, however small it is. */
  tolMm?: number;
}

/** A click (mouse/pen press) or a tap (touch lift without a drag). */
export function onClick(s: FillState, c: ClickInput): [FillState, FillEffect] {
  if (s.busy) return [s, { kind: "none" }];
  const pick: Pick = { at: c.at, face: c.known };
  if (c.shift) {
    // Add to the collection (a second Shift+click inside a collected area takes it out again).
    const i = s.pending.findIndex((k) => k.face && faceContains(k.face, c.at));
    if (i >= 0) return [{ ...s, pending: s.pending.filter((_, j) => j !== i), armed: null, error: null }, { kind: "none" }];
    const next = { ...s, pending: [...s.pending, pick], armed: null, error: null };
    return [next, c.known ? { kind: "none" } : { kind: "preview", at: c.at, why: "add" }];
  }
  if (c.touch) {
    const a = s.armed;
    const same = !!a && ((!!a.face && faceContains(a.face, c.at)) || Math.hypot(a.at[0] - c.at[0], a.at[1] - c.at[1]) <= (c.tolMm ?? 0));
    if (a && same && a.face) return [{ ...s, busy: true, error: null }, { kind: "commit", points: commitPoints(s, a.at) }];
    return [{ ...s, armed: pick, error: null }, c.known ? { kind: "none" } : { kind: "preview", at: c.at, why: "arm" }];
  }
  return [{ ...s, busy: true, error: null }, { kind: "commit", points: commitPoints(s, c.at) }];
}

/** The pointer moved without a button (mouse / pen). */
export function onHover(s: FillState, at: Vec2, known: Face | null): [FillState, FillEffect] {
  if (s.hover?.face && faceContains(s.hover.face, at)) return [{ ...s, hover: { at, face: s.hover.face } }, { kind: "none" }];
  const hover: Pick = { at, face: known };
  return [{ ...s, hover }, known ? { kind: "none" } : { kind: "preview", at, why: "hover" }];
}

/** A preview answer for the point `at` (stale answers, for points no longer shown, are ignored). */
export function onPreview(s: FillState, at: Vec2, ans: PointAnswer): FillState {
  const same = (k: Pick | null) => !!k && k.at[0] === at[0] && k.at[1] === at[1];
  const face = "face" in ans ? ans.face : null;
  const err = "error" in ans ? ans.error : null;
  let next = s;
  if (same(s.hover)) next = { ...next, hover: { at, face } };
  if (same(s.armed)) next = { ...next, armed: face ? { at, face } : null, error: err ?? next.error };
  const i = s.pending.findIndex(same);
  if (i >= 0) {
    if (face) {
      // A second Shift+click in an area already collected adds nothing.
      const dup = s.pending.some((k, j) => j !== i && k.face && faceContains(k.face, at));
      next = { ...next, pending: dup ? next.pending.filter((_, j) => j !== i) : next.pending.map((k, j) => (j === i ? { at, face } : k)) };
    } else next = { ...next, pending: next.pending.filter((_, j) => j !== i), error: err };
  }
  return next;
}

/** Keys: Enter fills the collection (and the armed area); Escape clears, or leaves the tool. */
export function onKey(s: FillState, key: string): [FillState, FillEffect] | null {
  if (key === "Escape") {
    if (s.pending.length || s.armed || s.error) return [{ ...s, pending: [], armed: null, error: null }, { kind: "none" }];
    return [s, { kind: "exit" }];
  }
  if (key === "Enter") {
    const pts = commitPoints(s, s.armed?.face ? s.armed.at : null);
    if (pts.length === 0 || s.busy) return null;
    return [{ ...s, busy: true, error: null }, { kind: "commit", points: pts }];
  }
  return null;
}

/** The bar's Add button: like Enter. */
export function onAdd(s: FillState): [FillState, FillEffect] {
  return onKey(s, "Enter") ?? [s, { kind: "none" }];
}

/** The fill came back: done (the collection is cleared), or failed (kept, with the message). */
export function onCommitted(s: FillState, error: string | null): FillState {
  if (error) return { ...s, busy: false, error };
  return { ...s, busy: false, pending: [], armed: null, error: null };
}

/** How many areas Enter / Add would fill. */
export function readyCount(s: FillState): number {
  return s.pending.filter((k) => k.face).length + (s.armed?.face && !collected(s, s.armed.at) ? 1 : 0);
}

/** The engine's error for a failed fill, without the command id and the closing "nothing was created". */
export function fillErrorText(msg: string): string {
  return msg
    .replace(/^.*?path\.fill_region(_preview)?:\s*/, "")
    .replace(/;\s*nothing was created\.?$/, "")
    .trim();
}

/** An area for the bar: "12.35 mm²", or square inches. */
export function areaText(mm2: number, units: "mm" | "in"): string {
  if (units === "in") return `${(mm2 / 645.16).toFixed(4)} in²`;
  return `${mm2 < 100 ? mm2.toFixed(2) : mm2.toFixed(1)} mm²`;
}
