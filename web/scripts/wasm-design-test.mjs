#!/usr/bin/env node
// The design-only build's wasm blockers (docs/design-only/plan.md, WP3), checked in Node on the real wasm:
// crates/webcore with feature `design`, built for wasm32-unknown-unknown and loaded through wasm-bindgen's
// experimental-nodejs-module glue. It imports DXF (a hand-written one and the core's own export), PDF and
// PDF-based .ai, adds a TrueType font (the generated fixture in crates/text/tests/fixtures/ttf), lays text out in it,
// checks font.list lists it, and shows {date} variable text from the browser clock.
//
//   node scripts/wasm-design-test.mjs          release build (profile wasm-release, as shipped), then the checks
//   node scripts/wasm-design-test.mjs --dev    cargo's dev profile (quicker to build)
//
// Output goes to <repo>/target/wasm-design-test (git ignores target). Tools as scripts/wasm.mjs finds them:
// $WASM_BINDGEN, else <repo>/target/tools/bin/wasm-bindgen; $WASM_OPT (binaryen's wasm-opt.js or a native wasm-opt)
// for the size report only. Nothing here is a runtime dependency.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { gzipSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
const dev = process.argv.includes("--dev");
const profile = dev ? "dev" : "wasm-release";
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repo, "target");
const out = join(targetDir, "wasm-design-test");

function run(cmd, args, opts = {}) {
  execFileSync(cmd, args, { stdio: "inherit", cwd: repo, ...opts });
}

// ---- build ------------------------------------------------------------------------------------------------------
console.log(`[design-test] cargo build -p webcore --features design (${profile})`);
run("cargo", ["build", "-p", "webcore", "--features", "design", "--target", "wasm32-unknown-unknown", "--profile", profile, "--locked"]);
const built = join(targetDir, "wasm32-unknown-unknown", dev ? "debug" : "wasm-release", "webcore.wasm");
const bindgen = process.env.WASM_BINDGEN || join(repo, "target", "tools", "bin", `wasm-bindgen${exe}`);
assert.ok(existsSync(bindgen), `wasm-bindgen not found at ${bindgen} (node scripts/wasm.mjs --tools installs it)`);
mkdirSync(out, { recursive: true });
run(bindgen, ["--target", "experimental-nodejs-module", "--no-typescript", "--out-dir", out, "--out-name", "webcore", built]);
const wasmFile = join(out, "webcore_bg.wasm");

