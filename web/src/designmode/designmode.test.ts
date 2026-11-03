import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { DESIGN_ONLY_ERROR, IDLE_JOB, NO_MACHINE, isMachineRoute, machineStub } from "./routes.ts";
import { DEFAULT_DESIGN_DEVICE, GENERIC_FIELDS_MM, designDeviceList, designDevices } from "./designDevices.ts";
import { DESIGN_FORBIDDEN, DESIGN_HIDDEN_PAGES, designGuideSource, designHiddenPages, hiddenInDesign } from "../guide/designFilter.ts";
import { hasFileImports, missingRequired, REQUIRED_COMMANDS, unavailableImports, withoutExtensions } from "../api/commands.ts";
import { buildFileMenu, labelPaths, type FileMenuInput } from "../components/fileMenuModel.ts";
import { DESIGN_ONLY } from "../mode.ts";

test("the test runner builds the full app", () => {
  assert.equal(DESIGN_ONLY, false);
});

test("machine routes get the design build's stubs; document routes go to the backend", () => {
  assert.deepEqual(machineStub("GET", "/api/machine"), { status: 200, body: NO_MACHINE });
  assert.deepEqual(machineStub("get", "/api/job"), { status: 200, body: IDLE_JOB });
  const machine = ["/api/machine/jog", "/api/machine/frame", "/api/runs", "/api/runs/r1/approve", "/api/job/start", "/api/camera/frame.png", "/api/devices/scan", "/api/devices/wizard", "/api/production", "/api/job-log", "/api/diagnostics/report", "/api/updates/check", "/api/export/rd"];
  for (const p of machine) {
    assert.ok(isMachineRoute(p), p);
    assert.deepEqual(machineStub("POST", p), { status: 404, body: DESIGN_ONLY_ERROR }, p);
  }
  const doc = ["/api/document", "/api/execute", "/api/devices", "/api/devices/x/activate", "/api/devices/import", "/api/export", "/api/preview.png", "/api/jobs", "/api/library/x", "/api/projects/save"];
  for (const p of doc) {
    assert.equal(isMachineRoute(p), false, p);
    assert.equal(machineStub("GET", p), null, p);
  }
  assert.equal(NO_MACHINE.connected, false);
});

test("the built-in design devices: the B6 first, then the generic galvo fields, all read-only and none driven", () => {
  const all = designDevices();
  assert.equal(all[0].id, DEFAULT_DESIGN_DEVICE);
  assert.deepEqual(
    all.slice(1).map((d) => d.workspace_mm[0]),
    [...GENERIC_FIELDS_MM],
  );
  for (const d of all) {
    assert.equal(d.source, "builtin");
    assert.equal(d.controllable, false);
    assert.equal(d.kind, "jcz_galvo");
    assert.equal(d.workspace_mm[0], d.workspace_mm[1]);
  }
  assert.equal(new Set(all.map((d) => d.id)).size, all.length);
  const list = designDeviceList("generic-galvo-200");
  assert.equal(list.active, "generic-galvo-200");
  assert.deepEqual(
    list.devices.filter((d) => (d as { active?: boolean }).active).map((d) => d.id),
    ["generic-galvo-200"],
  );
});

test("missing DXF, PDF or AI importers switch their entries off in the design build only", () => {
  const without = (gone: string[]) => (id: string) => !gone.includes(id);
  assert.deepEqual(unavailableImports(without(["art.import_pdf", "art.import_dxf"]), true), ["dxf", "pdf"]);
  assert.deepEqual(unavailableImports(without(["art.import_pdf"]), false), []);
  assert.equal(hasFileImports(without(["art.import_pdf"]), false), false);
  assert.equal(hasFileImports(without(["art.import_pdf"]), true), true);
  assert.equal(hasFileImports(without(["art.import_pdf", "art.import_eps"]), true), false);
  assert.equal(withoutExtensions(".svg,.dxf,.PDF,.ai,.png", ["dxf", "pdf"]), ".svg,.ai,.png");
  assert.equal(withoutExtensions(".svg", []), ".svg");
  // The startup check never counts the optional imports.
  assert.deepEqual(missingRequired(new Set(REQUIRED_COMMANDS)), []);
  assert.deepEqual(missingRequired(new Set(REQUIRED_COMMANDS.filter((c) => c !== "shape.rect"))), ["shape.rect"]);
});

test("the design File menu puts Download a copy and Export LightBurn at the top; the full app's is unchanged", () => {
  const noop = () => {};
  const base: FileMenuInput = {
    busy: false,
    newDocument: noop,
    open: noop,
    save: noop,
    saveAs: noop,
    importFiles: noop,
    importLightBurn: noop,
    exportLightBurn: noop,
    downloadOpenetch: noop,
    parts: {},
    labels: { new: "New", open: "Open", openRecent: "Open Recent", save: "Save", saveAs: "Save As", import: "Import", export: "Export", exportLightBurn: "Export to LightBurn", saveGcode: "G", saveRd: "R", downloadOpenetch: "Download .openetch", project: "Project", lightburn: "LightBurn", importLightBurn: "Import LightBurn", laser: "Laser", devices: "Devices", settings: "Settings" },
  };
  const full = labelPaths(buildFileMenu(base));
  const design = labelPaths(buildFileMenu({ ...base, design: { exportLightBurn: "Export LightBurn .lbrn2", downloadCopy: "Download a copy" } }));
  assert.ok(!full.includes("Download a copy"));
  assert.deepEqual(design.slice(0, 6), ["New", "Open", "Save", "Save As", "Download a copy", "Export LightBurn .lbrn2"]);
  assert.deepEqual(design.filter((x) => !["Download a copy", "Export LightBurn .lbrn2"].includes(x)), full);
});

const docs = fileURLToPath(new URL("../../../docs/guide/", import.meta.url));
const pages = readdirSync(docs)
  .filter((f) => f.endsWith(".md") && f !== "README.md")
  .map((f) => [f.slice(0, -3), readFileSync(docs + f, "utf8")] as [string, string]);

test("the design build's guide: machine and MCP pages keep only their front matter; nothing forbidden is left", () => {
  const hidden = designHiddenPages(pages);
  for (const s of DESIGN_HIDDEN_PAGES) assert.ok(hidden.includes(s), s);
  assert.ok(hidden.includes("running-a-job"));
  assert.ok(!hidden.includes("preview-and-framing"));
  for (const [slug, src] of pages) {
    const out = designGuideSource(slug, src);
    for (const s of DESIGN_FORBIDDEN) assert.ok(!out.includes(s), `${slug}: ${s}`);
    if (hiddenInDesign(slug, src)) {
      assert.match(out, /^---\n[\s\S]*\nneeds: machine\n---\n$/, slug);
      assert.match(out, /title:/, slug);
    } else assert.ok(out.length > src.length * 0.8, `${slug} kept its text`);
  }
});
