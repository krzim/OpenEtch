// Browser benchmark of the canvas's vector layer (bench/vectors.html; `npm run dev`, then open /bench/vectors.html,
// ideally in a 2560 x 1440 window at 150 % scaling). It draws the heavy six-coin document (heavyDoc.ts) at 30 zoom
// steps three ways and reports the time per step, synchronised with the GPU (readPixels / getImageData):
// - "svg": the old SVG layer (one <path> per object with vector-effect: non-scaling-stroke, the old Viewport markup)
//   rasterized at the new zoom, as the browser must after every zoom step (measured as an SVG image drawn into a
//   canvas, which rasterizes it on the main thread; in the page the same work happens on the raster threads, and
//   until it is done Chrome shows the previous tiles scaled, the blur);
// - "canvas2d": the 2D canvas fallback (VectorLayer.tsx paintVectors2D, cached Path2Ds under the view matrix);
// - "webgl2": the GPU renderer (vectorGL.ts), segments uploaded once.
// Results go to window.__vectorBench and the page.

import { heavyDoc } from "./heavyDoc.ts";
import { geometryCmds, toPathData } from "../geometry.ts";
import type { View } from "../sceneLayers.ts";
import { buildVectorScene, groupLook, segmentFlags, VECTOR_LOOK } from "../vectorScene.ts";
import { buildScene2D, combine2D, paintVectors2D } from "../components/VectorLayer.tsx";
import { rgbaOf } from "../components/SceneCanvas.tsx";
import { VectorGL, type RGBA, type VectorFrame } from "../components/vectorGL.ts";
import { canvasColor } from "../workspace/theme.ts";

interface Stat {
  median: number;
  p95: number;
  max: number;
}

function stat(xs: number[]): Stat {
  const s = [...xs].sort((a, b) => a - b);
  const at = (q: number) => s[Math.min(s.length - 1, Math.floor(q * s.length))];
  const r = (v: number) => Math.round(v * 100) / 100;
  return { median: r(at(0.5)), p95: r(at(0.95)), max: r(s[s.length - 1]) };
}

const log = (msg: string) => {
  const pre = document.getElementById("out");
  if (pre) pre.textContent += `${msg}\n`;
};

