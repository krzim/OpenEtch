// Point presses of the canvas tools (docs/ux/ui-plan-2026-11.md 1.2, audit T4/T16): Line, Measure, point picks and
// Trim act on the press with a mouse or pen (as LightBurn), but on lift with a finger: pressing shows where the point
// snaps, dragging adjusts it, lifting commits. Either way `cancelPress` rolls back what the press did, so a second
// finger (the start of a pinch) or a pointercancel never leaves a Line vertex, a measure point or a picked point
// behind. Pure: `apply` is the tool's own state update. `npm test` covers it (drawTools.test.ts).

import type { PointerClass } from "../pointer.ts";

export type PointTool = "line" | "measure" | "pick" | "trim";

/** When a point tool's press takes effect for a pointer class. */
export function commitOn(_tool: PointTool, cls: PointerClass): "down" | "up" {
  return cls === "touch" ? "up" : "down";
}

/** A point press in progress. `before` is the tool state before it, for the rollback. */
export interface PointPress<S, V> {
  mode: "down" | "up";
  value: V;
  before: S;
}

/** Start a press with `value` (the snapped point): applied now for "down", held for "up". */
export function beginPress<S, V>(state: S, value: V, mode: "down" | "up", apply: (s: S, v: V) => S): { state: S; press: PointPress<S, V> } {
  return { state: mode === "down" ? apply(state, value) : state, press: { mode, value, before: state } };
}

/** The pointer moved: a held press follows it (a press already applied keeps its point). */
export function movePress<S, V>(press: PointPress<S, V>, value: V): PointPress<S, V> {
  return press.mode === "up" ? { ...press, value } : press;
}

/** The pointer lifted: a held press is applied now. */
export function endPress<S, V>(state: S, press: PointPress<S, V>, apply: (s: S, v: V) => S): S {
  return press.mode === "up" ? apply(state, press.value) : state;
}

/** The press was cancelled: the state as it was before it. */
export function cancelPress<S, V>(_state: S, press: PointPress<S, V>): S {
  return press.before;
}
