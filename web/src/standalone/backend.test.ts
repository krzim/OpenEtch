// The backend around the routes: webcore's store writes persisted and loaded again at boot, the BroadcastChannel
// refresh between two tabs, the recovery copy after a reload or a crash, and a webcore panic.

import { test } from "node:test";
import assert from "node:assert/strict";
import { MemoryStore } from "./idb.ts";
import { CORE_PREFIX, S } from "./records.ts";
import { channelPair, makeTab, settle } from "./testkit.ts";

test("webcore's store writes are persisted and loaded into the next boot", async () => {
  const store = new MemoryStore();
  const a = makeTab({ store });
  const r = await a.call("PUT", "/api/prefs", { units: "in" });
  assert.equal(r.status, 200);
  assert.equal(await store.get(`${CORE_PREFIX}prefs`, "prefs"), JSON.stringify({ units: "in" }));
  const b = makeTab({ store, sessionId: "tab-b" });
  await b.backend.boot("fresh", 60);
  assert.deepEqual((await b.call("GET", "/api/prefs")).json, { units: "in" });
  assert.deepEqual(b.core.booted, { now_unix_ms: b.clock.t, tz_offset_min: 60 });
});

test("other tabs hear of saves and of webcore's store changes", async () => {
  const store = new MemoryStore();
  const [ca, cb] = channelPair();
  const a = makeTab({ store, channel: ca, sessionId: "a" });
  const b = makeTab({ store, channel: cb, sessionId: "b" });
  await a.backend.boot("fresh");
  await b.backend.boot("fresh");

  await a.call("POST", "/api/execute", { id: "shape.rect" });
  await a.call("POST", "/api/projects/save", { name: "Shared" });
  await settle(a, b);
  const heard = b.events.map((e) => JSON.parse(e)).filter((e) => e.type === "stores_changed");
  assert.equal(heard.length, 1);
  assert.ok(heard[0].stores.includes(S.projects));
  assert.deepEqual((await b.call("GET", "/api/projects")).json.map((p: { name: string }) => p.name), ["Shared"]);
  // The sender does not hear itself.
  assert.equal(a.events.some((e) => e.includes("stores_changed")), false);

  await a.call("PUT", "/api/prefs", { units: "in" });
  await settle(a, b);
  assert.deepEqual((await b.call("GET", "/api/prefs")).json, { units: "in" });
  const last = JSON.parse(b.events.at(-1) ?? "{}");
  assert.equal(last.type, "stores_changed");
  assert.deepEqual(last.stores, ["prefs"]);

  // A font added in tab a is loaded into tab b's webcore.
  const ttf = new Uint8Array(64);
  ttf.set([0x4f, 0x54, 0x54, 0x4f]);
  await a.call("POST", "/api/fonts/user?name=Brand.otf", ttf);
  await settle(a, b);
  assert.deepEqual(b.core.fonts, ["Brand"]);
});

test("a reload restores the tab's unsaved document from its recovery copy", async () => {
  const store = new MemoryStore();
  const a = makeTab({ store, sessionId: "s1" });
  await a.backend.boot("fresh");
  await a.call("POST", "/api/document/upload?format=openetch&name=Gift.openetch", new TextEncoder().encode(JSON.stringify({ format: "openetch", version: 1, name: "Gift", objects: [] })));
  await a.call("POST", "/api/execute", { id: "shape.rect" });
  await a.call("POST", "/api/execute", { id: "shape.ellipse" });
  // The debounced write is scheduled; the page's flush (pagehide) writes it now.
  assert.ok(a.timers.count() >= 1);
  await a.backend.flush();
  const saved = a.clock.t;

  // The reloaded tab: the same session id, a fresh worker.
  const b = makeTab({ store, sessionId: "s1" });
  const rec = await b.backend.boot("reload");
  assert.deepEqual(rec, { source: "recovery", savedMs: saved, name: "Gift" });
  assert.equal((await b.call("GET", "/api/document")).json.objects.length, 2);
  const p = (await b.call("GET", "/api/project")).json;
  assert.equal(p.display_name, "Gift");
  assert.equal(p.dirty, true);

  // Another tab (a new session) starts empty.
  const c = makeTab({ store, sessionId: "s2" });
  assert.equal(await c.backend.boot("fresh"), null);
  assert.equal((await c.call("GET", "/api/document")).json.objects.length, 0);
});

