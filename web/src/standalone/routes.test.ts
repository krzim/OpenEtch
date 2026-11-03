// The worker shell's storage routes against the in-memory store and the mock webcore: projects (save, list, open,
// 409 overwrite and the two-tab check), folders, thumbnails, autosave rotation, backups and restore, file settings,
// templates, user fonts and a full storage.

import { test } from "node:test";
import assert from "node:assert/strict";
import { MemoryStore } from "./idb.ts";
import { mockProjectFile } from "./mockCore.ts";
import { S } from "./records.ts";
import { makeTab } from "./testkit.ts";

async function shape(tab: ReturnType<typeof makeTab>, kind = "rect") {
  return tab.call("POST", "/api/execute", { id: `shape.${kind}`, params: {} });
}

test("save, list and open a project", async () => {
  const tab = makeTab();
  await shape(tab);
  let p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.dirty, true);

  const saved = await tab.call("POST", "/api/projects/save", { name: "Coaster (v2)", origin: { kind: "ui" } });
  assert.equal(saved.status, 200);
  assert.equal(saved.json.ok, true);
  assert.equal(saved.json.name, "Coaster (v2)");
  assert.equal(typeof saved.json.revision, "number");
  assert.ok(tab.types().includes("project_saved"));

  p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.name, "Coaster (v2)");
  assert.equal(p.dirty, false);

  const list = (await tab.call("GET", "/api/projects")).json;
  assert.equal(list.length, 1);
  assert.deepEqual(Object.keys(list[0]).sort(), ["folder", "modified_unix_s", "name", "size"]);
  assert.equal(list[0].modified_unix_s, Math.floor(tab.clock.t / 1000));

  await tab.call("POST", "/api/execute", { id: "doc.new" });
  assert.equal((await tab.call("GET", "/api/document")).json.objects.length, 0);
  // A name that differs only in case finds it; the project keeps its stored name.
  const opened = await tab.call("POST", "/api/projects/open", { name: "coaster (V2)" });
  assert.equal(opened.status, 200);
  assert.equal(opened.json.name, "Coaster (v2)");
  assert.equal((await tab.call("GET", "/api/document")).json.objects.length, 1);
  p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.name, "Coaster (v2)");
  assert.equal(p.dirty, false);
});

test("save and open refuse what the engine refuses", async () => {
  const tab = makeTab();
  for (const name of ["", "../x", ".hidden", "a:b", "CON", "templates/x", "a/b/c/d/e/f"]) {
    const r = await tab.call("POST", "/api/projects/save", { name });
    assert.equal(r.status, 400, name);
    assert.equal(r.json.ok, false);
  }
  assert.equal((await tab.call("POST", "/api/projects/save", {})).status, 400);
  assert.equal((await tab.call("POST", "/api/projects/save", { name: "x", extra: 1 })).status, 400);
  assert.equal((await tab.call("POST", "/api/projects/open", { name: "Missing" })).status, 404);
  assert.match((await tab.call("POST", "/api/projects/open", { name: "Missing" })).json.error, /no project named "Missing"/);
  assert.equal((await tab.call("POST", "/api/projects/open", { name: "x", overwrite: true })).status, 400);
  assert.equal((await tab.call("GET", "/api/projects?x=1")).status, 400);
});

test("saving over another project: 409 without overwrite, replaced with it", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Other" });
  await tab.call("POST", "/api/execute", { id: "doc.new" });
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Coaster" });
  // The current project may be saved over; another one only with overwrite.
  assert.equal((await tab.call("POST", "/api/projects/save", { name: "Coaster", overwrite: false })).status, 200);
  const r = await tab.call("POST", "/api/projects/save", { name: "Other", overwrite: false });
  assert.equal(r.status, 409);
  assert.match(r.json.error, /a project named "Other" already exists; pass overwrite: true/);
  const casing = await tab.call("POST", "/api/projects/save", { name: "OTHER", overwrite: false });
  assert.equal(casing.status, 409);
  assert.match(casing.json.error, /differ only in case/);
  // The engine's default is to replace.
  assert.equal((await tab.call("POST", "/api/projects/save", { name: "OTHER" })).status, 200);
  const names = (await tab.call("GET", "/api/projects")).json.map((p: { name: string }) => p.name);
  assert.deepEqual(names, ["Coaster", "OTHER"]);
});

