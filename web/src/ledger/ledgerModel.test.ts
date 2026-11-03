// The job ledger's model: entries as the engine sends them (and older job log lines), the search query string, the
// short settings text, totals, the storage settings and the Re-run answer.

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  DEFAULT_FILTER,
  bytesText,
  entryTitle,
  estimateError,
  imageDither,
  ledgerQuery,
  ledgerTotals,
  optionParts,
  parseLedger,
  parseLedgerSettings,
  rerunOutcome,
  settingParts,
  settingsBody,
  timeShares,
} from "./ledgerModel.ts";

const entry = {
  id: "1730000100-run-3",
  time_unix_s: 1730000100,
  run_id: "run-3",
  state: "finished",
  origin: "agent:claude",
  device: "b6",
  project: "Coins",
  document: "Coins",
  summary: "2 polylines on \"Blue\"",
  duration_s: 12.5,
  completed: 1,
  error: null,
  layers: [
    {
      id: "layer-1",
      name: "Blue",
      color: "#0000ff",
      output: true,
      objects: 2,
      settings: { mode: "fill", power_pct: 40, speed_mm_s: 1000, frequency_khz: 30, pulse_width_ns: 200, passes: 3, hatch: { interval_mm: 0.02 }, image: { dither: "slice3d" } },
      sub_layers: [{ power_pct: 10, speed_mm_s: 3000, frequency_khz: 600, pulse_width_ns: null, passes: 1 }],
      passes: 4,
      estimated_time_s: 9,
    },
    { id: "layer-2", name: "Frost", color: "#ff0000", output: true, objects: 1, settings: { power_pct: 20 }, estimated_time_s: 3 },
  ],
  variables: { current: 41, csv_file: "rows.csv", texts: [{ object: "obj-4", name: "Serial", mode: "serial", value: "041" }] },
  estimated_time_s: 12,
  mark_time_s: 13.2,
  options: { repeat: 3, selection: ["obj-2", "obj-3"] },
  verdict: "scrap",
  note: "Too dark",
  noted_by: "ui",
  noted_unix_s: 1730000200,
  thumbnail: true,
  rerun: true,
};

test("entries parse, newest first, and an older plain job log line still reads", () => {
  const old = { time_unix_s: 1720000000, run_id: "run-1", state: "failed", origin: "ui", device: "b6", project: null, summary: "s", duration_s: 2, completed: 0, error: "door open" };
  const rows = parseLedger({ entries: [old, entry, { nonsense: true }, 7] });
  assert.equal(rows.length, 3);
  assert.equal(rows[0].id, "1730000100-run-3");
  const e = rows[0];
  assert.equal(e.layers[0].name, "Blue");
  assert.equal(e.layers[0].subLayers.length, 1);
  assert.equal(e.variables?.texts[0].value, "041");
  assert.equal(e.variables?.csvFile, "rows.csv");
  assert.equal(e.verdict, "scrap");
  assert.equal(e.rerun, true);
  const o = rows.find((r) => r.runId === "run-1");
  assert.ok(o);
  assert.equal(o.id, "1720000000-run-1", "an old line gets the engine's id");
  assert.deepEqual([o.layers.length, o.variables, o.verdict, o.thumbnail, o.rerun], [0, null, null, false, false]);
  assert.equal(entryTitle(o), "run-1");
  assert.equal(entryTitle(e), "Coins");
});

test("the query string carries only what is set", () => {
  assert.equal(ledgerQuery(DEFAULT_FILTER), "limit=200");
  assert.equal(ledgerQuery({ q: "  blue coin ", verdict: "scrap", state: "finished", limit: 50 }, "csv"), "limit=50&q=blue+coin&verdict=scrap&state=finished&format=csv");
  assert.equal(ledgerQuery({ ...DEFAULT_FILTER, limit: 99999 }), "limit=2000");
});

test("settings read as a MOPA operator writes them", () => {
  const [e] = parseLedger({ entries: [entry] });
  assert.deepEqual(settingParts(e.layers[0].settings), ["40 %", "1000 mm/s", "30 kHz", "200 ns", "×3"]);
  assert.deepEqual(settingParts(e.layers[0].subLayers[0]), ["10 %", "3000 mm/s", "600 kHz"]);
  assert.equal(imageDither(e.layers[0].settings), "slice3d");
  assert.equal(imageDither(e.layers[1].settings), null);
  assert.deepEqual(optionParts(e.options), ["repeat:3", "selection:2"]);
  assert.deepEqual(optionParts(null), []);
  assert.equal(estimateError(e), 10);
  assert.deepEqual(timeShares(e.layers), [0.75, 0.25]);
});

test("totals count verdicts and failures", () => {
  const rows = parseLedger({ entries: [entry, { ...entry, id: "x", state: "aborted", verdict: "ok" }, { ...entry, id: "y", verdict: null }] });
  assert.deepEqual(ledgerTotals(rows), { runs: 3, finished: 2, failed: 1, ok: 1, scrap: 1, seconds: 37.5 });
});

test("storage settings round-trip and sizes read well", () => {
  const v = parseLedgerSettings({ settings: { keep_documents: 10, max_document_mb: 5, keep_thumbnails: 50, thumbnail_px: 96 }, usage: { thumbnails: 3, thumbnail_bytes: 12000, documents: 1, document_bytes: 3500000 } });
  assert.deepEqual(v.settings, { keepDocuments: 10, maxDocumentMb: 5, keepThumbnails: 50, thumbnailPx: 96 });
  assert.deepEqual(settingsBody(v.settings), { keep_documents: 10, max_document_mb: 5, keep_thumbnails: 50, thumbnail_px: 96, origin: "ui" });
  assert.deepEqual(parseLedgerSettings(null).settings, { keepDocuments: 100, maxDocumentMb: 25, keepThumbnails: 2000, thumbnailPx: 160 });
  assert.equal(bytesText(0), "0 B");
  assert.equal(bytesText(12000), "12 kB");
  assert.equal(bytesText(3500000), "3.3 MB");
});

test("a Re-run answer names the pending run, or why there is none", () => {
  assert.deepEqual(rerunOutcome({ ok: true, document_id: "doc-2", run: { id: "run-7", state: "pending" } }), { documentId: "doc-2", runId: "run-7", problem: null });
  assert.deepEqual(rerunOutcome({ ok: true, document_id: "doc-3", run_error: "the job has nothing to mark" }), { documentId: "doc-3", runId: null, problem: "the job has nothing to mark" });
  assert.equal(rerunOutcome({ document_id: "doc-4", device_mismatch: "burned on b6" }).problem, "burned on b6");
});
