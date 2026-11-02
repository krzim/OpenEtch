// The canvas HUD's layout (hud/CanvasHud.tsx): one shared placement for every control that floats over the canvas, so
// they stack instead of overlapping. Pure, so `npm test` covers it (hudLayout.test.ts).
//
// The HUD area is the canvas's box minus the safe-area bands. Obstacles are boxes the HUD must stay clear of but does
// not move: the rulers and their corner, the phone's bottom sheet at its current height, a tablet dock overlay. Items
// are placed one at a time in `rank` order (lower first), each in a named slot:
//
//   tl  tc  tr        top edge: the item starts against the top and moves down past whatever is in its way
//          c          centre: the largest free rectangle (the empty-document card)
//   bl  bc  br        bottom edge: starts against the bottom and moves up
//
// An anchored item (the selection action bar above the selection, the snap bubble above the finger) starts next to its
// anchor box instead, above it or below it, and moves away from it the same way.
//
// Placement runs in three rounds: the required controls (view buttons, tool bars, the player row, the off-field
// warning), then the messages (`late`: toasts, the import warnings), then the optional controls (modifier bar, the
// phone's undo / redo buttons, the readout, the selection bar, the snap bubble). So a message never covers a control
// (an optional one moves or hides for it), and when room runs out it is the optional controls that hide. An optional
// edge item also hides rather than wander past the middle of the area (a bottom bar does not end up at the top).
//
// At each height the item takes the free stretch of that row nearest its slot's side; an item with `minW` may shrink
// to fit a narrower stretch (then the caller measures its wrapped height and lays out again). An item that fits
// nowhere is hidden when `optional`, else kept at its slot's own spot (clear of the obstacles; hidden when even that
// is impossible).

export type HudSlot = "tl" | "tc" | "tr" | "bl" | "bc" | "br" | "c";

export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface HudItemSpec {
  id: string;
  slot: HudSlot;
  /** Placement order: lower ranks are placed first and so sit nearer their edge. */
  rank: number;
  /** Natural (max-content) size. */
  w: number;
  h: number;
  /** The item may shrink to this width (it wraps). Without it the item keeps its natural width. */
  minW?: number;
  /** Centre items: the least height worth showing (the item scrolls inside a shorter box). Default min(h, 120). */
  minH?: number;
  /** Edge items: shorter heights it can also take, tallest first (the toasts: only the newest ones). */
  shorter?: number[];
  /** Hide the item when there is no room (instead of keeping it at its slot's spot); placed in the last round. */
  optional?: boolean;
  /** A message (toast, warning list): placed after the required controls, before the optional ones. */
  late?: boolean;
  /** Place next to this box (HUD-local px) instead of a slot's edge; `slot` is then the fallback. */
  anchor?: { box: Box; side: "above" | "below"; gap?: number };
}

export interface HudPlacement {
  x: number;
  y: number;
  /** The width it was given (≤ its natural width). */
  w: number;
  h: number;
  /** A centre item's box is shorter than the item: it scrolls. */
  maxH?: number;
  hidden: boolean;
  /** No free room was found: the item sits at its slot's spot over something else. */
  overlaps?: boolean;
}

export const HUD_GAP = 8;

const right = (b: Box) => b.x + b.w;
const bottom = (b: Box) => b.y + b.h;

export function intersects(a: Box, b: Box): boolean {
  return a.x < right(b) && b.x < right(a) && a.y < bottom(b) && b.y < bottom(a);
}

function grow(b: Box, d: number): Box {
  return { x: b.x - d, y: b.y - d, w: b.w + 2 * d, h: b.h + 2 * d };
}

/** Clip `b` to `area`; null when nothing is left. */
export function clip(b: Box, area: Box): Box | null {
  const x0 = Math.max(b.x, area.x);
  const y0 = Math.max(b.y, area.y);
  const x1 = Math.min(right(b), right(area));
  const y1 = Math.min(bottom(b), bottom(area));
  return x1 > x0 && y1 > y0 ? { x: x0, y: y0, w: x1 - x0, h: y1 - y0 } : null;
}

/** The free stretches of [x0, x1] that none of `blocks` covers (they are already grown by the gap). */
export function freeSpans(x0: number, x1: number, blocks: Box[]): [number, number][] {
  const cuts = blocks.map((b) => [b.x, right(b)] as [number, number]).sort((a, b) => a[0] - b[0]);
  const out: [number, number][] = [];
  let at = x0;
  for (const [a, b] of cuts) {
    if (b <= at) continue;
    if (a > at) out.push([at, Math.min(a, x1)]);
    at = Math.max(at, b);
    if (at >= x1) break;
  }
  if (at < x1) out.push([at, x1]);
  return out.filter(([a, b]) => b > a);
}

