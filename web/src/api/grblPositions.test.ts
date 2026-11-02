// Unit tests for api/grblPositions.ts: saved Move positions and the head's last known position.

import assert from "node:assert/strict";
import { test } from "node:test";
import { parseGrblProfile } from "./grbl.ts";
import * as P from "./grblPositions.ts";

const profile = parseGrblProfile({ work_area_mm: [400, 300], origin: "bottom_left", frame_feed_mm_min: 3000, max_feed_mm_min: 6000 })!;

test("saved positions are read defensively from GET /api/prefs", () => {
  const prefs = { device: "d", device_prefs: { move_positions: [{ name: "Jig", x_mm: 10, y_mm: 20 }, { name: "bad" }, null] } };
  assert.deepEqual(P.parsePositions(prefs), [{ name: "Jig", x_mm: 10, y_mm: 20 }]);
  assert.deepEqual(P.parsePositions({ device_prefs: {} }), []);
  assert.deepEqual(P.parsePositions(null), []);
});

test("saving checks names and the bed, and a name saved again moves that position", () => {
  let list = P.withPosition([], { name: " Jig ", x_mm: 10.12345, y_mm: 20 });
  assert.deepEqual(list, [{ name: "Jig", x_mm: 10.123, y_mm: 20 }]);
  list = P.withPosition(list, { name: "Park", x_mm: 400, y_mm: 300 });
  list = P.withPosition(list, { name: "jig", x_mm: 1, y_mm: 2 });
  assert.deepEqual(list.map((m) => [m.name, m.x_mm, m.y_mm]), [["jig", 1, 2], ["Park", 400, 300]]);
  assert.equal(P.positionsProblem(profile, list), null);
  assert.match(P.positionsProblem(profile, [{ name: "Off", x_mm: 401, y_mm: 0 }]) ?? "", /off the bed/);
  assert.match(P.positionsProblem(profile, [{ name: " ", x_mm: 1, y_mm: 1 }]) ?? "", /name/);
  const many = Array.from({ length: P.MAX_POSITIONS + 1 }, (_, i) => ({ name: `p${i}`, x_mm: 1, y_mm: 1 }));
  assert.match(P.positionsProblem(profile, many) ?? "", /At most/);
  assert.deepEqual(P.positionsPatch([{ name: " A ", x_mm: 1, y_mm: 2 }]), { move_positions: [{ name: "A", x_mm: 1, y_mm: 2 }] });
  assert.equal(P.nextPositionName([{ name: "Position 1", x_mm: 0, y_mm: 0 }]), "Position 2");
});

test("the head's last position is a store the canvas follows", () => {
  const seen: P.HeadState[] = [];
  const off = P.subscribeHead(() => seen.push(P.headState()));
  P.noteHeadPosition([-150, 100]);
  P.noteHeadPosition([-150, 100]);
  assert.equal(seen.length, 1, "an unchanged position does not repaint");
  assert.deepEqual(P.headState().field, [-150, 100]);
  P.setShowLastPosition(true);
  assert.equal(P.headState().show, true);
  P.noteHeadPosition(null);
  assert.equal(P.headState().field, null);
  P.setShowLastPosition(false);
  off();
  P.noteHeadPosition([1, 2]);
  assert.equal(seen.length, 4, "no calls after unsubscribing");
});
