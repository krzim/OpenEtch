// Maths of the numeric edits toolbar (LightBurn's Numeric Edits: X, Y, W, H, rotation, lock aspect, 9-point anchor,
// mm or % sizing). Every result is an `object.transform` parameter set in field mm; the toolbar sends it as one
// command.

import type { Vec2 } from "../api/types.ts";
import type { Box } from "../geometry.ts";

/** Reference point of the selection's bounding box (nw = top left; +Y is up). */
export type Anchor = "nw" | "n" | "ne" | "w" | "c" | "e" | "sw" | "s" | "se";
export const ANCHORS: Anchor[] = ["nw", "n", "ne", "w", "c", "e", "sw", "s", "se"];

export function anchorPoint(b: Box, a: Anchor): Vec2 {
  const x = a.includes("w") ? b.x0 : a.includes("e") ? b.x1 : (b.x0 + b.x1) / 2;
  const y = a.startsWith("s") ? b.y0 : a.startsWith("n") ? b.y1 : (b.y0 + b.y1) / 2;
  return [x, y];
}

const r3 = (v: number) => Math.round(v * 1000) / 1000;
const r6 = (v: number) => Math.round(v * 1e6) / 1e6;

/** Translation that puts the anchor point of `b` at (x, y); null when nothing moves. */
export function moveTo(b: Box, a: Anchor, x: number, y: number): Vec2 | null {
  const [ax, ay] = anchorPoint(b, a);
  const d: Vec2 = [r3(x - ax), r3(y - ay)];
  return d[0] === 0 && d[1] === 0 ? null : d;
}

/**
 * Scale factors for a new width and/or height (null keeps that side; with `lock` the other side follows), about the
 * anchor point so it stays put. Null when the size is unchanged or invalid.
 */
export function resizeTo(b: Box, a: Anchor, w: number | null, h: number | null, lock: boolean): { scale: Vec2; about: Vec2 } | null {
  const bw = b.x1 - b.x0;
  const bh = b.y1 - b.y0;
  // A zero-width side (a vertical line) cannot be scaled; it keeps factor 1.
  let sx = w !== null && bw > 1e-9 ? w / bw : 1;
  let sy = h !== null && bh > 1e-9 ? h / bh : 1;
  if (lock) {
    if (w !== null && bw > 1e-9) sy = sx;
    else if (h !== null && bh > 1e-9) sx = sy;
  }
  return scaleResult(b, a, sx, sy);
}

/** Scale factors for percentages (100 = unchanged), about the anchor. */
export function scalePercent(b: Box, a: Anchor, px: number | null, py: number | null, lock: boolean): { scale: Vec2; about: Vec2 } | null {
  let sx = px !== null ? px / 100 : 1;
  let sy = py !== null ? py / 100 : 1;
  if (lock) {
    if (px !== null) sy = sx;
    else if (py !== null) sx = sy;
  }
  return scaleResult(b, a, sx, sy);
}

function scaleResult(b: Box, a: Anchor, sx: number, sy: number): { scale: Vec2; about: Vec2 } | null {
  if (!(sx > 0 && sy > 0 && Number.isFinite(sx) && Number.isFinite(sy))) return null;
  if (Math.abs(sx - 1) < 1e-9 && Math.abs(sy - 1) < 1e-9) return null;
  const [ax, ay] = anchorPoint(b, a);
  return { scale: [r6(sx), r6(sy)], about: [r3(ax), r3(ay)] };
}

/** Rotation to apply so the shown angle becomes `target` (a lone object) or by `target` (several), in -180..180. */
export function rotationDelta(current: number, target: number, absolute: boolean): number | null {
  let d = absolute ? target - current : target;
  d = d % 360;
  if (d <= -180) d += 360;
  if (d > 180) d -= 360;
  return Math.abs(d) < 1e-9 ? null : r6(d);
}
