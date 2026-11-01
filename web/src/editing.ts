// Pure editing maths for the canvas: arrow-key nudge and rotate-key steps, Shift-constrained moves, scale and rotate
// handles, and client-side align/distribute. Snapping lives in snapping.ts. No DOM, so `npm test` covers it.
// All values are field-frame mm (+Y away from the operator) and degrees counter-clockwise.
//
// LightBurn behaviour (docs.lightburnsoftware.com Reference/TransformControls, Snapping, Align, Distribute):
// - arrow keys nudge 5 mm, Ctrl 1 mm, Shift 20 mm, Ctrl+Shift 0.1 mm;
// - `.` / `,` rotate 90° clockwise / counter-clockwise; Shift 45°, Ctrl 15°, Ctrl+Shift 1.5°;
// - while moving, Shift constrains to horizontal, vertical or 45°; Ctrl turns snapping off; Alt snaps to alignment
//   guides only (snapping.ts);
// - corner handles keep the aspect ratio (Shift frees it), edge handles scale one axis, Ctrl scales about the centre;
// - rotate handles snap to 15° with Shift and 5° with Ctrl.

import type { Vec2 } from "./api/types.ts";
import { boxCenter, type Box } from "./geometry.ts";

export interface Mods {
  shift: boolean;
  ctrl: boolean;
  alt?: boolean;
}

/** Arrow-key nudge distances in mm (Preferences can change them). */
export interface NudgeSteps {
  normal: number;
  shift: number;
  ctrl: number;
  ctrlShift: number;
}

export const NUDGE_STEPS: NudgeSteps = { normal: 5, shift: 20, ctrl: 1, ctrlShift: 0.1 };

/** Nudge distance for an arrow key, in mm. */
export function nudgeStep(m: Mods, steps: NudgeSteps = NUDGE_STEPS): number {
  if (m.ctrl && m.shift) return steps.ctrlShift;
  if (m.ctrl) return steps.ctrl;
  if (m.shift) return steps.shift;
  return steps.normal;
}

/** Translation for an arrow key, or null for other keys. Up moves toward +Y. */
export function nudgeDelta(key: string, m: Mods, steps: NudgeSteps = NUDGE_STEPS): Vec2 | null {
  const s = nudgeStep(m, steps);
  switch (key) {
    case "ArrowLeft":
      return [-s, 0];
    case "ArrowRight":
      return [s, 0];
    case "ArrowUp":
      return [0, s];
    case "ArrowDown":
      return [0, -s];
    default:
      return null;
  }
}

/** Counter-clockwise rotation for `.` (clockwise) and `,` (counter-clockwise), or null for other keys. */
export function rotateKeyDeg(key: string, m: Mods): number | null {
  // With Shift held, browsers report `>` and `<` on US layouts.
  const cw = key === "." || key === ">";
  const ccw = key === "," || key === "<";
  if (!cw && !ccw) return null;
  const step = m.ctrl && m.shift ? 1.5 : m.ctrl ? 15 : m.shift ? 45 : 90;
  return cw ? -step : step;
}

/** Shift-constrained move: horizontal, vertical or 45°, whichever is closest to the drag direction. */
export function constrainDelta(d: Vec2): Vec2 {
  const [dx, dy] = d;
  const ax = Math.abs(dx);
  const ay = Math.abs(dy);
  if (ax === 0 && ay === 0) return d;
  const ang = Math.atan2(ay, ax); // 0..90°
  if (ang < Math.PI / 8) return [dx, 0];
  if (ang > (3 * Math.PI) / 8) return [0, dy];
  const m = (ax + ay) / 2;
  return [Math.sign(dx) * m, Math.sign(dy) * m];
}

/** Scale handle positions: corners and edge midpoints of the selection box. */
export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

export const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

/** Where a handle sits on a box (+Y up, so "n" is the top edge, y1). */
export function handlePoint(b: Box, h: Handle): Vec2 {
  const [cx, cy] = boxCenter(b);
  const x = h.includes("w") ? b.x0 : h.includes("e") ? b.x1 : cx;
  const y = h.includes("s") ? b.y0 : h.includes("n") ? b.y1 : cy;
  return [x, y];
}

