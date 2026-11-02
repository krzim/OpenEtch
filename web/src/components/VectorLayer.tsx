// The document's outlines and fills as a raster layer repainted every frame from view-independent caches, instead of
// SVG that the browser re-rasterizes after every zoom step (blurry low-resolution tiles first, then the sharp redraw).
// WebGL2 (vectorGL.ts) with the scene's segments uploaded once per document; without WebGL2, a 2D canvas strokes and
// fills cached Path2Ds per object under the view matrix. Both repaint in the commit of a view or drag update, so they
// stay in step with the interaction SVG, at most once per animation frame (the Viewport coalesces view updates). The
// marching ants of the selected outline advance from their own animation frame loop, half a pixel at a time, and only
// redraw when the phase moved and no view update is about to draw in the same frame. Never takes pointer events: hit-testing is geometric (selection.ts hitTest).

import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Affine, GDocument } from "../api/types.ts";
import { geometryCmds, mul, toPathData } from "../geometry.ts";
import type { View } from "../sceneLayers.ts";
import { viewMatrix } from "../sceneLayers.ts";
import { TOKEN_VARS, VECTOR_LOOK, antsPhase, groupLook, type CanvasTokens, type GroupLook, type VectorScene } from "../vectorScene.ts";
import type { Theme } from "../workspace/theme.ts";
import { CanvasLayer, rgbaOf, type PaintArgs } from "./SceneCanvas.tsx";
import { VectorGL, type RGBA, type VectorFrame } from "./vectorGL.ts";

/** Token values used before styles.css is read (light theme). */
const TOKEN_FALLBACK: CanvasTokens = {
  field: "#ffffff",
  fieldEdge: "#9aa0a8",
  gridMinor: "#eceef1",
  gridMajor: "#d5d8dd",
  axisX: "rgba(211, 47, 47, 0.45)",
  axisY: "rgba(46, 157, 74, 0.5)",
  stockFill: "rgba(170, 176, 186, 0.18)",
  stockStroke: "#6b7280",
  accent: "#0a73ad",
  accentFill: "rgba(10, 115, 173, 0.08)",
  outside: "rgba(211, 47, 47, 0.55)",
};

function readTokens(): CanvasTokens {
  if (typeof document === "undefined") return TOKEN_FALLBACK;
  const cs = getComputedStyle(document.documentElement);
  const out = { ...TOKEN_FALLBACK };
  for (const k of Object.keys(TOKEN_VARS) as (keyof CanvasTokens)[]) out[k] = cs.getPropertyValue(TOKEN_VARS[k]).trim() || TOKEN_FALLBACK[k];
  return out;
}

/** The canvas colours of the current theme (styles.css tokens), re-read when <html data-theme> changes. */
export function useCanvasTokens(): CanvasTokens {
  const [tokens, setTokens] = useState(readTokens);
  useLayoutEffect(() => {
    const update = () =>
      setTokens((prev) => {
        const next = readTokens();
        return (Object.keys(next) as (keyof CanvasTokens)[]).every((k) => next[k] === prev[k]) ? prev : next;
      });
    update();
    if (typeof MutationObserver !== "function") return;
    const mo = new MutationObserver(update);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => mo.disconnect();
  }, []);
  return tokens;
}

function prefersReducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

// --- 2D canvas fallback ---------------------------------------------------------------------------------------------

/** Path2D per scene object (field mm, exact curves), and which ones are never filled (image frames). */
export interface Scene2D {
  scene: VectorScene;
  paths: Path2D[];
  noFill: boolean[];
}

export function buildScene2D(doc: GDocument | null, scene: VectorScene): Scene2D {
  const byId = new Map((doc?.objects ?? []).map((o) => [o.id, o]));
  const paths: Path2D[] = [];
  const noFill: boolean[] = [];
  for (const so of scene.objects) {
    const o = byId.get(so.id);
    paths.push(new Path2D(o ? toPathData(geometryCmds(o.geometry), o.transform) : ""));
    noFill.push(o?.geometry.kind === "image");
  }
  return { scene, paths, noFill };
}

/** Per group, the objects' paths joined: static and moving strokes and fills; the selected and outside outlines. */
interface Combined2D {
  stroke: [Path2D | null, Path2D | null][];
  fill: [Path2D | null, Path2D | null][];
  sel: Path2D | null;
  outStatic: Path2D | null;
  outMoving: Path2D | null;
}