async function run(): Promise<Record<string, unknown>> {
  const W = Number(new URLSearchParams(location.search).get("w") ?? 2560);
  const H = Number(new URLSearchParams(location.search).get("h") ?? 1440);
  const dpr = Number(new URLSearchParams(location.search).get("dpr") ?? (window.devicePixelRatio || 1));
  const steps = 30;
  const size: [number, number] = [W, H];
  const dw = Math.round(W * dpr);
  const dh = Math.round(H * dpr);
  const doc = heavyDoc();
  const points = doc.objects.reduce((n, o) => n + (o.geometry.kind === "path" ? (String((o.geometry as { path: string }).path).match(/[-\d.]+,[-\d.]+/g) ?? []).length : 0), 0);
  // Zoom steps from fitting the 150 mm design to 20x, about the design centre (a quick wheel zoom).
  const views: View[] = [];
  for (let i = 0; i < steps; i++) views.push({ cx: 3 + i * 0.4, cy: 1 - i * 0.2, scale: (W / 160) * Math.pow(20, i / (steps - 1)) });
  const theme = "light" as const;
  const selected = new Set(doc.objects.filter((_, i) => i % 7 === 0).map((o) => o.id));
  const outside = new Set<string>();
  const result: Record<string, unknown> = { size: [W, H], dpr, device: [dw, dh], objects: doc.objects.length, points };

  // --- old SVG layer -----------------------------------------------------------------------------------------------
  const layer = new Map(doc.layers.map((l) => [l.id, l]));
  let body = "";
  for (const o of doc.objects) {
    const l = layer.get(o.layer);
    const d = toPathData(geometryCmds(o.geometry), o.transform);
    const op = l && !l.output ? ' opacity="0.4"' : "";
    body += `<g${op}><path d="${d}" fill="none" stroke="${canvasColor(l?.color, theme)}" stroke-width="1.3" vector-effect="non-scaling-stroke"/>`;
    if (selected.has(o.id)) body += `<path d="${d}" fill="none" stroke="#0a73ad" stroke-width="1.6" stroke-dasharray="5 4" vector-effect="non-scaling-stroke"/>`;
    body += "</g>";
  }
  const svgFor = (v: View) =>
    `<svg xmlns="http://www.w3.org/2000/svg" width="${dw}" height="${dh}"><g transform="matrix(${v.scale * dpr},0,0,${-v.scale * dpr},${(W / 2 - v.scale * v.cx) * dpr},${(H / 2 + v.scale * v.cy) * dpr})">${body}</g></svg>`;
  const c2 = document.createElement("canvas");
  c2.width = dw;
  c2.height = dh;
  const ctx = c2.getContext("2d") as CanvasRenderingContext2D;
  const sync2d = () => ctx.getImageData(0, 0, 1, 1);
  {
    const times: number[] = [];
    for (let i = 0; i < Math.min(steps, 12); i++) {
      const url = URL.createObjectURL(new Blob([svgFor(views[i])], { type: "image/svg+xml" }));
      const img = new Image();
      img.src = url;
      await img.decode();
      ctx.clearRect(0, 0, dw, dh);
      sync2d();
      const t0 = performance.now();
      ctx.drawImage(img, 0, 0);
      sync2d();
      times.push(performance.now() - t0);
      URL.revokeObjectURL(url);
    }
    result.svg = stat(times);
    log(`svg (old layer, raster per zoom step): ${JSON.stringify(result.svg)} ms`);
  }

  // --- caches (once per document) ------------------------------------------------------------------------------------
  let t0 = performance.now();
  const scene = buildVectorScene(doc);
  const flags = segmentFlags(scene, selected, outside);
  result.sceneBuildMs = Math.round((performance.now() - t0) * 10) / 10;
  result.segments = scene.count;
  const looks = scene.groups.map((g) => groupLook(g, theme, false, false));

  // --- 2D canvas -------------------------------------------------------------------------------------------------
  {
    t0 = performance.now();
    const s2 = buildScene2D(doc, scene);
    const comb = combine2D(s2, selected, outside, false);
    result.path2dBuildMs = Math.round((performance.now() - t0) * 10) / 10;
    const times: number[] = [];
    for (const v of views) {
      const t = performance.now();
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, dw, dh);
      paintVectors2D(ctx, { view: v, size, dpr }, comb, { looks, live: null, accent: "#0a73ad", outside: "#d32f2f", phase: 0 });
      sync2d();
      times.push(performance.now() - t);
    }
    result.canvas2d = stat(times);
    log(`canvas2d (Path2D per frame): ${JSON.stringify(result.canvas2d)} ms`);
  }

  // --- WebGL2 --------------------------------------------------------------------------------------------------------
  {
    const c = document.createElement("canvas");
    document.body.appendChild(c);
    c.style.cssText = `width:${W}px;height:${H}px;position:absolute;left:0;top:0`;
    let g: VectorGL | null = null;
    try {
      g = new VectorGL(c);
    } catch {
      log("webgl2: not available");
    }
    if (g) {
      const gl = c.getContext("webgl2") as WebGL2RenderingContext;
      const dbg = gl.getExtension("WEBGL_debug_renderer_info");
      result.renderer = dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER);
      t0 = performance.now();
      g.setScene(scene);
      g.setFlags(flags);
      const px = new Uint8Array(4);
      gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
      result.uploadMs = Math.round((performance.now() - t0) * 10) / 10;
      const frameOf = (filled: boolean): VectorFrame => ({
        groups: scene.groups.map((gr) => {
          const l = groupLook(gr, theme, filled, false);
          return {
            stroke: rgbaOf(l.stroke, l.strokeAlpha) as RGBA,
            fill: l.fill ? (rgbaOf(l.fill, l.fillAlpha) as RGBA) : null,
            movingFill: l.fill ? (rgbaOf(l.fill, l.movingFillAlpha) as RGBA) : null,
          };
        }),
        live: null,
        objWidth: VECTOR_LOOK.objWidth,
        sel: { rgba: [0.04, 0.45, 0.68, 1], width: VECTOR_LOOK.selWidth, dash: VECTOR_LOOK.selDash, phase: 0 },
        outside: null,
      });
      for (const [name, f] of [
        ["webgl2", frameOf(false)],
        ["webgl2Filled", frameOf(true)],
        ["webgl2Drag", { ...frameOf(true), live: [1, 0, 0, 1, 3, 2] as VectorFrame["live"] }],
      ] as const) {
        // Warm up (shader compile, first allocation).
        g.draw(views[0], size, dpr, f);
        gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
        const times: number[] = [];
        for (const v of views) {
          const t = performance.now();
          g.draw(v, size, dpr, f);
          gl.readPixels(0, 0, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
          times.push(performance.now() - t);
        }
        result[name] = stat(times);
        log(`${name}: ${JSON.stringify(result[name])} ms`);
      }
      g.dispose();
    }
  }
  log(JSON.stringify(result));
  return result;
}

declare global {
  interface Window {
    __vectorBench?: Record<string, unknown> | { error: string };
  }
}

run().then(
  (r) => (window.__vectorBench = r),
  (e) => {
    window.__vectorBench = { error: String(e) };
    log(String(e));
  },
);