/** The point opposite a handle, which stays put while scaling. */
function oppositePoint(b: Box, h: Handle): Vec2 {
  const flip: Record<Handle, Handle> = { nw: "se", n: "s", ne: "sw", e: "w", se: "nw", s: "n", sw: "ne", w: "e" };
  return handlePoint(b, flip[h]);
}

/** Smallest width or height a handle drag leaves, in mm (LightBurn-like: an object never collapses to nothing). */
export const MIN_HANDLE_SIZE_MM = 0.1;

/**
 * Scale for dragging handle `h` of box `b` to field point `p`: factors and the fixed point. Corner handles keep the
 * aspect ratio unless Shift is held; Ctrl scales about the centre. Dragging past the fixed point gives a negative
 * factor, which mirrors the selection (as LightBurn does). Each scaled side keeps at least `minMm` (or its current
 * size, when that is already smaller), so the selection never collapses.
 */
export function scaleFromHandle(b: Box, h: Handle, p: Vec2, m: Mods, minMm = MIN_HANDLE_SIZE_MM): { sx: number; sy: number; about: Vec2 } {
  const about = m.ctrl ? boxCenter(b) : oppositePoint(b, h);
  const start = handlePoint(b, h);
  const fx = (v0: number, v1: number, a: number) => {
    const d0 = v0 - a;
    return Math.abs(d0) < 1e-9 ? 1 : (v1 - a) / d0;
  };
  let sx = h === "n" || h === "s" ? 1 : fx(start[0], p[0], about[0]);
  let sy = h === "e" || h === "w" ? 1 : fx(start[1], p[1], about[1]);
  // Smallest factor magnitude per axis: `minMm` of the current size, never more than 1 (a tiny object stays as is).
  const minFactor = (size: number) => (size > 1e-9 ? Math.min(1, minMm / size) : 1);
  let minX = minFactor(b.x1 - b.x0);
  let minY = minFactor(b.y1 - b.y0);
  const corner = h.length === 2;
  if (corner && !m.shift) {
    // Keep the aspect ratio: follow whichever axis the pointer moved further.
    const s = Math.abs(sx - 1) >= Math.abs(sy - 1) ? sx : sy;
    sx = sy = s;
    minX = minY = Math.max(minX, minY);
  }
  // Keep the sign (a flip), clamp the magnitude; exactly 0 counts as not flipped.
  const clamp = (s: number, min: number) => (Math.abs(s) >= min ? s : s < 0 ? -min : min);
  return {
    sx: h === "n" || h === "s" ? sx : clamp(sx, minX),
    sy: h === "e" || h === "w" ? sy : clamp(sy, minY),
    about,
  };
}

/**
 * A handle scale made uniform, for a shape that must stay round (a locked circle): edge handles drive both axes by
 * their own factor (the other axis keeps a positive sign, so the circle grows evenly about the edge's centre line),
 * corner handles use whichever axis moved further, even with Shift.
 */
export function keepRound(sc: { sx: number; sy: number; about: Vec2 }, h: Handle): { sx: number; sy: number; about: Vec2 } {
  if (h === "e" || h === "w") return { ...sc, sy: Math.abs(sc.sx) };
  if (h === "n" || h === "s") return { ...sc, sx: Math.abs(sc.sy) };
  const s = Math.abs(sc.sx - 1) >= Math.abs(sc.sy - 1) ? sc.sx : sc.sy;
  return { ...sc, sx: s, sy: s };
}

/**
 * `object.transform` parameters for signed scale factors: the engine takes positive factors only, so a negative factor
 * becomes a mirror on that axis, and both negative (a point reflection) a 180° rotation. The order the engine applies
 * (scale, mirror, rotate, all about the pivot) gives exactly `scaleAbout(sx, sy, about)`.
 */
export function signedScaleParams(sx: number, sy: number): { scale: Vec2; mirror?: "x" | "y"; rotate_deg?: number } {
  const scale: Vec2 = [Math.abs(sx), Math.abs(sy)];
  if (sx < 0 && sy < 0) return { scale, rotate_deg: 180 };
  if (sx < 0) return { scale, mirror: "x" };
  if (sy < 0) return { scale, mirror: "y" };
  return { scale };
}

