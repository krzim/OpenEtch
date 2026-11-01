// Pure maths for the drawing and measuring tools: angle constraint (Shift, 15° steps), rectangle/ellipse drags with
// LightBurn's modifiers (Shift square, Ctrl from the centre), polygon/star drags, the pen tool's path data, the
// measure readout and ruler ticks. Tested in drawTools.test.ts.

import type { Vec2 } from "./api/types.ts";

/** LightBurn's and Fusion's angle step while Shift is held. */
export const ANGLE_STEP_DEG = 15;

/** `to` turned about `from` onto the nearest multiple of `stepDeg`, keeping its distance along that direction. */
export function constrainAngle(from: Vec2, to: Vec2, stepDeg = ANGLE_STEP_DEG): Vec2 {
  const dx = to[0] - from[0];
  const dy = to[1] - from[1];
  const len = Math.hypot(dx, dy);
  if (len === 0) return [from[0], from[1]];
  const step = (stepDeg * Math.PI) / 180;
  const a = Math.round(Math.atan2(dy, dx) / step) * step;
  // Project onto the constrained direction (so the cursor's position along it is kept).
  const ux = Math.cos(a);
  const uy = Math.sin(a);
  const l = dx * ux + dy * uy;
  return [from[0] + clean(ux * l), from[1] + clean(uy * l)];
}

/** Rounds away float noise such as 6e-17 so straight lines come out exactly straight. */
function clean(v: number): number {
  return Math.abs(v) < 1e-12 ? 0 : v;
}

/**
 * Corners of a rectangle or ellipse drag from `start` to `end`: Shift makes it square (the larger side wins, the drag's
 * direction kept), Ctrl draws from the centre (`start` is the centre, `end` a corner).
 */
export function dragCorners(start: Vec2, end: Vec2, m: { shift: boolean; ctrl: boolean }): { a: Vec2; b: Vec2 } {
  let dx = end[0] - start[0];
  let dy = end[1] - start[1];
  if (m.shift) {
    const side = Math.max(Math.abs(dx), Math.abs(dy));
    dx = dx < 0 ? -side : side;
    dy = dy < 0 ? -side : side;
  }
  const b: Vec2 = [start[0] + dx, start[1] + dy];
  const a: Vec2 = m.ctrl ? [start[0] - dx, start[1] - dy] : [start[0], start[1]];
  return { a, b };
}

/**
 * A polygon dragged from its centre `c` to a corner `p`: the corner radius and the rotation that puts the first corner
 * under the pointer (shape.polygon's first corner points up at rotation 0). Shift turns in 15° steps.
 */
export function polygonFromDrag(c: Vec2, p: Vec2, shift: boolean): { radius: number; rotationDeg: number } {
  const q = shift ? constrainAngle(c, p) : p;
  const radius = Math.hypot(q[0] - c[0], q[1] - c[1]);
  if (radius === 0) return { radius: 0, rotationDeg: 0 };
  const deg = (Math.atan2(q[1] - c[1], q[0] - c[0]) * 180) / Math.PI - 90;
  return { radius, rotationDeg: normalizeDeg(deg) };
}

/** Degrees into (-180, 180], rounded to 1e-6. */
export function normalizeDeg(d: number): number {
  let v = d % 360;
  if (v <= -180) v += 360;
  if (v > 180) v -= 360;
  const r = Math.round(v * 1e6) / 1e6;
  return Object.is(r, -0) ? 0 : r;
}

/** Corner points of a regular polygon or star (inner ratio 0..1), first corner at `rotationDeg` from +Y, counter-clockwise. */
export function polygonPoints(c: Vec2, sides: number, radius: number, rotationDeg: number, starRatio: number | null): Vec2[] {
  const n = Math.max(3, Math.floor(sides));
  const pts: Vec2[] = [];
  const base = Math.PI / 2 + (rotationDeg * Math.PI) / 180;
  const star = starRatio !== null && starRatio > 0 && starRatio < 1;
  const k = star ? 2 * n : n;
  for (let i = 0; i < k; i++) {
    const a = base + (2 * Math.PI * i) / k;
    const r = star && i % 2 === 1 ? radius * (starRatio as number) : radius;
    pts.push([c[0] + r * Math.cos(a), c[1] + r * Math.sin(a)]);
  }
  return pts;
}

/** A pen node: the point and its optional handles (field mm). */
export interface PenNode {
  p: Vec2;
  hin?: Vec2 | null;
  hout?: Vec2 | null;
}