test("recovery of a project: unchanged it is reopened, changed it stays the project's and dirty", async () => {
  const store = new MemoryStore();
  const a = makeTab({ store, sessionId: "s1" });
  await a.call("POST", "/api/execute", { id: "shape.rect" });
  await a.call("POST", "/api/projects/save", { name: "Plate" });
  await a.backend.flush();
  let b = makeTab({ store, sessionId: "s1" });
  let rec = await b.backend.boot("reload");
  assert.equal(rec?.source, "project");
  let p = (await b.call("GET", "/api/project")).json;
  assert.equal(p.name, "Plate");
  assert.equal(p.dirty, false);

  await b.call("POST", "/api/execute", { id: "shape.rect" });
  await b.backend.flush();
  const c = makeTab({ store, sessionId: "s1" });
  rec = await c.backend.boot("restart");
  assert.equal(rec?.source, "recovery");
  p = (await c.call("GET", "/api/project")).json;
  assert.equal(p.name, "Plate");
  assert.equal(p.dirty, true);
  assert.equal((await c.call("GET", "/api/document")).json.objects.length, 2);
  // The recovered tab still knows which save it built on: another tab's later save is caught.
  b = makeTab({ store, sessionId: "s3" });
  await b.call("POST", "/api/projects/open", { name: "Plate" });
  b.clock.t = c.clock.t + 5000;
  await b.call("POST", "/api/projects/save", { name: "Plate" });
  const r = await c.call("POST", "/api/projects/save", { name: "Plate" });
  assert.equal(r.status, 409);
  assert.equal(r.json.code, "changed_elsewhere");
});

test("a clean untitled document leaves no recovery copy; old copies of closed tabs are pruned", async () => {
  const store = new MemoryStore();
  const a = makeTab({ store, sessionId: "old" });
  await a.call("POST", "/api/execute", { id: "shape.rect" });
  await a.backend.flush();
  assert.ok(await store.get(S.recovery, "old"));
  await a.call("POST", "/api/execute", { id: "doc.new" });
  await a.backend.flush();
  assert.equal(await store.get(S.recovery, "old"), undefined);

  await a.call("POST", "/api/execute", { id: "shape.rect" });
  await a.backend.flush();
  const later = makeTab({ store, sessionId: "new", clock: { t: a.clock.t + 15 * 24 * 3600_000 } });
  await later.backend.boot("fresh");
  assert.equal(await store.get(S.recovery, "old"), undefined);
  assert.equal(await store.get(S.recoveryData, "old"), undefined);
});

test("at most 8 other tabs' recovery copies are kept, the newest", async () => {
  const store = new MemoryStore();
  const clock = { t: Date.UTC(2026, 10, 3, 9, 0, 0) };
  for (let i = 0; i < 11; i++) {
    const t = makeTab({ store, clock, sessionId: `s${i}` });
    await t.call("POST", "/api/execute", { id: "shape.rect" });
    await t.backend.flush();
    clock.t += 1000;
  }
  const me = makeTab({ store, clock, sessionId: "me" });
  await me.backend.boot("fresh");
  assert.deepEqual(await store.keys(S.recovery), ["s10", "s3", "s4", "s5", "s6", "s7", "s8", "s9"]);
  assert.equal((await store.keys(S.recoveryData)).length, 8);
});

test("a webcore panic: 500 editor_crashed, then 503 for everything, and onCrash once", async () => {
  const tab = makeTab();
  await tab.call("POST", "/api/execute", { id: "shape.rect" });
  const r = await tab.call("POST", "/api/_mock/panic");
  assert.equal(r.status, 500);
  assert.equal(r.json.code, "editor_crashed");
  assert.equal(tab.crashes.length, 1);
  assert.ok(tab.backend.isCrashed);
  const after = await tab.call("GET", "/api/project");
  assert.equal(after.status, 503);
  assert.equal(after.json.code, "editor_restarting");
  // Storage routes stop too: nothing is written from a poisoned instance.
  assert.equal((await tab.call("POST", "/api/projects/save", { name: "X" })).status, 503);
  assert.equal(tab.crashes.length, 1);
});

test("requests run one at a time, in order", async () => {
  const tab = makeTab();
  const order: string[] = [];
  const save = tab.call("POST", "/api/projects/save", { name: "First" }).then(() => order.push("save"));
  const edit = tab.call("POST", "/api/execute", { id: "shape.rect" }).then(() => order.push("edit"));
  await Promise.all([save, edit]);
  assert.deepEqual(order, ["save", "edit"]);
  // The save saw the document before the edit.
  assert.equal((await tab.call("GET", "/api/project")).json.dirty, true);
});