/** One arrow-key press for nudging: what it moves, which key with which modifiers, and whether it is a key repeat. */
export interface NudgeKey {
  /** What the nudge moves: "stock" or the selected object ids, joined. */
  target: string;
  /** The key and modifiers, e.g. "ArrowLeft|ctrl"; see `nudgeKeyId`. */
  key: string;
  /** `KeyboardEvent.repeat`: the key is being held down. */
  repeat: boolean;
}

/** Identity of an arrow key with its modifiers, so Ctrl+Right and Shift+Up never count as the same held key. */
export function nudgeKeyId(key: string, m: Mods): string {
  return [key, m.ctrl ? "ctrl" : "", m.shift ? "shift" : ""].filter(Boolean).join("|");
}

/**
 * Whether nudge `next` may be merged into the nudge still waiting to be sent (`queued`), making one undo step. Only
 * auto-repeats of the same held key on the same target merge: every separate press is its own undo step (spec), and
 * so is a different key or modifier, even when pressed quickly while a command is in flight.
 */
export function mergesNudge(queued: Pick<NudgeKey, "target" | "key"> | null, next: NudgeKey): boolean {
  return !!queued && next.repeat && queued.target === next.target && queued.key === next.key;
}

/**
 * Add nudge `next` to the queue of nudges waiting to be sent (oldest first, one command each): merged into the last
 * one when `mergesNudge` allows, else appended. Mutates and returns `queue`.
 */
export function queueNudge<T extends { target: string; key: string; d: Vec2 }>(queue: T[], next: T, repeat: boolean): T[] {
  const last = queue.length ? queue[queue.length - 1] : null;
  if (last && mergesNudge(last, { target: next.target, key: next.key, repeat })) last.d = [last.d[0] + next.d[0], last.d[1] + next.d[1]];
  else queue.push(next);
  return queue;
}

/** Rotation in degrees for a rotate-handle drag about `c` from `start` to `p`: Shift snaps to 15°, Ctrl to 5°. */
export function rotateFromHandle(c: Vec2, start: Vec2, p: Vec2, m: Mods): number {
  const a0 = Math.atan2(start[1] - c[1], start[0] - c[0]);
  const a1 = Math.atan2(p[1] - c[1], p[0] - c[0]);
  let deg = ((a1 - a0) * 180) / Math.PI;
  if (deg > 180) deg -= 360;
  if (deg <= -180) deg += 360;
  const step = m.shift ? 15 : m.ctrl ? 5 : 0;
  return step ? Math.round(deg / step) * step : deg;
}

/** Affine (SVG matrix order) that scales by sx, sy about `c`. */
export function scaleAbout(sx: number, sy: number, c: Vec2): [number, number, number, number, number, number] {
  return [sx, 0, 0, sy, c[0] - sx * c[0], c[1] - sy * c[1]];
}

/** Affine (SVG matrix order) that rotates by `deg` counter-clockwise about `c`. */
export function rotateAbout(deg: number, c: Vec2): [number, number, number, number, number, number] {
  const r = (deg * Math.PI) / 180;
  const cs = Math.cos(r);
  const sn = Math.sin(r);
  return [cs, sn, -sn, cs, c[0] - cs * c[0] + sn * c[1], c[1] - sn * c[0] - cs * c[1]];
}

export type AlignX = "left" | "center" | "right";
export type AlignY = "top" | "center" | "bottom";

function keyX(b: Box, x: AlignX): number {
  return x === "left" ? b.x0 : x === "right" ? b.x1 : (b.x0 + b.x1) / 2;
}
function keyY(b: Box, y: AlignY): number {
  return y === "bottom" ? b.y0 : y === "top" ? b.y1 : (b.y0 + b.y1) / 2;
}

/**
 * Moves that align each unit's x and/or y keypoint with the reference box's same keypoint. Units whose move is zero
 * are left out. A group is one unit with its overall bounds.
 */