test("a project saved in another tab since this one opened it: 409 changed_elsewhere", async () => {
  const store = new MemoryStore();
  const clock = { t: Date.UTC(2026, 10, 3, 9, 0, 0) };
  const a = makeTab({ store, clock, sessionId: "a" });
  const b = makeTab({ store, clock, sessionId: "b" });
  await shape(a);
  await a.call("POST", "/api/projects/save", { name: "Badge" });
  assert.equal((await b.call("POST", "/api/projects/open", { name: "Badge" })).status, 200);
  await shape(b);
  clock.t += 1000;
  assert.equal((await b.call("POST", "/api/projects/save", { name: "Badge" })).status, 200);
  // Tab a still holds the older copy.
  await shape(a);
  const r = await a.call("POST", "/api/projects/save", { name: "Badge" });
  assert.equal(r.status, 409);
  assert.equal(r.json.code, "changed_elsewhere");
  assert.match(r.json.error, /saved in another tab or window/);
  // The person chose to replace it.
  assert.equal((await a.call("POST", "/api/projects/save", { name: "Badge", overwrite: true })).status, 200);
  // And now tab a knows the new stamp: a second save goes through.
  await shape(a);
  assert.equal((await a.call("POST", "/api/projects/save", { name: "Badge" })).status, 200);
});

test("saves are stamped strictly later even within one millisecond", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Same" });
  const first = (await tab.store.get<{ modified_ms: number }>(S.projects, "Same"))!.modified_ms;
  await tab.call("POST", "/api/projects/save", { name: "Same" });
  const second = (await tab.store.get<{ modified_ms: number }>(S.projects, "Same"))!.modified_ms;
  assert.equal(second, first + 1);
});

test("folders: projects in folders and empty folders", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Clients/Acme/badge" });
  assert.equal((await tab.call("POST", "/api/projects/folders", { name: "Stock/Coasters" })).status, 200);
  const f = (await tab.call("GET", "/api/projects/folders")).json;
  assert.deepEqual(f.folders, ["Clients", "Clients/Acme", "Stock", "Stock/Coasters"]);
  assert.deepEqual(f.locations, []);
  const list = (await tab.call("GET", "/api/projects")).json;
  assert.equal(list[0].folder, "Clients/Acme");
  assert.equal((await tab.call("POST", "/api/projects/folders", { name: "templates" })).status, 400);
  assert.deepEqual((await tab.call("GET", "/api/projects/locations")).json, { locations: [] });
  const put = await tab.call("PUT", "/api/projects/locations", { locations: [] });
  assert.equal(put.status, 404);
  assert.equal(put.json.code, "design_only");
});

test("thumbnails are rendered by webcore once per saved version and width", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Clients/Acme/badge" });
  const url = `/api/projects/${encodeURIComponent("Clients/Acme/badge")}/thumbnail.png?width=128`;
  const r = await tab.call("GET", url);
  assert.equal(r.status, 200);
  assert.deepEqual(r.headers.find(([k]) => k === "content-type"), ["content-type", "image/png"]);
  assert.deepEqual([...r.body.slice(1, 4)], [0x50, 0x4e, 0x47]);
  await tab.call("GET", url);
  assert.equal(tab.core.log.filter((l) => l.startsWith("POST /api/_shell/thumbnail")).length, 1);
  // A new save makes a new picture.
  await tab.call("POST", "/api/projects/save", { name: "Clients/Acme/badge" });
  await tab.call("GET", url);
  assert.equal(tab.core.log.filter((l) => l.startsWith("POST /api/_shell/thumbnail")).length, 2);
  assert.equal((await tab.call("GET", url.replace("128", "16"))).status, 400);
  assert.equal((await tab.call("GET", "/api/projects/Nope/thumbnail.png")).status, 404);
});

