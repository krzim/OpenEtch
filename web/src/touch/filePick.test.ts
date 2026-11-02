// Unit tests for touch/filePick.ts: no `accept` on iOS/iPadOS (custom extensions grey out), and the check after the pick.

import assert from "node:assert/strict";
import { test } from "node:test";
import { acceptLabel, acceptsFile, isAppleTouch, pickerAccept, splitPicked } from "./filePick.ts";

const ipad = { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15", platform: "MacIntel", maxTouchPoints: 5 };
const mac = { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)", platform: "MacIntel", maxTouchPoints: 0 };
const iphone = { userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X)", platform: "iPhone" };
const win = { userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)", platform: "Win32", maxTouchPoints: 10 };

test("iOS and iPadOS (also posing as a Mac) get no accept; others keep it", () => {
  assert.equal(isAppleTouch(ipad), true, "iPadOS desktop-class Safari");
  assert.equal(isAppleTouch(iphone), true);
  assert.equal(isAppleTouch(mac), false);
  assert.equal(isAppleTouch(win), false, "a Windows touch laptop keeps accept");
  assert.equal(pickerAccept(".clb", ipad), undefined);
  assert.equal(pickerAccept(".clb", mac), ".clb");
  assert.equal(pickerAccept(".openetch,.lbrn2", win), ".openetch,.lbrn2");
});

test("acceptsFile: extensions (any case) and MIME types", () => {
  assert.equal(acceptsFile({ name: "Tiles.CLB" }, ".clb"), true);
  assert.equal(acceptsFile({ name: "logo.svg", type: "image/svg+xml" }, ".oeart,.lbart"), false);
  assert.equal(acceptsFile({ name: "a.lbart" }, ".oeart, .lbart"), true);
  assert.equal(acceptsFile({ name: "photo", type: "image/jpeg" }, "image/*"), true);
  assert.equal(acceptsFile({ name: "x.pdf", type: "application/pdf" }, "application/pdf"), true);
  assert.equal(acceptsFile({ name: "anything.bin" }, ""), true, "no list: anything");
});

test("splitPicked and the message's type list", () => {
  const r = splitPicked([{ name: "a.svg" }, { name: "b.txt" }, { name: "c.DXF" }], ".svg,.dxf");
  assert.deepEqual(r.ok.map((f) => f.name), ["a.svg", "c.DXF"]);
  assert.deepEqual(r.wrong, ["b.txt"]);
  assert.equal(acceptLabel(".clb"), ".clb");
  assert.equal(acceptLabel(".oeart,.lbart"), ".oeart or .lbart");
  assert.equal(acceptLabel(".a,.b,.c"), ".a, .b or .c");
});