export function alignMoves(units: readonly { id: string; box: Box }[], ref: Box, x?: AlignX, y?: AlignY): { id: string; d: Vec2 }[] {
  const out: { id: string; d: Vec2 }[] = [];
  for (const u of units) {
    const dx = x ? keyX(ref, x) - keyX(u.box, x) : 0;
    const dy = y ? keyY(ref, y) - keyY(u.box, y) : 0;
    if (Math.abs(dx) > 1e-9 || Math.abs(dy) > 1e-9) out.push({ id: u.id, d: [dx, dy] });
  }
  return out;
}

/**
 * Moves that space units evenly along an axis, keeping the first and last (by position) in place: `centers` evens out
 * the centre spacing, `gaps` the space between edges. Needs at least three units.
 */
export function distributeMoves(units: readonly { id: string; box: Box }[], axis: "x" | "y", by: "centers" | "gaps"): { id: string; d: Vec2 }[] {
  if (units.length < 3) return [];
  const lo = (b: Box) => (axis === "x" ? b.x0 : b.y0);
  const hi = (b: Box) => (axis === "x" ? b.x1 : b.y1);
  const mid = (b: Box) => (lo(b) + hi(b)) / 2;
  const sorted = [...units].sort((a, b) => mid(a.box) - mid(b.box));
  const n = sorted.length;
  const first = sorted[0].box;
  const last = sorted[n - 1].box;
  const out: { id: string; d: Vec2 }[] = [];
  const push = (id: string, v: number) => {
    if (Math.abs(v) > 1e-9) out.push({ id, d: axis === "x" ? [v, 0] : [0, v] });
  };
  if (by === "centers") {
    const step = (mid(last) - mid(first)) / (n - 1);
    sorted.forEach((u, i) => i > 0 && i < n - 1 && push(u.id, mid(first) + step * i - mid(u.box)));
  } else {
    const sizes = sorted.reduce((a, u) => a + (hi(u.box) - lo(u.box)), 0);
    const gap = (hi(last) - lo(first) - sizes) / (n - 1);
    let at = hi(first) + gap;
    for (let i = 1; i < n - 1; i++) {
      const u = sorted[i];
      push(u.id, at - lo(u.box));
      at += hi(u.box) - lo(u.box) + gap;
    }
  }
  return out;
}

/** Round to micrometres, so moves sent to the engine never carry float noise like 3.4930000000000003. */
export function roundMm(v: number): number {
  const r = Math.round(v * 1000) / 1000;
  return Object.is(r, -0) ? 0 : r;
}

/** Evaluate a number typed into a field: plain numbers or simple arithmetic (+ - * / and parentheses). */
export function evalNumber(text: string): number | null {
  const t = text.trim().replace(/,/g, ".");
  if (t === "") return null;
  if (!/^[\d\s.+\-*/()eE]+$/.test(t)) return null;
  let i = 0;
  const peek = () => t[i];
  const skip = () => {
    while (t[i] === " ") i++;
  };
  const num = (): number => {
    skip();
    const m = /^(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?/.exec(t.slice(i));
    if (!m) throw new Error("number");
    i += m[0].length;
    return Number(m[0]);
  };
  const factor = (): number => {
    skip();
    if (peek() === "-") {
      i++;
      return -factor();
    }
    if (peek() === "+") {
      i++;
      return factor();
    }
    if (peek() === "(") {
      i++;
      const v = expr();
      skip();
      if (peek() !== ")") throw new Error(")");
      i++;
      return v;
    }
    return num();
  };
  const term = (): number => {
    let v = factor();
    for (;;) {
      skip();
      const c = peek();
      if (c !== "*" && c !== "/") return v;
      i++;
      const r = factor();
      v = c === "*" ? v * r : v / r;
    }
  };
  const expr = (): number => {
    let v = term();
    for (;;) {
      skip();
      const c = peek();
      if (c !== "+" && c !== "-") return v;
      i++;
      const r = term();
      v = c === "+" ? v + r : v - r;
    }
  };
  try {
    const v = expr();
    skip();
    return i === t.length && Number.isFinite(v) ? v : null;
  } catch {
    return null;
  }
}