test("autosave: only when changed, every autosave_minutes, the newest 10 kept", async () => {
  const tab = makeTab();
  // Nothing changed yet: no autosave.
  assert.equal(await tab.backend.autosaver.autosaveNow(false), null);
  await shape(tab);
  // The interval has not passed.
  assert.equal(await tab.backend.autosaver.tick(), null);
  tab.clock.t += 5 * 60_000;
  const first = await tab.backend.autosaver.tick();
  assert.match(first ?? "", /^Untitled\.\d+\.openetch$/);
  // Unchanged since: none.
  tab.clock.t += 5 * 60_000;
  assert.equal(await tab.backend.autosaver.tick(), null);
  for (let i = 0; i < 14; i++) {
    await shape(tab);
    tab.clock.t += 5 * 60_000;
    assert.ok(await tab.backend.autosaver.tick());
  }
  const b = (await tab.call("GET", "/api/backups")).json;
  assert.equal(b.autosaves.length, 10);
  assert.equal(b.backups.length, 0);
  const times = b.autosaves.map((x: { saved_unix_s: number }) => x.saved_unix_s);
  assert.deepEqual(times, [...times].sort((x: number, y: number) => y - x));
  assert.equal(b.autosaves[0].project, "Untitled");
  assert.equal((await tab.store.keys(S.autosaveData)).length, 10);
  // autosave_minutes 0 turns it off.
  await tab.call("PUT", "/api/files/settings", { autosave_minutes: 0 });
  await shape(tab);
  tab.clock.t += 60 * 60_000;
  assert.equal(await tab.backend.autosaver.tick(), null);
});

test("autosaves of a project are named after it, folders flattened, and listed per project", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Clients/Acme/badge" });
  await shape(tab);
  const file = await tab.backend.autosaver.autosaveNow(false);
  assert.match(file ?? "", /^Clients__Acme__badge\.\d+\.openetch$/);
  // Two in the same second get distinct names.
  await shape(tab);
  const second = await tab.backend.autosaver.autosaveNow(false);
  assert.notEqual(second, file);
  const mine = (await tab.call("GET", `/api/backups?project=${encodeURIComponent("clients/acme/BADGE")}`)).json;
  assert.equal(mine.autosaves.length, 2);
  assert.equal((await tab.call("GET", "/api/backups?project=Other")).json.autosaves.length, 0);
});

test("a save backs up the file it replaces; backups_keep copies; restore opens one unsaved", async () => {
  const tab = makeTab();
  await tab.call("PUT", "/api/files/settings", { backups_keep: 2 });
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Mug" });
  assert.equal((await tab.call("GET", "/api/backups")).json.backups.length, 0);
  for (let i = 0; i < 3; i++) {
    await shape(tab);
    tab.clock.t += 1000;
    await tab.call("POST", "/api/projects/save", { name: "Mug" });
  }
  const list = (await tab.call("GET", "/api/backups?project=Mug")).json.backups;
  assert.equal(list.length, 2);
  // The newest backup is the save before the last one: 3 objects.
  const r = await tab.call("POST", "/api/backups/restore", { kind: "backup", file: list[0].file, origin: { kind: "ui" } });
  assert.equal(r.status, 200);
  assert.equal(r.json.ok, true);
  assert.equal((await tab.call("GET", "/api/document")).json.objects.length, 3);
  const p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.name, null);
  assert.equal(p.display_name, "Mug");
  // Refused like the engine.
  assert.equal((await tab.call("POST", "/api/backups/restore", { kind: "trash", file: list[0].file })).status, 400);
  assert.equal((await tab.call("POST", "/api/backups/restore", { kind: "backup", file: "../x.openetch" })).status, 400);
  assert.equal((await tab.call("POST", "/api/backups/restore", { kind: "autosave", file: list[0].file })).status, 404);
  // backups_keep 0: no backups made.
  await tab.call("PUT", "/api/files/settings", { backups_keep: 0 });
  await tab.call("POST", "/api/projects/save", { name: "Mug" });
  assert.equal((await tab.call("GET", "/api/backups?project=Mug")).json.backups.length, 2);
});