export function combine2D(s: Scene2D, selected: ReadonlySet<string>, outside: ReadonlySet<string>, live: boolean): Combined2D {
  const add = (p: Path2D | null, q: Path2D) => {
    const r = p ?? new Path2D();
    r.addPath(q);
    return r;
  };
  const stroke = s.scene.groups.map(() => [null, null] as [Path2D | null, Path2D | null]);
  const fill = s.scene.groups.map(() => [null, null] as [Path2D | null, Path2D | null]);
  let sel: Path2D | null = null;
  let outStatic: Path2D | null = null;
  let outMoving: Path2D | null = null;
  s.scene.objects.forEach((o, i) => {
    const p = s.paths[i];
    const isSel = selected.has(o.id);
    const k = live && isSel ? 1 : 0;
    stroke[o.group][k] = add(stroke[o.group][k], p);
    if (!s.noFill[i]) fill[o.group][k] = add(fill[o.group][k], p);
    if (isSel) sel = add(sel, p);
    if (outside.has(o.id)) {
      if (k) outMoving = add(outMoving, p);
      else outStatic = add(outStatic, p);
    }
  });
  return { stroke, fill, sel, outStatic, outMoving };
}

export interface Frame2D {
  looks: GroupLook[];
  live: Affine | null;
  accent: string;
  outside: string;
  phase: number;
}

/** The 2D canvas version of VectorGL.draw: the same passes, line widths divided by the zoom. */
export function paintVectors2D(ctx: CanvasRenderingContext2D, a: PaintArgs, c: Combined2D, f: Frame2D): void {
  const vm = viewMatrix(a.view, a.size, a.dpr);
  const mm = f.live ? mul(vm, f.live) : vm;
  const kLive = f.live ? Math.sqrt(Math.abs(f.live[0] * f.live[3] - f.live[1] * f.live[2])) || 1 : 1;
  const px = 1 / (a.view.scale * a.dpr);
  const use = (moving: boolean) => {
    const m = moving ? mm : vm;
    ctx.setTransform(m[0], m[1], m[2], m[3], m[4], m[5]);
    return px * a.dpr * (moving ? 1 / kLive : 1);
  };
  f.looks.forEach((l, g) => {
    if (!l.fill) return;
    for (const k of [0, 1] as const) {
      const p = c.fill[g][k];
      const alpha = k ? l.movingFillAlpha : l.fillAlpha;
      if (!p || alpha <= 0 || (k && !f.live)) continue;
      use(k === 1);
      ctx.globalAlpha = alpha;
      ctx.fillStyle = l.fill;
      ctx.fill(p, "evenodd");
    }
  });
  ctx.setLineDash([]);
  f.looks.forEach((l, g) => {
    for (const k of [0, 1] as const) {
      const p = c.stroke[g][k];
      if (!p || l.strokeAlpha <= 0) continue;
      const u = use(k === 1);
      ctx.globalAlpha = l.strokeAlpha;
      ctx.strokeStyle = l.stroke;
      ctx.lineWidth = VECTOR_LOOK.objWidth * u;
      ctx.stroke(p);
    }
  });
  ctx.globalAlpha = 1;
  if (c.sel) {
    const u = use(!!f.live);
    ctx.strokeStyle = f.accent;
    ctx.lineWidth = VECTOR_LOOK.selWidth * u;
    ctx.setLineDash(VECTOR_LOOK.selDash.map((d) => d * u));
    ctx.lineDashOffset = -f.phase * u;
    ctx.stroke(c.sel);
    ctx.setLineDash([]);
    ctx.lineDashOffset = 0;
  }
  for (const [p, moving] of [
    [c.outStatic, false],
    [c.outMoving, true],
  ] as const) {
    if (!p) continue;
    const u = use(moving);
    ctx.strokeStyle = f.outside;
    ctx.lineWidth = VECTOR_LOOK.outsideWidth * u;
    ctx.stroke(p);
  }
  ctx.setTransform(1, 0, 0, 1, 0, 0);
}

// --- the layer ------------------------------------------------------------------------------------------------------

export interface VectorLayerProps {
  view: View;
  size: [number, number];
  dpr: number;
  doc: GDocument | null;
  scene: VectorScene;
  /** Per-segment flags (vectorScene.ts segmentFlags) for `selected` and `outside`. */
  flags: Uint8Array;
  selected: ReadonlySet<string>;
  outside: ReadonlySet<string>;
  /** The transient transform of a drag (selected objects), or null. */
  live: Affine | null;
  theme: Theme;
  filled: boolean;
  /** The toolpath preview is shown: everything but the highlights at 30 %. */
  dimmed: boolean;
  tokens: CanvasTokens;
  /** A view update is waiting for this frame's commit (which draws anyway): the ants then skip their own draw. */
  viewPending?: () => boolean;
}

