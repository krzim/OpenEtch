// Node benchmark of the canvas's CPU-side work on the heavy six-coin document (heavyDoc.ts): what the old SVG layer
// computed per document and per drag move, against the raster scene (vectorScene.ts) and the geometric hit test.
// Run: npm run bench. The paint cost itself needs a browser: bench/vectors.html (src/bench/vectorsBrowser.ts).

import { geometryCmds, toPathData } from "../geometry.ts";
import { buildShapeIndex, hitTest } from "../selection.ts";
import { buildVectorScene, segmentFlags } from "../vectorScene.ts";
import { heavyDoc } from "./heavyDoc.ts";

function time(label: string, runs: number, f: () => unknown): number {
  f(); // warm up
  const ts: number[] = [];
  for (let i = 0; i < runs; i++) {
    const t = performance.now();
    f();
    ts.push(performance.now() - t);
  }
  ts.sort((a, b) => a - b);
  const med = ts[Math.floor(ts.length / 2)];
  console.log(`${label.padEnd(58)} ${med.toFixed(2).padStart(8)} ms (median of ${runs})`);
  return med;
}

const doc = heavyDoc();
const sel = new Set(doc.objects.filter((_, i) => i % 7 === 0).map((o) => o.id));
console.log(`heavy document: ${doc.objects.length} objects`);

// Old SVG layer: path data per object once per document, and in the filled view the per-layer fill strings again on
// every drag move (they left out the moving objects), before React reconciled thousands of <path> elements.
const paths = new Map<string, string>();
time("old: SVG path data, per document change", 5, () => {
  paths.clear();
  for (const o of doc.objects) paths.set(o.id, toPathData(geometryCmds(o.geometry), o.transform));
});
time("old: filled-view fill strings, per drag move", 10, () =>
  doc.layers.map((l) =>
    doc.objects
      .filter((o) => o.layer === l.id && !sel.has(o.id))
      .map((o) => paths.get(o.id) ?? "")
      .join(" "),
  ),
);

// New: segments once per document (cold: new geometry; warm: a refetch that keeps the objects), flags per selection.
// Cold: every object new or changed (a different document each run).
const fresh = Array.from({ length: 6 }, (_, i) => heavyDoc({ seed: 100 + i }));
let n = 0;
time("new: vector scene, per document change (all objects new)", 5, () => buildVectorScene(fresh[n++ % fresh.length]));
// A refetch after an edit: the same objects as new values (the UI refetches the whole document), one of them moved.
const refetch = () => {
  const d = structuredClone(doc);
  d.revision++;
  d.objects[0].transform = [1, 0, 0, 1, 5, 5];
  return d;
};
buildVectorScene(doc);
const refetched = Array.from({ length: 6 }, refetch);
n = 0;
time("new: vector scene, per edit (refetched document)", 5, () => buildVectorScene(refetched[n++ % refetched.length]));
const scene = buildVectorScene(doc);
console.log(`scene: ${scene.count} segments in ${scene.groups.length} layer groups`);
time("new: segment flags, per selection change", 20, () => segmentFlags(scene, sel, new Set()));
console.log("new: per view change and per drag move: no CPU rebuild (view and live transform are GPU uniforms)");

// Hit testing is geometric (as before): index once per document, then a click.
n = 0;
time("hit-test index, per document change (all objects new)", 5, () => buildShapeIndex(fresh[n++ % fresh.length]));
buildShapeIndex(doc);
n = 0;
time("hit-test index, per edit (refetched document)", 5, () => buildShapeIndex(refetched[n++ % refetched.length]));
const index = buildShapeIndex(doc);
let k = 0;
time("hit test, per click (mouse tolerance at 10 px/mm)", 200, () => hitTest(index, [-44 + (k++ % 30) - 15, 22], 5 / 10));
