// Unit tests for shortcut hints (formatChord.ts) on Windows and on a Mac.

import { test } from "node:test";
import assert from "node:assert/strict";
import { formatChord, platformOf } from "./formatChord.ts";

test("Windows and Linux keep the chord as written", () => {
  for (const c of ["Ctrl+S", "Ctrl+Shift+H", "Alt+Enter", "Del", "Ctrl+Shift++", "F1", "S"]) assert.equal(formatChord(c, "win"), c);
});

test("a Mac shows ⌥⇧⌘ in Apple's order and symbol keys", () => {
  assert.equal(formatChord("Ctrl+S", "mac"), "⌘S");
  assert.equal(formatChord("Ctrl+Shift+S", "mac"), "⇧⌘S");
  assert.equal(formatChord("Shift+Ctrl+H", "mac"), "⇧⌘H", "order is Apple's, not the chord's");
  assert.equal(formatChord("Alt+Shift+M", "mac"), "⌥⇧M");
  assert.equal(formatChord("Ctrl+Alt+F", "mac"), "⌥⌘F");
  assert.equal(formatChord("Alt+Enter", "mac"), "⌥↩");
  assert.equal(formatChord("Ctrl+.", "mac"), "⌘.");
  assert.equal(formatChord("Ctrl+Shift++", "mac"), "⇧⌘+");
  assert.equal(formatChord("Alt+Left", "mac"), "⌥←");
  assert.equal(formatChord("Ctrl+PgUp", "mac"), "⌘⇞");
  assert.equal(formatChord("Del", "mac"), "⌦");
  assert.equal(formatChord("Esc", "mac"), "⎋");
  assert.equal(formatChord("F1", "mac"), "F1");
  assert.equal(formatChord("s", "mac"), "S");
  assert.equal(formatChord("Cmd+Option+K", "mac"), "⌥⌘K", "Cmd and Option spellings");
});

test("text that is not a chord is left alone, and lists are formatted item by item", () => {
  for (const s of [".lbrn2", "↗", "", "Double-click", "Ctrl+Click+X"]) assert.equal(formatChord(s, "mac"), s);
  assert.equal(formatChord("Ctrl+Z, Ctrl+Y", "mac"), "⌘Z, ⌘Y");
  assert.equal(formatChord("Tab / Shift+Tab", "mac"), "⇥ / ⇧⇥");
});

test("the platform comes from navigator.platform or the user agent", () => {
  assert.equal(platformOf({ platform: "MacIntel" }), "mac");
  assert.equal(platformOf({ platform: "", userAgent: "Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)" }), "mac");
  assert.equal(platformOf({ platform: "Win32", userAgent: "Mozilla/5.0 (Windows NT 10.0)" }), "win");
  assert.equal(platformOf({ platform: "Linux x86_64" }), "win");
  assert.equal(platformOf(undefined), "win");
});