// Size report: as wasm-bindgen wrote it, and after wasm-opt -Oz when a wasm-opt is at hand (as scripts/wasm.mjs).
const raw = readFileSync(wasmFile);
const sizes = { raw: raw.length, gz: gzipSync(raw, { level: 9 }).length };
const wasmOpt = process.env.WASM_OPT;
if (!dev && wasmOpt && existsSync(wasmOpt)) {
  const optOut = join(out, "webcore_opt.wasm");
  const flags = ["-Oz", "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "--enable-sign-ext", "--enable-mutable-globals", "--enable-reference-types", "--enable-multivalue", "--strip-debug", "--strip-producers"];
  const [cmd, lead] = wasmOpt.endsWith(".js") ? [process.execPath, [wasmOpt]] : [wasmOpt, []];
  run(cmd, [...lead, wasmFile, "-o", optOut, ...flags]);
  const opt = readFileSync(optOut);
  Object.assign(sizes, { opt: opt.length, optGz: gzipSync(opt, { level: 9 }).length });
}
const mb = (n) => `${(n / 1048576).toFixed(2)} MB`;
console.log(`[design-test] wasm ${mb(sizes.raw)} (${mb(sizes.gz)} gz)${sizes.opt ? `; wasm-opt -Oz ${mb(sizes.opt)} (${mb(sizes.optGz)} gz)` : ""}`);

// ---- checks -----------------------------------------------------------------------------------------------------
const wasm = await import(pathToFileURL(join(out, "webcore.js")).href);
const c = new wasm.Commands();

function call(id, params = {}) {
  const r = JSON.parse(c.run(id, JSON.stringify(params)));
  assert.ok(!("error" in r), `${id}: ${r.error}`);
  return r.result;
}
function fails(id, params) {
  const r = JSON.parse(c.run(id, JSON.stringify(params)));
  assert.ok("error" in r, `${id} should have failed: ${JSON.stringify(r)}`);
  return r.error;
}
const near = (a, b, tol = 1e-3) => Math.abs(a - b) <= tol;
const checks = [];
function check(name, fn) {
  fn();
  checks.push(name);
  console.log(`  ok  ${name}`);
}

const ids = JSON.parse(c.ids());
check("the import commands are registered on wasm", () => {
  for (const id of ["art.import_dxf", "art.import_pdf", "art.import_ai", "jig.import", "font.list", "shape.text"]) {
    assert.ok(ids.includes(id), `${id} missing`);
  }
});

// A hand-written DXF R12: a 30 mm line and a circle of radius 5 at (10, 10).
const DXF = ["0", "SECTION", "2", "ENTITIES",
  "0", "LINE", "8", "Cut", "10", "0.0", "20", "0.0", "30", "0.0", "11", "30.0", "21", "0.0", "31", "0.0",
  "0", "CIRCLE", "8", "Cut", "10", "10.0", "20", "10.0", "30", "0.0", "40", "5.0",
  "0", "ENDSEC", "0", "EOF", ""].join("\n");
check("DXF import (hand-written)", () => {
  const r = call("art.import_dxf", { content: DXF, keep_position: true });
  assert.equal(r.created.length, 2, JSON.stringify(r));
  const b = r.bounds;
  assert.ok(near(b.min[0], 0) && near(b.max[0], 30) && near(b.min[1], 0) && near(b.max[1], 15), JSON.stringify(b));
});

call("shape.rect", { width_mm: 20, height_mm: 10, center: [50, 50] });
check("DXF round trip (doc.export dxf, then art.import_dxf)", () => {
  const e = call("doc.export", { format: "dxf" });
  assert.ok(e.content.includes("ENTITIES"));
  const r = call("art.import_dxf", { content: e.content, keep_position: true, name: "Round trip" });
  assert.ok(r.created.length >= 1, JSON.stringify(r));
});

check("PDF import (doc.export pdf, then art.import_pdf)", () => {
  const e = call("doc.export", { format: "pdf" });
  assert.ok(Buffer.from(e.content_base64, "base64").subarray(0, 5).toString() === "%PDF-");
  const r = call("art.import_pdf", { data_base64: e.content_base64, keep_position: true });
  assert.ok(r.created.length >= 1, JSON.stringify(r));
  assert.ok(near(r.bounds.size[0], e.bounds.size[0], 0.05), `${JSON.stringify(r.bounds)} vs ${JSON.stringify(e.bounds)}`);
});

check("PDF-based Illustrator import (doc.export ai, then art.import_ai)", () => {
  const e = call("doc.export", { format: "ai" });
  const r = call("art.import_ai", { data_base64: e.content_base64, keep_position: true });
  assert.ok(r.created.length >= 1, JSON.stringify(r));
});

check("a damaged PDF is an error, not a crash", () => {
  const err = fails("art.import_pdf", { data_base64: Buffer.from("%PDF-1.4 nothing here").toString("base64") });
  assert.match(err, /pdf/i);
});

check("DXF jig (jig.import)", () => {
  const sq = ["0", "SECTION", "2", "ENTITIES",
    "0", "LWPOLYLINE", "8", "0", "90", "4", "70", "1", "10", "0", "20", "0", "10", "40", "20", "0", "10", "40", "20", "40", "10", "0", "20", "40",
    "0", "LWPOLYLINE", "8", "0", "90", "4", "70", "1", "10", "10", "20", "10", "10", "20", "20", "10", "10", "20", "20", "20", "10", "10", "20", "20",
    "0", "ENDSEC", "0", "EOF", ""].join("\n");
  const r = call("jig.import", { format: "dxf", content: sq });
  assert.ok(JSON.stringify(r).includes("pocket"), JSON.stringify(r).slice(0, 400));
});

// Fonts: the generated TrueType fixture, as the worker shell would add an upload kept in IndexedDB.
const ttf = readFileSync(join(repo, "crates", "text", "tests", "fixtures", "ttf", "openetch-box.ttf"));
check("add_font registers a TrueType font and font.list lists it", () => {
  assert.deepEqual(JSON.parse(wasm.add_font("upload.ttf", new Uint8Array(ttf))), { families: ["OpenEtch Box"] });
  // Again (the shell adds its stored fonts at every start): still one face.
  assert.deepEqual(JSON.parse(wasm.add_font("upload.ttf", new Uint8Array(ttf))), { families: ["OpenEtch Box"] });
  const list = call("font.list", {});
  const box = list.filter((f) => f.family === "OpenEtch Box");
  assert.deepEqual(box, [{ family: "OpenEtch Box", kind: "system", bold: false, italic: false }]);
  assert.ok(list.some((f) => f.family === "Hershey Sans" && f.kind === "stroke"), "bundled Hershey fonts stay");
  assert.match(JSON.parse(wasm.add_font("junk", new Uint8Array([1, 2, 3]))).error, /not a TrueType/);
});

check("text lays out in the added font", () => {
  // One 0.7 em box per non-space character: 10 mm text, 7 mm tall boxes.
  const r = call("shape.text", { text: "OE 12", font: { family: "OpenEtch Box" }, height_mm: 10, align_v: "bottom", align_h: "left" });
  assert.ok(near(r.bounds.size[1], 7, 1e-6), JSON.stringify(r.bounds));
  // Boxes from 1 mm (the first one's side bearing) to 26 mm (advances 6 + 6 + 3 + 6, then the last box's 5 mm).
  assert.ok(near(r.bounds.size[0], 25, 1e-6), JSON.stringify(r.bounds));
  const missing = fails("shape.text", { text: "x", font: { family: "Not A Font" } });
  assert.match(missing, /not installed/);
});

function pad(n, w = 2) {
  return String(n).padStart(w, "0");
}
function shownOf(id) {
  const doc = JSON.parse(c.document());
  let found;
  JSON.stringify(doc, (k, v) => {
    if (v && typeof v === "object" && v.id === id) found = v;
    return v;
  });
  assert.ok(found, `object ${id} not in the document`);
  let shown;
  JSON.stringify(found, (k, v) => {
    if (k === "shown" && typeof v === "string") shown = v;
    return v;
  });
  return shown;
}
check("{date} variable text shows the browser's local date", () => {
  const before = new Date();
  const r = call("shape.text", { text: "yyyy-MM-dd", font: { family: "Hershey Sans", kind: "stroke" }, variable: { mode: "date_time" } });
  const after = new Date();
  const today = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  const shown = shownOf(r.created[0]);
  assert.ok(shown === today(before) || shown === today(after), `shown ${shown}, today ${today(before)}`);
});

check("{time} variable text shows the browser's local time", () => {
  const r = call("shape.text", { text: "HH:mm", font: { family: "Hershey Sans", kind: "stroke" }, variable: { mode: "date_time" } });
  const shown = shownOf(r.created[0]);
  const now = new Date();
  const mins = now.getHours() * 60 + now.getMinutes();
  const [h, m] = shown.split(":").map(Number);
  assert.ok(Math.abs(h * 60 + m - mins) <= 1 || Math.abs(h * 60 + m - mins) >= 1439, `shown ${shown} at ${now.toTimeString()}`);
});

check("remove_font takes the added font away again", () => {
  assert.equal(wasm.remove_font("openetch box"), 1);
  assert.ok(!call("font.list", {}).some((f) => f.family === "OpenEtch Box"));
});

writeFileSync(join(out, "result.json"), JSON.stringify({ checks, sizes, profile }, null, 2));
console.log(`[design-test] ${checks.length} checks passed`);
