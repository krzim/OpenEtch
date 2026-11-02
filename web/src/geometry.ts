// Geometry helpers for display: affine maths, document geometry to SVG path data, and field-frame bounds. These only
// read the document; nothing here produces edits. Pure functions, no DOM, so they run under `node --test`.

import type { Affine, BezPathJson, DocObject, Geometry, Stock, Vec2 } from "./api/types.ts";

export const IDENTITY: Affine = [1, 0, 0, 1, 0, 0];

/** Axis-aligned box in field mm. */
export interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** Apply `a` to a point. */
export function apply(a: Affine, p: Vec2): Vec2 {
  return [a[0] * p[0] + a[2] * p[1] + a[4], a[1] * p[0] + a[3] * p[1] + a[5]];
}

/** `a * b` (apply `b` first), matching kurbo's `Mul`. */
export function mul(a: Affine, b: Affine): Affine {
  return [
    a[0] * b[0] + a[2] * b[1],
    a[1] * b[0] + a[3] * b[1],
    a[0] * b[2] + a[2] * b[3],
    a[1] * b[2] + a[3] * b[3],
    a[0] * b[4] + a[2] * b[5] + a[4],
    a[1] * b[4] + a[3] * b[5] + a[5],
  ];
}

export function translate(dx: number, dy: number): Affine {
  return [1, 0, 0, 1, dx, dy];
}

/** Counter-clockwise rotation in degrees (y-up field frame). */
export function rotate(deg: number): Affine {
  const r = (deg * Math.PI) / 180;
  const c = Math.cos(r);
  const s = Math.sin(r);
  return [c, s, -s, c, 0, 0];
}

/** Rotation part of an affine in degrees, from the image of the x axis. */
export function rotationDeg(a: Affine): number {
  return (Math.atan2(a[1], a[0]) * 180) / Math.PI;
}

/** A mirror in an object's transform, as Properties shows it: none, horizontal (X negated) or vertical (Y negated). */
export type Flip = "none" | "horizontal" | "vertical";

/**
 * Rotation and flip of an affine, for display. A transform without a mirror is a rotation (from the image of the x
 * axis). A mirrored one (negative determinant) is shown as a flip followed by a rotation, choosing whichever flip needs
 * the smaller rotation, so a plain horizontal or vertical flip reads "rotation 0" instead of "180" (a horizontal flip
 * rotated 180 degrees is the same shape as a vertical flip). Rotation is in (-180, 180].
 */
export function transformParts(a: Affine): { rotationDeg: number; flip: Flip } {
  const det = a[0] * a[3] - a[1] * a[2];
  if (!(det < 0)) return { rotationDeg: normDeg(rotationDeg(a)), flip: "none" };
  // Rotation after a horizontal flip maps the x axis to -(a, b); after a vertical flip, to (a, b).
  const h = normDeg((Math.atan2(-a[1], -a[0]) * 180) / Math.PI);
  const v = normDeg((Math.atan2(a[1], a[0]) * 180) / Math.PI);
  // `+ 0` turns -0 (atan2 of a negated zero) into 0, so the field never reads "-0".
  return Math.abs(v) < Math.abs(h) - 1e-9 ? { rotationDeg: v + 0, flip: "vertical" } : { rotationDeg: h + 0, flip: "horizontal" };
}

/** Normalise an angle to (-180, 180]. */
export function normDeg(d: number): number {
  let r = d % 360;
  if (r <= -180) r += 360;
  if (r > 180) r -= 360;
  return r;
}

/** One absolute path command in object-local coordinates. */
export type PathCmd =
  | { c: "M" | "L"; p: Vec2 }
  | { c: "Q"; p1: Vec2; p: Vec2 }
  | { c: "C"; p1: Vec2; p2: Vec2; p: Vec2 }
  | { c: "Z" };

/** Parse a kurbo BezPath as JSON (SVG string or element array) into absolute commands. Unknown input yields []. */
export function parseBezPath(path: BezPathJson): PathCmd[] {
  if (typeof path === "string") return parseSvgPath(path);
  if (!Array.isArray(path)) return [];
  const out: PathCmd[] = [];
  const pt = (q: { x: number; y: number }): Vec2 => [q.x, q.y];
  for (const el of path) {
    if (el === "ClosePath") out.push({ c: "Z" });
    else if (typeof el !== "object" || el === null) continue;
    else if ("MoveTo" in el) out.push({ c: "M", p: pt(el.MoveTo) });
    else if ("LineTo" in el) out.push({ c: "L", p: pt(el.LineTo) });
    else if ("QuadTo" in el) out.push({ c: "Q", p1: pt(el.QuadTo[0]), p: pt(el.QuadTo[1]) });
    else if ("CurveTo" in el) out.push({ c: "C", p1: pt(el.CurveTo[0]), p2: pt(el.CurveTo[1]), p: pt(el.CurveTo[2]) });
  }
  return out;
}