test("file settings: defaults, partial PUT, ranges", async () => {
  const tab = makeTab();
  assert.deepEqual((await tab.call("GET", "/api/files/settings")).json, { autosave_minutes: 5, autosave_keep: 10, backups_keep: 5 });
  const r = await tab.call("PUT", "/api/files/settings", { autosave_keep: 3, origin: { kind: "ui" } });
  assert.deepEqual(r.json, { autosave_minutes: 5, autosave_keep: 3, backups_keep: 5 });
  for (const bad of [{ autosave_keep: 0 }, { autosave_keep: 51 }, { backups_keep: 51 }, { autosave_minutes: 1441 }, { autosave_minutes: -1 }, { autosave_minutes: 1.5 }, { other: 1 }]) {
    assert.equal((await tab.call("PUT", "/api/files/settings", bad)).status, 400, JSON.stringify(bad));
  }
  assert.equal((await tab.call("GET", "/api/files/settings")).json.autosave_keep, 3);
});

test("templates: save, 409 without overwrite, a LightBurn .lbt, open as a new untitled document", async () => {
  const tab = makeTab();
  await shape(tab);
  await shape(tab);
  assert.equal((await tab.call("POST", "/api/templates/save", { name: "Name badge" })).status, 200);
  const again = await tab.call("POST", "/api/templates/save", { name: "name BADGE" });
  assert.equal(again.status, 409);
  assert.equal((await tab.call("POST", "/api/templates/save", { name: "Name badge", overwrite: true })).status, 200);
  assert.equal((await tab.call("POST", "/api/templates/save", { name: "a/b" })).status, 400);

  const lbt = '<?xml version="1.0"?><LightBurnProject AppVersion="1.0"></LightBurnProject>';
  assert.equal((await tab.call("POST", "/api/templates/upload?name=Tag", lbt)).status, 200);
  assert.equal((await tab.call("POST", "/api/templates/upload?name=Bad", "<svg/>")).status, 400);
  assert.equal((await tab.call("POST", "/api/templates/upload", lbt)).status, 400);
  assert.equal((await tab.call("POST", "/api/templates/upload?name=Bin", new Uint8Array([0xff, 0xfe, 0x00]))).status, 400);

  const list = (await tab.call("GET", "/api/templates")).json;
  assert.deepEqual(list.map((t: { name: string; kind: string }) => [t.name, t.kind]), [["Name badge", "openetch"], ["Tag", "lbt"]]);

  await tab.call("POST", "/api/projects/save", { name: "Current" });
  const o = await tab.call("POST", "/api/templates/open", { name: "name badge" });
  assert.equal(o.status, 200);
  assert.equal((await tab.call("GET", "/api/document")).json.objects.length, 2);
  // Never the current project.
  const p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.name, null);
  assert.equal(p.display_name, null);
  assert.equal((await tab.call("POST", "/api/templates/open", { name: "Tag" })).status, 200);
  assert.ok(tab.core.log.includes("POST /api/document/upload?format=lbrn2&mode=replace"));
  assert.equal((await tab.call("POST", "/api/templates/open", { name: "Missing" })).status, 404);
});

