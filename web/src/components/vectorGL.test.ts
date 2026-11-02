// Unit tests for vectorGL.ts against a recording stand-in for WebGL2: a lost context is reported (the layer then draws
// on the 2D canvas, so the design never disappears), a restored one gets the scene and flags again and is reported,
// and a disposed renderer frees its context once the canvas has left the page, never while it is still there.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { GDocument } from "../api/types.ts";
import { buildVectorScene, segmentFlags } from "../vectorScene.ts";
import { VectorGL } from "./vectorGL.ts";

/** A WebGL2 stand-in: every method records its call and returns a token; status queries succeed. */
function fakeGL() {
  const calls: { name: string; args: unknown[] }[] = [];
  const state = { lost: false, released: 0 };
  const gl = new Proxy({} as Record<string, unknown>, {
    get(_t, name: string) {
      if (name === "isContextLost") return () => state.lost;
      if (name === "getShaderParameter" || name === "getProgramParameter") return () => true;
      if (name === "getAttribLocation") return (_p: unknown, n: string) => ["a_seg", "a_meta", "a_flags"].indexOf(n);
      if (name === "getExtension")
        return (n: string) => (n === "WEBGL_lose_context" ? { loseContext: () => state.released++ } : null);
      if (/^[A-Z_0-9]+$/.test(name)) return name;
      return (...args: unknown[]) => {
        calls.push({ name, args });
        return { token: name };
      };
    },
  });
  return { gl, calls, state };
}

function fakeCanvas(gl: unknown) {
  const handlers = new Map<string, (e: Event) => void>();
  const canvas = {
    width: 0,
    height: 0,
    isConnected: true,
    getContext: () => gl,
    addEventListener: (t: string, h: (e: Event) => void) => handlers.set(t, h),
    removeEventListener: (t: string) => handlers.delete(t),
  };
  const fire = (t: string) => handlers.get(t)?.({ preventDefault() {} } as Event);
  return { canvas: canvas as unknown as HTMLCanvasElement & { isConnected: boolean }, fire, handlers };
}

const doc = {
  revision: 1,
  name: "t",
  stock: null,
  layers: [{ id: "L", name: "L", color: "#000", visible: true, output: true, settings: { mode: "line" }, sub_layers: [] }],
  objects: [{ id: "r", name: "r", layer: "L", geometry: { kind: "rect", width_mm: 10, height_mm: 10 }, transform: [1, 0, 0, 1, 0, 0] }],
  groups: [],
} as unknown as GDocument;

const frame = { groups: [{ stroke: [0, 0, 0, 1] as [number, number, number, number], fill: null, movingFill: null }], live: null, objWidth: 1.3, sel: null, outside: null };
const tick = () => new Promise((r) => setTimeout(r, 5));

test("a lost context is reported; on restore the scene and flags are uploaded again, reported, and drawn", () => {
  const { gl, calls } = fakeGL();
  const { canvas, fire } = fakeCanvas(gl);
  const seen: boolean[] = [];
  const g = new VectorGL(canvas, (lost) => seen.push(lost));
  const scene = buildVectorScene(doc);
  g.setScene(scene);
  g.setFlags(segmentFlags(scene, new Set(), new Set()));
  fire("webglcontextlost");
  assert.deepEqual(seen, [true]);
  assert.equal(g.lost, true);
  calls.length = 0;
  fire("webglcontextrestored");
  assert.deepEqual(seen, [true, false]);
  assert.equal(g.lost, false);
  const uploads = calls.filter((c) => c.name === "bufferData").map((c) => c.args[1]);
  assert.ok(uploads.includes(scene.segs), "the segments are uploaded again");
  assert.ok(uploads.some((u) => u instanceof Uint8Array && u.length === scene.count), "and the flags");
  calls.length = 0;
  g.draw({ cx: 0, cy: 0, scale: 4 }, [100, 100], 1, frame);
  assert.ok(calls.some((c) => c.name === "drawArraysInstanced"), "draws after the restore");
  g.dispose();
});

test("dispose frees the context once the canvas has left the page, never while it is still there", async () => {
  {
    const { gl, state } = fakeGL();
    const { canvas, handlers } = fakeCanvas(gl);
    const seen: boolean[] = [];
    new VectorGL(canvas, (l) => seen.push(l)).dispose();
    assert.equal(handlers.size, 0, "listeners removed");
    await tick();
    assert.equal(state.released, 0, "an effect run again on the same canvas keeps its context");
  }
  {
    const { gl, state } = fakeGL();
    const { canvas } = fakeCanvas(gl);
    new VectorGL(canvas).dispose();
    canvas.isConnected = false;
    await tick();
    assert.equal(state.released, 1);
  }
});
