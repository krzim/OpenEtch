// LightBurn's Docking tool (Arrange > Dock, the Docking toolbar; docs.lightburnsoftware.com/latest/Reference/Dock/):
// move the selected shapes left, right, up or down until they touch another shape's outline or the workspace edge.
// Pure, so `npm test` covers it; App turns the result into one `object.transform` batch (one undo step).
//
// - Contact is between the shapes' flattened outlines (each moving vertex cast along the direction against the other
//   shape's edges, and the other shape's vertices cast back), so shapes dock snugly, not by their bounding boxes.
// - A shape that sits inside another (its box inside the other's) is not stopped by that container's inner edges: it
//   goes on to the outer edge of another shape or the workspace (LightBurn). Shapes that overlap a moving one at the
//   start are not obstacles to it either (each finds its own shape to dock against).
// - Without "Move as group" each selected unit (a group moves as one) docks on its own, those nearest the destination
//   first, so a row of shapes stacks up against each other. With it, all stop when the first one touches.
// - "Lock inner objects": selected shapes inside another selected shape move with it.
// - Padding: the gap kept in the direction of travel; a shape already closer than that does not move.

import type { Vec2 } from "../api/types.ts";
import { flattenCmds, geometryCmds, type Box } from "../geometry.ts";
import type { DocObject } from "../api/types.ts";

/** An outline as points; a closed one repeats its first point at the end. */
export type Polyline = Vec2[];

export type DockDir = "left" | "right" | "up" | "down";

export interface DockShape {
  /** Unit id (an object, or the top group). */
  id: string;
  polys: Polyline[];
  box: Box;
}

export interface DockOptions {
  moveAsGroup: boolean;
  lockInner: boolean;
  /** mm, >= 0. */
  padding: number;
}

/** A shape's outline for docking: its flattened geometry under its transform. */
export function objectOutline(o: DocObject): Polyline[] {
  return flattenCmds(geometryCmds(o.geometry), o.transform, 16)
    .filter((pl) => pl.pts.length > 0)
    .map((pl) => (pl.closed && pl.pts.length > 1 ? [...pl.pts, pl.pts[0]] : pl.pts));
}

/** The bounding box of outlines, or null when they have no points. */
export function polysBox(polys: readonly Polyline[]): Box | null {
  let b: Box | null = null;
  for (const pl of polys)
    for (const [x, y] of pl) b = b ? { x0: Math.min(b.x0, x), y0: Math.min(b.y0, y), x1: Math.max(b.x1, x), y1: Math.max(b.y1, y) } : { x0: x, y0: y, x1: x, y1: y };
  return b;
}

export const DEFAULT_DOCK_OPTIONS: DockOptions = { moveAsGroup: false, lockInner: true, padding: 0 };

const DIR: Record<DockDir, Vec2> = { left: [-1, 0], right: [1, 0], up: [0, 1], down: [0, -1] };

const EPS = 1e-9;

/** Distance along unit vector `d` from point `p` to segment a-b, or Infinity when the ray misses it. */
function rayToSegment(p: Vec2, d: Vec2, a: Vec2, b: Vec2): number {
  const ex = b[0] - a[0];
  const ey = b[1] - a[1];
  const den = d[0] * ey - d[1] * ex;
  if (Math.abs(den) < EPS) return Infinity; // parallel: the end points of the segment are cast the other way
  const wx = a[0] - p[0];
  const wy = a[1] - p[1];
  const s = (wx * ey - wy * ex) / den;
  const u = (wx * d[1] - wy * d[0]) / den;
  if (s < -1e-7 || u < -1e-9 || u > 1 + 1e-9) return Infinity;
  return Math.max(0, s);
}

function segments(polys: Polyline[]): [Vec2, Vec2][] {
  const out: [Vec2, Vec2][] = [];
  for (const pl of polys) for (let i = 1; i < pl.length; i++) out.push([pl[i - 1], pl[i]]);
  return out;
}

function vertices(polys: Polyline[]): Vec2[] {
  return polys.flat();
}

/** How far shape `a` can travel along `d` before its outline touches `b`'s (Infinity: never). */
export function contactDistance(a: Polyline[], b: Polyline[], d: Vec2): number {
  const sa = segments(a);
  const sb = segments(b);
  const back: Vec2 = [-d[0], -d[1]];
  let best = Infinity;
  for (const p of vertices(a)) for (const [s0, s1] of sb) best = Math.min(best, rayToSegment(p, d, s0, s1));
  for (const p of vertices(b)) for (const [s0, s1] of sa) best = Math.min(best, rayToSegment(p, back, s0, s1));
  return best;
}

function boxInside(inner: Box, outer: Box): boolean {
  return inner.x0 >= outer.x0 - 1e-6 && inner.x1 <= outer.x1 + 1e-6 && inner.y0 >= outer.y0 - 1e-6 && inner.y1 <= outer.y1 + 1e-6;
}

function boxesOverlap(a: Box, b: Box): boolean {
  return a.x0 < b.x1 - 1e-6 && b.x0 < a.x1 - 1e-6 && a.y0 < b.y1 - 1e-6 && b.y0 < a.y1 - 1e-6;
}