type Side = "left" | "center" | "right";

function sideOf(slot: HudSlot): Side {
  return slot === "tl" || slot === "bl" ? "left" : slot === "tr" || slot === "br" ? "right" : "center";
}

/**
 * The best spot in the row [y, y + h] for an item of natural width `w` (shrinkable to `minW`) that would like to sit
 * at `prefX`: the free stretch nearest to it. Null when no stretch is wide enough.
 */
function fitRow(area: Box, placed: Box[], gap: number, y: number, w: number, h: number, minW: number, prefX: number, side: Side): { x: number; w: number } | null {
  const row = { x: area.x, y, w: area.w, h };
  const mid = area.x + area.w / 2;
  const blocks = placed.map((b) => grow(b, gap)).filter((b) => intersects(b, row));
  let best: { x: number; w: number; d: number } | null = null;
  for (const [a, b] of freeSpans(area.x, right(area), blocks)) {
    const cw = Math.min(w, b - a);
    if (cw < minW - 0.5) continue;
    const x = Math.min(Math.max(prefX, a), b - cw);
    // A side's item stays on its own half (it moves up or down instead of drifting to the other side).
    if ((side === "left" && x > mid + 0.5) || (side === "right" && x + cw < mid - 0.5)) continue;
    const d = Math.abs(x - prefX);
    if (!best || d < best.d - 0.5) best = { x, w: cw, d };
  }
  return best ? { x: best.x, w: best.w } : null;
}

/**
 * Walk from `y0` in direction `dir` (+1 down, -1 up) until the row is free enough, jumping each time just past the
 * nearest thing in the way. Null when the item runs out of the area.
 */
function walk(area: Box, placed: Box[], gap: number, y0: number, dir: 1 | -1, w: number, h: number, minW: number, prefX: number, side: Side = "center"): Box | null {
  let y = y0;
  for (let guard = 0; guard < 200; guard++) {
    if (y < area.y - 0.5 || y + h > bottom(area) + 0.5) return null;
    const fit = fitRow(area, placed, gap, y, w, h, minW, prefX, side);
    if (fit) return { x: fit.x, y, w: fit.w, h };
    // Jump past the nearest box in this row.
    const row = { x: area.x, y, w: area.w, h };
    const inRow = placed.map((b) => grow(b, gap)).filter((b) => intersects(b, row));
    if (!inRow.length) return null;
    y = dir > 0 ? Math.min(...inRow.map(bottom)) : Math.max(...inRow.map((b) => b.y - h));
  }
  return null;
}

/** The largest free rectangle (scored by how much of the item it shows, then nearness to the centre). */
function centreFit(area: Box, placed: Box[], gap: number, w: number, h: number, minW: number, minH: number): Box | null {
  const blocks = placed.map((b) => grow(b, gap)).map((b) => clip(b, area)).filter((b): b is Box => !!b);
  const xs = [area.x, right(area), ...blocks.flatMap((b) => [b.x, right(b)])].sort((a, b) => a - b);
  const ys = [area.y, bottom(area), ...blocks.flatMap((b) => [b.y, bottom(b)])].sort((a, b) => a - b);
  const ux = [...new Set(xs)];
  const uy = [...new Set(ys)];
  const cx = area.x + area.w / 2;
  const cy = area.y + area.h / 2;
  let best: { box: Box; score: number } | null = null;
  for (let i = 0; i < ux.length; i++)
    for (let j = i + 1; j < ux.length; j++) {
      const rw = ux[j] - ux[i];
      if (rw < minW - 0.5) continue;
      for (let k = 0; k < uy.length; k++)
        for (let l = k + 1; l < uy.length; l++) {
          const rh = uy[l] - uy[k];
          if (rh < minH - 0.5) continue;
          const r = { x: ux[i], y: uy[k], w: rw, h: rh };
          if (blocks.some((b) => intersects(b, r))) continue;
          const fw = Math.min(w, rw);
          const fh = Math.min(h, rh);
          const fx = Math.min(Math.max(cx - fw / 2, r.x), right(r) - fw);
          const fy = Math.min(Math.max(cy - fh / 2, r.y), bottom(r) - fh);
          const score = fw * fh - 0.01 * (Math.abs(fx + fw / 2 - cx) + Math.abs(fy + fh / 2 - cy));
          if (!best || score > best.score) best = { box: { x: fx, y: fy, w: fw, h: fh }, score };
        }
    }
  return best?.box ?? null;
}

/**
 * Lay out the HUD's items in `area` (HUD-local px) around `obstacles`. Items are placed in rank order (ties keep the
 * given order); every placed item becomes an obstacle for the ones after it.
 */
