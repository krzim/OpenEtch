// Unit tests for touch/virtualMods.ts: the modifier bar's latch (one gesture) and lock (until tapped again), and how
// modsOf (pointer.ts) ORs them with the keyboard, including the Mac mapping.

import assert from "node:assert/strict";
import { test } from "node:test";
import { modsOf } from "../pointer.ts";
import { clearLatched, getVirtualMods, latch, lock, resetVirtualMods, subscribeVirtualMods } from "./virtualMods.ts";

const keys = (o: Partial<{ shiftKey: boolean; ctrlKey: boolean; metaKey: boolean; altKey: boolean }> = {}) => ({ shiftKey: false, ctrlKey: false, metaKey: false, altKey: false, ...o });

test("a latch lasts one gesture", () => {
  resetVirtualMods();
  latch("shift");
  assert.equal(getVirtualMods().shift, true);
  assert.deepEqual(getVirtualMods().latched, ["shift"]);
  clearLatched();
  assert.equal(getVirtualMods().shift, false, "the gesture ended");
  assert.deepEqual(getVirtualMods().latched, []);
});

test("a lock persists across gestures until tapped again", () => {
  resetVirtualMods();
  lock("ctrl");
  clearLatched();
  clearLatched();
  assert.equal(getVirtualMods().ctrl, true);
  assert.deepEqual(getVirtualMods().locked, ["ctrl"]);
  lock("ctrl");
  assert.equal(getVirtualMods().ctrl, false, "double-tap again unlocks");
  lock("alt");
  latch("alt");
  assert.equal(getVirtualMods().alt, false, "a single tap on a locked toggle also turns it off");
});

test("latch toggles, and latch then lock becomes a lock", () => {
  resetVirtualMods();
  latch("shift");
  latch("shift");
  assert.equal(getVirtualMods().shift, false, "tap again releases");
  latch("shift");
  lock("shift");
  assert.deepEqual(getVirtualMods().latched, []);
  assert.deepEqual(getVirtualMods().locked, ["shift"]);
  clearLatched();
  assert.equal(getVirtualMods().shift, true);
  resetVirtualMods();
});

test("modsOf ORs the virtual modifiers with the physical keys, with the Mac meta mapping", () => {
  resetVirtualMods();
  latch("ctrl");
  const v = getVirtualMods();
  assert.deepEqual(modsOf(keys(), v, false), { shift: false, ctrl: true, alt: false });
  assert.deepEqual(modsOf(keys({ shiftKey: true }), v, false), { shift: true, ctrl: true, alt: false });
  assert.deepEqual(modsOf(keys(), v, true), { shift: false, ctrl: true, alt: false }, "virtual Ctrl on a Mac");
  clearLatched();
  const w = getVirtualMods();
  assert.deepEqual(modsOf(keys({ metaKey: true }), w, true), { shift: false, ctrl: true, alt: false }, "Cmd");
  assert.deepEqual(modsOf(keys({ ctrlKey: true }), w, true), { shift: false, ctrl: false, alt: false }, "Ctrl is the Mac right-click");
});

test("subscribers hear changes, not no-ops", () => {
  resetVirtualMods();
  let n = 0;
  const off = subscribeVirtualMods(() => n++);
  clearLatched();
  assert.equal(n, 0, "nothing latched: no change");
  latch("alt");
  clearLatched();
  assert.equal(n, 2);
  off();
  latch("alt");
  assert.equal(n, 2);
  resetVirtualMods();
});
