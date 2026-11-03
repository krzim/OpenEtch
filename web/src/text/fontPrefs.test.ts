// The font picker's favourites, hidden fonts and grouping (fontPrefs.ts), and the sample queue's pure parts
// (fontSamples.ts).

import assert from "node:assert/strict";
import { test } from "node:test";
import { isFavourite, isHidden, parseFontPrefs, pickerGroups, toggleFavourite, toggleHidden, NO_FONT_PREFS } from "./fontPrefs.ts";
import { BATCH, batches, parsePreview, sampleKey, sampleText } from "./fontSamples.ts";
import type { FontInfo } from "./textModel.ts";

const f = (family: string, kind: FontInfo["kind"] = "system"): FontInfo => ({ family, kind, bold: false, italic: false });
const LIST = [f("Arial"), f("Zapf"), f("Comic"), f("Hershey Sans", "stroke"), f("Hershey Script", "stroke"), f("romans", "shx")];

test("favourites come first, hidden fonts leave the list until shown", () => {
  let p = toggleFavourite(NO_FONT_PREFS, f("Hershey Script", "stroke"));
  p = toggleFavourite(p, f("Zapf"));
  p = toggleHidden(p, f("Comic"));
  const { groups, hiddenCount } = pickerGroups(LIST, "", p, false);
  assert.deepEqual(
    groups.map((g) => [g.id, g.fonts.map((x) => x.family)]),
    [
      ["favourites", ["Hershey Script", "Zapf"]],
      ["system", ["Arial", "Zapf"]],
      ["stroke", ["Hershey Sans", "Hershey Script"]],
      ["shx", ["romans"]],
    ],
  );
  assert.equal(hiddenCount, 1);
  assert.ok(pickerGroups(LIST, "", p, true).groups[1].fonts.some((x) => x.family === "Comic"), "show hidden");
  // The font in use stays listed even when hidden.
  assert.ok(pickerGroups(LIST, "", p, false, { family: "comic", kind: "system" }).groups[1].fonts.some((x) => x.family === "Comic"));
  // Search filters every group, favourites too.
  assert.deepEqual(
    pickerGroups(LIST, "script", p, false).groups.map((g) => g.id),
    ["favourites", "stroke"],
  );
});

test("a favourite is never hidden and toggles undo themselves", () => {
  const a = f("Arial");
  let p = toggleHidden(NO_FONT_PREFS, a);
  assert.ok(isHidden(p, a));
  p = toggleFavourite(p, a);
  assert.ok(isFavourite(p, a) && !isHidden(p, a), "starring shows it again");
  p = toggleHidden(p, a);
  assert.ok(!isFavourite(p, a) && isHidden(p, a), "hiding unstars it");
  assert.deepEqual(toggleHidden(p, a), NO_FONT_PREFS);
  // Same font whatever the case; the kind matters.
  assert.ok(isHidden(p, f("ARIAL")) && !isHidden(p, f("Arial", "shx")));
});

test("the engine's lists are checked", () => {
  assert.deepEqual(parseFontPrefs({ favourites: [{ family: "A", kind: "stroke" }, { family: "" }, 3, { family: "B", kind: "nope" }], hidden: "x" }), {
    favourites: [
      { family: "A", kind: "stroke" },
      { family: "B", kind: "system" },
    ],
    hidden: [],
  });
  assert.deepEqual(parseFontPrefs(null), NO_FONT_PREFS);
});

test("samples: one line of at most 40 characters, batched per text", () => {
  assert.equal(sampleText(""), "AaBbCc 123");
  assert.equal(sampleText("\n  Ada   Lovelace \nsecond"), "Ada Lovelace");
  assert.equal([...sampleText("x".repeat(60))].length, 40);
  const q = Array.from({ length: BATCH + 3 }, (_, i) => ({ font: f(`F${i}`), text: "Ab" }));
  q.push({ font: f("F1"), text: "Ab" }, { font: f("F1"), text: "Cd" });
  const b = batches(q);
  assert.deepEqual(
    b.map((x) => [x.text, x.fonts.length]),
    [
      ["Ab", BATCH],
      ["Ab", 3],
      ["Cd", 1],
    ],
  );
  const m = parsePreview(
    [
      { family: "Hershey Sans", kind: "stroke", d: "M0 0L1 1", view_box: [0, -10, 20, 10] },
      { family: "Gone", kind: "system", error: "not installed" },
      { family: "Bad", kind: "system", d: "M0 0", view_box: [0, 0, 0, 0] },
    ],
    "Ab",
  );
  assert.deepEqual(m.get(sampleKey(f("hershey sans", "stroke"), "Ab")), { d: "M0 0L1 1", viewBox: [0, -10, 20, 10] });
  assert.deepEqual(m.get(sampleKey(f("Gone"), "Ab")), { error: "not installed" });
  assert.equal(m.size, 2, "an empty box is dropped");
});