test("opening a project in a new document tab, and an open one activated instead", async () => {
  const tab = makeTab();
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Plate" });
  const fresh = await tab.call("POST", "/api/documents", {});
  assert.equal(fresh.status, 200);
  const second = tab.core.activeId();
  assert.notEqual(second, "default");
  const r = await tab.call("POST", "/api/documents", { project: "plate" });
  assert.equal(r.status, 200);
  assert.equal(r.json.existing, true);
  assert.equal(r.json.id, "default");
  assert.equal((await tab.call("POST", "/api/documents", { project: "Missing" })).status, 404);
  assert.equal((await tab.call("POST", "/api/documents", { project: "Plate", activate: false })).status, 400);
});

test("user fonts: added, listed, handed to webcore at boot, removed", async () => {
  const store = new MemoryStore();
  const tab = makeTab({ store });
  const ttf = new Uint8Array(64);
  ttf.set([0, 1, 0, 0]);
  const r = await tab.call("POST", "/api/fonts/user?name=Liberation%20Sans.ttf", ttf);
  assert.equal(r.status, 200);
  assert.equal(r.json.family, "Liberation Sans");
  const woff = new Uint8Array(64);
  woff.set([0x77, 0x4f, 0x46, 0x32]);
  assert.match((await tab.call("POST", "/api/fonts/user?name=x.ttf", woff)).json.error, /WOFF/);
  assert.equal((await tab.call("POST", "/api/fonts/user?name=x.ttf", new Uint8Array(64))).status, 400);
  assert.equal((await tab.call("POST", "/api/fonts/user?name=x.exe", ttf)).status, 400);
  assert.equal((await tab.call("GET", "/api/fonts/user")).json.fonts.length, 1);

  const next = makeTab({ store, sessionId: "tab-b" });
  await next.backend.boot("fresh");
  assert.deepEqual(next.core.fonts, ["Liberation Sans"]);

  assert.equal((await tab.call("DELETE", `/api/fonts/user/${encodeURIComponent("Liberation Sans.ttf")}`)).status, 200);
  assert.equal((await tab.call("GET", "/api/fonts/user")).json.fonts.length, 0);
  assert.equal((await tab.call("DELETE", "/api/fonts/user/none.ttf")).status, 404);
});

test("a full storage answers 507 storage_full and keeps what was there", async () => {
  const store = new MemoryStore();
  const tab = makeTab({ store });
  await shape(tab);
  await tab.call("POST", "/api/projects/save", { name: "Small" });
  store.quotaBytes = 10;
  await shape(tab);
  const r = await tab.call("POST", "/api/projects/save", { name: "Big" });
  assert.equal(r.status, 507);
  assert.equal(r.json.code, "storage_full");
  assert.deepEqual((await tab.call("GET", "/api/projects")).json.map((p: { name: string }) => p.name), ["Small"]);
  // The project status did not claim a save.
  assert.equal((await tab.call("GET", "/api/project")).json.name, "Small");
  assert.equal((await tab.call("GET", "/api/project")).json.dirty, true);
});

test("routes the shell does not own go to webcore; unknown shell routes and internal ones are refused", async () => {
  const tab = makeTab();
  assert.equal((await tab.call("GET", "/api/machine")).json.design_only, true);
  const jog = await tab.call("POST", "/api/machine/jog", {});
  assert.equal(jog.status, 404);
  assert.equal(jog.json.code, "design_only");
  assert.equal((await tab.call("GET", "/api/projects/nothing")).status, 404);
  assert.equal((await tab.call("POST", "/api/_shell/project_saved", { name: "x", revision: 1, document_id: "default" })).status, 404);
  assert.equal(tab.core.log.some((l) => l.includes("project_saved")), false);
});

test("an uploaded project file opens through webcore unchanged (the shell only stores)", async () => {
  const tab = makeTab();
  const r = await tab.call("POST", "/api/document/upload?format=openetch&name=Gift.openetch", mockProjectFile("Gift", 4));
  assert.equal(r.status, 200);
  const p = (await tab.call("GET", "/api/project")).json;
  assert.equal(p.display_name, "Gift");
  assert.equal((await tab.call("GET", "/api/document")).json.objects.length, 4);
});
