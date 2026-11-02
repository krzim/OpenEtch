// A heavy synthetic document for the canvas benchmarks (vectorScene.bench.ts in Node, bench/vectors.html in a
// browser): six coins in a 3 x 2 grid, each a pair of rings plus hundreds of traced-looking closed blobs and a ring of
// curved "lettering", like a traced six-coin ghost design. About 3,700 paths and 200k points by default. Deterministic.

import type { CutSettings, DocObject, GDocument, Layer } from "../api/types.ts";

const cut = (mode: CutSettings["mode"]): CutSettings => ({
  mode,
  power_pct: 40,
  speed_mm_s: 1000,
  frequency_khz: 30,
  pulse_width_ns: 200,
  passes: 1,
  hatch: { interval_mm: 0.05, angle_deg: 0, angle_increment_deg: 0, cross_hatch: false, bidirectional: true },
  timing: null,
});

function rng(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const f = (v: number) => v.toFixed(3);

export interface HeavyOptions {
  /** Traced blobs per coin. */
  blobs?: number;
  /** Points per blob (average). */
  points?: number;
  /** Curved glyphs per coin. */
  glyphs?: number;
  /** Random seed (a different seed: a different document of the same size). */
  seed?: number;
}

/** The benchmark document. Defaults give ~3,700 paths and ~200k points (polyline points plus curve control points). */
export function heavyDoc(o: HeavyOptions = {}): GDocument {
  const blobs = o.blobs ?? 560;
  const points = o.points ?? 56;
  const glyphs = o.glyphs ?? 48;
  const r = rng(o.seed ?? 42);
  const layers: Layer[] = [
    { id: "L0", name: "Rings", color: "#0000ff", visible: true, output: true, settings: cut("line"), sub_layers: [] },
    { id: "L1", name: "Ghost", color: "#d22020", visible: true, output: true, settings: cut("fill"), sub_layers: [] },
    { id: "L2", name: "Lettering", color: "#2a9d4a", visible: true, output: false, settings: cut("fill"), sub_layers: [] },
  ];
  const objects: DocObject[] = [];
  let id = 0;
  const add = (layer: string, path: string, tx: number, ty: number) =>
    objects.push({ id: `o${id++}`, name: "", layer, geometry: { kind: "path", path } as DocObject["geometry"], transform: [1, 0, 0, 1, tx, ty] });
  const R = 19;
  for (let c = 0; c < 6; c++) {
    const cx = (c % 3) * 44 - 44;
    const cy = c < 3 ? 22 : -22;
    objects.push({ id: `o${id++}`, name: "", layer: "L0", geometry: { kind: "ellipse", rx_mm: R, ry_mm: R } as DocObject["geometry"], transform: [1, 0, 0, 1, cx, cy] });
    objects.push({ id: `o${id++}`, name: "", layer: "L0", geometry: { kind: "ellipse", rx_mm: R - 1.5, ry_mm: R - 1.5 } as DocObject["geometry"], transform: [1, 0, 0, 1, cx, cy] });
    for (let b = 0; b < blobs; b++) {
      // A wobbly closed contour somewhere inside the inner ring.
      const a = r() * Math.PI * 2;
      const d = Math.sqrt(r()) * (R - 4);
      const bx = Math.cos(a) * d;
      const by = Math.sin(a) * d;
      const size = 0.3 + r() * 1.6;
      const n = Math.max(6, Math.round(points * (0.5 + r())));
      const ph = r() * 6.28;
      let p = "";
      for (let i = 0; i < n; i++) {
        const t = (i / n) * Math.PI * 2;
        const rr = size * (1 + 0.25 * Math.sin(3 * t + ph) + 0.1 * Math.sin(7 * t));
        p += `${i ? "L" : "M"}${f(bx + Math.cos(t) * rr)},${f(by + Math.sin(t) * rr)}`;
      }
      add("L1", `${p}Z`, cx, cy);
    }
    for (let g = 0; g < glyphs; g++) {
      // Curved lettering around the rim: an "O"-like glyph of eight cubics with a counter.
      const a = (g / glyphs) * Math.PI * 2;
      const gx = Math.cos(a) * (R - 3);
      const gy = Math.sin(a) * (R - 3);
      const glyph = (rx: number, ry: number) => {
        const k = 0.5523;
        return (
          `M${f(gx + rx)},${f(gy)}` +
          `C${f(gx + rx)},${f(gy + k * ry)} ${f(gx + k * rx)},${f(gy + ry)} ${f(gx)},${f(gy + ry)}` +
          `C${f(gx - k * rx)},${f(gy + ry)} ${f(gx - rx)},${f(gy + k * ry)} ${f(gx - rx)},${f(gy)}` +
          `C${f(gx - rx)},${f(gy - k * ry)} ${f(gx - k * rx)},${f(gy - ry)} ${f(gx)},${f(gy - ry)}` +
          `C${f(gx + k * rx)},${f(gy - ry)} ${f(gx + rx)},${f(gy - k * ry)} ${f(gx + rx)},${f(gy)}Z`
        );
      };
      add("L2", glyph(0.5, 0.7) + glyph(0.25, 0.45), cx, cy);
    }
  }
  return { revision: 1, name: "heavy", stock: null, layers, objects, next_id: id } as GDocument;
}
