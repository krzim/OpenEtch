// Unit tests for touch/gesture.ts, the gesture arbiter: one finger is the left mouse button, two fingers move the view
// (docs/ux/ui-plan-2026-11.md 1.2, audit T1/T2/T4/T15).

import assert from "node:assert/strict";
import { test } from "node:test";
import type { Vec2 } from "../api/types.ts";
import { GestureArbiter, type Effect } from "./gesture.ts";

const kinds = (fx: Effect[]) => fx.map((e) => e.kind);
const down = (a: GestureArbiter, id: number, p: Vec2, t: number, cls: "mouse" | "pen" | "touch" = "touch") => a.feed({ kind: "down", id, cls, p, t });
const move = (a: GestureArbiter, id: number, p: Vec2, t: number) => a.feed({ kind: "move", id, p, t });
const up = (a: GestureArbiter, id: number, p: Vec2, t: number) => a.feed({ kind: "up", id, p, t });

test("mouse: the press starts at once, drags and releases (unchanged)", () => {
  const a = new GestureArbiter();
  assert.deepEqual(down(a, 1, [10, 10], 0, "mouse"), [{ kind: "press", id: 1, cls: "mouse", p: [10, 10] }]);
  assert.deepEqual(kinds(move(a, 1, [11, 10], 5)), ["drag"], "even within the slop: the Viewport applies its own");
  assert.deepEqual(up(a, 1, [11, 10], 9), [{ kind: "release", id: 1, p: [11, 10] }]);
  assert.equal(a.state, "idle");
});

test("tap: nothing on down; the press and release come together on lift", () => {
  const a = new GestureArbiter();
  assert.deepEqual(down(a, 1, [50, 50], 0), []);
  assert.equal(a.state, "pending");
  assert.deepEqual(move(a, 1, [55, 53], 30), [], "within the 10 px slop");
  assert.deepEqual(up(a, 1, [55, 53], 80), [
    { kind: "press", id: 1, cls: "touch", p: [50, 50] },
    { kind: "release", id: 1, p: [55, 53] },
  ]);
  assert.equal(a.state, "idle");
});

test("drag past the slop: the press starts at the down point, then drags", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  assert.deepEqual(move(a, 1, [12, 0], 40), [
    { kind: "press", id: 1, cls: "touch", p: [0, 0] },
    { kind: "drag", id: 1, p: [12, 0] },
  ]);
  assert.deepEqual(kinds(move(a, 1, [30, 5], 60)), ["drag"]);
  assert.deepEqual(kinds(up(a, 1, [30, 5], 90)), ["release"]);
});

test("a second finger before the slop: no press at all, pan/pinch instead (no stray Line vertex)", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  const fx = down(a, 2, [100, 0], 60);
  assert.deepEqual(fx, [{ kind: "navStart", pts: [[0, 0], [100, 0]] }]);
  assert.ok(!fx.some((e) => e.kind === "press" || e.kind === "abort"));
  assert.deepEqual(kinds(move(a, 2, [150, 0], 80)), ["nav"]);
  assert.deepEqual(kinds(up(a, 2, [150, 0], 200)), ["navEnd"]);
  assert.deepEqual(up(a, 1, [0, 0], 220), [], "the last finger does nothing");
  assert.equal(a.state, "idle");
});

test("a second finger mid-drag cancels the drag (the touch Esc) and navigates", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  move(a, 1, [40, 0], 50);
  assert.deepEqual(kinds(down(a, 2, [100, 100], 400)), ["abort", "navStart"]);
  assert.deepEqual(kinds(move(a, 1, [45, 0], 420)), ["nav"]);
  assert.deepEqual(kinds(up(a, 1, [45, 0], 500)), ["navEnd"]);
  assert.deepEqual(up(a, 2, [100, 100], 520), [], "no release after the abort");
});

test("long-press at 500 ms, then the lift does nothing (the menu is open)", () => {
  const a = new GestureArbiter();
  down(a, 1, [20, 20], 1000);
  assert.equal(a.nextDeadline(), 1500);
  assert.deepEqual(a.feed({ kind: "tick", t: 1499 }), [], "not yet");
  assert.deepEqual(a.feed({ kind: "tick", t: 1500 }), [{ kind: "longPress", id: 1, p: [20, 20] }]);
  assert.equal(a.state, "held");
  assert.equal(a.nextDeadline(), null);
  assert.deepEqual(move(a, 1, [60, 20], 1600), [], "moving after the long-press does not drag");
  assert.deepEqual(up(a, 1, [60, 20], 1700), []);
  assert.equal(a.state, "idle");
});