/** Pen node made by dragging from `p` to `drag`: the outgoing handle follows the pointer, the incoming one mirrors it. */
export function penNode(p: Vec2, drag: Vec2 | null): PenNode {
  if (!drag || (drag[0] === p[0] && drag[1] === p[1])) return { p };
  return { p, hout: drag, hin: [2 * p[0] - drag[0], 2 * p[1] - drag[1]] };
}

/** Whether any segment of the pen path is curved. */
export function penHasCurves(nodes: readonly PenNode[], closed: boolean): boolean {
  return nodes.some((n, i) => (n.hout && (closed || i < nodes.length - 1)) || (n.hin && (closed || i > 0)));
}

const r4 = (v: number) => Math.round(v * 1e4) / 1e4;
const fmt = (p: Vec2) => `${r4(p[0])},${r4(p[1])}`;

function penSeg(a: PenNode, b: PenNode): string {
  if (!a.hout && !b.hin) return `L${fmt(b.p)}`;
  return `C${fmt(a.hout ?? a.p)} ${fmt(b.hin ?? b.p)} ${fmt(b.p)}`;
}

/** SVG path data (field mm) through the pen nodes: lines between plain nodes, cubics where a handle exists. */
export function penPathData(nodes: readonly PenNode[], closed: boolean): string {
  if (!nodes.length) return "";
  let d = `M${fmt(nodes[0].p)}`;
  for (let i = 1; i < nodes.length; i++) d += penSeg(nodes[i - 1], nodes[i]);
  if (closed && nodes.length > 1) {
    const last = nodes[nodes.length - 1];
    const first = nodes[0];
    if (last.hout || first.hin) d += penSeg(last, first);
    d += "Z";
  }
  return d;
}

/** Points of the finished line: consecutive duplicates (a double-click's second press) removed. */
export function cleanPoints(pts: readonly Vec2[], eps = 1e-6): Vec2[] {
  const out: Vec2[] = [];
  for (const p of pts) {
    const q = out[out.length - 1];
    if (!q || Math.hypot(p[0] - q[0], p[1] - q[1]) > eps) out.push(p);
  }
  return out;
}

export interface Measure {
  dist: number;
  dx: number;
  dy: number;
  /** Counter-clockwise from +X, (-180, 180]. */
  angleDeg: number;
}

/** Distance, deltas and angle from `a` to `b`. */
export function measure(a: Vec2, b: Vec2): Measure {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  return { dist: Math.hypot(dx, dy), dx, dy, angleDeg: dx === 0 && dy === 0 ? 0 : normalizeDeg((Math.atan2(dy, dx) * 180) / Math.PI) };
}

/** "12.35 mm · ΔX 10.00 · ΔY 7.25 · 35.9°". */
export function measureText(m: Measure): string {
  return `${m.dist.toFixed(2)} mm · ΔX ${m.dx.toFixed(2)} · ΔY ${m.dy.toFixed(2)} · ${m.angleDeg.toFixed(1)}°`;
}

export interface Tick {
  v: number;
  major: boolean;
  label?: string;
}

/**
 * Ruler ticks between field coordinates `v0` and `v1` at `scale` px/mm: a labelled major tick every 1/2/5 × 10^n mm
 * with at least `minLabelPx` between labels, and minor ticks between them (no closer than 5 px).
 */
export function rulerTicks(v0: number, v1: number, scale: number, minLabelPx = 60): Tick[] {
  const lo = Math.min(v0, v1);
  const hi = Math.max(v0, v1);
  if (!(scale > 0) || !Number.isFinite(lo) || !Number.isFinite(hi)) return [];
  const raw = minLabelPx / scale;
  const pow = Math.pow(10, Math.floor(Math.log10(raw)));
  const major = [1, 2, 5, 10].map((m) => m * pow).find((s) => s >= raw) ?? 10 * pow;
  const divs = [10, 5, 2].find((d) => (major / d) * scale >= 5) ?? 1;
  const minor = major / divs;
  const out: Tick[] = [];
  const start = Math.ceil(lo / minor);
  const end = Math.floor(hi / minor);
  if (end - start > 5000) return [];
  const digits = Math.max(0, -Math.floor(Math.log10(major) + 1e-9));
  for (let i = start; i <= end; i++) {
    const v = i * minor;
    const isMajor = i % divs === 0;
    const vv = Math.abs(v) < minor * 1e-6 ? 0 : v;
    out.push(isMajor ? { v: vv, major: true, label: vv.toFixed(digits) } : { v: vv, major: false });
  }
  return out;
}
