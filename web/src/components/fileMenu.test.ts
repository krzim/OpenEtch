// Unit tests for the File menu's structure (fileMenuModel.ts).

import { test } from "node:test";
import assert from "node:assert/strict";
import { FILE_MENU_MAX_TOP, actionIdsOf, buildFileMenu, labelPaths, topLevelCount, type FileMenuInput } from "./fileMenuModel.ts";
import type { MenuItem } from "./Menu.tsx";

const a = () => {};
const it = (label: string, extra: Partial<Exclude<MenuItem, { sep: true }>> = {}): MenuItem => ({ label, action: a, ...extra });

const LABELS: FileMenuInput["labels"] = {
  new: "New",
  open: "Open…",
  openRecent: "Open Recent",
  save: "Save",
  saveAs: "Save as…",
  import: "Import…",
  export: "Export",
  exportLightBurn: "Export to LightBurn…",
  saveGcode: "Save G-code…",
  saveRd: "Save RD file…",
  downloadOpenetch: "Download .openetch",
  project: "Project",
  lightburn: "LightBurn",
  importLightBurn: "Import LightBurn…",
  laser: "Laser",
  devices: "Devices…",
  settings: "Settings",
};

/** Everything the app contributes today. */
function full(): FileMenuInput {
  return {
    busy: false,
    newDocument: a,
    open: a,
    recent: { label: "Open Recent", content: () => null },
    save: a,
    saveAs: a,
    importFiles: a,
    importLightBurn: a,
    exportLightBurn: a,
    downloadOpenetch: a,
    saveGcode: a,
    saveRd: a,
    devices: a,
    labels: LABELS,
    parts: {
      export: [it("Export…", { actionId: "file.export" }), it("Print…", { actionId: "file.print" }), it("Export image…"), it("Save processed bitmap…")],
      project: [it("Open template…"), it("Recent templates", { content: () => null }), it("Save as template…"), { sep: true }, it("Notes…", { actionId: "window.notes" }), it("Restore from autosave / backup…")],
      lightburn: [it("Preferences and bundles…")],
      laser: [it("Material Library", { actionId: "window.library" }), it("Find My Laser / Add galvo…"), it("Batch run…"), it("Job log…", { actionId: "laser.jobLog" })],
      settings: [it("Preferences…", { actionId: "app.preferences" }), it("Edit hotkeys…"), it("Import options…")],
      top: [it("New tab", { actionId: "window.newTab" })],
    },
  };
}

test("at most 14 top-level rows with everything contributed", () => {
  const m = buildFileMenu(full());
  assert.ok(topLevelCount(m) <= FILE_MENU_MAX_TOP, `${topLevelCount(m)} rows`);
  assert.deepEqual(
    m.filter((x) => !x.sep).map((x) => !x.sep && x.label),
    ["New", "Open…", "Open Recent", "Save", "Save as…", "Import…", "Export", "Project", "LightBurn", "Laser", "New tab", "Settings"],
  );
});

test("no action id appears twice", () => {
  const ids = actionIdsOf(buildFileMenu(full()));
  assert.equal(new Set(ids).size, ids.length, ids.join(", "));
  assert.ok(ids.includes("file.export") && ids.includes("window.devices"));
});

test("no entry is listed twice and the moved entries are in their submenus", () => {
  const paths = labelPaths(buildFileMenu(full()));
  assert.equal(new Set(paths).size, paths.length);
  for (const p of ["Laser › Devices…", "Laser › Material Library", "Laser › Find My Laser / Add galvo…", "Laser › Batch run…", "Laser › Job log…"]) assert.ok(paths.includes(p), p);
  for (const p of ["Export › Print…", "Export › Save G-code…", "Export › Save RD file…", "Export › Download .openetch", "LightBurn › Import LightBurn…", "LightBurn › Export to LightBurn…", "Settings › Preferences…"])
    assert.ok(paths.includes(p), p);
});

test("empty submenus are left out; a lone settings entry stays a plain row", () => {
  const i = full();
  i.parts = { settings: [it("Preferences…", { actionId: "app.preferences" })] };
  i.devices = undefined;
  i.saveGcode = undefined;
  i.saveRd = undefined;
  i.recent = undefined;
  const m = buildFileMenu(i);
  const labels = m.filter((x) => !x.sep).map((x) => !x.sep && x.label);
  assert.ok(!labels.includes("Project") && !labels.includes("Laser") && !labels.includes("Open Recent"));
  assert.ok(labels.includes("Preferences…"));
  assert.ok(labels.includes("Export"), "Export still has Download .openetch");
});

test("busy disables the file actions", () => {
  const i = full();
  i.busy = true;
  const m = buildFileMenu(i);
  const save = m.find((x) => !x.sep && x.label === "Save");
  assert.ok(save && !save.sep && save.disabled);
});