/**
 * Parse SVG path data with M/L/H/V/Q/C/Z in absolute or relative form (what kurbo's `to_svg` writes, plus the common
 * relative variants). Arcs and smooth curves are not produced by kurbo and are skipped.
 */
export function parseSvgPath(d: string): PathCmd[] {
  const tokens = d.match(/[a-zA-Z]|[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?/g) ?? [];
  const out: PathCmd[] = [];
  let i = 0;
  let cmd = "";
  let cur: Vec2 = [0, 0];
  let start: Vec2 = [0, 0];
  const num = (): number => {
    const t = tokens[i++];
    const v = t === undefined ? NaN : Number(t);
    return v;
  };
  const isNum = (t: string | undefined) => t !== undefined && !/^[a-zA-Z]$/.test(t);
  while (i < tokens.length) {
    if (!isNum(tokens[i])) cmd = tokens[i++] ?? "";
    const rel = cmd === cmd.toLowerCase();
    const P = (): Vec2 => {
      const x = num();
      const y = num();
      return rel ? [cur[0] + x, cur[1] + y] : [x, y];
    };
    switch (cmd.toUpperCase()) {
      case "M": {
        const p = P();
        out.push({ c: "M", p });
        cur = start = p;
        cmd = rel ? "l" : "L"; // implicit lineto after the first pair
        break;
      }
      case "L": {
        const p = P();
        out.push({ c: "L", p });
        cur = p;
        break;
      }
      case "H": {
        const x = num();
        cur = [rel ? cur[0] + x : x, cur[1]];
        out.push({ c: "L", p: cur });
        break;
      }
      case "V": {
        const y = num();
        cur = [cur[0], rel ? cur[1] + y : y];
        out.push({ c: "L", p: cur });
        break;
      }
      case "Q": {
        const p1 = P();
        const p = P();
        out.push({ c: "Q", p1, p });
        cur = p;
        break;
      }
      case "C": {
        const p1 = P();
        const p2 = P();
        const p = P();
        out.push({ c: "C", p1, p2, p });
        cur = p;
        break;
      }
      case "Z":
        out.push({ c: "Z" });
        cur = start;
        if (isNum(tokens[i])) cmd = "L";
        break;
      default:
        // Unsupported command: skip its numbers.
        while (isNum(tokens[i])) i++;
    }
    if (out.length > 0 && Number.isNaN(cur[0] + cur[1])) return out.slice(0, -1);
  }
  return out;
}

/** Serialize commands to SVG path data, optionally mapping every point through `a`. */
export function toPathData(cmds: PathCmd[], a: Affine = IDENTITY): string {
  const f = (p: Vec2) => {
    const q = apply(a, p);
    return `${round(q[0])},${round(q[1])}`;
  };
  return cmds
    .map((k) => {
      switch (k.c) {
        case "M":
        case "L":
          return `${k.c}${f(k.p)}`;
        case "Q":
          return `Q${f(k.p1)} ${f(k.p)}`;
        case "C":
          return `C${f(k.p1)} ${f(k.p2)} ${f(k.p)}`;
        case "Z":
          return "Z";
      }
    })
    .join(" ");
}

function round(v: number): number {
  return Math.round(v * 1e4) / 1e4;
}

/** Bezier magic number for quarter-circle arcs. */
const K = 0.5522847498;

/** Ellipse of radii rx, ry centred at the origin, as four cubics. */
function ellipseCmds(rx: number, ry: number): PathCmd[] {
  return [
    { c: "M", p: [rx, 0] },
    { c: "C", p1: [rx, K * ry], p2: [K * rx, ry], p: [0, ry] },
    { c: "C", p1: [-K * rx, ry], p2: [-rx, K * ry], p: [-rx, 0] },
    { c: "C", p1: [-rx, -K * ry], p2: [-K * rx, -ry], p: [0, -ry] },
    { c: "C", p1: [K * rx, -ry], p2: [rx, -K * ry], p: [rx, 0] },
    { c: "Z" },
  ];
}

/** Rectangle centred at the origin with optional corner radius (clamped to half the short side). */
function rectCmds(w: number, h: number, r: number): PathCmd[] {
  const x0 = -w / 2;
  const y0 = -h / 2;
  const x1 = w / 2;
  const y1 = h / 2;
  const rr = Math.max(0, Math.min(r, Math.abs(w) / 2, Math.abs(h) / 2));
  if (rr === 0) {
    return [{ c: "M", p: [x0, y0] }, { c: "L", p: [x1, y0] }, { c: "L", p: [x1, y1] }, { c: "L", p: [x0, y1] }, { c: "Z" }];
  }
  const k = rr * (1 - K);
  return [
    { c: "M", p: [x0 + rr, y0] },
    { c: "L", p: [x1 - rr, y0] },
    { c: "C", p1: [x1 - k, y0], p2: [x1, y0 + k], p: [x1, y0 + rr] },
    { c: "L", p: [x1, y1 - rr] },
    { c: "C", p1: [x1, y1 - k], p2: [x1 - k, y1], p: [x1 - rr, y1] },
    { c: "L", p: [x0 + rr, y1] },
    { c: "C", p1: [x0 + k, y1], p2: [x0, y1 - k], p: [x0, y1 - rr] },
    { c: "L", p: [x0, y0 + rr] },
    { c: "C", p1: [x0, y0 + k], p2: [x0 + k, y0], p: [x0 + rr, y0] },
    { c: "Z" },
  ];
}

/**
 * Corner points of a regular polygon (or a star, `starRatio` set) on a circle of `radius`, the first one up (+Y).
 * `ry` stretches it to an ellipse `radius` x `ry`; `halfStep` turns it by 180° / sides (LightBurn's placement, an edge
 * centred on +Y), as geom::shapes polygon_points_at.
 */
export function polygonCorners(sides: number, radius: number, starRatio?: number | null, ry?: number | null, halfStep = false): Vec2[] {
  const n = Math.round(sides);
  if (!(n >= 3 && n <= 1000) || !(radius > 0) || !Number.isFinite(radius)) return [];
  const rY = ry ?? radius;
  if (!(rY > 0) || !Number.isFinite(rY)) return [];
  const start = Math.PI / 2 + (halfStep ? Math.PI / n : 0);
  const at = (k: number, a: number): Vec2 => [radius * k * Math.cos(a), rY * k * Math.sin(a)];
  if (starRatio !== null && starRatio !== undefined) {
    if (!(starRatio > 0 && starRatio < 1)) return [];
    return Array.from({ length: 2 * n }, (_, i) => at(i % 2 === 0 ? 1 : starRatio, start + (Math.PI * i) / n));
  }
  return Array.from({ length: n }, (_, i) => at(1, start + (2 * Math.PI * i) / n));
}

/** Cubics of a circular arc from angle `a0` sweeping `sweep` radians about `c` (pieces of at most 90 degrees). */
function arcCubics(c: Vec2, r: number, a0: number, sweep: number): PathCmd[] {
  const n = Math.max(1, Math.ceil(Math.abs(sweep) / (Math.PI / 2) - 1e-9));
  const d = sweep / n;
  const k = (4 / 3) * Math.tan(d / 4);
  const out: PathCmd[] = [];
  for (let i = 0; i < n; i++) {
    const s = a0 + i * d;
    const e = s + d;
    const [cs, ss, ce, se] = [Math.cos(s), Math.sin(s), Math.cos(e), Math.sin(e)];
    out.push({
      c: "C",
      p1: [c[0] + r * (cs - k * ss), c[1] + r * (ss + k * cs)],
      p2: [c[0] + r * (ce + k * se), c[1] + r * (se - k * ce)],
      p: [c[0] + r * ce, c[1] + r * se],
    });
  }
  return out;
}

/**
 * A parametric polygon's outline, as the engine builds it (geom::shapes polygon_path): every corner rounded by
 * `cornerRadius` where the rounding fits in half of each neighbouring side, else left sharp.
 */
export function polygonCmds(
  sides: number,
  radius: number,
  starRatio?: number | null,
  cornerRadius = 0,
  ry?: number | null,
  halfStep = false,
): PathCmd[] {
  const pts = polygonCorners(sides, radius, starRatio, ry, halfStep);
  const n = pts.length;
  if (n < 3) return [];
  const r = cornerRadius > 0 && Number.isFinite(cornerRadius) ? cornerRadius : 0;
  const corner = pts.map((c, k) => {
    const prev = pts[(k + n - 1) % n];
    const next = pts[(k + 1) % n];
    const la = Math.hypot(c[0] - prev[0], c[1] - prev[1]);
    const lb = Math.hypot(next[0] - c[0], next[1] - c[1]);
    if (r === 0 || la <= 0 || lb <= 0) return { pin: c, pout: c, arc: [] as PathCmd[] };
    const u: Vec2 = [(c[0] - prev[0]) / la, (c[1] - prev[1]) / la];
    const v: Vec2 = [(next[0] - c[0]) / lb, (next[1] - c[1]) / lb];
    const turn = Math.atan2(u[0] * v[1] - u[1] * v[0], u[0] * v[0] + u[1] * v[1]);
    const t = r * Math.tan(Math.abs(turn) / 2);
    if (Math.abs(turn) <= (Math.PI / 180) || !(t <= la / 2 + 1e-9 && t <= lb / 2 + 1e-9)) return { pin: c, pout: c, arc: [] as PathCmd[] };
    const pin: Vec2 = [c[0] - u[0] * t, c[1] - u[1] * t];
    const pout: Vec2 = [c[0] + v[0] * t, c[1] + v[1] * t];
    const normal: Vec2 = turn > 0 ? [-u[1], u[0]] : [u[1], -u[0]];
    const centre: Vec2 = [pin[0] + normal[0] * r, pin[1] + normal[1] * r];
    return { pin, pout, arc: arcCubics(centre, r, Math.atan2(pin[1] - centre[1], pin[0] - centre[0]), turn) };
  });
  const out: PathCmd[] = [{ c: "M", p: corner[0].pout }];
  for (let i = 1; i <= n; i++) {
    const k = corner[i % n];
    out.push({ c: "L", p: k.pin }, ...k.arc);
  }
  out.push({ c: "Z" });
  return out;
}

/** Object-local commands for a document geometry. */
export function geometryCmds(g: Geometry): PathCmd[] {
  switch (g.kind) {
    case "rect":
      return rectCmds(g.width_mm, g.height_mm, g.corner_radius_mm ?? 0);
    case "ellipse":
      return ellipseCmds(g.rx_mm, g.ry_mm);
    case "polygon":
      return polygonCmds(g.sides, g.radius_mm, g.star_ratio, g.corner_radius_mm ?? 0, g.ry_mm, g.half_step ?? false);
    case "path":
      return parseBezPath(g.path);
    case "text":
      return parseBezPath(g.outline);
    case "image":
      // The image's frame: used for bounds, hit testing and snapping; the bitmap itself is drawn separately.
      return rectCmds(g.width_mm, g.height_mm, 0);
    default:
      return [];
  }
}

/**
 * A string equal for equal geometry, for caches that outlive a document refetch (the UI refetches the whole document
 * after every edit, so unchanged objects arrive as new values): the path data itself for paths and text, the size for
 * images (never their pixels), the whole value for the parametric shapes.
 */
export function geometryKey(g: Geometry): string {
  const any = g as { kind: string; path?: unknown; outline?: unknown; width_mm?: number; height_mm?: number };
  switch (any.kind) {
    case "path":
      return typeof any.path === "string" ? `p${any.path}` : `P${JSON.stringify(any.path)}`;
    case "text":
      return typeof any.outline === "string" ? `t${any.outline}` : `T${JSON.stringify(any.outline)}`;
    case "image":
      return `i${any.width_mm}x${any.height_mm}`;
    default:
      return JSON.stringify(g);
  }
}

/** Stock outline in the field frame. */
export function stockCmds(s: Stock): { cmds: PathCmd[]; transform: Affine } {
  const local =
    s.shape.kind === "rect"
      ? rectCmds(s.shape.width_mm, s.shape.height_mm, s.shape.corner_radius_mm ?? 0)
      : ellipseCmds(s.shape.diameter_mm / 2, s.shape.diameter_mm / 2);
  return { cmds: local, transform: mul(translate(s.center[0], s.center[1]), rotate(s.rotation_deg)) };
}

/** Field-frame bounds of commands under `a`, sampling curves so the box is tight. Null for an empty path. */
export function cmdsBounds(cmds: PathCmd[], a: Affine = IDENTITY): Box | null {
  let b: Box | null = null;
  const add = (p: Vec2) => {
    const q = apply(a, p);
    if (!Number.isFinite(q[0]) || !Number.isFinite(q[1])) return;
    if (!b) b = { x0: q[0], y0: q[1], x1: q[0], y1: q[1] };
    else {
      b.x0 = Math.min(b.x0, q[0]);
      b.y0 = Math.min(b.y0, q[1]);
      b.x1 = Math.max(b.x1, q[0]);
      b.y1 = Math.max(b.y1, q[1]);
    }
  };
  let cur: Vec2 = [0, 0];
  let start: Vec2 = [0, 0];
  const N = 16;
  for (const k of cmds) {
    switch (k.c) {
      case "M":
        cur = start = k.p;
        add(cur);
        break;
      case "L":
        cur = k.p;
        add(cur);
        break;
      case "Q":
        for (let i = 1; i <= N; i++) {
          const t = i / N;
          const u = 1 - t;
          add([
            u * u * cur[0] + 2 * u * t * k.p1[0] + t * t * k.p[0],
            u * u * cur[1] + 2 * u * t * k.p1[1] + t * t * k.p[1],
          ]);
        }
        cur = k.p;
        break;
      case "C":
        for (let i = 1; i <= N; i++) {
          const t = i / N;
          const u = 1 - t;
          const w0 = u * u * u;
          const w1 = 3 * u * u * t;
          const w2 = 3 * u * t * t;
          const w3 = t * t * t;
          add([
            w0 * cur[0] + w1 * k.p1[0] + w2 * k.p2[0] + w3 * k.p[0],
            w0 * cur[1] + w1 * k.p1[1] + w2 * k.p2[1] + w3 * k.p[1],
          ]);
        }
        cur = k.p;
        break;
      case "Z":
        cur = start;
        break;
    }
  }
  return b;
}

/** Field-frame bounds of an object. */
export function objectBounds(o: DocObject): Box | null {
  return cmdsBounds(geometryCmds(o.geometry), o.transform);
}

/** Union of boxes, ignoring nulls. */
export function unionBoxes(boxes: (Box | null)[]): Box | null {
  let out: Box | null = null;
  for (const b of boxes) {
    if (!b) continue;
    out = out ? { x0: Math.min(out.x0, b.x0), y0: Math.min(out.y0, b.y0), x1: Math.max(out.x1, b.x1), y1: Math.max(out.y1, b.y1) } : { ...b };
  }
  return out;
}

export function boxCenter(b: Box): Vec2 {
  return [(b.x0 + b.x1) / 2, (b.y0 + b.y1) / 2];
}

export function boxSize(b: Box): Vec2 {
  return [b.x1 - b.x0, b.y1 - b.y0];
}

export function boxArea(b: Box): number {
  return Math.max(0, b.x1 - b.x0) * Math.max(0, b.y1 - b.y0);
}

/** `inner` lies entirely inside `outer` (edges may touch). */
export function boxInside(inner: Box, outer: Box): boolean {
  return inner.x0 >= outer.x0 && inner.x1 <= outer.x1 && inner.y0 >= outer.y0 && inner.y1 <= outer.y1;
}

export function boxesIntersect(a: Box, b: Box): boolean {
  return a.x0 <= b.x1 && a.x1 >= b.x0 && a.y0 <= b.y1 && a.y1 >= b.y0;
}

/** Box spanned by two points. */
export function boxFromPoints(a: Vec2, b: Vec2): Box {
  return { x0: Math.min(a[0], b[0]), y0: Math.min(a[1], b[1]), x1: Math.max(a[0], b[0]), y1: Math.max(a[1], b[1]) };
}

export function translateBox(b: Box, d: Vec2): Box {
  return { x0: b.x0 + d[0], y0: b.y0 + d[1], x1: b.x1 + d[0], y1: b.y1 + d[1] };
}

/** One flattened subpath in the field frame. */
export interface Polyline {
  pts: Vec2[];
  closed: boolean;
}

/** Flatten commands under `a` into polylines (curves sampled into `n` steps each). */
export function flattenCmds(cmds: PathCmd[], a: Affine = IDENTITY, n = 12): Polyline[] {
  const out: Polyline[] = [];
  let cur: Polyline | null = null;
  let p: Vec2 = [0, 0];
  let start: Vec2 = [0, 0];
  const push = (q: Vec2) => {
    if (!cur) {
      cur = { pts: [apply(a, p)], closed: false };
      out.push(cur);
    }
    cur.pts.push(apply(a, q));
  };
  for (const k of cmds) {
    switch (k.c) {
      case "M":
        p = start = k.p;
        cur = { pts: [apply(a, p)], closed: false };
        out.push(cur);
        break;
      case "L":
        push(k.p);
        p = k.p;
        break;
      case "Q":
        for (let i = 1; i <= n; i++) {
          const t = i / n;
          const u = 1 - t;
          push([u * u * p[0] + 2 * u * t * k.p1[0] + t * t * k.p[0], u * u * p[1] + 2 * u * t * k.p1[1] + t * t * k.p[1]]);
        }
        p = k.p;
        break;
      case "C":
        for (let i = 1; i <= n; i++) {
          const t = i / n;
          const u = 1 - t;
          const w0 = u * u * u;
          const w1 = 3 * u * u * t;
          const w2 = 3 * u * t * t;
          const w3 = t * t * t;
          push([
            w0 * p[0] + w1 * k.p1[0] + w2 * k.p2[0] + w3 * k.p[0],
            w0 * p[1] + w1 * k.p1[1] + w2 * k.p2[1] + w3 * k.p[1],
          ]);
        }
        p = k.p;
        break;
      case "Z":
        if (cur) {
          (cur as Polyline).closed = true;
          cur = null;
        }
        p = start;
        break;
    }
  }
  return out.filter((pl) => pl.pts.length > 0);
}

/** Distance from `p` to segment ab. */
export function distToSegment(p: Vec2, a: Vec2, b: Vec2): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const l2 = dx * dx + dy * dy;
  const t = l2 > 0 ? Math.max(0, Math.min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2)) : 0;
  return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy));
}

