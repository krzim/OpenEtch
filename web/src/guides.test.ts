// Unit tests for guides.ts: stored guides, labels, hit testing and the snap part the snapping module uses.

import assert from "node:assert/strict";
import { test } from "node:test";
import { guideAt, guideLabel, guideSnapPart, guidesKey, newGuideId, parseGuides, type Guide } from "./guides.ts";
import { buildSnapIndex, findSnap } from "./snapping.ts";

const guides: Guide[] = [
  { id: "g1", axis: "x", v: 10 },
  { id: "g2", axis: "y", v: -5 },
];

test("stored guides parse defensively", () => {
  assert.deepEqual(parseGuides(JSON.stringify(guides)), guides);
  assert.deepEqual(parseGuides("nonsense"), []);
  assert.deepEqual(parseGuides(JSON.stringify([{ axis: "z", v: 1 }, { axis: "x", v: "1" }, { axis: "y", v: 2 }])), [{ id: "g0", axis: "y", v: 2 }]);
  assert.equal(guidesKey(""), "openetch.guides.Untitled");
  assert.equal(guidesKey("Coaster"), "openetch.guides.Coaster");
});

test("guide ids, labels and hits", () => {
  assert.equal(newGuideId(guides), "g3");
  assert.equal(guideLabel(guides[0]), "Vertical guide X 10");
  assert.equal(guideAt(guides, [10.3, 40], 0.5)?.id, "g1");
  assert.equal(guideAt(guides, [3, -5.2], 0.5)?.id, "g2");
  assert.equal(guideAt(guides, [3, 3], 0.5), null);
});

test("guides are snap targets: their crossing and the nearest point on a guide", () => {
  assert.equal(guideSnapPart([], { x0: -100, y0: -100, x1: 100, y1: 100 }), null);
  const part = guideSnapPart(guides, { x0: -100, y0: -100, x1: 100, y1: 100 });
  assert.ok(part);
  const ix = buildSnapIndex([part]);
  const opts = { tol: 1, points: true, nearest: true, guides: true, grid: 0 };
  const cross = findSnap(ix, [10.4, -5.3], opts);
  assert.deepEqual(cross?.p, [10, -5]);
  assert.equal(cross?.kind, "intersection");
  const on = findSnap(ix, [10.5, 30], opts);
  assert.equal(on?.kind, "nearest");
  assert.ok(on && Math.abs(on.p[0] - 10) < 1e-9 && Math.abs(on.p[1] - 30) < 1e-9);
});
