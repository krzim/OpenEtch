// Unit tests for guides.ts: stored guides, labels, hit testing and the snap part the snapping module uses.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  UNSAVED_SCOPE,
  carryScopedGuides,
  guideAt,
  guideLabel,
  guideScope,
  guideSnapPart,
  guidesKey,
  loadScopedGuides,
  newGuideId,
  parseGuides,
  saveScopedGuides,
  scopedGuidesKey,
  type Guide,
  type GuideStore,
} from "./guides.ts";
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

/** An in-memory `Storage`. */
function memStore(init: Record<string, string> = {}): GuideStore & { map: Map<string, string> } {
  const map = new Map(Object.entries(init));
  return {
    map,
    getItem: (k) => map.get(k) ?? null,
    setItem: (k, v) => void map.set(k, v),
    removeItem: (k) => void map.delete(k),
  };
}

test("guides belong to the project, not the document name", () => {
  assert.equal(guideScope("Coaster", null), "project:coaster");
  assert.equal(guideScope("coaster", "Old file"), "project:coaster", "names compare ignoring case, like project files");
  assert.equal(guideScope(null, "badge"), "file:badge");
  assert.equal(guideScope(null, null), UNSAVED_SCOPE);
  assert.equal(guideScope("  ", ""), UNSAVED_SCOPE);

  const store = memStore();
  saveScopedGuides(store, guideScope("A", null), guides);
  assert.deepEqual(loadScopedGuides(store, guideScope("B", null), "Untitled"), [], "project B does not see A's guides");
  assert.deepEqual(loadScopedGuides(store, guideScope("a", null), "Untitled"), guides);
  saveScopedGuides(store, guideScope("A", null), []);
  assert.equal(store.map.size, 0, "no guides: nothing stored");
});

test("older guides kept under the document name move into the first project that loads with that name", () => {
  const store = memStore({ [guidesKey("Untitled")]: JSON.stringify(guides) });
  assert.deepEqual(loadScopedGuides(store, UNSAVED_SCOPE, "Untitled"), guides);
  assert.equal(store.getItem(guidesKey("Untitled")), null, "the old key is gone");
  assert.deepEqual(loadScopedGuides(store, guideScope("Other", null), "Untitled"), [], "adopted once only");
  // A scope with an entry of its own is never refilled from an old key.
  const s2 = memStore({ [guidesKey("Coaster")]: JSON.stringify(guides), [scopedGuidesKey("project:coaster")]: "[]" });
  assert.deepEqual(loadScopedGuides(s2, "project:coaster", "Coaster"), []);
});

test("saving carries the guides to the project the document is saved as", () => {
  const store = memStore();
  saveScopedGuides(store, UNSAVED_SCOPE, guides);
  carryScopedGuides(store, UNSAVED_SCOPE, guideScope("Coaster", null));
  assert.deepEqual(loadScopedGuides(store, guideScope("Coaster", null), null), guides);
  assert.equal(store.getItem(scopedGuidesKey(UNSAVED_SCOPE)), null, "the unsaved slot is emptied");
  // Save as another project: the copy gets them, the original keeps its own.
  carryScopedGuides(store, guideScope("Coaster", null), guideScope("Coaster 2", null));
  assert.deepEqual(loadScopedGuides(store, guideScope("Coaster 2", null), null), guides);
  assert.deepEqual(loadScopedGuides(store, guideScope("Coaster", null), null), guides);
  // Replacing a project with a document that has no guides drops the replaced project's guides.
  carryScopedGuides(store, UNSAVED_SCOPE, guideScope("Coaster", null));
  assert.deepEqual(loadScopedGuides(store, guideScope("Coaster", null), null), []);
  carryScopedGuides(store, "file:x", "file:x");
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