function segsCross(p: Vec2, q: Vec2, r: Vec2, s: Vec2): boolean {
  const o = (a: Vec2, b: Vec2, c: Vec2) => (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
  const d1 = o(r, s, p);
  const d2 = o(r, s, q);
  const d3 = o(p, q, r);
  const d4 = o(p, q, s);
  return d1 * d2 < -EPS && d3 * d4 < -EPS;
}

/** The outlines cross (the shapes overlap at the start): not an obstacle for each other. */
function overlapping(a: DockShape, b: DockShape): boolean {
  if (!boxesOverlap(a.box, b.box)) return false;
  const sb = segments(b.polys);
  for (const [p, q] of segments(a.polys)) for (const [r, s] of sb) if (segsCross(p, q, r, s)) return true;
  return false;
}

function shift(s: DockShape, v: Vec2): DockShape {
  if (v[0] === 0 && v[1] === 0) return s;
  return {
    id: s.id,
    polys: s.polys.map((pl) => pl.map((p) => [p[0] + v[0], p[1] + v[1]] as Vec2)),
    box: { x0: s.box.x0 + v[0], y0: s.box.y0 + v[1], x1: s.box.x1 + v[0], y1: s.box.y1 + v[1] },
  };
}

function merge(id: string, parts: DockShape[]): DockShape {
  return {
    id,
    polys: parts.flatMap((p) => p.polys),
    box: {
      x0: Math.min(...parts.map((p) => p.box.x0)),
      y0: Math.min(...parts.map((p) => p.box.y0)),
      x1: Math.max(...parts.map((p) => p.box.x1)),
      y1: Math.max(...parts.map((p) => p.box.y1)),
    },
  };
}

/** Free travel of `m` along `dir` to the workspace edge (centred on 0, size `ws`) and to each obstacle, minus padding. */
function travel(m: DockShape, obstacles: readonly DockShape[], dir: DockDir, ws: Vec2, padding: number): number {
  const d = DIR[dir];
  const [hw, hh] = [ws[0] / 2, ws[1] / 2];
  const edge = dir === "left" ? m.box.x0 + hw : dir === "right" ? hw - m.box.x1 : dir === "up" ? hh - m.box.y1 : m.box.y0 + hh;
  let best = edge - padding;
  for (const o of obstacles) {
    // A container does not stop what is inside it; what the moving shape contains or crosses is no obstacle either.
    if (boxInside(m.box, o.box) || boxInside(o.box, m.box) || overlapping(m, o)) continue;
    // Only shapes ahead in the direction's lane can be hit.
    const lane = dir === "left" || dir === "right" ? o.box.y1 > m.box.y0 + 1e-9 && o.box.y0 < m.box.y1 - 1e-9 : o.box.x1 > m.box.x0 + 1e-9 && o.box.x0 < m.box.x1 - 1e-9;
    if (!lane) continue;
    const s = contactDistance(m.polys, o.polys, d);
    if (Number.isFinite(s)) best = Math.min(best, s - padding);
  }
  return Math.max(0, best);
}

/**
 * The move of each moving unit (ids from `moving`) for docking `dir`: translations in mm, units that do not move
 * left out. `others` are the shapes that stay (obstacles); `ws` the workspace size, centred on the origin.
 */
export function dockMoves(moving: readonly DockShape[], others: readonly DockShape[], dir: DockDir, ws: Vec2, opt: DockOptions): { id: string; d: Vec2 }[] {
  if (moving.length === 0) return [];
  const pad = Math.max(0, Number.isFinite(opt.padding) ? opt.padding : 0);
  const d = DIR[dir];
  // Lock inner objects: a selected unit inside another selected one travels with it.
  const owner = new Map<string, string>();
  if (opt.lockInner && !opt.moveAsGroup) {
    for (const a of moving) {
      const host = moving.filter((b) => b !== a && boxInside(a.box, b.box) && !boxInside(b.box, a.box));
      // The smallest container holds it.
      host.sort((x, y) => (x.box.x1 - x.box.x0) * (x.box.y1 - x.box.y0) - (y.box.x1 - y.box.x0) * (y.box.y1 - y.box.y0));
      if (host[0]) owner.set(a.id, host[0].id);
    }
  }
  const root = (id: string): string => {
    let r = id;
    for (let i = 0; i < 64 && owner.has(r); i++) r = owner.get(r)!;
    return r;
  };
  if (opt.moveAsGroup) {
    const all = merge("*", [...moving]);
    const s = travel(all, others, dir, ws, pad);
    return s > 1e-9 ? moving.map((m) => ({ id: m.id, d: [d[0] * s, d[1] * s] as Vec2 })) : [];
  }
  // Frames: each root with the inner units it carries.
  const frames = new Map<string, DockShape[]>();
  for (const m of moving) frames.set(root(m.id), [...(frames.get(root(m.id)) ?? []), m]);
  const units = [...frames.entries()].map(([id, parts]) => ({ id, parts, shape: merge(id, parts) }));
  // Nearest the destination first, so later ones stack against them.
  const lead = (b: Box) => (dir === "left" ? b.x0 : dir === "right" ? -b.x1 : dir === "up" ? -b.y1 : b.y0);
  units.sort((a, b) => lead(a.shape.box) - lead(b.shape.box));
  const placed = new Map<string, DockShape>(units.map((u) => [u.id, u.shape]));
  const out: { id: string; d: Vec2 }[] = [];
  for (const u of units) {
    const obstacles = [...others, ...units.filter((x) => x !== u).map((x) => placed.get(x.id)!)];
    const s = travel(u.shape, obstacles, dir, ws, pad);
    if (s <= 1e-9) continue;
    const v: Vec2 = [d[0] * s, d[1] * s];
    placed.set(u.id, shift(u.shape, v));
    for (const p of u.parts) out.push({ id: p.id, d: v });
  }
  return out;
}
