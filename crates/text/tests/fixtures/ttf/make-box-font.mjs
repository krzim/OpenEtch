#!/usr/bin/env node
// Writes openetch-box.ttf beside this script: a tiny TrueType font made for the tests (no font file of anyone else's
// is committed). Family "OpenEtch Box", Regular, 1000 units per em. Space is blank (300 wide); every other printable
// ASCII character (U+0021-U+007E) is the same 400 x 700 box (600 wide), one closed contour each, so a laid-out
// string has one contour per non-space character.
//
//   node crates/text/tests/fixtures/ttf/make-box-font.mjs
//
// The output is deterministic (no timestamps); the tests use the committed file.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

class W {
  constructor() {
    this.b = [];
  }
  u8(v) {
    this.b.push(v & 0xff);
    return this;
  }
  u16(v) {
    return this.u8(v >> 8).u8(v);
  }
  i16(v) {
    return this.u16(v < 0 ? v + 0x10000 : v);
  }
  u32(v) {
    return this.u16(Math.floor(v / 0x10000) & 0xffff).u16(v & 0xffff);
  }
  bytes(a) {
    for (const x of a) this.u8(x);
    return this;
  }
  get out() {
    return Uint8Array.from(this.b);
  }
}

const FAMILY = "OpenEtch Box";
const UPEM = 1000;
const ASC = 800;
const DESC = -200;
const BOX = { x0: 100, y0: 0, x1: 500, y1: 700, adv: 600 };

// glyf: gid 0 .notdef (empty), gid 1 space (empty), gid 2 the box (clockwise, on-curve points).
const box = new W()
  .i16(1) // one contour
  .i16(BOX.x0).i16(BOX.y0).i16(BOX.x1).i16(BOX.y1)
  .u16(3) // end point of contour 0
  .u16(0) // no instructions
  .bytes([1, 1, 1, 1]) // on curve, x and y as int16 deltas
  .i16(BOX.x0).i16(0).i16(BOX.x1 - BOX.x0).i16(0) // x: 100,100,500,500
  .i16(BOX.y0).i16(BOX.y1 - BOX.y0).i16(0).i16(BOX.y0 - BOX.y1) // y: 0,700,700,0
  .out;
const glyf = new Uint8Array(box.length + (box.length % 4 ? 4 - (box.length % 4) : 0));
glyf.set(box);
const loca = new W().u16(0).u16(0).u16(0).u16(glyf.length / 2).out;

const head = new W()
  .u32(0x00010000).u32(0x00010000)
  .u32(0) // checkSumAdjustment, set below
  .u32(0x5f0f3cf5)
  .u16(0x000b).u16(UPEM)
  .u32(0).u32(0).u32(0).u32(0) // created, modified
  .i16(BOX.x0).i16(BOX.y0).i16(BOX.x1).i16(BOX.y1)
  .u16(0) // macStyle
  .u16(8) // lowestRecPPEM
  .i16(2) // fontDirectionHint
  .i16(0) // short loca
  .i16(0).out;

const hhea = new W()
  .u32(0x00010000).i16(ASC).i16(DESC).i16(0)
  .u16(BOX.adv).i16(0).i16(0).i16(BOX.x1)
  .i16(1).i16(0).i16(0)
  .i16(0).i16(0).i16(0).i16(0)
  .i16(0).u16(3).out;

const maxp = new W().u32(0x00010000).u16(3).u16(4).u16(1).u16(0).u16(0).u16(2).u16(0).u16(0).u16(0).u16(0).u16(0).u16(0).u16(0).u16(0).out;

const os2 = new W()
  .u16(4).i16(BOX.adv).u16(400).u16(5).u16(0)
  .i16(650).i16(600).i16(0).i16(75).i16(650).i16(600).i16(0).i16(350)
  .i16(50).i16(300).i16(0)
  .bytes(new Array(10).fill(0)) // panose
  .u32(1).u32(0).u32(0).u32(0) // Basic Latin
  .bytes([..."NONE"].map((c) => c.charCodeAt(0)))
  .u16(0x0040) // REGULAR
  .u16(0x20).u16(0x7e)
  .i16(ASC).i16(DESC).i16(0)
  .u16(ASC).u16(-DESC)
  .u32(1).u32(0)
  .i16(500).i16(BOX.y1).u16(0).u16(0x20).u16(1).out;