/** Shortest distance from `p` to any line of the polylines (closing segments included). */
export function distToPolylines(p: Vec2, polys: Polyline[]): number {
  let best = Infinity;
  for (const pl of polys) {
    const n = pl.pts.length;
    if (n === 1) best = Math.min(best, Math.hypot(p[0] - pl.pts[0][0], p[1] - pl.pts[0][1]));
    for (let i = 1; i < n; i++) best = Math.min(best, distToSegment(p, pl.pts[i - 1], pl.pts[i]));
    if (pl.closed && n > 2) best = Math.min(best, distToSegment(p, pl.pts[n - 1], pl.pts[0]));
  }
  return best;
}

/** Even-odd inside test over the closed polylines. */
export function insideEvenOdd(p: Vec2, polys: Polyline[]): boolean {
  let inside = false;
  for (const pl of polys) {
    if (!pl.closed) continue;
    const pts = pl.pts;
    for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
      const [xi, yi] = pts[i];
      const [xj, yj] = pts[j];
      if (yi > p[1] !== yj > p[1] && p[0] < ((xj - xi) * (p[1] - yi)) / (yj - yi) + xi) inside = !inside;
    }
  }
  return inside;
}

function segIntersectsSeg(a: Vec2, b: Vec2, c: Vec2, d: Vec2): boolean {
  const o = (p: Vec2, q: Vec2, r: Vec2) => (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
  const d1 = o(c, d, a);
  const d2 = o(c, d, b);
  const d3 = o(a, b, c);
  const d4 = o(a, b, d);
  return ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0));
}

const inBox = (p: Vec2, b: Box) => p[0] >= b.x0 && p[0] <= b.x1 && p[1] >= b.y0 && p[1] <= b.y1;

/** Whether any line of the polylines lies in or crosses `box` (a crossing selection box). */
export function polylinesTouchBox(polys: Polyline[], box: Box): boolean {
  const edges: [Vec2, Vec2][] = [
    [[box.x0, box.y0], [box.x1, box.y0]],
    [[box.x1, box.y0], [box.x1, box.y1]],
    [[box.x1, box.y1], [box.x0, box.y1]],
    [[box.x0, box.y1], [box.x0, box.y0]],
  ];
  for (const pl of polys) {
    const n = pl.pts.length;
    for (let i = 0; i < n; i++) {
      const a = pl.pts[i];
      if (inBox(a, box)) return true;
      const b = i + 1 < n ? pl.pts[i + 1] : pl.closed ? pl.pts[0] : null;
      if (b && edges.some(([c, d]) => segIntersectsSeg(a, b, c, d))) return true;
    }
  }
  return false;
}
