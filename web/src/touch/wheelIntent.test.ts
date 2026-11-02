// Unit tests for touch/wheelIntent.ts: a wheel stream latches pan or zoom and never flips halfway (audit T6).

import assert from "node:assert/strict";
import { test } from "node:test";
import { looksLikeMouseWheel, newWheelLatch, wheelIntent, wheelPanDelta, type WheelLike } from "./wheelIntent.ts";

const ev = (o: Partial<WheelLike>): WheelLike => ({ deltaX: 0, deltaY: 0, deltaMode: 0, ctrlKey: false, shiftKey: false, timeStamp: 0, ...o });

test("a two-finger swipe pans from start to end, even when it turns purely vertical (Mac default)", () => {
  const l = newWheelLatch();
  const seq = [ev({ deltaY: 3.5, timeStamp: 0 }), ev({ deltaY: 6, deltaX: 1, timeStamp: 16 }), ev({ deltaY: 4, timeStamp: 32 }), ev({ deltaY: 2, timeStamp: 48 })];
  assert.deepEqual(
    seq.map((e) => wheelIntent(l, e, "auto", true)),
    ["pan", "pan", "pan", "pan"],
  );
});

test("the latch never flips mid-stream, whatever the deltas do (pref off: the old rule decides once)", () => {
  const l = newWheelLatch();
  // Starts purely vertical (zoom under the old rule), then gains sideways deltas: stays zoom.
  const a = [0, 10, 20, 30, 40].map((t, i) => wheelIntent(l, ev({ deltaY: 4, deltaX: i > 1 ? 3 : 0, timeStamp: t }), "off", false));
  assert.deepEqual(a, ["zoom", "zoom", "zoom", "zoom", "zoom"]);
  // Starts sideways (pan), then turns vertical: stays pan.
  const l2 = newWheelLatch();
  const b = [0, 10, 20, 30].map((t, i) => wheelIntent(l2, ev({ deltaY: 4, deltaX: i === 0 ? 2 : 0, timeStamp: t }), "off", false));
  assert.deepEqual(b, ["pan", "pan", "pan", "pan"]);
  // A long stream with random-looking deltas keeps its first decision.
  const l3 = newWheelLatch();
  const first = wheelIntent(l3, ev({ deltaY: 2.5, timeStamp: 0 }), "on", false);
  for (let i = 1; i < 60; i++) {
    const e = ev({ deltaY: (i % 7) - 3, deltaX: i % 3 === 0 ? 0 : 1.5, timeStamp: i * 20, wheelDeltaY: i % 5 === 0 ? -120 : undefined });
    assert.equal(wheelIntent(l3, e, "on", false), first, `event ${i}`);
  }
});

test("a pause of 150 ms starts a new stream", () => {
  const l = newWheelLatch();
  assert.equal(wheelIntent(l, ev({ deltaY: 4, deltaX: 2, timeStamp: 0 }), "off", false), "pan");
  assert.equal(wheelIntent(l, ev({ deltaY: 4, timeStamp: 100 }), "off", false), "pan", "still the same stream");
  assert.equal(wheelIntent(l, ev({ deltaY: 4, timeStamp: 260 }), "off", false), "zoom", "160 ms later: a new decision");
});

test("Ctrl+wheel (a pinch) zooms, and does not re-latch a pan stream", () => {
  const l = newWheelLatch();
  assert.equal(wheelIntent(l, ev({ deltaY: 2.5, timeStamp: 0 }), "on", false), "pan");
  assert.equal(wheelIntent(l, ev({ deltaY: 1, ctrlKey: true, timeStamp: 10 }), "on", false), "zoom");
  assert.equal(wheelIntent(l, ev({ deltaY: 2, timeStamp: 20 }), "on", false), "pan");
});

test("a mouse wheel keeps zooming (LightBurn), also with trackpad panning on", () => {
  const mouse = ev({ deltaY: 100, wheelDeltaY: -120 });
  assert.equal(looksLikeMouseWheel(mouse), true);
  assert.equal(looksLikeMouseWheel(ev({ deltaY: 4, wheelDeltaY: -12 })), false, "trackpad on Chrome/Safari");
  assert.equal(looksLikeMouseWheel(ev({ deltaY: 3, deltaMode: 1 })), true, "Firefox lines");
  assert.equal(wheelIntent(newWheelLatch(), mouse, "on", true), "zoom");
  assert.equal(wheelIntent(newWheelLatch(), ev({ deltaY: 100 }), "auto", false), "zoom", "Windows wheel, no trackpad seen");
});

test("Auto turns trackpad panning on once a trackpad has been seen (not only on a Mac)", () => {
  const l = newWheelLatch();
  assert.equal(wheelIntent(l, ev({ deltaY: 4, timeStamp: 0 }), "auto", false), "zoom", "nothing seen yet");
  wheelIntent(l, ev({ deltaY: 1.25, deltaX: 0.5, timeStamp: 500 }), "auto", false);
  assert.equal(l.trackpadSeen, true);
  assert.equal(wheelIntent(l, ev({ deltaY: 4, timeStamp: 1000 }), "auto", false), "pan");
  assert.equal(wheelIntent(newWheelLatch(), ev({ deltaY: 4.5 }), "off", true), "zoom", "Off keeps the old rule");
});

test("Shift+wheel pans sideways", () => {
  const e = ev({ deltaY: 100, shiftKey: true, wheelDeltaY: -120 });
  assert.equal(wheelIntent(newWheelLatch(), e, "off", false), "pan");
  assert.deepEqual(wheelPanDelta(e), [100, 0]);
  assert.deepEqual(wheelPanDelta(ev({ deltaX: 2, deltaY: 3 })), [2, 3]);
  assert.deepEqual(wheelPanDelta(ev({ deltaY: 3, deltaMode: 1 })), [0, 120]);
});