const hmtx = new W().u16(BOX.adv).i16(0).u16(300).i16(0).u16(BOX.adv).i16(BOX.x0).out;

// cmap: Windows Unicode BMP, format 4: space -> 1 (by idDelta), U+0021-U+007E -> 2 (through glyphIdArray, which
// starts right after idRangeOffset[2], two words on from idRangeOffset[1]).
const PRINTABLE = 0x7e - 0x21 + 1;
const segs = [
  // [start, end, idDelta, idRangeOffset]
  [0x20, 0x20, 1 - 0x20, 0],
  [0x21, 0x7e, 0, 4],
  [0xffff, 0xffff, 1, 0],
];
const sub = new W().u16(4).u16(16 + segs.length * 8 + PRINTABLE * 2).u16(0).u16(segs.length * 2).u16(4).u16(1).u16(segs.length * 2 - 4);
for (const s of segs) sub.u16(s[1]);
sub.u16(0);
for (const s of segs) sub.u16(s[0]);
for (const s of segs) sub.u16((s[2] + 0x10000) & 0xffff);
for (const s of segs) sub.u16(s[3]);
for (let i = 0; i < PRINTABLE; i++) sub.u16(2);
const cmap = new W().u16(0).u16(1).u16(3).u16(1).u32(12).bytes(sub.out).out;

// name: family, subfamily, full name, PostScript name (Windows, en-US, UTF-16BE).
const names = [
  [1, FAMILY],
  [2, "Regular"],
  [4, `${FAMILY} Regular`],
  [6, "OpenEtchBox-Regular"],
];
const strings = new W();
const recs = [];
for (const [id, s] of names) {
  const start = strings.b.length;
  for (const c of s) strings.u16(c.charCodeAt(0));
  recs.push([id, start, strings.b.length - start]);
}
const name = new W().u16(0).u16(recs.length).u16(6 + recs.length * 12);
for (const [id, off, len] of recs) name.u16(3).u16(1).u16(0x0409).u16(id).u16(len).u16(off);
name.bytes(strings.out);

const post = new W().u32(0x00030000).u32(0).i16(-100).i16(50).u32(0).u32(0).u32(0).u32(0).u32(0).out;

const tables = { "OS/2": os2, cmap, glyf, head, hhea, hmtx, loca, maxp, name: name.out, post };
const tags = Object.keys(tables).sort();

function checksum(a) {
  let sum = 0;
  for (let i = 0; i < a.length; i += 4) {
    sum = (sum + (((a[i] << 24) | ((a[i + 1] ?? 0) << 16) | ((a[i + 2] ?? 0) << 8) | (a[i + 3] ?? 0)) >>> 0)) >>> 0;
  }
  return sum;
}

const n = tags.length;
let p = 1;
let e = 0;
while (p * 2 <= n) {
  p *= 2;
  e += 1;
}
const font = new W().u32(0x00010000).u16(n).u16(p * 16).u16(e).u16(n * 16 - p * 16);
let offset = 12 + n * 16;
const placed = [];
for (const tag of tags) {
  const t = tables[tag];
  font.bytes([...tag].map((c) => c.charCodeAt(0))).u32(checksum(t)).u32(offset).u32(t.length);
  placed.push([tag, offset]);
  offset += t.length + ((4 - (t.length % 4)) % 4);
}
for (const tag of tags) {
  const t = tables[tag];
  font.bytes(t);
  while (font.b.length % 4) font.u8(0);
}
const bytes = font.out;
const adj = (0xb1b0afba - checksum(bytes)) >>> 0;
const headAt = placed.find(([t]) => t === "head")[1];
new DataView(bytes.buffer).setUint32(headAt + 8, adj);

const out = join(dirname(fileURLToPath(import.meta.url)), "openetch-box.ttf");
writeFileSync(out, bytes);
console.log(`wrote ${out} (${bytes.length} bytes)`);