export function layoutHud(area: Box, obstacles: Box[], items: HudItemSpec[], gap = HUD_GAP): Record<string, HudPlacement> {
  const placed: Box[] = obstacles.map((o) => clip(o, area)).filter((b): b is Box => !!b);
  const out: Record<string, HudPlacement> = {};
  const round = (it: HudItemSpec) => (it.optional ? 2 : it.late ? 1 : 0);
  const order = items.map((it, i) => ({ it, i })).sort((a, b) => round(a.it) - round(b.it) || a.it.rank - b.it.rank || a.i - b.i);
  // Edge items keep a gap from the area's edges too.
  const inner: Box = { x: area.x + gap, y: area.y + gap, w: Math.max(0, area.w - 2 * gap), h: Math.max(0, area.h - 2 * gap) };
  for (const { it } of order) {
    const w = Math.min(it.w, inner.w);
    const minW = Math.min(it.minW ?? it.w, w);
    const h = it.h;
    const side = sideOf(it.slot);
    const prefFor = (cw: number) => (side === "left" ? inner.x : side === "right" ? right(inner) - cw : inner.x + (inner.w - cw) / 2);
    let box: Box | null = null;
    let maxH: number | undefined;
    if (it.anchor) {
      const a = it.anchor.box;
      const ag = it.anchor.gap ?? 12;
      const prefX = a.x + a.w / 2 - w / 2;
      const above = () => walk(inner, placed, gap, Math.min(a.y - ag - h, bottom(inner) - h), -1, w, h, minW, prefX);
      const below = () => walk(inner, placed, gap, Math.max(bottom(a) + ag, inner.y), 1, w, h, minW, prefX);
      box = it.anchor.side === "above" ? (above() ?? below()) : (below() ?? above());
    } else if (it.slot === "c") {
      const minH = Math.min(it.minH ?? 120, h);
      box = centreFit(inner, placed, gap, w, h, minW, minH);
      if (box && box.h < h) maxH = box.h;
    } else {
      const top = it.slot[0] === "t";
      const mid = inner.y + inner.h / 2;
      for (const hh of [h, ...(it.shorter ?? []).filter((x) => x < h)]) {
        box = walk(inner, placed, gap, top ? inner.y : bottom(inner) - hh, top ? 1 : -1, w, hh, minW, prefFor(w), side);
        // An optional item stays on its own half: its near edge does not cross the middle.
        if (box && it.optional && (top ? box.y > mid : bottom(box) < mid)) box = null;
        if (box) break;
      }
      if (box && box.h < h) maxH = box.h;
    }
    if (box) {
      placed.push(box);
      out[it.id] = { x: box.x, y: box.y, w: box.w, h: box.h, maxH, hidden: false };
    } else if (it.optional) {
      out[it.id] = { x: 0, y: 0, w, h, hidden: true };
    } else {
      // Nowhere free: over the other items, but still clear of the obstacles (rulers, sheet). Where even that is
      // impossible (a sheet over nearly all the canvas) it hides: the sheet lies above the HUD and would cut it.
      const top = it.slot[0] !== "b";
      const hh = Math.min(h, ...(it.shorter ?? []));
      const y = it.slot === "c" ? inner.y + Math.max(0, (inner.h - h) / 2) : top ? inner.y : Math.max(inner.y, bottom(inner) - hh);
      const fixed = obstacles.map((o) => clip(o, area)).filter((b): b is Box => !!b);
      const free = it.slot === "c" ? null : walk(inner, fixed, gap, y, top ? 1 : -1, w, hh, minW, prefFor(w), side);
      if (!free && it.slot !== "c" && fixed.length) {
        out[it.id] = { x: 0, y: 0, w, h, hidden: true, overlaps: true };
        continue;
      }
      const fb = free ?? { x: prefFor(w), y, w, h: hh };
      if (fb.h < h) maxH = fb.h;
      placed.push(fb);
      out[it.id] = { ...fb, hidden: false, overlaps: true, maxH: it.slot === "c" && h > inner.h ? inner.h : maxH };
    }
  }
  return out;
}

/** The area left for the HUD: `box` minus the safe-area bands it reaches into (insets in viewport px). */
export function safeArea(box: Box, viewport: { w: number; h: number }, safe: { top: number; right: number; bottom: number; left: number }): Box {
  const l = Math.max(0, safe.left - box.x);
  const t = Math.max(0, safe.top - box.y);
  const r = Math.max(0, right(box) - (viewport.w - safe.right));
  const b = Math.max(0, bottom(box) - (viewport.h - safe.bottom));
  return { x: 0 + l, y: 0 + t, w: Math.max(0, box.w - l - r), h: Math.max(0, box.h - t - b) };
}
