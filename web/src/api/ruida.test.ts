// Unit tests for api/ruida.ts: the File List's parsing and names, Save RD file's name and the New Ruida form.

import assert from "node:assert/strict";
import { test } from "node:test";
import * as R from "./ruida.ts";

test("only a Ruida machine offers the File List", () => {
  assert.equal(R.isRuidaMachine({ connected: true, driver: "ruida" }), true);
  assert.equal(R.isRuidaMachine({ connected: true, driver: "grbl" }), false);
  assert.equal(R.isRuidaMachine(null), false);
});

test("GET /api/machine/files is read defensively", () => {
  assert.deepEqual(R.parseFiles({ ok: true, files: [{ index: 1, name: "A" }, { index: 0, name: "B" }, { index: 2 }, null, { index: 3, name: "C" }] }), [
    { index: 1, name: "A" },
    { index: 3, name: "C" },
  ]);
  assert.deepEqual(R.parseFiles(null), []);
  assert.deepEqual(R.parseFiles({ files: "x" }), []);
});

test("controller file names are short plain ASCII", () => {
  assert.equal(R.fileNameProblem("JOB 1"), null);
  assert.ok(R.fileNameProblem(""));
  assert.ok(R.fileNameProblem("é"));
  assert.ok(R.fileNameProblem("x".repeat(33)));
  assert.equal(R.suggestFileName("Café sign.lbrn"), "Cafe sign");
  assert.equal(R.suggestFileName("日本"), "JOB");
  assert.equal(R.suggestFileName(null), "JOB");
  assert.equal(R.suggestFileName("y".repeat(40)).length, 32);
  assert.equal(R.rdFileName("Plate.rd", "x"), "Plate.rd");
  assert.equal(R.rdFileName(null, "Plate.lbrn"), "Plate.rd");
  assert.equal(R.rdFileName("weird.bin", null), "job.rd");
});

test("the New Ruida form becomes a profile or a plain problem", () => {
  const ok = R.ruidaDeviceBody({ ...R.RUIDA_FORM_DEFAULT, address: "192.168.1.100", rdc634xg: true });
  assert.ok("body" in ok);
  assert.deepEqual(ok.body.profile, {
    address: "192.168.1.100",
    magic: 0x11,
    work_area_mm: [600, 400],
    origin: "top_right",
    max_power_pct: 100,
    max_speed_mm_s: 500,
  });
  for (const bad of [
    { displayName: " " },
    { address: "laser.local" },
    { address: "300.1.1.1" },
    { widthMm: "0" },
    { maxPowerPct: "120" },
    { maxSpeedMmS: "" },
  ]) {
    assert.ok("error" in R.ruidaDeviceBody({ ...R.RUIDA_FORM_DEFAULT, ...bad }), JSON.stringify(bad));
  }
  assert.ok("body" in R.ruidaDeviceBody(R.RUIDA_FORM_DEFAULT), "the address can be set later");
});