test("a long-press nobody wants is promoted to a press (a held tap still places a Line point)", () => {
  const a = new GestureArbiter();
  down(a, 1, [5, 5], 0);
  a.feed({ kind: "tick", t: 600 });
  assert.deepEqual(a.feed({ kind: "promote", t: 600 }), [{ kind: "press", id: 1, cls: "touch", p: [5, 5], longPress: true }]);
  assert.deepEqual(kinds(up(a, 1, [5, 5], 700)), ["release"]);
});

test("moving over the slop before 500 ms cancels the long-press", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  move(a, 1, [0, 15], 300);
  assert.equal(a.state, "pressed");
  assert.equal(a.nextDeadline(), null);
  assert.deepEqual(a.feed({ kind: "tick", t: 600 }), [], "no long-press during a drag");
});

test("pointercancel while pending: nothing; while pressed: abort", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  assert.deepEqual(a.feed({ kind: "cancel", id: 1, t: 10 }), []);
  down(a, 2, [0, 0], 20);
  move(a, 2, [30, 0], 40);
  assert.deepEqual(a.feed({ kind: "cancel", id: 2, t: 50 }), [{ kind: "abort" }]);
  assert.equal(a.state, "idle");
});

test("pen palm rejection: touches are ignored while the pen is down, and only navigate for a second after", () => {
  const a = new GestureArbiter();
  assert.deepEqual(kinds(down(a, 9, [0, 0], 0, "pen")), ["press"], "the pen draws at once");
  assert.deepEqual(down(a, 1, [300, 300], 50), [], "a palm lands: ignored");
  assert.deepEqual(move(a, 1, [310, 320], 60), []);
  assert.deepEqual(kinds(move(a, 9, [20, 0], 70)), ["drag"], "the stroke goes on");
  assert.deepEqual(up(a, 1, [310, 320], 80), [], "the palm lifts: nothing");
  assert.deepEqual(kinds(up(a, 9, [20, 0], 100)), ["release"]);
  // 400 ms after the pen: one finger pans instead of selecting.
  assert.deepEqual(down(a, 2, [50, 50], 500), [{ kind: "navStart", pts: [[50, 50]] }]);
  assert.deepEqual(kinds(move(a, 2, [80, 50], 520)), ["nav"]);
  assert.deepEqual(kinds(up(a, 2, [80, 50], 540)), ["navEnd"]);
  // Two seconds later a finger is the left button again.
  assert.deepEqual(down(a, 3, [50, 50], 2200), []);
  assert.equal(a.state, "pending");
});

test("two- and three-finger taps (undo / redo); a moved or slow one is not a tap", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  down(a, 2, [50, 0], 20);
  assert.deepEqual(kinds(up(a, 1, [0, 0], 120)), ["navEnd"]);
  assert.deepEqual(up(a, 2, [50, 0], 140), [{ kind: "multiTap", fingers: 2 }]);

  down(a, 1, [0, 0], 1000);
  down(a, 2, [50, 0], 1010);
  down(a, 3, [100, 0], 1020);
  assert.deepEqual(kinds(up(a, 1, [0, 0], 1100)), ["navStart"], "two still down: keep pinching");
  up(a, 2, [50, 0], 1110);
  assert.deepEqual(up(a, 3, [100, 0], 1120), [{ kind: "multiTap", fingers: 3 }]);

  down(a, 1, [0, 0], 2000);
  down(a, 2, [50, 0], 2010);
  move(a, 2, [90, 0], 2050);
  up(a, 1, [0, 0], 2100);
  assert.deepEqual(up(a, 2, [90, 0], 2110), [], "a pinch is not a tap");

  down(a, 1, [0, 0], 3000);
  down(a, 2, [50, 0], 3010);
  up(a, 1, [0, 0], 3500);
  assert.deepEqual(up(a, 2, [50, 0], 3510), [], "too slow");
});

test("after a pinch, a new second finger pinches again", () => {
  const a = new GestureArbiter();
  down(a, 1, [0, 0], 0);
  down(a, 2, [50, 0], 10);
  move(a, 2, [80, 0], 30);
  up(a, 2, [80, 0], 60);
  assert.equal(a.state, "dead");
  assert.deepEqual(kinds(down(a, 3, [10, 10], 200)), ["navStart"]);
});