/** The document's vectors: WebGL2, or the 2D canvas without it. */
export const VectorLayer = memo(function VectorLayer(p: VectorLayerProps) {
  const { view, size, dpr, scene, flags, live, theme, filled, dimmed, tokens } = p;
  const ref = useRef<HTMLCanvasElement>(null);
  const gl = useRef<VectorGL | null>(null);
  const [fallback, setFallback] = useState(false);
  useLayoutEffect(() => {
    if (fallback || !ref.current) return;
    try {
      gl.current = new VectorGL(ref.current);
    } catch {
      setFallback(true);
      return;
    }
    return () => {
      gl.current?.dispose();
      gl.current = null;
    };
  }, [fallback]);
  useLayoutEffect(() => gl.current?.setScene(scene), [scene, fallback]);
  useLayoutEffect(() => gl.current?.setFlags(flags), [flags, scene, fallback]);

  const looks = useMemo(() => scene.groups.map((g) => groupLook(g, theme, filled, dimmed)), [scene, theme, filled, dimmed]);
  const anySelected = p.selected.size > 0;
  const frame = useMemo(() => {
    const groups = looks.map((l) => ({
      stroke: rgbaOf(l.stroke, l.strokeAlpha) as RGBA,
      fill: l.fill ? (rgbaOf(l.fill, l.fillAlpha) as RGBA) : null,
      movingFill: l.fill ? (rgbaOf(l.fill, l.movingFillAlpha) as RGBA) : null,
    }));
    return {
      groups,
      live,
      objWidth: VECTOR_LOOK.objWidth,
      sel: anySelected ? { rgba: rgbaOf(tokens.accent, 1) as RGBA, width: VECTOR_LOOK.selWidth, dash: VECTOR_LOOK.selDash, phase: 0 } : null,
      outside: p.outside.size > 0 ? { rgba: rgbaOf(tokens.outside, 1) as RGBA, width: VECTOR_LOOK.outsideWidth } : null,
    } satisfies VectorFrame;
  }, [looks, live, anySelected, p.outside, tokens]);

  const reduced = useRef(prefersReducedMotion());
  // One draw with the current ants phase; the ants loop calls the newest one.
  const lastPhase = useRef(-1);
  const drawGL = useRef<() => void>(() => {});
  drawGL.current = () => {
    const g = gl.current;
    if (!g) return;
    const phase = frame.sel && !reduced.current ? antsPhase(performance.now()) : 0;
    lastPhase.current = phase;
    g.draw(view, size, dpr, frame.sel ? { ...frame, sel: { ...frame.sel, phase } } : frame);
  };
  const viewPending = useRef(p.viewPending);
  viewPending.current = p.viewPending;
  useLayoutEffect(() => drawGL.current(), [view, size, dpr, frame, scene, flags, fallback]);

  // Marching ants: redraw when the phase moves (half-pixel steps); the 2D fallback keeps them still.
  useEffect(() => {
    if (!anySelected || fallback || reduced.current) return;
    let raf = 0;
    const tick = () => {
      if (antsPhase(performance.now()) !== lastPhase.current && !viewPending.current?.()) drawGL.current();
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [anySelected, fallback]);

  // 2D fallback: Path2Ds per object once per document, joined per selection.
  const s2d = useMemo(() => (fallback ? buildScene2D(p.doc, scene) : null), [fallback, p.doc, scene]);
  const combined = useMemo(() => (s2d ? combine2D(s2d, p.selected, p.outside, !!live) : null), [s2d, p.selected, p.outside, !!live]);
  const paint2d = useCallback(
    (ctx: CanvasRenderingContext2D, a: PaintArgs) => {
      if (combined) paintVectors2D(ctx, a, combined, { looks, live, accent: tokens.accent, outside: tokens.outside, phase: 0 });
    },
    [combined, looks, live, tokens],
  );
  const data = useMemo(() => ({ "vector-segments": scene.count, "vector-renderer": "2d" }), [scene]);
  if (fallback) return <CanvasLayer className="vp-objects" view={view} size={size} dpr={dpr} paint={paint2d} data={data} />;
  return (
    <canvas
      ref={ref}
      className="vp-layer vp-objects"
      style={{ width: size[0], height: size[1] }}
      data-vector-segments={scene.count}
      data-vector-renderer="webgl2"
    />
  );
});
